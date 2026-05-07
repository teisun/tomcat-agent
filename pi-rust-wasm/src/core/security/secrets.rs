//! # `edit` / `write` 工具：T3-K secrets 扫描
//!
//! 在 `write_file_atomic` 之前对 `new_content` 做轻量正则匹配；命中即让上层
//! 走 `require_user_confirmation` 兜底（命中不一定是真密钥；硬拒会破坏「贴
//! `.env` 模板」等合法工作流）。
//!
//! 规则集**冻结在本文件**，新增规则需另起 PR + 单测同步。当前规则覆盖：
//!
//! | 规则名 | 正则要点 | 命中样例 |
//! | --- | --- | --- |
//! | OpenAI key | `sk-[A-Za-z0-9_-]{20,}` | `sk-ABCDEFGHIJKLMNOPQRSTUV` |
//! | AWS access key id | `AKIA[0-9A-Z]{16}` | `AKIAIOSFODNN7EXAMPLE` |
//! | Slack token | `xox[baprs]-[A-Za-z0-9-]{10,}` | `xoxb-1234567890-abc...` |
//! | Generic high-entropy hex | 40+ 位连续 hex | `40` 位以上的随机 hex 串 |

use regex::Regex;
use std::sync::OnceLock;

/// 一个命中条目（文件位置 + 规则名 + 命中片段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretHit {
    /// 命中规则名（用于 confirm preview）。
    pub rule: &'static str,
    /// 命中片段在 `content` 中的字节起始偏移。
    pub byte_offset: usize,
    /// 命中片段的字面量（用于人类预览，**不**直接写日志）。
    pub matched: String,
}

struct CompiledRule {
    name: &'static str,
    re: Regex,
}

fn rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            CompiledRule {
                name: "openai_api_key",
                re: Regex::new(r"sk-[A-Za-z0-9_-]{20,}").unwrap(),
            },
            CompiledRule {
                name: "aws_access_key_id",
                re: Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            },
            CompiledRule {
                name: "slack_token",
                re: Regex::new(r"xox[baprs]-[A-Za-z0-9-]{10,}").unwrap(),
            },
            CompiledRule {
                name: "high_entropy_hex",
                re: Regex::new(r"\b[0-9a-fA-F]{40,}\b").unwrap(),
            },
        ]
    })
}

/// 扫描 `content` 中的潜在敏感信息。返回所有命中（按 byte_offset 升序）。
///
/// 性能：每条规则单次线性扫描；4 条规则总成本 ~O(n)。
pub fn scan(content: &str) -> Vec<SecretHit> {
    let mut hits = Vec::new();
    for rule in rules() {
        for m in rule.re.find_iter(content) {
            hits.push(SecretHit {
                rule: rule.name,
                byte_offset: m.start(),
                matched: m.as_str().to_string(),
            });
        }
    }
    hits.sort_by_key(|h| h.byte_offset);
    hits
}

/// 给 `require_user_confirmation` 用的人类可读 preview 文案。
///
/// 不直接打印完整命中（只显示首尾 4 字符 + `…`），既给用户足够的判断信号，
/// 又避免把密钥原文写到日志 / 审计落库。
pub fn format_preview(hits: &[SecretHit]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "edit / write 内容命中 {} 条潜在敏感信息：\n",
        hits.len()
    ));
    for (i, h) in hits.iter().enumerate() {
        let mask = if h.matched.len() > 8 {
            let prefix = &h.matched[..4];
            let suffix = &h.matched[h.matched.len() - 4..];
            format!("{}…{}", prefix, suffix)
        } else {
            "<masked>".to_string()
        };
        out.push_str(&format!(
            "  {}. [{}] @byte {} → {}\n",
            i + 1,
            h.rule,
            h.byte_offset,
            mask
        ));
        if i + 1 >= 5 && hits.len() > 5 {
            out.push_str(&format!("  …还有 {} 条已折叠\n", hits.len() - 5));
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_detects_openai_key() {
        let hits = scan("let key = \"sk-ABCDEFGHIJKLMNOPQRSTUV\"");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule, "openai_api_key");
    }

    #[test]
    fn scan_detects_aws_key() {
        let hits = scan("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n");
        assert!(hits.iter().any(|h| h.rule == "aws_access_key_id"));
    }

    #[test]
    fn scan_returns_empty_for_plain_code() {
        let hits = scan("fn main() { println!(\"hello\"); }");
        assert!(hits.is_empty(), "普通代码不应触发：{:?}", hits);
    }

    #[test]
    fn scan_orders_hits_by_offset() {
        let body = format!(
            "{}{}",
            "padding ", "AKIAIOSFODNN7EXAMPLE first sk-ABCDEFGHIJKLMNOPQRSTUV second"
        );
        let hits = scan(&body);
        assert!(hits.len() >= 2);
        for w in hits.windows(2) {
            assert!(w[0].byte_offset <= w[1].byte_offset);
        }
    }

    #[test]
    fn format_preview_masks_middle() {
        let hits = vec![SecretHit {
            rule: "openai_api_key",
            byte_offset: 10,
            matched: "sk-ABCDEFGHIJKLMNOPQRSTUV".to_string(),
        }];
        let p = format_preview(&hits);
        assert!(p.contains("openai_api_key"));
        // 应当含掩码省略号，且不含中间字符
        assert!(p.contains("…"));
        assert!(!p.contains("sk-ABCDEFGHIJKLMNOPQRSTUV"));
    }
}
