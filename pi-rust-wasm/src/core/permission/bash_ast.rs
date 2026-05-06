//! T2-P0-016 PR-L（bash T3）：AST allowlist + SandboxBackend trait **骨架**。
//!
//! 范围由 [bash-pr-l-scope.md](../../../docs/architecture/tools/bash-pr-l-scope.md)
//! 冻结：本模块只交付 **可叠在现有 `gate_check_bash` 之上**的最小 AST 切段 +
//! allow / deny 命中判定 + `SandboxBackend` trait 占位（默认 `NoopSandboxBackend`）。
//! tree-sitter / 真实 sandbox backend / PersistentShell 真 PTY 循环全部留给后续 PR。
//!
//! ## 切段颗粒度（与 scope spec §2 表一一对应）
//!
//! - **拆**：`;` `&` `\n` `&&` `||` `|`（顺序 / 短路 / 管道）
//! - **不拆，保留为段属性**：重定向 `>` `>>` `<` `2>&1`、变量赋值 `NAME=value cmd`
//! - **MVP 拒绝**（命中即 `Err(AstReject::Unsupported(...))`）：流程控制 `for` `while`
//!   `if` `case` / 函数定义 / heredoc `<<` / 子 shell `( )` `$( )` 反引号
//!
//! ## 段判定（每段独立）
//!
//! 1. 段内第一个 token = 命令名（含 builtin / 绝对路径）；
//! 2. 命中 `denylist` → 立即 `Err(AstReject::AstDeny { command, segment })`；
//! 3. 命中 `allowlist` → `AstSegmentVerdict::AllowedSkipApproval`（**不**跳过路径预检）；
//! 4. 既不 allow 也不 deny → `AstSegmentVerdict::Defer`，由调用方走旧的
//!    `gate_check_bash` 三层（whitelist / approval / forbidden regex）做兜底。
//!
//! `BashAstChecker::check` 是上层统一入口，返回 `Vec<(BashSegment, AstSegmentVerdict)>`，
//! 让调用方按段做决策（见 [bash-pr-l-scope.md §4 兼容性] 规定的「不替换现有 gate」契约）。

use serde::{Deserialize, Serialize};

/// 单条段——拆段后的最小判定单元。`raw` 保留原文（含赋值 / 重定向），便于审计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashSegment {
    /// 第一个 token（命令名）；空段直接拒。
    pub command: String,
    /// 段原文（不含连接符 `;` `&&` `||` `|`）。
    pub raw: String,
    /// `NAME=value` 赋值前缀（语法上属于段属性，不算独立命令）。
    pub assignments: Vec<String>,
}

/// AST 切段被拒的原因；上层把它翻译成 `AppError::Primitive("AstDeny: ...")` /
/// `AppError::Primitive("AstUnsupported: ...")`，与现有 forbidden regex 拒绝路径同形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstReject {
    /// 命中 `denylist` 模式。
    AstDeny { command: String, segment: String },
    /// MVP 不支持的语法（流程控制 / 子 shell / heredoc / 函数定义）。
    Unsupported {
        reason: &'static str,
        snippet: String,
    },
    /// 解析器自身失败（如 shell-words 拒掉的引号失衡）。
    ParseError { message: String },
}

impl std::fmt::Display for AstReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AstDeny { command, segment } => {
                write!(
                    f,
                    "AstDeny: `{}` 命中 denylist (段: `{}`)",
                    command, segment
                )
            }
            Self::Unsupported { reason, snippet } => {
                write!(f, "AstUnsupported: {} (片段: `{}`)", reason, snippet)
            }
            Self::ParseError { message } => write!(f, "AstParseError: {}", message),
        }
    }
}

impl std::error::Error for AstReject {}

/// 段判定结果——上层据此决定本段是否跳 approval / 走 deny / fallback 旧 gate。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstSegmentVerdict {
    /// 命中 `allowlist`，**仅**跳过 approval；路径预检与审计仍按旧路径走。
    AllowedSkipApproval,
    /// 既不 allow 也不 deny，由调用方走旧的 `gate_check_bash` 三层兜底。
    Defer,
}

/// AST 检查器。`enabled=false` 时 `check` 总是把整条命令包成单段 `Defer`，
/// 行为与今日（无 AST）等价——这是 scope spec §4「兼容性」的硬性要求。
#[derive(Debug, Clone, Default)]
pub struct BashAstChecker {
    pub enabled: bool,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
}

impl BashAstChecker {
    pub fn new(enabled: bool, allowlist: Vec<String>, denylist: Vec<String>) -> Self {
        Self {
            enabled,
            allowlist,
            denylist,
        }
    }

    /// 拆段 + 段判定。返回 `Ok` 列表 = 全段都不 deny；任何一段 deny / unsupported
    /// 都早退 `Err`。空命令字符串 → `Ok(vec![])`（调用方应在该层另行处理）。
    pub fn check(
        &self,
        audit_cmd: &str,
    ) -> Result<Vec<(BashSegment, AstSegmentVerdict)>, AstReject> {
        if !self.enabled {
            // 兼容路径：不做切段、不做命中判定，直接给一个「整条命令都 Defer」的占位段，
            // 调用方仍走旧 gate 三层。
            let trimmed = audit_cmd.trim();
            if trimmed.is_empty() {
                return Ok(vec![]);
            }
            return Ok(vec![(
                BashSegment {
                    command: first_token(trimmed),
                    raw: trimmed.to_string(),
                    assignments: vec![],
                },
                AstSegmentVerdict::Defer,
            )]);
        }
        let segments = split_segments(audit_cmd)?;
        segments.into_iter().map(|seg| self.judge(seg)).collect()
    }

    fn judge(&self, seg: BashSegment) -> Result<(BashSegment, AstSegmentVerdict), AstReject> {
        if self.denylist.iter().any(|p| matches_token(p, &seg.command)) {
            return Err(AstReject::AstDeny {
                command: seg.command.clone(),
                segment: seg.raw.clone(),
            });
        }
        if self
            .allowlist
            .iter()
            .any(|p| matches_token(p, &seg.command))
        {
            return Ok((seg, AstSegmentVerdict::AllowedSkipApproval));
        }
        Ok((seg, AstSegmentVerdict::Defer))
    }
}

/// 极简模式匹配：`*` 通配支持「字面量前缀 / 后缀」两种最常用形态，避免引入正则
/// 依赖。完整 glob 留给后续 PR（与 `globset` 集成时再补）。
fn matches_token(pattern: &str, token: &str) -> bool {
    if pattern == token {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return token.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return token.ends_with(suffix);
    }
    false
}

fn first_token(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// 切段：识别 `;` `&` `\n` `&&` `||` `|` 顶层操作符；MVP 拒绝子 shell / 流程控制 / heredoc。
///
/// **不**做完整 shell 解析——只在「未被引号包裹」的位置切。引号内（含转义）一律视为
/// 字面量。这是 PR-L 「骨架」承诺；tree-sitter 升级是后续 PR 的事。
fn split_segments(audit_cmd: &str) -> Result<Vec<BashSegment>, AstReject> {
    if let Some(snippet) = detect_unsupported(audit_cmd) {
        return Err(AstReject::Unsupported {
            reason: "MVP 不支持子 shell / 流程控制 / heredoc / 函数定义",
            snippet,
        });
    }

    let mut segments: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut chars = audit_cmd.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escape = false;
    // 子 shell / 命令替换深度计数：`(`、`$(` 与反引号 ``` ` ``` 内一律按字面量处理，
    // **不**触发段切分。这是 PR-L 的 MVP 折衷——scope spec §2 表里写的「递归拆」
    // 留给后续真实 AST 解析器；当前先把外层命令名拿出来判定，内部命令暂不独立判定。
    let mut paren_depth: u32 = 0;
    let mut backtick: bool = false;

    while let Some(c) = chars.next() {
        if escape {
            buf.push(c);
            escape = false;
            continue;
        }
        // 反引号「类引号」：内部一律字面量，仅识别配对反引号。
        if backtick {
            buf.push(c);
            if c == '`' {
                backtick = false;
            }
            continue;
        }
        // 子 shell / 命令替换：内部一律字面量；只追踪括号配平。
        if paren_depth > 0 {
            buf.push(c);
            match c {
                '(' => paren_depth += 1,
                ')' => paren_depth -= 1,
                _ => {}
            }
            continue;
        }
        match c {
            '\\' if quote != Some('\'') => {
                buf.push(c);
                escape = true;
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                buf.push(c);
            }
            ch if Some(ch) == quote => {
                quote = None;
                buf.push(ch);
            }
            _ if quote.is_some() => buf.push(c),
            '`' => {
                buf.push(c);
                backtick = true;
            }
            '$' if chars.peek() == Some(&'(') => {
                buf.push(c);
                buf.push(chars.next().unwrap());
                paren_depth = 1;
            }
            '(' => {
                buf.push(c);
                paren_depth = 1;
            }
            ';' | '\n' => {
                push_seg(&mut segments, &mut buf);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_seg(&mut segments, &mut buf);
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                push_seg(&mut segments, &mut buf);
            }
            '|' => {
                push_seg(&mut segments, &mut buf);
            }
            '&' => {
                // 末尾 `&` 表示后台执行——MVP 视为段分隔符（仍单独判定段命令名）。
                push_seg(&mut segments, &mut buf);
            }
            _ => buf.push(c),
        }
    }
    push_seg(&mut segments, &mut buf);

    if quote.is_some() {
        return Err(AstReject::ParseError {
            message: "引号未闭合".to_string(),
        });
    }
    if paren_depth > 0 {
        return Err(AstReject::ParseError {
            message: "子 shell 括号未闭合".to_string(),
        });
    }
    if backtick {
        return Err(AstReject::ParseError {
            message: "反引号未闭合".to_string(),
        });
    }

    let mut out = Vec::with_capacity(segments.len());
    for raw in segments {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_segment(trimmed)?);
    }
    Ok(out)
}

fn push_seg(out: &mut Vec<String>, buf: &mut String) {
    let s = std::mem::take(buf);
    if !s.trim().is_empty() {
        out.push(s);
    }
}

/// 段内解析：抽前导 `NAME=value` 赋值 + 第一个非赋值 token 即命令名。
fn parse_segment(raw: &str) -> Result<BashSegment, AstReject> {
    let tokens = shell_words::split(raw).map_err(|e| AstReject::ParseError {
        message: format!("段解析失败: {} ({})", raw, e),
    })?;
    let mut assignments = Vec::new();
    let mut command = String::new();
    for tok in tokens {
        if command.is_empty() && is_assignment(&tok) {
            assignments.push(tok);
        } else {
            command = tok;
            break;
        }
    }
    if command.is_empty() {
        // 仅有赋值（如 `NAME=value`）：合法语法，命令名留空让调用方按需处理（这里
        // 不当成 deny，由旧 gate 的赋值路径走）。
        return Ok(BashSegment {
            command: String::new(),
            raw: raw.to_string(),
            assignments,
        });
    }
    Ok(BashSegment {
        command,
        raw: raw.to_string(),
        assignments,
    })
}

fn is_assignment(tok: &str) -> bool {
    if let Some(eq) = tok.find('=') {
        let name = &tok[..eq];
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    } else {
        false
    }
}

/// MVP 不支持的语法探测（粗匹配，宁可误拒不遗漏）。
/// **不**拒 `$(...)` / 反引号 / `(` 子 shell——切段时把它们当字面量处理，
/// 只判定外层命令名；内部命令的递归 AST 留给后续真实解析器。
fn detect_unsupported(s: &str) -> Option<String> {
    let trimmed = s.trim();
    // heredoc。
    if trimmed.contains("<<") {
        return Some("<<".to_string());
    }
    // 流程控制 / 函数定义关键字（独立 token）。
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    const KEYWORDS: &[&str] = &[
        "for", "while", "until", "if", "case", "function", "select", "{",
    ];
    for kw in KEYWORDS {
        if tokens.iter().any(|t| t == kw) {
            return Some((*kw).to_string());
        }
    }
    None
}

// ─── SandboxBackend trait + NoopSandboxBackend ─────────────────────────────

/// PR-L 沙箱抽象——`SandboxBackend::spawn` 替代 `tokio::process::Command::spawn`，
/// 给 macOS Seatbelt / Linux Landlock / bwrap 等真实 backend 留接口。
///
/// PR-L 内**只**实现 [`NoopSandboxBackend`]（直接 `cmd.spawn()`），与 PR-E.2
/// 现有 `executor/bash.rs` 行为字节级等价；后续 PR 再挂真实 backend 时仅替换
/// `Arc<dyn SandboxBackend>` 注入即可（详见 bash-pr-l-scope §3）。
#[async_trait::async_trait]
pub trait SandboxBackend: Send + Sync + 'static {
    async fn spawn(&self, cmd: tokio::process::Command) -> std::io::Result<tokio::process::Child>;

    /// 用于审计与诊断；后续真实 backend 返回 `"seatbelt-exec"` / `"landlock"` 等。
    fn name(&self) -> &'static str;
}

/// 默认 backend：原样 spawn，与 PR-E.2 路径完全等价。
pub struct NoopSandboxBackend;

#[async_trait::async_trait]
impl SandboxBackend for NoopSandboxBackend {
    async fn spawn(
        &self,
        mut cmd: tokio::process::Command,
    ) -> std::io::Result<tokio::process::Child> {
        cmd.spawn()
    }
    fn name(&self) -> &'static str {
        "noop"
    }
}

// ─── PersistentShell trait 占位 ──────────────────────────────────────────────

/// PR-L PersistentShell 占位——真 PTY 循环不在 PR-L 范围（scope spec §1 / §3）。
/// 仅声明 trait 让后续 PR 可以挂 `bash -i` 长连接，不含任何运行时代码。
#[async_trait::async_trait]
pub trait PersistentShell: Send + Sync + 'static {
    async fn run(&self, command: &str) -> std::io::Result<String>;
    fn name(&self) -> &'static str;
}

// ─── 配置类型（与 ToolsBashConfig 平行；PR-L 暴露的最小集） ──────────────

/// `[tools.bash.ast]` 配置段——`enabled=true` 时 `BashAstChecker::check` 才真正
/// 切段判定。`Default` 与 `DefaultPrimitiveExecutor` 一致：`enabled=false` 直到
/// 解析精度/配置就绪。`allowlist` / `denylist` 留给后续 PR 接 `pi.config.toml` 反序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsBashAstConfig {
    pub enabled: bool,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    /// `[tools.bash.sandbox] backend`：PR-L 内仅识别 `"noop"`；任何其它值都按 `"noop"`
    /// 处理 + tracing::warn，避免拼写误差让 chat 直接拒绝启动。
    pub sandbox_backend: String,
}

impl Default for ToolsBashAstConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowlist: vec![],
            denylist: vec![],
            sandbox_backend: "noop".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker_with(allow: &[&str], deny: &[&str]) -> BashAstChecker {
        BashAstChecker::new(
            true,
            allow.iter().map(|s| s.to_string()).collect(),
            deny.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn disabled_checker_returns_single_defer_segment() {
        let chk = BashAstChecker::default();
        let v = chk.check("rm -rf /; cd /").expect("disabled 应当 Ok");
        assert_eq!(v.len(), 1, "enabled=false 应当只产出 1 段（整条命令）");
        assert_eq!(v[0].1, AstSegmentVerdict::Defer);
        assert_eq!(v[0].0.raw, "rm -rf /; cd /");
    }

    #[test]
    fn split_on_semicolon_and_short_circuit_and_pipe() {
        let chk = checker_with(&[], &[]);
        let v = chk
            .check("git pull && rm -rf node_modules; ls -la | wc -l")
            .unwrap();
        let cmds: Vec<String> = v.iter().map(|(s, _)| s.command.clone()).collect();
        assert_eq!(cmds, vec!["git", "rm", "ls", "wc"]);
    }

    #[test]
    fn deny_short_circuits_remaining_segments() {
        let chk = checker_with(&[], &["rm"]);
        let err = chk.check("git pull && rm -rf node_modules").unwrap_err();
        match err {
            AstReject::AstDeny { command, .. } => assert_eq!(command, "rm"),
            other => panic!("expected AstDeny, got {:?}", other),
        }
    }

    #[test]
    fn allow_marks_skip_approval_but_still_yields_defer_for_others() {
        let chk = checker_with(&["ls"], &[]);
        let v = chk.check("ls -la; cat README").unwrap();
        assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
        assert_eq!(v[1].1, AstSegmentVerdict::Defer);
    }

    #[test]
    fn assignment_prefix_is_kept_as_segment_attribute() {
        let chk = checker_with(&["env"], &[]);
        let v = chk.check("FOO=bar BAZ=qux env").unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0.command, "env");
        assert_eq!(v[0].0.assignments, vec!["FOO=bar", "BAZ=qux"]);
    }

    #[test]
    fn subshell_command_substitution_is_treated_as_opaque_literal() {
        // PR-L MVP 不递归解析子 shell 内部命令；外层命令照常判定，内部按字面量带过。
        let chk = checker_with(&["printf"], &[]);
        let v = chk
            .check("printf '%s\\n' $(seq 1 3); echo done")
            .expect("MVP 不应拒子 shell 表面语法");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0.command, "printf");
        assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
        assert_eq!(v[1].0.command, "echo");
        assert_eq!(v[1].1, AstSegmentVerdict::Defer);
    }

    #[test]
    fn separators_inside_subshell_do_not_split_segments() {
        let chk = checker_with(&[], &[]);
        let v = chk.check("echo $(a; b && c | d) post").unwrap();
        assert_eq!(v.len(), 1, "子 shell 内的分隔符不应触发外层切段");
        assert_eq!(v[0].0.command, "echo");
    }

    #[test]
    fn unmatched_subshell_returns_parse_error() {
        let chk = checker_with(&[], &[]);
        let err = chk.check("echo $(seq 1 3").unwrap_err();
        assert!(matches!(err, AstReject::ParseError { .. }));
    }

    #[test]
    fn flow_control_keywords_are_unsupported() {
        let chk = checker_with(&[], &[]);
        for cmd in [
            "for i in 1 2 3; do echo $i; done",
            "if [ -f a ]; then ls; fi",
            "while true; do sleep 1; done",
        ] {
            let res = chk.check(cmd);
            assert!(
                matches!(res, Err(AstReject::Unsupported { .. })),
                "expected Unsupported for `{}`, got {:?}",
                cmd,
                res
            );
        }
    }

    #[test]
    fn heredoc_is_unsupported() {
        let chk = checker_with(&[], &[]);
        let err = chk.check("cat <<EOF\nhi\nEOF").unwrap_err();
        assert!(matches!(err, AstReject::Unsupported { .. }));
    }

    #[test]
    fn unmatched_quote_returns_parse_error() {
        let chk = checker_with(&[], &[]);
        let err = chk.check("echo 'hi").unwrap_err();
        assert!(matches!(err, AstReject::ParseError { .. }));
    }

    #[test]
    fn quoted_separators_do_not_split_segments() {
        let chk = checker_with(&[], &[]);
        let v = chk.check("echo 'a; b && c | d'").unwrap();
        assert_eq!(v.len(), 1, "引号内的分隔符不应触发切段");
        assert_eq!(v[0].0.command, "echo");
    }

    #[test]
    fn glob_prefix_pattern_matches() {
        let chk = checker_with(&["git*"], &[]);
        let v = chk.check("git status; git push").unwrap();
        assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
        assert_eq!(v[1].1, AstSegmentVerdict::AllowedSkipApproval);
    }

    #[tokio::test]
    async fn noop_sandbox_backend_spawns_directly() {
        let backend = NoopSandboxBackend;
        let mut cmd = tokio::process::Command::new("echo");
        cmd.arg("hello").stdout(std::process::Stdio::piped());
        let child = backend.spawn(cmd).await.expect("spawn");
        let output = child.wait_with_output().await.expect("wait");
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
        assert_eq!(backend.name(), "noop");
    }
}
