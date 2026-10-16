//! 表格抽取（PARSE-07/08）。
//!
//! 语义（跨行跨列不丢格）：
//! - 先在逻辑网格上定位每个 td/th 的起点，再按 rowspan/colspan 占位；
//! - grid[row][col] 是 cells 的下标：起点格给出真实文本，被覆盖格指向同一个
//!   起点格，因此**每个网格位置都有确定归属**，不存在空洞；
//! - 行/列长度不齐时补 synthetic=true 的空格子，保证矩形网格；
//! - rowspan 超出最后一行时按需扩展行，不丢跨行格。
//! - colspan=0 / rowspan=0 / 非法值按 1 处理；超过 MAX_SPAN 时钳制并整体标记
//!   LIMIT（degraded_from="table-span-clamped"），不静默截断。

use std::time::Instant;

use scraper::ElementRef;
use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{elapsed_ms, html};

/// 单个跨度上限，防超长 colspan/rowspan 撑爆内存。
pub const MAX_SPAN: usize = 1000;

/// 表格中的一个逻辑格子（起点格）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableCell {
    /// 起点行（0 起）。
    pub row: usize,
    /// 起点列（0 起）。
    pub col: usize,
    /// 纵向跨度（>=1）。
    pub row_span: usize,
    /// 横向跨度（>=1）。
    pub col_span: usize,
    /// 是否来自 th。
    pub header: bool,
    /// 是否为补齐矩形网格而生成的空格子。
    pub synthetic: bool,
    /// 单元格文本（折叠空白）。
    pub text: String,
}

/// 一张表格的矩形网格视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// 文档序序号（0 起）。
    pub index: usize,
    /// caption 文本。
    pub caption: Option<String>,
    /// 网格行数（含 rowspan 扩展出的行）。
    pub rows: usize,
    /// 网格列数（各行补齐后的最大宽度）。
    pub columns: usize,
    /// 起点格清单（文档序）。
    pub cells: Vec<TableCell>,
    /// grid[row][col] -> cells 下标；被 rowspan/colspan 覆盖的位置指向起点格。
    pub grid: Vec<Vec<usize>>,
}

/// 抽取全部表格；零命中是合法空结果。
pub fn extract_tables(html_text: &str) -> Envelope<Vec<Table>> {
    let started = Instant::now();
    let css = match html::compile_selector("table") {
        Ok(css) => css,
        Err(message) => return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started)),
    };
    let document = html::parse_document(html_text);
    let mut clamped = false;
    let tables = document
        .select(&css)
        .enumerate()
        .map(|(index, element)| {
            let (table, was_clamped) = build_table(index, element);
            clamped |= was_clamped;
            table
        })
        .collect::<Vec<_>>();
    if clamped {
        Envelope::degraded(
            tables,
            "table-span-clamped",
            ErrorCode::Limit,
            elapsed_ms(started),
        )
    } else {
        Envelope::ok(tables, Trust::UntrustedPageContent, elapsed_ms(started))
    }
}

fn build_table(index: usize, table: ElementRef<'_>) -> (Table, bool) {
    let mut caption = None;
    for child in table.children() {
        if let Some(element) = ElementRef::wrap(child)
            && element.value().name() == "caption"
        {
            caption = Some(html::collapse_ws(&element.text().collect::<String>()));
            break;
        }
    }

    let mut row_elements: Vec<ElementRef<'_>> = Vec::new();
    for child in table.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        let tag = element.value().name();
        if tag == "tr" {
            row_elements.push(element);
        } else if matches!(tag, "thead" | "tbody" | "tfoot") {
            for row in element.children() {
                if let Some(row_element) = ElementRef::wrap(row)
                    && row_element.value().name() == "tr"
                {
                    row_elements.push(row_element);
                }
            }
        }
    }

    let mut cells: Vec<TableCell> = Vec::new();
    let mut grid: Vec<Vec<usize>> = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut clamped = false;

    for (row_index, row) in row_elements.iter().enumerate() {
        ensure_row(&mut grid, &mut occupied, row_index);
        let mut col = 0usize;
        for cell_element in row.children().filter_map(ElementRef::wrap) {
            let tag = cell_element.value().name();
            if tag != "td" && tag != "th" {
                continue;
            }
            while occupied[row_index].get(col).copied().unwrap_or(false) {
                col += 1;
            }
            let (col_span, col_clamped) = read_span(cell_element.attr("colspan"));
            let (row_span, row_clamped) = read_span(cell_element.attr("rowspan"));
            clamped |= col_clamped || row_clamped;
            let cell_index = cells.len();
            cells.push(TableCell {
                row: row_index,
                col,
                row_span,
                col_span,
                header: tag == "th",
                synthetic: false,
                text: html::collapse_ws(&cell_element.text().collect::<String>()),
            });
            for dr in 0..row_span {
                let target_row = row_index + dr;
                ensure_row(&mut grid, &mut occupied, target_row);
                for dc in 0..col_span {
                    let target_col = col + dc;
                    ensure_col(&mut grid, &mut occupied, target_row, target_col);
                    if !occupied[target_row][target_col] {
                        occupied[target_row][target_col] = true;
                        grid[target_row][target_col] = cell_index;
                    }
                }
            }
            col += col_span;
        }
    }

    let columns = grid.iter().map(Vec::len).max().unwrap_or(0);
    for (row_index, row) in grid.iter_mut().enumerate() {
        while row.len() < columns {
            let cell_index = cells.len();
            cells.push(TableCell {
                row: row_index,
                col: row.len(),
                row_span: 1,
                col_span: 1,
                header: false,
                synthetic: true,
                text: String::new(),
            });
            row.push(cell_index);
        }
    }

    (
        Table {
            index,
            caption,
            rows: grid.len(),
            columns,
            cells,
            grid,
        },
        clamped,
    )
}

fn ensure_row(grid: &mut Vec<Vec<usize>>, occupied: &mut Vec<Vec<bool>>, row: usize) {
    while grid.len() <= row {
        grid.push(Vec::new());
        occupied.push(Vec::new());
    }
}

fn ensure_col(grid: &mut [Vec<usize>], occupied: &mut [Vec<bool>], row: usize, col: usize) {
    while grid[row].len() <= col {
        grid[row].push(usize::MAX);
        occupied[row].push(false);
    }
}

fn read_span(value: Option<&str>) -> (usize, bool) {
    let Some(value) = value else {
        return (1, false);
    };
    match value.trim().parse::<usize>() {
        Ok(span) if (1..=MAX_SPAN).contains(&span) => (span, false),
        Ok(span) if span > MAX_SPAN => (MAX_SPAN, true),
        _ => (1, false),
    }
}
