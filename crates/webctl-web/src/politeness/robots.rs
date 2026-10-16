//! robots.txt 的最小可用解析与匹配（PLAN v0.4 §3.8 / ADR-0004）。
//!
//! 只支持实际会用到的子集：`User-agent` / `Allow` / `Disallow` / `Sitemap`，注释与空行忽略；
//! 选择"最匹配的 user-agent 组"（精确 token 优先于 `*`），路径用"最长匹配优先、同长 Allow 胜"。
//! 不实现 `Crawl-delay`（限速由本模块外的 HostGate 统一控制）与正则扩展。
use serde::{Deserialize, Serialize};

/// 一条规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Rule {
    allow: bool,
    path: String,
}

/// 某个 user-agent 组的规则集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRules {
    /// 规则列表（保持文件顺序，便于诊断）。
    pub rules: Vec<(bool, String)>,
}

impl AgentRules {
    /// 该路径是否被允许。
    pub fn allows(&self, path: &str) -> bool {
        let path = normalize_path(path);
        let mut best: Option<(usize, bool)> = None;
        for (allow, pattern) in &self.rules {
            if pattern.is_empty() {
                continue;
            }
            if let Some(len) = match_pattern(pattern, &path) {
                match best {
                    Some((best_len, best_allow)) => {
                        if len > best_len || (len == best_len && *allow && !best_allow) {
                            best = Some((len, *allow));
                        }
                    }
                    None => best = Some((len, *allow)),
                }
            }
        }
        best.map(|(_, allow)| allow).unwrap_or(true)
    }

    /// 规则条数。
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// 是否没有任何规则。
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// 解析结果：`*` 组 + 各具名组。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsRules {
    /// `User-agent: *` 组。
    pub wildcard: AgentRules,
    /// 具名组（小写 token）。
    pub named: Vec<(String, AgentRules)>,
}

impl RobotsRules {
    /// 解析 robots.txt 文本。任何非法行都被忽略（robots.txt 宽容解析是行业惯例）。
    pub fn parse(text: &str) -> Self {
        let mut result = RobotsRules::default();
        let mut current: Option<String> = None; // None = 尚未出现 User-agent
        let mut targets: Vec<String> = Vec::new();

        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" | "useragent" => {
                    let token = value.to_ascii_lowercase();
                    if current.as_deref() != Some(token.as_str()) {
                        targets.clear();
                        current = Some(token.clone());
                    }
                    targets.push(token);
                }
                "allow" | "disallow" => {
                    if targets.is_empty() {
                        continue;
                    }
                    let allow = key == "allow";
                    let path = value.to_owned();
                    for target in &targets {
                        let group = if target == "*" {
                            &mut result.wildcard
                        } else {
                            let entry = result
                                .named
                                .iter_mut()
                                .find(|(name, _)| name == target);
                            match entry {
                                Some((_, rules)) => rules,
                                None => {
                                    result.named.push((target.clone(), AgentRules::default()));
                                    let last = result.named.len() - 1;
                                    &mut result.named[last].1
                                }
                            }
                        };
                        group.rules.push((allow, path.clone()));
                    }
                }
                _ => {}
            }
        }
        result
    }

    /// 针对某个 user-agent 选出规则组：精确 token 优先，其次 token 前缀匹配，最后 `*`。
    pub fn rules_for(&self, user_agent: &str) -> &AgentRules {
        let ua = user_agent.to_ascii_lowercase();
        if let Some((_, rules)) = self.named.iter().find(|(name, _)| ua.contains(name.as_str())) {
            return rules;
        }
        &self.wildcard
    }
}

/// 该 URL 路径是否被允许（无规则时允许）。
pub fn allows(rules: &RobotsRules, user_agent: &str, path: &str) -> bool {
    rules.rules_for(user_agent).allows(path)
}

fn normalize_path(path: &str) -> String {
    let path = path.split_once('#').map(|(p, _)| p).unwrap_or(path);
    let path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    }
}

/// 匹配路径模式（支持 `*` 通配与 `$` 结尾锚定），返回匹配长度（越长越具体）。
fn match_pattern(pattern: &str, path: &str) -> Option<usize> {
    let anchored = pattern.ends_with('$');
    let pattern = pattern.strip_suffix('$').unwrap_or(pattern);
    if !pattern.contains('*') {
        if anchored {
            return (pattern == path).then(|| pattern.len());
        }
        return path.starts_with(pattern).then(|| pattern.len());
    }
    // 逐段展开通配
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor = 0usize;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !path.starts_with(part) {
                return None;
            }
            cursor = part.len();
        } else {
            let Some(found) = path[cursor..].find(part) else {
                return None;
            };
            cursor += found + part.len();
        }
    }
    let last = parts.last().copied().unwrap_or("");
    if anchored && !path.ends_with(last) {
        return None;
    }
    Some(cursor.max(pattern.len().saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# 注释行
User-agent: *
Disallow: /private/
Allow: /private/public/
Disallow: /*.pdf$
Sitemap: https://example.test/sitemap.xml

User-agent: webctl
Disallow:
"#;

    #[test]
    fn parses_groups_and_answers_policy() {
        let rules = RobotsRules::parse(SAMPLE);
        assert!(rules.wildcard.len() >= 3);

        // `*` 组
        assert!(allows(&rules, "some-bot", "/"));
        assert!(!allows(&rules, "some-bot", "/private/x"));
        assert!(allows(&rules, "some-bot", "/private/public/x"), "更长 Allow 胜出");
        assert!(!allows(&rules, "some-bot", "/a/b.pdf"), "\$ 锚定生效");
        assert!(allows(&rules, "some-bot", "/a/b.pdf.html"));

        // 具名组：webctl 的 Disallow 为空 = 全允许（比 `*` 组更具体）
        assert!(allows(&rules, "webctl/0.1", "/private/x"));
    }

    #[test]
    fn empty_or_broken_input_allows_everything() {
        for text in ["", "   ", "这不是 robots", "Disallow: /x"] {
            let rules = RobotsRules::parse(text);
            assert!(allows(&rules, "any", "/whatever"), "输入 {text:?} 应当全允许");
        }
    }

    #[test]
    fn longest_match_wins_and_allow_beats_disallow_on_tie() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /a\nAllow: /a\nDisallow: /a/b\n");
        assert!(allows(&rules, "x", "/a"), "同长时 Allow 胜");
        assert!(!allows(&rules, "x", "/a/b"), "更长 Disallow 胜");
        assert!(allows(&rules, "x", "/c"));
    }

    #[test]
    fn query_and_fragment_are_ignored_when_matching() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /search\n");
        assert!(!allows(&rules, "x", "/search?q=1"));
        assert!(!allows(&rules, "x", "/search#frag"));
    }
}
