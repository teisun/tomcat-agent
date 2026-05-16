//! # 内置默认规则常量
//!
//! 与 [`workspace_permission_tiers_design plan`](../../../../.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md) §4 / §5 对齐：
//!
//! - `BUILTIN_DEFAULT_PATH_RULES`：凭据保护 + Agent 自我提权防护 + Agent 历史只读
//! - `BUILTIN_BASH_FORBIDDEN`：系统级灾难命令 / 凭据泄漏防护 / Agent 自我提权防护
//! - `BUILTIN_BASH_APPROVAL_REQUIRED`：高危但可允许场景（rm -rf、sudo、git --force 等）
//!
//! **关键不变量**：用户**永远无法**通过 TOML / `config_set` 移除 builtin 列表项；
//! 要"放行某条 builtin"必须改代码源。坏 regex 静默跳过 + warning，不影响其他规则。

use super::path_rule::PathRule;
use super::types::PathRuleMode;

/// 内置默认 path_rules。
///
/// 涉及凭据保护（`~/.ssh` / `~/.aws` 等）+ tomcat 自身配置/凭据保护（`~/.tomcat/...`）
/// + Agent 数据目录可读不可写（`~/.tomcat/agents/*/{sessions,logs,audit}`）。
pub const BUILTIN_DEFAULT_PATH_RULES: &[(&str, PathRuleMode)] = &[
    // ── 凭据保护 ──
    // 不含 glob 字符 → 走前缀匹配，自动覆盖目录内全部文件。
    ("~/.ssh", PathRuleMode::Deny),
    ("~/.aws", PathRuleMode::Deny),
    ("~/.gnupg", PathRuleMode::Deny),
    ("~/.config/gh", PathRuleMode::Deny),
    // ── tomcat 自身配置/凭据保护 ──
    ("~/.tomcat/tomcat.config.toml", PathRuleMode::Deny),
    ("~/.tomcat/credentials", PathRuleMode::Deny),
    // 含 glob 字符 → globset 严格匹配整条路径；目录类规则需同时显式覆盖
    // 目录本身（`*/sessions`）和子内容（`*/sessions/**`），否则 `sessions/foo.jsonl`
    // 无法命中。auth-profiles 文件级规则用 `*` 通配符能直接匹配文件。
    (
        "~/.tomcat/agents/*/agent/auth-profiles*.json",
        PathRuleMode::Deny,
    ),
    ("~/.tomcat/agents/*/agent/credentials", PathRuleMode::Deny),
    (
        "~/.tomcat/agents/*/agent/credentials/**",
        PathRuleMode::Deny,
    ),
    // ── Agent 历史与审计：可读不可写 ──
    ("~/.tomcat/agents/*/sessions", PathRuleMode::Readonly),
    ("~/.tomcat/agents/*/sessions/**", PathRuleMode::Readonly),
    ("~/.tomcat/agents/*/logs", PathRuleMode::Readonly),
    ("~/.tomcat/agents/*/logs/**", PathRuleMode::Readonly),
    ("~/.tomcat/agents/*/audit", PathRuleMode::Readonly),
    ("~/.tomcat/agents/*/audit/**", PathRuleMode::Readonly),
];

/// 把 `BUILTIN_DEFAULT_PATH_RULES` 转成 `Vec<PathRule>`。
pub fn builtin_default_rules() -> Vec<PathRule> {
    BUILTIN_DEFAULT_PATH_RULES
        .iter()
        .map(|(p, m)| PathRule::new(p.to_string(), *m))
        .collect()
}

// ───────────────────────────────────────────────────────────────────────────
// Bash 默认值
// ───────────────────────────────────────────────────────────────────────────

/// 命中即整命令拒绝（**不可被用户配置弱化**）。
///
/// 三大类：
/// 1. 系统级灾难命令（rm -rf /、mkfs、dd、fork bomb 等）
/// 2. 凭据泄漏防护（echo $XX_KEY、cat ~/.ssh/ 等）
/// 3. **Agent 自我提权防护**（tomcat config set、>~/.tomcat/...、sed -i ~/.tomcat/）
pub const BUILTIN_BASH_FORBIDDEN: &[&str] = &[
    // ── §4.1 系统级灾难命令 ──
    r#"^\s*rm\s+-[rRfF]+\s+(/|/\*|~|\$HOME)\s*$"#,
    r#"^\s*mkfs(\.[a-z0-9]+)?\b"#,
    r#"^\s*dd\b.*if=.*/dev/.*\bof=/dev/"#,
    r#"^\s*:\(\)\s*\{\s*:\|:&\s*\}"#,
    r#"^\s*chmod\s+-R\s+777\s+/"#,
    r#"^\s*find\s+/.*-(delete|exec\s+rm)"#,
    r#"^\s*(shutdown|reboot|halt|poweroff)\b"#,
    r#">\s*/dev/(sd|disk|nvme)"#,
    // ── §4.2 凭据泄漏防护 ──
    r#"echo\s+\$.*KEY"#,
    r#"printenv\s+.*KEY"#,
    r#"env\s*\|.*grep.*KEY"#,
    r#"cat\s+~/.ssh/"#,
    r#"cat\s+~/.aws/"#,
    r#"cat\s+~/.gnupg/"#,
    // ── §4.3 Agent 自我提权防护（**最高优先级**） ──
    r#"^\s*tomcat\s+config\s+set\b"#,
    r#"^\s*tomcat\s+config\s+edit\b"#,
    r#"^\s*tomcat\s+pathrules\s+(add|remove|clear-session)\b"#,
    r#"^\s*tomcat\s+workspace\s+(add|remove)\b"#,
    r#">\s*~/\.tomcat/tomcat\.config\.toml"#,
    r#">\s*~/\.tomcat/credentials"#,
    r#"^\s*(sed|awk|perl)\b.*-i.*~/\.tomcat/"#,
];

/// 命中弹 confirm（**不可被用户弱化**）。
pub const BUILTIN_BASH_APPROVAL_REQUIRED: &[&str] = &[
    r#"\brm\s+-[rRfF]+\b"#,
    r#"^\s*sudo\s+"#,
    r#"\bchmod\s+-R\b"#,
    r#"\bchown\s+-R\b"#,
    r#"\bgit\s+push\s+(--force|-f)\b"#,
    r#"\bgit\s+reset\s+--hard\b"#,
    r#"\bgit\s+clean\s+-f"#,
    r#"\b(npm|cargo|pip)\s+publish\b"#,
    r#"\|\s*(sh|bash|zsh)\b"#,
    r#">\s*(/etc/|/usr/|~/.ssh/|~/.aws/|~/.gnupg/)"#,
];
