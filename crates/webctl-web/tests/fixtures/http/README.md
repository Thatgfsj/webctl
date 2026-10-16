# http fixture 数据

本目录只服务议题 #22 的 fixture HTTP 服务自测（`tests/http_fixture.rs`）。
解析站 / 表单站 / PDF 样本由其他 PR 占用各自的子目录；这里**只放 HTTP 注入样本**。

| 文件 | 内容 | 用途 |
|---|---|---|
| `robots.txt` | 允许 `/`、禁止 `/private/` | POLITE-06/07、robots 解析 |
| `article.html` | 含正文标记、导航/页脚噪声与相对链接 | PARSE-01/02/05 的取回侧样本 |
| `gbk.hex` | GBK 字节（十六进制文本） | HTTP-02、PARSE-04 |
| `shift_jis.hex` | Shift_JIS 字节 | HTTP-03 |
| `gzip.hex` / `brotli.hex` | 已压缩字节 | HTTP-05/06 |

非 UTF-8 样本统一存成 `.hex`（`#` 起注释），由
`support/http_fixture.rs::read_fixture_hex` 解码，避免仓库里出现无法审阅的二进制。

> 数据文件不计入 CONTRIBUTING §2 的行数口径，但需在 PR 描述里申报实际行数。
