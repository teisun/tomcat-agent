//! # 4+1 原语执行引擎（DefaultPrimitiveExecutor）
//!
//! 实现 [`super::PrimitiveExecutor`] trait，是 Agent 与文件系统 / Shell
//! 的 **唯一受信通道**：任何 LLM 工具调用最终都要落到这 5 个方法上，安全策略
//! （`PermissionGate` 三层决策 / 用户确认 / 备份 / 原子写 / 审计）全部在此横切。
//!
//! ## 5 个原语 + 共享安全流水
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  入口（trait 方法）                                                       │
//! │   ├─ read_file(path, plugin_id)                                          │
//! │   ├─ list_dir(path, plugin_id)                                           │
//! │   ├─ write_file(path, content, plugin_id, ...)                           │
//! │   ├─ edit_file(path, operations, plugin_id, ...)                         │
//! │   └─ execute_bash(cmd, args, plugin_id, ...)                             │
//! └─────────────────────────────────────────────────────────────────────────┘
//!    │
//!    │ ① 路径规范化（infra::platform::normalize_path）：~ 展开、symlink 还原、
//!    │   Windows 反斜杠归一。
//!    │
//!    │ ② 路径授权（统一走 PermissionGate）
//!    │   gate.check(op, normalized_path)
//!    │     ├─ Allow            ► 继续
//!    │     ├─ Deny             ► AppError::Permission(原因)
//!    │     └─ NeedConfirm      ► confirmation.confirm_decision(...)
//!    │                            ├─ AllowOnce → grant_session → 重试
//!    │                            ├─ AllowAndPersistRoot → grant_session → 重试
//!    │                            └─ Deny → AppError::Permission(用户拒绝)
//!    │
//!    │ ③ Bash 命令决策（execute_bash 专属）
//!    │   gate.check_bash(audit_cmd) 同样三态；命中后再用 `bash_parser::extract_paths`
//!    │   把命令里出现的路径逐一交给 gate.check(Bash) 做路径级预检
//!    │   （layer-1 deny / layer-2 confirm）。
//!    │
//!    │ ④ 业务执行（按方法分支）
//!    │   read_file    ► 大小预检（≤ MAX_READ_BYTES=10MB）► read_file_utf8
//!    │   list_dir     ► std::fs::read_dir → DirEntry 列表
//!    │   write_file   ► （可选）备份原文件 ► write_file_atomic（写临时 + rename）
//!    │   edit_file    ► load → apply EditOperation* → diff → 备份 ► atomic 写
//!    │   execute_bash ► tokio::process::Command spawn
//!    │
//!    │ ⑤ 审计落库（无论 Ok / Err）
//!    │   audit.record_primitive(PrimitiveAuditEntry {
//!    │     plugin_id, op: AuditPrimitiveOp::*, success, detail,
//!    │     permission_scope, grant_type, grant_trigger, ...
//!    │   })
//!    ▼
//!   Result<T, AppError>
//! ```
//!
//! ## 横切配置
//!
//! - `PrimitiveConfig`（来自 [`crate::infra::PrimitiveConfig`]）：bash 禁止 / 审批
//!   regex 与备份 / `auto_confirm` 策略；路径规则已迁入 `PermissionGate`。
//! - `MAX_READ_BYTES = 10 MiB`：read_file 单次读上限，防 OOM。
//! - `BASH_TIMEOUT_SECS = 30`：bash 默认超时（当前预留，后续接 `tokio::time::timeout`）。
//! - `gate: Arc<dyn PermissionGate>`：构造期强制注入，承担路径 / bash / 审计来源
//!   的全部决策；不存在「未注入 gate」的执行路径。
//!
//! ## 与同族子模块的边界
//!
//! - `super::diff::build_simple_diff`：edit_file 的 diff 文本生成。
//! - `super`：原语 trait / 类型与用户确认 trait。
//! - 调用方：`agent_loop::tool_exec::execute_tool` 是唯一直接调用方，所有
//!   LLM 工具调用都从那里 dispatch 进来。

use crate::core::permission::{
    GrantTrace, GrantTrigger, GrantType, PathRule, PathRuleMode, PermissionDecision,
    PermissionGate, PermissionScope,
};
use crate::core::tools::primitive::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, ReadBinaryResult, ReadResult, ReadTextResult, SearchFileCount,
    SearchFileMatch, SearchFilesArgs, SearchFilesOutput, SearchFilesOutputMode, SearchFilesQuery,
    SearchFilesResultMode, SearchFilesStats, SearchFilesTarget, WriteFileResult,
};
use crate::core::tools::primitive::{ConfirmDecision, UserConfirmationProvider};
use crate::infra::audit::{AuditPrimitiveOp, AuditRecorder, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::{normalize_path, read_file_utf8, write_file_atomic};
use crate::infra::PrimitiveConfig;
use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{DirEntry as IgnoreEntry, WalkBuilder};
use regex::RegexBuilder;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

use super::diff::build_simple_diff;

/// 单次读取文件最大字节数，避免 OOM。
///
/// PR-RB（T1）将上限从历史 10 MiB 提升到 **25 MiB**，介于 cc-fork 256 KiB 与
/// pi_agent_rust 100 MiB 之间——兼顾「合理 dump 文件」与「防爆 ctx」。
/// 详见 `openspec/specs/architecture/tools/read.md` §2.5 决策表 R6 #2。
///
/// **作用范围**：仅在 [`DefaultPrimitiveExecutor::read`] 的「无 `offset` / 无 `limit`」
/// 路径生效（`metadata.len() > MAX_READ_BYTES` → 拒绝并提示加 offset/limit 重试）。
/// 传入分窗时该上限被绕过——大日志可被分窗取「特定窗口」。
///
/// 默认值与 [`crate::infra::DEFAULT_TOOLS_READ_MAX_BYTES`] 保持一致；
/// 可通过 [`DefaultPrimitiveExecutor::with_read_max_bytes`] 覆盖（生产由
/// `[tools.read] max_bytes` config 注入，测试用于做小阈值快速覆盖）。
const MAX_READ_BYTES: u64 = 25 * 1024 * 1024; // 25 MiB

/// PR-RB（T1）流式分块读的固定 buffer 大小。
///
/// 64 KiB 是 wasm 友好的小块（堆压力低），同时与典型 page cache 命中粒度对齐；
/// 加大没有明显收益，加小会让 syscall 数量过多。
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// PR-RB（T1）默认 limit（行数），与 cc-fork `MAX_LINES_TO_READ` 对齐。
const READ_DEFAULT_LIMIT_LINES: u64 = 2000;
/// bash 执行默认超时（预留，后续可配合 tokio::time::timeout 使用）。
#[allow(dead_code)]
const BASH_TIMEOUT_SECS: u64 = 30;
const SEARCH_CONTENT_TIMEOUT_SECS: u64 = 5;
const SEARCH_FILES_TIMEOUT_SECS: u64 = 60;
/// Tier2 单查询墙钟（plan §7.3 冻结值：10s）。
const SEARCH_FALLBACK_TIMEOUT_SECS: u64 = 10;
const SEARCH_CONTENT_DEFAULT_LIMIT: usize = 64;
const SEARCH_FILES_DEFAULT_LIMIT: usize = 128;
const SEARCH_LIMIT_HARD_CAP: usize = 1024;
/// Tier2 单文件大小阈值（plan §7.3 冻结值：5 MiB），超过则跳过并 warning。
const SEARCH_FALLBACK_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Tier2 二进制嗅探读取的字节数；命中 NUL 即判定为二进制文件并跳过（T9）。
const SEARCH_FALLBACK_BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// 环境变量：覆盖 Tier2 单查询墙钟（毫秒），便于 CI/性能调优（plan §5.6）。
const SEARCH_FALLBACK_DEADLINE_ENV: &str = "PI_SEARCH_TIER2_DEADLINE_MS";

/// 4 原语执行引擎默认实现：路径权限、用户确认、备份、原子化与审计。
///
/// **权限模型**：构造期强制注入 [`PermissionGate`]；路径 / bash / 审计来源
/// 全部走 gate 三层决策。无 legacy fallback 通道。
pub struct DefaultPrimitiveExecutor {
    config: PrimitiveConfig,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    /// 路径与 bash 权限决策入口；由调用方注入并与
    /// `permission::cwd_lazy` / `tools::config` 共享同一份 `SessionGrants` 视图。
    gate: Arc<dyn PermissionGate>,
    /// PR-RB（T1）read 工具文本路径的「裸读字节上限」。
    ///
    /// 默认 [`MAX_READ_BYTES`]（25 MiB）；可由
    /// [`Self::with_read_max_bytes`] 覆盖。仅当模型未传 `offset`/`limit` 时生效。
    read_max_bytes: u64,
}

impl DefaultPrimitiveExecutor {
    pub fn new(
        config: PrimitiveConfig,
        confirmation: Arc<dyn UserConfirmationProvider>,
        audit: Arc<dyn AuditRecorder>,
        gate: Arc<dyn PermissionGate>,
    ) -> Self {
        Self {
            config,
            confirmation,
            audit,
            gate,
            read_max_bytes: MAX_READ_BYTES,
        }
    }

    /// PR-RB（T1）覆盖 read 工具文本路径的字节上限。
    ///
    /// **生产路径**：由 `[tools.read] max_bytes` config 在 `api/chat` 装配
    /// `DefaultPrimitiveExecutor` 时调用（后续 PR 接线）。
    /// **测试路径**：用极小阈值（如 64 字节）让 fixture 文件触发拒绝分支，
    /// 避免单测生成 25 MiB+ 的临时文件。
    pub fn with_read_max_bytes(mut self, bytes: u64) -> Self {
        self.read_max_bytes = bytes;
        self
    }

    // ─────────────────────────────────────────────────────────────────────
    // PermissionGate 桥接（gate 始终存在）
    // ─────────────────────────────────────────────────────────────────────

    /// 经 gate 决定一个原语对路径的访问，必要时弹 confirm 完成 layer-2。
    ///
    /// 返回 `Ok((path_buf, scope, grant))` 表示放行；
    /// `Err(AppError::Permission)` 表示被 gate 拒绝或用户拒绝 confirm。
    async fn gate_check_path(
        &self,
        op: PrimitiveOperation,
        path: &str,
        plugin_id: &str,
    ) -> Result<(PathBuf, PermissionScope, GrantTrace), AppError> {
        let gate = &self.gate;
        let normalized = normalize_path(path)?;
        loop {
            let decision = gate.check(op, &normalized.to_string_lossy())?;
            match decision {
                PermissionDecision::Allow { grant, scope } => {
                    return Ok((normalized, scope, grant))
                }
                PermissionDecision::Deny { reason } => {
                    return Err(AppError::Permission(reason));
                }
                PermissionDecision::NeedConfirm {
                    reason,
                    suggested_root,
                } => {
                    let preview = format!(
                        "[{:?}] {}\n路径: {}\n原因: {}",
                        op,
                        op_summary(op),
                        normalized.display(),
                        reason
                    );
                    let dec = self
                        .confirmation
                        .confirm_decision(op, &preview, plugin_id, suggested_root.clone())
                        .await?;
                    match dec {
                        ConfirmDecision::Deny => {
                            return Err(AppError::Permission(format!(
                                "用户拒绝授权: {}。下次工具再次访问该路径时会重新弹出 [s]/[w]/[c] 授权选项；也可以执行 `pi workspace add {}` 一次性永久授权。",
                                normalized.display(),
                                normalized.display()
                            )));
                        }
                        ConfirmDecision::AllowOnce => {
                            // 落 SessionGrants：AllowOnce 授权当前目标路径本身。
                            gate.grant_session(normalized.clone(), GrantTrigger::UserConfirm);
                            // 重新 check：现在应该 Allow。
                            continue;
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            // 1) 同时落 SessionGrants（本会话生效）。
                            gate.grant_session(root.clone(), GrantTrigger::UserConfirm);
                            // 2) 持久化由 caller（CLI confirm 实现）负责调用
                            //    `pi workspace add` 等价的 append_workspace_root_to_disk；
                            //    这里只标记会话授权，避免和 disk 写入耦合。
                            //    重新 check 应 Allow。
                            continue;
                        }
                    }
                }
            }
        }
    }

    /// 经 gate 决定一条 bash 命令是否放行；layer-2 命中弹 confirm。
    async fn gate_check_bash(
        &self,
        command: &str,
        plugin_id: &str,
    ) -> Result<(PermissionScope, GrantTrace), AppError> {
        let gate = &self.gate;
        let decision = gate.check_bash(command)?;
        match decision {
            PermissionDecision::Allow { grant, scope } => Ok((scope, grant)),
            PermissionDecision::Deny { reason } => Err(AppError::Permission(reason)),
            PermissionDecision::NeedConfirm { reason, .. } => {
                let preview = format!(
                    "[Bash] 危险命令命中确认列表\n命令: {}\n原因: {}",
                    command, reason
                );
                let dec = self
                    .confirmation
                    .confirm_decision(PrimitiveOperation::Bash, &preview, plugin_id, None)
                    .await?;
                match dec {
                    ConfirmDecision::AllowOnce | ConfirmDecision::AllowAndPersistRoot { .. } => {
                        Ok((
                            PermissionScope::BashApproval,
                            GrantTrace::new(GrantType::BashPolicy, GrantTrigger::UserConfirm),
                        ))
                    }
                    ConfirmDecision::Deny => {
                        Err(AppError::Permission("用户拒绝 bash 确认".to_string()))
                    }
                }
            }
        }
    }

    async fn run_search_command(
        &self,
        mut command: Command,
        timeout_secs: u64,
    ) -> Result<std::process::Output, AppError> {
        match tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(AppError::Primitive(e.to_string())),
            Err(_) => Err(AppError::Primitive(format!(
                "search_files timed out after {}s. Narrow path/glob or lower head_limit.",
                timeout_secs
            ))),
        }
    }
}

fn op_summary(op: PrimitiveOperation) -> &'static str {
    match op {
        PrimitiveOperation::Read => "读取",
        PrimitiveOperation::Write => "写入",
        PrimitiveOperation::Edit => "编辑",
        PrimitiveOperation::Bash => "执行命令",
    }
}

/// 把 [`PermissionScope`] 序列化为审计字符串（与 serde rename_all = snake_case 一致）。
fn permission_scope_str(scope: PermissionScope) -> String {
    match scope {
        PermissionScope::Read => "read",
        PermissionScope::Write => "write",
        PermissionScope::Bash => "bash",
        PermissionScope::BashApproval => "bash_approval",
        PermissionScope::Forbidden => "forbidden",
    }
    .to_string()
}

/// 把 [`GrantType`] 序列化为审计字符串。
fn grant_type_str(s: GrantType) -> String {
    match s {
        GrantType::AgentDefinitionDir => "agent_definition_dir",
        GrantType::AgentWorkspaceRoot => "agent_workspace_root",
        GrantType::SessionScope => "session_scope",
        GrantType::PathRuleReadOnly => "path_rule_read_only",
        GrantType::AgentTrailDir => "agent_trail_dir",
        GrantType::BashPolicy => "bash_policy",
    }
    .to_string()
}

/// 把 [`GrantTrigger`] 序列化为审计字符串。
fn grant_trigger_str(s: GrantTrigger) -> String {
    match s {
        GrantTrigger::BuiltinDefault => "builtin_default",
        GrantTrigger::WorkspaceRootsConfig => "workspace_roots_config",
        GrantTrigger::PathRulesConfig => "path_rules_config",
        GrantTrigger::BashRegexConfig => "bash_regex_config",
        GrantTrigger::UserConfirm => "user_confirm",
        GrantTrigger::CwdLazyPrompt => "cwd_lazy_prompt",
        GrantTrigger::DraggedPathMenu => "dragged_path_menu",
        GrantTrigger::AutoConfirmFlag => "auto_confirm_flag",
    }
    .to_string()
}

fn find_binary(candidates: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
            #[cfg(windows)]
            {
                let exe = dir.join(format!("{}.exe", candidate));
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}

fn resolve_search_limit(args: &SearchFilesArgs) -> Result<Option<usize>, AppError> {
    let limit = match args.head_limit {
        None => Some(match args.target {
            SearchFilesTarget::Content => SEARCH_CONTENT_DEFAULT_LIMIT,
            SearchFilesTarget::Files => SEARCH_FILES_DEFAULT_LIMIT,
        }),
        Some(None) => None,
        Some(Some(0)) => {
            return Err(AppError::Primitive(
                "search_files.head_limit must be 1..=1024 or null; 0 is not accepted".to_string(),
            ))
        }
        Some(Some(n)) if n > SEARCH_LIMIT_HARD_CAP => {
            return Err(AppError::Primitive(format!(
                "search_files.head_limit must be <= {}",
                SEARCH_LIMIT_HARD_CAP
            )))
        }
        Some(Some(n)) => Some(n),
    };
    Ok(limit)
}

fn search_root_and_arg(path: &Path) -> (PathBuf, String) {
    if path.is_dir() {
        return (path.to_path_buf(), ".".to_string());
    }
    let root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let arg = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    (root, arg)
}

fn parse_rg_match_line(line: &str) -> Option<SearchFileMatch> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?.to_string();
    let line_no = parts.next()?.parse::<u64>().ok()?;
    let _column = parts.next()?;
    let text = parts.next().unwrap_or("").to_string();
    Some(SearchFileMatch {
        path,
        line: line_no,
        text,
        before: Vec::new(),
        after: Vec::new(),
    })
}

fn parse_rg_count_line(line: &str) -> Option<SearchFileCount> {
    let (path, count) = line.rsplit_once(':')?;
    Some(SearchFileCount {
        path: path.to_string(),
        count: count.parse::<u64>().ok()?,
    })
}

fn paginate<T>(
    items: Vec<T>,
    offset: usize,
    limit: Option<usize>,
) -> (Vec<T>, bool, Option<usize>) {
    let total = items.len();
    let start = offset.min(total);
    let end = match limit {
        Some(limit) => (start + limit).min(total),
        None => total,
    };
    let truncated = end < total;
    let page = items.into_iter().skip(start).take(end - start).collect();
    (page, truncated, truncated.then_some(end))
}

fn absolute_result_path(root: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        root.join(p)
    }
}

fn filter_denied_files(
    root: &Path,
    files: Vec<String>,
    deny_rules: &[PathRule],
) -> (Vec<String>, usize) {
    let mut skipped = 0;
    let kept = files
        .into_iter()
        .filter(|path| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn filter_denied_matches(
    root: &Path,
    matches: Vec<SearchFileMatch>,
    deny_rules: &[PathRule],
) -> (Vec<SearchFileMatch>, usize) {
    let mut skipped = 0;
    let kept = matches
        .into_iter()
        .filter(|item| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, &item.path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn filter_denied_counts(
    root: &Path,
    counts: Vec<SearchFileCount>,
    deny_rules: &[PathRule],
) -> (Vec<SearchFileCount>, usize) {
    let mut skipped = 0;
    let kept = counts
        .into_iter()
        .filter(|item| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, &item.path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn search_files_query(
    args: &SearchFilesArgs,
    path: &Path,
    limit: Option<usize>,
    output_mode: Option<SearchFilesOutputMode>,
) -> SearchFilesQuery {
    SearchFilesQuery {
        pattern: args.pattern.clone(),
        target: args.target,
        path: path.to_string_lossy().into_owned(),
        glob: if args.target == SearchFilesTarget::Files {
            None
        } else {
            args.glob.clone()
        },
        file_type: if args.target == SearchFilesTarget::Files {
            None
        } else {
            args.file_type.clone()
        },
        output_mode,
        head_limit: limit,
        offset: args.offset,
        case_insensitive: if args.target == SearchFilesTarget::Files {
            false
        } else {
            args.case_insensitive
        },
        include_hidden: args.include_hidden,
    }
}

fn fallback_warning(warnings: &mut Vec<String>) {
    warnings.push(
        "implementation=tier2 rust-fallback; regex dialect is Rust regex and may differ from ripgrep; .gitignore/.ignore are respected by default"
            .to_string(),
    );
}

fn tier1_warning(warnings: &mut Vec<String>) {
    warnings.push("implementation=tier1 rg/fd".to_string());
}

fn normalize_rel_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn build_globset(pattern: &str) -> Result<GlobSet, AppError> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern).map_err(|e| AppError::Primitive(e.to_string()))?);
    if !pattern.contains('/') && !pattern.contains('\\') {
        let recursive = format!("**/{}", pattern);
        builder.add(Glob::new(&recursive).map_err(|e| AppError::Primitive(e.to_string()))?);
    }
    builder
        .build()
        .map_err(|e| AppError::Primitive(e.to_string()))
}

fn file_type_extension(file_type: &str) -> Option<&'static str> {
    match file_type.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some("rs"),
        "javascript" | "js" => Some("js"),
        "typescript" | "ts" => Some("ts"),
        "python" | "py" => Some("py"),
        "markdown" | "md" => Some("md"),
        "toml" => Some("toml"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yml"),
        _ => None,
    }
}

fn matches_file_type(path: &Path, file_type: Option<&str>, warnings: &mut Vec<String>) -> bool {
    let Some(file_type) = file_type else {
        return true;
    };
    let Some(expected) = file_type_extension(file_type) else {
        warnings.push(format!(
            "tier2 ignored unsupported file_type={}; filtering by type was not applied",
            file_type
        ));
        return true;
    };
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    if expected == "yml" {
        ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
    } else {
        ext.eq_ignore_ascii_case(expected)
    }
}

/// 计算 Tier2 单查询墙钟；`PI_SEARCH_TIER2_DEADLINE_MS` 可覆盖默认 10s。
fn fallback_deadline() -> Duration {
    if let Some(ms) = std::env::var(SEARCH_FALLBACK_DEADLINE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(SEARCH_FALLBACK_TIMEOUT_SECS)
}

/// 是否触达 Tier2 墙钟；命中后调用方应 `truncated=true` 并写 warning，不返回错误。
fn fallback_deadline_hit(started: Instant, deadline: Duration) -> bool {
    started.elapsed() >= deadline
}

fn fallback_timeout_warning(warnings: &mut Vec<String>, deadline: Duration) {
    warnings.push(format!(
        "tier2 wall-clock budget {}ms exhausted; result truncated. Override with {}=<ms> or narrow path/glob.",
        deadline.as_millis(),
        SEARCH_FALLBACK_DEADLINE_ENV
    ));
}

/// 嗅探文件前若干字节，命中 NUL 即视为二进制文件，配合大文件阈值过滤掉媒体/可执行文件。
fn is_binary_file(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; SEARCH_FALLBACK_BINARY_SNIFF_BYTES];
    match file.read(&mut buf) {
        Ok(n) => buf[..n].contains(&0),
        Err(_) => false,
    }
}

/// 用 `ignore::WalkBuilder` 列举授权根下的候选文件。
///
/// - 默认遵守 `.gitignore`/`.ignore`/`.git/info/exclude`。
/// - `filter_entry` 阶段对 deny 路径剪枝：拒绝目录直接不递归，避免越权 IO。
/// - 大文件 / 二进制文件直接跳过并写 warning（T9）。
fn collect_fallback_files(
    root: &Path,
    path: &Path,
    args: &SearchFilesArgs,
    deny_rules: &[PathRule],
    warnings: &mut Vec<String>,
) -> Result<Vec<(String, PathBuf)>, AppError> {
    let globset = match args.target {
        SearchFilesTarget::Files => Some(build_globset(&args.pattern)?),
        SearchFilesTarget::Content => args.glob.as_deref().map(build_globset).transpose()?,
    };
    let start = if path.is_file() { path } else { root };

    let mut builder = WalkBuilder::new(start);
    builder
        .standard_filters(true)
        .hidden(!args.include_hidden)
        .follow_links(false);
    let deny_clone = deny_rules.to_vec();
    let pruned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pruned_for_filter = Arc::clone(&pruned);
    builder.filter_entry(move |entry: &IgnoreEntry| {
        if deny_clone.iter().any(|rule| rule.matches(entry.path())) {
            pruned_for_filter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        true
    });

    let mut files = Vec::new();
    let mut skipped_large = 0usize;
    let mut skipped_binary = 0usize;
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let extension_filter = (args.target == SearchFilesTarget::Content)
            .then_some(args.file_type.as_deref())
            .flatten();
        if !matches_file_type(&abs, extension_filter, warnings) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&abs) {
            if meta.len() > SEARCH_FALLBACK_MAX_FILE_BYTES {
                skipped_large += 1;
                continue;
            }
        }
        if args.target == SearchFilesTarget::Content && is_binary_file(&abs) {
            skipped_binary += 1;
            continue;
        }
        let rel = abs.strip_prefix(root).unwrap_or(&abs).to_path_buf();
        let rel_str = normalize_rel_path(&rel);
        if globset
            .as_ref()
            .is_some_and(|globset| !globset.is_match(&rel_str))
        {
            continue;
        }
        files.push((rel_str, abs));
    }
    if skipped_large > 0 {
        warnings.push(format!(
            "tier2 skipped {} files larger than {} bytes",
            skipped_large, SEARCH_FALLBACK_MAX_FILE_BYTES
        ));
    }
    if skipped_binary > 0 {
        warnings.push(format!(
            "tier2 skipped {} binary files (NUL byte detected in first {} bytes)",
            skipped_binary, SEARCH_FALLBACK_BINARY_SNIFF_BYTES
        ));
    }
    let pruned_count = pruned.load(std::sync::atomic::Ordering::Relaxed);
    if pruned_count > 0 {
        warnings.push(format!(
            "skipped {} paths due to read deny (tier2 pruned at filter_entry)",
            pruned_count
        ));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Tier2 主入口：在阻塞线程上执行（由调用方包裹 `spawn_blocking`）。
///
/// 关键约束：
/// - 墙钟超时：截断 + warning，**不返回 Err**（plan §5.6）。
/// - regex 编译失败：返回空命中集 + warning，**不 panic / 不 Err**（T8 lookaround / back-ref）。
/// - deny 已在 `collect_fallback_files` 的 `filter_entry` 阶段剪枝；此处是叶子复检，避免越权 IO。
fn search_files_fallback(
    args: SearchFilesArgs,
    root: PathBuf,
    path: PathBuf,
    limit: Option<usize>,
    deny_rules: Vec<PathRule>,
    started: Instant,
) -> Result<SearchFilesOutput, AppError> {
    let mut warnings = Vec::new();
    fallback_warning(&mut warnings);
    let deadline = fallback_deadline();
    let candidates = collect_fallback_files(&root, &path, &args, &deny_rules, &mut warnings)?;

    match args.target {
        SearchFilesTarget::Files => {
            let files = candidates
                .into_iter()
                .map(|(rel, _)| rel)
                .collect::<Vec<_>>();
            let scanned = files.len();
            let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
            if skipped > 0 {
                warnings.push(format!("skipped {} paths due to read deny", skipped));
            }
            let (files, mut truncated, next_offset) = paginate(files, args.offset, limit);
            if fallback_deadline_hit(started, deadline) {
                truncated = true;
                fallback_timeout_warning(&mut warnings, deadline);
            }
            Ok(SearchFilesOutput {
                mode: SearchFilesResultMode::Files,
                query: search_files_query(&args, &path, limit, None),
                files: Some(files),
                matches: None,
                counts: None,
                stats: SearchFilesStats {
                    scanned_files: scanned,
                    elapsed_ms: started.elapsed().as_millis(),
                },
                truncated,
                next_offset,
                warnings,
            })
        }
        SearchFilesTarget::Content => {
            let regex = match RegexBuilder::new(&args.pattern)
                .case_insensitive(args.case_insensitive)
                .build()
            {
                Ok(re) => Some(re),
                Err(e) => {
                    warnings.push(format!(
                        "tier2 unsupported regex (likely lookaround/back-reference): {}; returning empty match set",
                        e
                    ));
                    None
                }
            };
            if args.context.is_some_and(|context| context > 0)
                && args.output_mode == SearchFilesOutputMode::Content
            {
                warnings.push(
                    "tier2 does not currently include before/after context lines".to_string(),
                );
            }
            let scanned = candidates.len();
            let mut deadline_tripped = false;
            match args.output_mode {
                SearchFilesOutputMode::FilesWithMatches => {
                    let mut files = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            if file_has_match(&abs, regex, &mut warnings)? {
                                files.push(rel);
                            }
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (files, mut truncated, next_offset) = paginate(files, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentFiles,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: Some(files),
                        matches: None,
                        counts: None,
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
                SearchFilesOutputMode::Count => {
                    let mut counts = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            let count = file_match_count(&abs, regex, &mut warnings)?;
                            if count > 0 {
                                counts.push(SearchFileCount { path: rel, count });
                            }
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (counts, mut truncated, next_offset) = paginate(counts, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentCount,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: None,
                        matches: None,
                        counts: Some(counts),
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
                SearchFilesOutputMode::Content => {
                    let mut matches = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            collect_file_matches(&rel, &abs, regex, &mut matches, &mut warnings)?;
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (matches, mut truncated, next_offset) =
                        paginate(matches, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentLines,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: None,
                        matches: Some(matches),
                        counts: None,
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
            }
        }
    }
}

fn file_has_match(
    path: &Path,
    regex: &regex::Regex,
    warnings: &mut Vec<String>,
) -> Result<bool, AppError> {
    Ok(file_match_count(path, regex, warnings)? > 0)
}

fn file_match_count(
    path: &Path,
    regex: &regex::Regex,
    warnings: &mut Vec<String>,
) -> Result<u64, AppError> {
    let mut count = 0;
    visit_text_lines(path, warnings, |_, line| {
        if regex.is_match(line) {
            count += 1;
        }
    })?;
    Ok(count)
}

fn collect_file_matches(
    rel: &str,
    path: &Path,
    regex: &regex::Regex,
    matches: &mut Vec<SearchFileMatch>,
    warnings: &mut Vec<String>,
) -> Result<(), AppError> {
    visit_text_lines(path, warnings, |line_no, line| {
        if regex.is_match(line) {
            matches.push(SearchFileMatch {
                path: rel.to_string(),
                line: line_no,
                text: line.trim_end_matches(['\r', '\n']).to_string(),
                before: Vec::new(),
                after: Vec::new(),
            });
        }
    })
}

fn visit_text_lines<F>(
    path: &Path,
    warnings: &mut Vec<String>,
    mut visitor: F,
) -> Result<(), AppError>
where
    F: FnMut(u64, &str),
{
    let meta = std::fs::metadata(path).map_err(AppError::Io)?;
    if meta.len() > SEARCH_FALLBACK_MAX_FILE_BYTES {
        warnings.push(format!(
            "tier2 skipped large file over {} bytes: {}",
            SEARCH_FALLBACK_MAX_FILE_BYTES,
            path.display()
        ));
        return Ok(());
    }
    let file = std::fs::File::open(path).map_err(AppError::Io)?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = Vec::new();
    let mut line_no = 1u64;
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).map_err(AppError::Io)?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        visitor(line_no, &line);
        line_no += 1;
    }
    Ok(())
}

/// PR-RJ（T3-b）`read` 工具的 mime 路由：扩展名 + 头几字节 magic 双重校验。
///
/// 仅返回 image / PDF 两类「需要走 inline content part 通道」的 mime；
/// 其他（包括 `.txt` 之外的扩展名 + 任何二进制 fallback）都返回 `None`，
/// 走文本 / 二进制 hint 路径（与 PR-RB §2.3 一致）。
///
/// **设计权衡**（详见 `read.md` §4.1 的「不引解码 / 缩放依赖」论述）：
/// - **不**引 `image` / `infer` 等 crate，`Cargo.lock` 零增长；
/// - 扩展名先行，magic 兜底——避免 `.png` 后缀挂着 PDF 字节这类小概率的误路由；
/// - PDF 的 magic 是 `%PDF-`（5 字节），PNG 是 `89 50 4E 47`，JPEG 是 `FF D8 FF`，
///   GIF 是 `47 49 46 38`，WebP 需要在 RIFF 头里看 `WEBP`（`52 49 46 46 .. .. .. .. 57 45 42 50`）。
pub(crate) fn detect_inline_mime(path: &Path) -> Option<DetectedInlineMime> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let ext = ext.as_deref()?;
    let candidate = match ext {
        "png" => Some(("image/png", InlineKind::Image)),
        "jpg" | "jpeg" => Some(("image/jpeg", InlineKind::Image)),
        "gif" => Some(("image/gif", InlineKind::Image)),
        "webp" => Some(("image/webp", InlineKind::Image)),
        "pdf" => Some(("application/pdf", InlineKind::Pdf)),
        _ => None,
    }?;
    // 头 12 字节足够覆盖以上所有 magic（最长是 WebP 的 RIFF + WEBP = 12 字节）。
    let head = read_head_bytes(path, 12).ok()?;
    if !magic_matches(candidate.0, &head) {
        return None;
    }
    Some(DetectedInlineMime {
        mime: candidate.0.to_string(),
        kind: candidate.1,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineKind {
    Image,
    Pdf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedInlineMime {
    pub(crate) mime: String,
    pub(crate) kind: InlineKind,
}

fn read_head_bytes(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf)?;
    buf.truncate(read);
    Ok(buf)
}

fn magic_matches(mime: &str, head: &[u8]) -> bool {
    match mime {
        "image/png" => head.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
        "image/jpeg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => head.starts_with(b"GIF8"),
        "image/webp" => head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "application/pdf" => head.starts_with(b"%PDF-"),
        _ => false,
    }
}

/// PR-RM（T3 hashline）`pi_agent_rust::compute_line_hash` 25 行实现的等价 Rust 版。
///
/// 算法（与 `pi_agent_rust/src/tools.rs` 5451–5466 一字对齐）：
/// 1. `strip_suffix('\r')`：去 Windows 换行残留；
/// 2. 移除所有空白（`char::is_whitespace`）得到 `significant`——「缩进改动**不影响 hash**」；
/// 3. seed：含字母数字字符 → 0；纯标点 / 空行 → 行号（让空行也有唯一 hash）；
/// 4. `xxh32(significant_bytes, seed) & 0xFF`；
/// 5. 取低字节按 4-bit nibble 拆 → 字典 `b"ZPMQVRWSNKTXJBYH"` 映射为 2 字符
///    （字典刻意避开 `O / I / 0 / 1` 等易混字符，便于人眼粘贴 / 比对）。
///
/// hashline 与 cat-n 行号互斥：spec §3.1 规定「hashline 优先」，
/// 调用方在 [`crate::core::agent_loop::tool_exec`] 入口已做去抖。
pub(crate) fn compute_line_hash(line: &str, line_no: u64) -> String {
    let trimmed = line.strip_suffix('\r').unwrap_or(line);
    let significant: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    let seed: u32 = if trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        0
    } else {
        // pi_agent_rust 用行号低 32 位作为 seed；这里同样 cast，避免大文件 wrap。
        line_no as u32
    };
    let raw = xxhash_rust::xxh32::xxh32(significant.as_bytes(), seed);
    let low = (raw & 0xFF) as u8;
    const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";
    let high_nibble = ALPHABET[((low >> 4) & 0x0F) as usize] as char;
    let low_nibble = ALPHABET[(low & 0x0F) as usize] as char;
    let mut s = String::with_capacity(2);
    s.push(high_nibble);
    s.push(low_nibble);
    s
}

/// PR-RM（T3 hashline）`{1-based 行号}#{2 字符 hash}:{原行内容}` 渲染。
///
/// 与 [`format_with_line_numbers`] 互斥：本函数被调用时**必然** `hashline=true`，
/// 此时 cat-n 行号被忽略（避免双重行号噪音）。
///
/// 行尾保留：`split_inclusive('\n')` 让 trailing newline 落到原行结尾，
/// 与上游 `pi_agent_rust` 输出一致；空 body 直接返回空串。
pub(crate) fn format_with_hashlines(start_line: u64, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(body.len() + body.len() / 16);
    let mut line_no = start_line;
    for line in body.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        let tag = compute_line_hash(bare, line_no);
        out.push_str(&format!("{:>6}#{}:{}", line_no, tag, line));
        line_no = line_no.saturating_add(1);
    }
    out
}

/// PR-RF（T2-a）`cat -n` 风格行号渲染：每行前缀 `{:>6}\t`（6 格右对齐 + Tab）。
///
/// - **行号语义**：`start_line` 是该 body **第一行的绝对行号**（1-based）；
///   后续行依次递增。截断尾注由调用方追加，**不**进入本函数。
/// - **格式来源**：与 `cc-fork-01` `addLineNumbers` 一致，便于 IDE / diff 工具
///   横向比对（详见 `openspec/specs/architecture/tools/read.md` §3.1）。
/// - **行尾处理**：`split_inclusive('\n')` 保留每行末尾换行；最后一行若无换行
///   也按裸行渲染（与原始内容一致，不补 `\n`）。
/// - **空 body**：返回空字符串（不强行打印 `1\t`），与 cat -n 行为一致。
pub(crate) fn format_with_line_numbers(start_line: u64, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(body.len() + body.len() / 32);
    let mut line_no = start_line;
    for line in body.split_inclusive('\n') {
        out.push_str(&format!("{:>6}\t{}", line_no, line));
        line_no = line_no.saturating_add(1);
    }
    out
}

/// PR-RB（T1）`read` 工具流式抽窗的返回值。
///
/// `Binary` 用于二进制 / 非 UTF-8 文件的早期检测：
/// 第一块（最多 [`READ_CHUNK_BYTES`]）若含 `\x00` → 立即判定，**不**继续扫描。
/// 这与 grep/cat 行业惯例一致，避免把超大二进制读到一半才发现要拒。
enum ReadWindowOutcome {
    Text {
        /// 窗口字节（已包含每行尾部 `\n`，最后一行若无换行也保留原样）。
        window: Vec<u8>,
        /// 是否因达到 `limit` 行被截断（`true` → 调用方应附续读 hint）。
        truncated: bool,
        /// 截断后**剩余的行数**（仅在 `truncated == true` 时有意义）。
        ///
        /// 计算方式：在收齐窗口后**继续扫换行符**到 EOF，但**不**缓存内容——
        /// 仅 `memchr` 计数，零额外字符串分配。
        remaining_lines: u64,
    },
    Binary {
        /// 触发判定的字节十六进制（如 `"89"` 提示 PNG，`"25"` 提示 PDF）。
        first_byte_hex: String,
    },
}

/// PR-RB（T1）阻塞式分块读 + memchr 单循环抽窗。
///
/// 在 [`tokio::task::spawn_blocking`] 里跑，避免阻塞 reactor。
///
/// 算法（与 `read.md` §2.4 对齐）：
/// 1. 按 [`READ_CHUNK_BYTES`] 反复 `read`；
/// 2. 用 `memchr::memchr_iter(b'\n', chunk)` 数换行；
/// 3. 维护 `current_line`（1-based）：
///    - `current_line < start_line` → **跳过**（指针 + 计数，不分配 String）；
///    - `start_line ≤ current_line < start_line + limit_lines` → 收到 `window`；
///    - `current_line ≥ start_line + limit_lines` → 进入「仅计数尾部」阶段；
/// 4. 第一块若含 `\x00` → `Binary` 早返；
/// 5. EOF 后若仍有 leftover（无换行结尾） → 按是否在窗口内补齐。
fn read_window_blocking(
    path: &Path,
    start_line: u64,
    limit_lines: u64,
) -> Result<ReadWindowOutcome, AppError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(AppError::Io)?;
    let mut buf = vec![0u8; READ_CHUNK_BYTES];
    let mut window: Vec<u8> = Vec::new();
    let mut leftover: Vec<u8> = Vec::new();
    let mut current_line: u64 = 1;
    let mut window_lines: u64 = 0;
    let end_line_exclusive = start_line.saturating_add(limit_lines);
    let mut truncated = false;
    let mut remaining_lines: u64 = 0;
    let mut first_chunk = true;

    loop {
        let n = file.read(&mut buf).map_err(AppError::Io)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];

        if first_chunk {
            first_chunk = false;
            if memchr::memchr(0, chunk).is_some() {
                let first_byte_hex = format!("{:02X}", chunk[0]);
                return Ok(ReadWindowOutcome::Binary { first_byte_hex });
            }
        }

        let mut last_consumed = 0usize;
        for nl in memchr::memchr_iter(b'\n', chunk) {
            let line_slice = &chunk[last_consumed..=nl];
            last_consumed = nl + 1;

            if truncated {
                remaining_lines = remaining_lines.saturating_add(1);
                continue;
            }

            if current_line >= start_line && current_line < end_line_exclusive {
                if !leftover.is_empty() {
                    window.extend_from_slice(&leftover);
                    leftover.clear();
                }
                window.extend_from_slice(line_slice);
                window_lines = window_lines.saturating_add(1);
            } else if !leftover.is_empty() {
                leftover.clear();
            }

            current_line = current_line.saturating_add(1);

            if window_lines >= limit_lines && !truncated {
                truncated = true;
            }
        }

        let tail = &chunk[last_consumed..];
        if !tail.is_empty() {
            if truncated {
            } else if current_line >= start_line && current_line < end_line_exclusive {
                leftover.extend_from_slice(tail);
            }
        }
    }

    if !leftover.is_empty()
        && !truncated
        && current_line >= start_line
        && current_line < end_line_exclusive
    {
        window.extend_from_slice(&leftover);
        // Trailing line without `\n` is preserved as-is in `window`; we don't
        // bump `window_lines` here because no further branch reads it after EOF.
    }

    Ok(ReadWindowOutcome::Text {
        window,
        truncated,
        remaining_lines,
    })
}

#[async_trait]
impl PrimitiveExecutor for DefaultPrimitiveExecutor {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
            .await?;
        let meta = std::fs::metadata(&path_buf).map_err(AppError::Io)?;
        if meta.is_dir() {
            return Err(AppError::Primitive(
                "路径是目录，无法读取为文件".to_string(),
            ));
        }
        if meta.len() > self.read_max_bytes {
            return Err(AppError::Primitive(format!(
                "文件过大 ({} bytes)，超过限制 {} bytes",
                meta.len(),
                self.read_max_bytes
            )));
        }
        let content = read_file_utf8(&path_buf).map_err(|e| match e {
            AppError::Config(msg) if msg.contains("invalid utf-8") => AppError::Primitive(format!(
                "文件存在且权限已通过检查，但它是二进制或非 UTF-8 文本，不能用 read_file 按文本读取：{}",
                path_buf.display()
            )),
            other => other,
        })?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(content)
    }

    /// PR-RB（T1）`read` 工具入口：metadata 阶段大小预检 + 分块流式 + memchr 单循环抽窗。
    ///
    /// 详见 `openspec/specs/architecture/tools/read.md` §2.1–§2.5。
    /// `offset`/`limit` 的边界（`offset >= 1` / `1 ≤ limit ≤ 10000`）已在
    /// [`crate::core::agent_loop::tool_exec`] 入口（§2.6）兜底，本方法仅
    /// `clamp` 防御。
    async fn read(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        line_numbers: bool,
        hashline: bool,
        plugin_id: &str,
    ) -> Result<ReadResult, AppError> {
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
            .await?;
        let meta = std::fs::metadata(&path_buf).map_err(AppError::Io)?;
        if meta.is_dir() {
            return Err(AppError::Primitive(
                "路径是目录，无法读取为文件".to_string(),
            ));
        }

        // PR-RJ T3-b：image / PDF 路由。`offset`/`limit` 对二进制无意义——
        // 命中即按 inline 通道走，metadata 阶段判大小（不读字节、不 base64）。
        if let Some(detected) = detect_inline_mime(&path_buf) {
            let (max_bytes, label) = match detected.kind {
                InlineKind::Image => (
                    crate::core::llm::IMAGE_MAX_BYTES as u64,
                    "IMAGE_MAX_BYTES (4.5 MiB)",
                ),
                InlineKind::Pdf => (
                    crate::core::llm::FILE_MAX_BYTES as u64,
                    "FILE_MAX_BYTES (25 MiB)",
                ),
            };
            if meta.len() > max_bytes {
                return Err(AppError::Primitive(format!(
                    "File ({} bytes, mime={}) exceeds {} for inline content parts. Either trim the asset, host it externally, or upload via the Files API once the upload manager lands (T2-P0-013).",
                    meta.len(),
                    detected.mime,
                    label
                )));
            }
            let filename = path_buf
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path_buf.display().to_string());
            let binary = ReadBinaryResult {
                mime: detected.mime,
                original_size: meta.len(),
                path: path_buf.clone(),
                filename,
            };
            self.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Read,
                path_or_cmd: path.to_string(),
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: true,
                detail: Some(format!(
                    "read inline kind={:?} mime={} bytes={}",
                    detected.kind, binary.mime, binary.original_size
                )),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            return Ok(match detected.kind {
                InlineKind::Image => ReadResult::Image(binary),
                InlineKind::Pdf => ReadResult::Pdf(binary),
            });
        }

        let has_window = offset.is_some() || limit.is_some();
        if !has_window && meta.len() > self.read_max_bytes {
            return Err(AppError::Primitive(format!(
                "File is large ({} bytes > {} bytes). Pass `offset` and `limit` to read a specific window, e.g. `read(path, offset=1, limit=2000)`. (decision: openspec/specs/architecture/tools/read.md §2.5)",
                meta.len(),
                self.read_max_bytes
            )));
        }

        let start_line = offset.unwrap_or(1).max(1);
        let limit_lines = limit.unwrap_or(READ_DEFAULT_LIMIT_LINES).max(1);

        let path_clone = path_buf.clone();
        let read_outcome = tokio::task::spawn_blocking(move || {
            read_window_blocking(&path_clone, start_line, limit_lines)
        })
        .await
        .map_err(|e| AppError::Primitive(format!("read join error: {}", e)))??;

        let text = match read_outcome {
            ReadWindowOutcome::Text {
                window,
                truncated,
                remaining_lines,
            } => {
                let body = String::from_utf8(window).map_err(|e| {
                    AppError::Primitive(format!(
                        "File contains invalid UTF-8 mid-stream (byte {} not a valid sequence start): {}",
                        e.utf8_error().valid_up_to(),
                        path_buf.display()
                    ))
                })?;
                // PR-RM：hashline 优先于 line_numbers（与 spec §3.1 一致）。
                let mut s = if hashline {
                    format_with_hashlines(start_line, &body)
                } else if line_numbers {
                    format_with_line_numbers(start_line, &body)
                } else {
                    body
                };
                let num_lines = s.lines().count() as u64;
                if truncated {
                    if !s.ends_with('\n') {
                        s.push('\n');
                    }
                    let next_offset = start_line.saturating_add(limit_lines);
                    if remaining_lines > 0 {
                        s.push_str(&format!(
                            "... [{} more lines truncated; resume with offset={}, limit={}]\n",
                            remaining_lines, next_offset, limit_lines
                        ));
                    } else {
                        s.push_str(&format!(
                            "... [more lines truncated; resume with offset={}, limit={}]\n",
                            next_offset, limit_lines
                        ));
                    }
                }
                ReadTextResult {
                    content: s,
                    start_line,
                    num_lines,
                    truncated,
                    remaining_lines,
                }
            }
            ReadWindowOutcome::Binary { first_byte_hex } => {
                return Err(AppError::Primitive(format!(
                    "File is binary or non-UTF-8 (detected: 0x{first}). • try `bash file <path>` to inspect the type; • multimodal image/PDF will be supported in a later read upgrade (T3, openspec/specs/architecture/tools/read.md §4.1).",
                    first = first_byte_hex
                )));
            }
        };

        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: Some(format!(
                "read offset={} limit={} bytes_returned={} num_lines={}",
                start_line,
                limit_lines,
                text.content.len(),
                text.num_lines
            )),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(ReadResult::Text(text))
    }

    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Read, path, plugin_id)
            .await?;
        let read = std::fs::read_dir(&path_buf).map_err(AppError::Io)?;
        let mut entries = Vec::new();
        for e in read {
            let e = e.map_err(AppError::Io)?;
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map_err(AppError::Io)?.is_dir();
            entries.push(DirEntry { name, is_dir });
        }
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: path.to_string(),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: Some(format!("list_dir {} entries", entries.len())),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(entries)
    }

    async fn search_files(
        &self,
        args: SearchFilesArgs,
        plugin_id: &str,
    ) -> Result<SearchFilesOutput, AppError> {
        if args.pattern.trim().is_empty() {
            return Err(AppError::Primitive(
                "search_files.pattern is required".to_string(),
            ));
        }

        let started = Instant::now();
        let requested_path = args.path.clone().unwrap_or_else(|| ".".to_string());
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Read, &requested_path, plugin_id)
            .await?;
        let (root, search_arg) = search_root_and_arg(&path_buf);
        let limit = resolve_search_limit(&args)?;
        let deny_rules: Vec<PathRule> = self
            .gate
            .effective_path_rules()
            .into_iter()
            .filter(|rule| rule.mode == PathRuleMode::Deny)
            .collect();
        let mut warnings = Vec::new();

        let output = match args.target {
            SearchFilesTarget::Files => {
                if let Some(fd) = find_binary(&["fd", "fdfind"]) {
                    tier1_warning(&mut warnings);
                    let mut cmd = Command::new(fd);
                    cmd.arg("--color=never")
                        .arg("--type")
                        .arg("f")
                        .arg("--glob")
                        .arg(&args.pattern);
                    if args.include_hidden {
                        cmd.arg("--hidden");
                    }
                    cmd.arg(&search_arg).current_dir(&root).kill_on_drop(true);
                    let output = self
                        .run_search_command(cmd, SEARCH_FILES_TIMEOUT_SECS)
                        .await?;
                    if !output.status.success() {
                        return Err(AppError::Primitive(
                            String::from_utf8_lossy(&output.stderr).trim().to_string(),
                        ));
                    }
                    let files = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
                    if skipped > 0 {
                        warnings.push(format!("skipped {} paths due to read deny", skipped));
                    }
                    let scanned = files.len();
                    let (files, truncated, next_offset) = paginate(files, args.offset, limit);
                    SearchFilesOutput {
                        mode: SearchFilesResultMode::Files,
                        query: search_files_query(&args, &path_buf, limit, None),
                        files: Some(files),
                        matches: None,
                        counts: None,
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    }
                } else {
                    let args = args.clone();
                    let root = root.clone();
                    let path_buf = path_buf.clone();
                    let deny_rules = deny_rules.clone();
                    tokio::task::spawn_blocking(move || {
                        search_files_fallback(args, root, path_buf, limit, deny_rules, started)
                    })
                    .await
                    .map_err(|e| AppError::Primitive(e.to_string()))??
                }
            }
            SearchFilesTarget::Content => {
                if let Some(rg) = find_binary(&["rg", "ripgrep"]) {
                    tier1_warning(&mut warnings);
                    let mut cmd = Command::new(rg);
                    cmd.arg("--color=never");
                    match args.output_mode {
                        SearchFilesOutputMode::FilesWithMatches => {
                            cmd.arg("--files-with-matches");
                        }
                        SearchFilesOutputMode::Count => {
                            cmd.arg("--count");
                        }
                        SearchFilesOutputMode::Content => {
                            cmd.arg("--line-number")
                                .arg("--column")
                                .arg("--with-filename")
                                .arg("--no-heading")
                                .arg("--max-columns")
                                .arg("500");
                            if let Some(context) = args.context.filter(|context| *context > 0) {
                                cmd.arg("-C").arg(context.to_string());
                            }
                        }
                    }
                    if args.case_insensitive {
                        cmd.arg("-i");
                    }
                    if args.include_hidden {
                        cmd.arg("--hidden");
                    }
                    if let Some(glob) = args.glob.as_deref() {
                        cmd.arg("--glob").arg(glob);
                    }
                    if let Some(file_type) = args.file_type.as_deref() {
                        cmd.arg("--type").arg(file_type);
                    }
                    cmd.arg(&args.pattern)
                        .arg(&search_arg)
                        .current_dir(&root)
                        .kill_on_drop(true);
                    let output = self
                        .run_search_command(cmd, SEARCH_CONTENT_TIMEOUT_SECS)
                        .await?;
                    let exit = output.status.code().unwrap_or(-1);
                    if exit > 1 {
                        return Err(AppError::Primitive(
                            String::from_utf8_lossy(&output.stderr).trim().to_string(),
                        ));
                    }
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    match args.output_mode {
                        SearchFilesOutputMode::FilesWithMatches => {
                            let files = stdout
                                .lines()
                                .filter(|line| !line.trim().is_empty())
                                .map(str::to_string)
                                .collect::<Vec<_>>();
                            let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
                            if skipped > 0 {
                                warnings
                                    .push(format!("skipped {} paths due to read deny", skipped));
                            }
                            let scanned = files.len();
                            let (files, truncated, next_offset) =
                                paginate(files, args.offset, limit);
                            SearchFilesOutput {
                                mode: SearchFilesResultMode::ContentFiles,
                                query: SearchFilesQuery {
                                    pattern: args.pattern.clone(),
                                    target: args.target,
                                    path: path_buf.to_string_lossy().into_owned(),
                                    glob: args.glob.clone(),
                                    file_type: args.file_type.clone(),
                                    output_mode: Some(args.output_mode),
                                    head_limit: limit,
                                    offset: args.offset,
                                    case_insensitive: args.case_insensitive,
                                    include_hidden: args.include_hidden,
                                },
                                files: Some(files),
                                matches: None,
                                counts: None,
                                stats: SearchFilesStats {
                                    scanned_files: scanned,
                                    elapsed_ms: started.elapsed().as_millis(),
                                },
                                truncated,
                                next_offset,
                                warnings,
                            }
                        }
                        SearchFilesOutputMode::Count => {
                            let counts = stdout
                                .lines()
                                .filter_map(parse_rg_count_line)
                                .collect::<Vec<_>>();
                            let (counts, skipped) =
                                filter_denied_counts(&root, counts, &deny_rules);
                            if skipped > 0 {
                                warnings
                                    .push(format!("skipped {} paths due to read deny", skipped));
                            }
                            let scanned = counts.len();
                            let (counts, truncated, next_offset) =
                                paginate(counts, args.offset, limit);
                            SearchFilesOutput {
                                mode: SearchFilesResultMode::ContentCount,
                                query: SearchFilesQuery {
                                    pattern: args.pattern.clone(),
                                    target: args.target,
                                    path: path_buf.to_string_lossy().into_owned(),
                                    glob: args.glob.clone(),
                                    file_type: args.file_type.clone(),
                                    output_mode: Some(args.output_mode),
                                    head_limit: limit,
                                    offset: args.offset,
                                    case_insensitive: args.case_insensitive,
                                    include_hidden: args.include_hidden,
                                },
                                files: None,
                                matches: None,
                                counts: Some(counts),
                                stats: SearchFilesStats {
                                    scanned_files: scanned,
                                    elapsed_ms: started.elapsed().as_millis(),
                                },
                                truncated,
                                next_offset,
                                warnings,
                            }
                        }
                        SearchFilesOutputMode::Content => {
                            let matches = stdout
                                .lines()
                                .filter_map(parse_rg_match_line)
                                .collect::<Vec<_>>();
                            let (matches, skipped) =
                                filter_denied_matches(&root, matches, &deny_rules);
                            if skipped > 0 {
                                warnings
                                    .push(format!("skipped {} paths due to read deny", skipped));
                            }
                            let scanned = matches.len();
                            let (matches, truncated, next_offset) =
                                paginate(matches, args.offset, limit);
                            SearchFilesOutput {
                                mode: SearchFilesResultMode::ContentLines,
                                query: SearchFilesQuery {
                                    pattern: args.pattern.clone(),
                                    target: args.target,
                                    path: path_buf.to_string_lossy().into_owned(),
                                    glob: args.glob.clone(),
                                    file_type: args.file_type.clone(),
                                    output_mode: Some(args.output_mode),
                                    head_limit: limit,
                                    offset: args.offset,
                                    case_insensitive: args.case_insensitive,
                                    include_hidden: args.include_hidden,
                                },
                                files: None,
                                matches: Some(matches),
                                counts: None,
                                stats: SearchFilesStats {
                                    scanned_files: scanned,
                                    elapsed_ms: started.elapsed().as_millis(),
                                },
                                truncated,
                                next_offset,
                                warnings,
                            }
                        }
                    }
                } else {
                    let args = args.clone();
                    let root = root.clone();
                    let path_buf = path_buf.clone();
                    let deny_rules = deny_rules.clone();
                    tokio::task::spawn_blocking(move || {
                        search_files_fallback(args, root, path_buf, limit, deny_rules, started)
                    })
                    .await
                    .map_err(|e| AppError::Primitive(e.to_string()))??
                }
            }
        };

        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Read,
            path_or_cmd: format!("search_files {}", requested_path),
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: Some(format!(
                "mode={:?} truncated={}",
                output.mode, output.truncated
            )),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });

        Ok(output)
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Write, path, plugin_id)
            .await?;
        let path_str = path_buf.to_string_lossy().to_string();

        if overwrite && path_buf.exists() {
            let backup = path_buf.with_extension("bak");
            let _ = std::fs::copy(&path_buf, &backup);
        }

        write_file_atomic(&path_buf, content.as_bytes())?;
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Write,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
        })
    }

    async fn edit_file(
        &self,
        path: &str,
        edits: Vec<EditOperation>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        let (path_buf, scope, grant) = self
            .gate_check_path(PrimitiveOperation::Edit, path, plugin_id)
            .await?;
        let path_str = path_buf.to_string_lossy().to_string();
        let content = read_file_utf8(&path_buf)?;
        let mut lines: Vec<String> = content.lines().map(String::from).collect();

        let backup_path = path_buf.with_extension("bak");
        std::fs::copy(&path_buf, &backup_path).map_err(AppError::Io)?;

        for edit in &edits {
            match edit.operation_type {
                EditOperationType::Replace => {
                    if edit.start_line.is_none() {
                        if let Some(ref old) = edit.old_content {
                            let full_text = lines.join("\n");
                            let count = full_text.matches(old.as_str()).count();
                            if count == 0 {
                                let _ = std::fs::copy(&backup_path, &path_buf);
                                return Err(AppError::Primitive(format!(
                                    "edit_file: 未找到匹配的 old_content（文件 {}）",
                                    path
                                )));
                            }
                            if count > 1 {
                                let _ = std::fs::copy(&backup_path, &path_buf);
                                return Err(AppError::Primitive(format!(
                                    "edit_file: old_content 在文件中出现 {} 次，需要更多上下文使其唯一",
                                    count
                                )));
                            }
                            let new_text = full_text.replacen(old.as_str(), &edit.new_content, 1);
                            lines = new_text.lines().map(String::from).collect();
                        }
                    } else if let Some(start_line_val) = edit.start_line {
                        let start = start_line_val as usize;
                        let end = edit.end_line.unwrap_or(start_line_val) as usize;
                        if start < 1 || end > lines.len() || start > end {
                            let _ = std::fs::copy(&backup_path, &path_buf);
                            return Err(AppError::Primitive(format!(
                                "Replace 行号无效: {}..{}",
                                start, end
                            )));
                        }
                        let idx = start - 1;
                        let new_lines: Vec<String> =
                            edit.new_content.lines().map(String::from).collect();
                        for (i, nl) in new_lines.iter().enumerate() {
                            if idx + i < lines.len() {
                                lines[idx + i] = nl.clone();
                            } else {
                                lines.push(nl.clone());
                            }
                        }
                        for i in (idx + new_lines.len())..end {
                            if i < lines.len() {
                                lines[i] = String::new();
                            }
                        }
                    }
                }
                EditOperationType::Insert => {
                    let at = edit.start_line.unwrap_or(0) as usize;
                    if at > lines.len() {
                        let _ = std::fs::copy(&backup_path, &path_buf);
                        return Err(AppError::Primitive(format!("Insert 行号超出: {}", at)));
                    }
                    lines.insert(at, edit.new_content.clone());
                }
                EditOperationType::Delete => {
                    let start = edit.start_line.unwrap_or(1) as usize;
                    let end = edit.end_line.unwrap_or(start as u64) as usize;
                    if start < 1 || end > lines.len() || start > end {
                        let _ = std::fs::copy(&backup_path, &path_buf);
                        return Err(AppError::Primitive(format!(
                            "Delete 行号无效: {}..{}",
                            start, end
                        )));
                    }
                    for _ in 0..=(end - start) {
                        if start <= lines.len() {
                            lines.remove(start - 1);
                        }
                    }
                }
            }
        }

        let new_content = lines.join("\n");
        let _ = build_simple_diff(content.as_str(), &new_content);

        if let Err(e) = write_file_atomic(&path_buf, new_content.as_bytes()) {
            let _ = std::fs::copy(&backup_path, &path_buf);
            let _ = std::fs::remove_file(&backup_path);
            self.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Edit,
                path_or_cmd: path_str,
                plugin_id: plugin_id.to_string(),
                user_approved: true,
                success: false,
                detail: Some(e.to_string()),
                permission_scope: Some(permission_scope_str(scope)),
                grant_type: Some(grant_type_str(grant.grant_type)),
                grant_trigger: Some(grant_trigger_str(grant.trigger)),
            });
            return Err(e);
        }
        let _ = std::fs::remove_file(&backup_path);
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Edit,
            path_or_cmd: path_str,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: true,
            detail: None,
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
        Ok(EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }

    async fn execute_bash(
        &self,
        command: &str,
        cwd: Option<&str>,
        plugin_id: &str,
        argv: Option<&[String]>,
    ) -> Result<BashResult, AppError> {
        let cwd_path = if let Some(c) = cwd {
            let (p, _l, _s) = self
                .gate_check_path(PrimitiveOperation::Read, c, plugin_id)
                .await?;
            p
        } else {
            PathBuf::from(".")
        };

        let audit_cmd = match argv {
            None => command.to_string(),
            Some(args) => {
                let mut s = command.to_string();
                for a in args {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }
        };

        // bash 决策来源（whitelist / approval）—— 走 gate 三层。
        let (bash_scope, bash_grant) = match self.gate_check_bash(&audit_cmd, plugin_id).await {
            Ok((scope, grant)) => (scope, grant),
            Err(e) => {
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Bash,
                    path_or_cmd: audit_cmd.clone(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some(e.to_string()),
                    ..Default::default()
                });
                return Err(e);
            }
        };

        // 然后把命令里出现的路径逐一交给 gate.check 处理（layer-1 deny / layer-2 confirm）。
        // 仅作"尽力而为"——shell_words 解析失败的命令，依赖 forbidden regex 兜底。
        // 见 docs/TODOS.md `T-147`：动态路径访问的静态预检盲区由后续提示词注入防御方向覆盖。
        for raw in crate::core::permission::bash_parser::extract_paths(&audit_cmd) {
            if let Err(e) = self
                .gate_check_path(PrimitiveOperation::Bash, &raw, plugin_id)
                .await
            {
                self.audit.record_primitive(PrimitiveAuditEntry {
                    operation: AuditPrimitiveOp::Bash,
                    path_or_cmd: audit_cmd.clone(),
                    plugin_id: plugin_id.to_string(),
                    user_approved: false,
                    success: false,
                    detail: Some(format!("路径未授权: {} ({})", raw, e)),
                    ..Default::default()
                });
                return Err(e);
            }
        }

        let output = match argv {
            None => {
                #[cfg(unix)]
                let script = {
                    let env_path = self
                        .config
                        .wasmedge_env_path
                        .as_deref()
                        .unwrap_or(r#"$HOME/.wasmedge/env"#);
                    format!(r#"[ -f "{0}" ] && . "{0}"; {1}"#, env_path, command)
                };
                #[cfg(windows)]
                let script = command.to_string();
                #[cfg(unix)]
                let (shell, shell_arg) = ("sh", "-c");
                #[cfg(windows)]
                let (shell, shell_arg) = ("cmd", "/C");
                Command::new(shell)
                    .arg(shell_arg)
                    .arg(&script)
                    .current_dir(&cwd_path)
                    .kill_on_drop(true)
                    .output()
                    .await
            }
            Some(args) => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .current_dir(&cwd_path)
                    .kill_on_drop(true)
                    .output()
                    .await
            }
        }
        .map_err(|e| AppError::Primitive(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Bash,
            path_or_cmd: audit_cmd,
            plugin_id: plugin_id.to_string(),
            user_approved: true,
            success: exit_code == 0,
            detail: Some(format!(
                "exit_code={} stdout_len={} stderr_len={}",
                exit_code,
                stdout.len(),
                stderr.len()
            )),
            permission_scope: Some(permission_scope_str(bash_scope)),
            grant_type: Some(grant_type_str(bash_grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(bash_grant.trigger)),
        });
        Ok(BashResult {
            stdout,
            stderr,
            exit_code,
        })
    }

    async fn require_user_confirmation(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        // 路径授权统一走 `gate_check_path` / `gate_check_bash`，
        // 此 trait 方法仅保留供外部直接调用 `confirmation.confirm` 的兼容入口：
        // - Read 操作不需要 confirm；
        // - `auto_confirm = true` 时直接放行；
        // - 其他情况转发给底层 `UserConfirmationProvider::confirm`。
        if matches!(operation, PrimitiveOperation::Read) || self.config.auto_confirm {
            return Ok(true);
        }
        self.confirmation
            .confirm(operation, preview, plugin_id)
            .await
    }
}
