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
//!    │   gate.check_bash(audit_cmd) 同样三态；本期不再对命令字符串做路径级预检，
//!    │   避免 `node:fs/promises` / `@scope/pkg` / heredoc 脚本等被误当成磁盘路径。
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
//! - `MAX_READ_BYTES = 25 MiB`：read_file 单次读上限，防 OOM；详见 [`MAX_READ_BYTES`]。
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
//!
//! ## 子模块划分（L-3 拆分整改后）
//!
//! 单文件 `executor.rs`（2105 行）按职责拆分到：
//!
//! - [`gate`]：`PermissionGate` 桥接（`gate_check_path` / `gate_check_bash`）+ `run_search_command`
//! - [`helpers`]：审计字符串化 + `find_binary` 等无状态小工具
//! - [`read`]：`read_file` / `read` / `list_dir` 实现 + cat-n / hashline / 多模态 magic
//! - [`search`]：`search_files` Tier1（rg/fd）+ Tier2（rust-fallback）
//! - [`write_edit`]：`write_file` / `edit_file` 实现
//! - [`bash`]：`execute_bash` 实现
//! - [`confirm`]：`require_user_confirmation` 兼容入口
//!
//! `impl PrimitiveExecutor for DefaultPrimitiveExecutor` 整块留在本文件，
//! 每个方法做一行委托——trait 不能跨文件实现，但方法体可以下沉。

use crate::core::tools::contract::confirmation::UserConfirmationProvider;
use crate::core::tools::primitive::{
    BashResult, DirEntry, EditFileResult, EditOperation, PrimitiveExecutor, PrimitiveOperation,
    ReadResult, SearchFilesArgs, SearchFilesOutput, WriteFileResult,
};
use crate::infra::audit::AuditRecorder;
use crate::infra::error::AppError;
use crate::infra::PrimitiveConfig;
use async_trait::async_trait;
use std::sync::Arc;

use crate::core::permission::PermissionGate;

mod bash;
mod confirm;
mod gate;
pub(crate) mod hashline_edit;
mod helpers;
pub(crate) mod output_accum;
mod read;
mod search;
mod write_edit;

#[cfg(test)]
mod tests;

// 重导出供 tests/read_window_test.rs 与潜在外部读取的私有 helper：
// 拆分前路径是 `primitive::executor::{xxx}`；拆分后保持完全等价，避免引用方
// import 路径变化（spec L-3 拆分整改要求「外部 API 零改动」）。
// `#[allow(unused_imports)]` 因为 lib-only 编译看不到 #[cfg(test)] 引用方。
#[allow(unused_imports)]
pub(crate) use read::{
    compute_line_hash, detect_inline_mime, format_with_hashlines, format_with_line_numbers,
    DetectedInlineMime, InlineKind,
};

/// 单次读取文件最大字节数，避免 OOM。
///
/// PR-RB（T1）将上限从历史 10 MiB 提升到 **25 MiB**，介于 cc-fork 256 KiB 与
/// pi_agent_rust 100 MiB 之间——兼顾「合理 dump 文件」与「防爆 ctx」。
/// 详见 `docs/architecture/tools/read.md` §2.5 决策表 R6 #2。
///
/// **作用范围**：仅在 [`DefaultPrimitiveExecutor::read`] 的「无 `offset` / 无 `limit`」
/// 路径生效（`metadata.len() > MAX_READ_BYTES` → 拒绝并提示加 offset/limit 重试）。
/// 传入分窗时该上限被绕过——大日志可被分窗取「特定窗口」。
///
/// 默认值与 [`crate::infra::DEFAULT_TOOLS_READ_MAX_BYTES`] 保持一致；
/// 可通过 [`DefaultPrimitiveExecutor::with_read_max_bytes`] 覆盖（生产由
/// `[tools.read] max_bytes` config 注入，测试用于做小阈值快速覆盖）。
const MAX_READ_BYTES: u64 = 25 * 1024 * 1024; // 25 MiB

/// 4 原语执行引擎默认实现：路径权限、用户确认、备份、原子化与审计。
///
/// **权限模型**：构造期强制注入 [`PermissionGate`]；路径 / bash / 审计来源
/// 全部走 gate 三层决策。无 legacy fallback 通道。
pub struct DefaultPrimitiveExecutor {
    pub(super) config: PrimitiveConfig,
    pub(super) confirmation: Arc<dyn UserConfirmationProvider>,
    pub(super) audit: Arc<dyn AuditRecorder>,
    /// 路径与 bash 权限决策入口；由调用方注入并与
    /// `permission::cwd_lazy` / `tools::config_tool` 共享同一份 `SessionGrants` 视图。
    pub(super) gate: Arc<dyn PermissionGate>,
    /// PR-RB（T1）read 工具文本路径的「裸读字节上限」。
    ///
    /// 默认 [`MAX_READ_BYTES`]（25 MiB）；可由
    /// [`Self::with_read_max_bytes`] 覆盖。仅当模型未传 `offset`/`limit` 时生效。
    pub(super) read_max_bytes: u64,
    /// T2-P0-016 PR-G：write 工具是否在写盘前把 `\r\n` 折叠为 `\n`。
    ///
    /// 默认 [`crate::infra::DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF`]（`true`）；
    /// 由 [`Self::with_write_normalize_crlf`] 覆盖（生产由 `[tools.write] normalize_crlf`
    /// config 注入，测试可关掉验证「字节透传」语义）。详见
    /// `docs/architecture/tools/write.md` §3.3 / §8。
    pub(super) write_normalize_crlf: bool,
    /// T2-P0-016 PR-E.2：bash 工具墙钟超时（毫秒）；默认 [`crate::infra::DEFAULT_TOOLS_BASH_TIMEOUT_MS`]。
    /// 由 [`Self::with_bash_timeout_ms`] 覆盖（生产由 `[tools.bash] timeout_ms` config 注入）。
    pub(super) bash_timeout_ms: u64,
    /// T2-P0-016 PR-E.2：bash 工具单流字符上限（stdout / stderr 各算一份）；默认
    /// [`crate::infra::DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS`]。Phase-E.3 起接入
    /// `output_accum.rs`，超限走头尾保留 + 落盘 `bash_persist_dir`。
    pub(super) bash_max_output_chars: usize,
    /// T2-P0-016 PR-E.3：bash 工具超限输出的落盘目录；`None` 时不落盘（仅截断）。
    /// 生产路径：由 `api/chat` 装配时调用 [`Self::with_bash_persist_dir`] 注入
    /// [`crate::infra::resolve_agent_trail_dir`] + `/tool-results`。测试可设 `tempfile::tempdir()`
    /// 验证「超限落盘」路径，或保持 `None` 验证「仅截断」路径。
    pub(super) bash_persist_dir: Option<std::path::PathBuf>,
    /// T2-P0-016 PR-L（bash T3）：AST allowlist 检查器，**叠在** `gate_check_bash`
    /// 之前生效（详见 [bash-pr-l-scope.md §1 / §4](../../../../docs/architecture/tools/bash-pr-l-scope.md)）。
    /// 默认 **`enabled=false`**（`BashAstChecker::new(false, …)`）：不切段、不跑
    /// `detect_unsupported`，与无 AST 栈行为一致；需要切段/allow/deny 时
    /// 用 [`Self::with_bash_ast`] 或后续 `[tools.bash.ast]` 配置注入 `enabled=true`。
    pub(super) bash_ast: crate::core::permission::BashAstChecker,
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
            write_normalize_crlf: crate::infra::DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF,
            bash_timeout_ms: crate::infra::DEFAULT_TOOLS_BASH_TIMEOUT_MS,
            bash_max_output_chars: crate::infra::DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS,
            bash_persist_dir: None,
            bash_ast: crate::core::permission::BashAstChecker::new(false, vec![], vec![]),
        }
    }

    /// T2-P0-016 PR-L：注入自定义 AST allow/deny（含 `enabled`）。生产路径后续由
    /// `[tools.bash.ast]` config 反序列化注入；当前为 builder 入口。
    pub fn with_bash_ast(mut self, checker: crate::core::permission::BashAstChecker) -> Self {
        self.bash_ast = checker;
        self
    }

    /// T2-P0-016 PR-E.2：覆盖 bash 工具默认墙钟超时。
    ///
    /// **生产路径**：由 `[tools.bash] timeout_ms` config 在 `api/chat` 装配
    /// `DefaultPrimitiveExecutor` 时调用（与 [`Self::with_read_max_bytes`] 同形）。
    /// **测试路径**：可设小到 50 ms 模拟 wall-clock kill 行为。
    pub fn with_bash_timeout_ms(mut self, ms: u64) -> Self {
        self.bash_timeout_ms = if ms == 0 {
            crate::infra::DEFAULT_TOOLS_BASH_TIMEOUT_MS
        } else {
            ms.min(crate::infra::MAX_TOOLS_BASH_TIMEOUT_MS)
        };
        self
    }

    /// T2-P0-016 PR-E.2：覆盖 bash 工具单流字符上限。
    ///
    /// 测试侧用极小值（如 64）让 fixture 命令 stdout 触发头尾保留分支。
    pub fn with_bash_max_output_chars(mut self, n: usize) -> Self {
        self.bash_max_output_chars = if n == 0 {
            crate::infra::DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
        } else {
            n.min(crate::infra::MAX_TOOLS_BASH_MAX_OUTPUT_CHARS)
        };
        self
    }

    /// T2-P0-016 PR-E.3：注入超限输出落盘目录（生产侧 `~/.tomcat/agents/<id>/tool-results/`）。
    /// `None` 表示「不落盘，仅截断」（测试默认 + 极小心智的 mock）。
    pub fn with_bash_persist_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.bash_persist_dir = Some(dir);
        self
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

    /// T2-P0-016 PR-G 覆盖 write 工具的 LF 规范化开关。
    ///
    /// **生产路径**：由 `[tools.write] normalize_crlf` config 在 `api/chat` 装配时调用。
    /// **测试路径**：可置 `false` 验证「字节透传」语义，或置 `true` 验证 CRLF → LF 折叠。
    pub fn with_write_normalize_crlf(mut self, on: bool) -> Self {
        self.write_normalize_crlf = on;
        self
    }
}

#[async_trait]
impl PrimitiveExecutor for DefaultPrimitiveExecutor {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        read::read_file_impl(self, path, plugin_id).await
    }

    async fn read(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        line_numbers: bool,
        hashline: bool,
        plugin_id: &str,
    ) -> Result<ReadResult, AppError> {
        read::read_impl(self, path, offset, limit, line_numbers, hashline, plugin_id).await
    }

    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        read::list_dir_impl(self, path, plugin_id).await
    }

    async fn search_files(
        &self,
        args: SearchFilesArgs,
        plugin_id: &str,
    ) -> Result<SearchFilesOutput, AppError> {
        search::search_files_impl(self, args, plugin_id).await
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        write_edit::write_file_impl(self, path, content, overwrite, plugin_id).await
    }

    async fn edit_file(
        &self,
        path: &str,
        edits: Vec<EditOperation>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        write_edit::edit_file_impl(self, path, edits, plugin_id).await
    }

    async fn hashline_edit(
        &self,
        path: &str,
        segments: Vec<crate::core::tools::primitive::HashlineSegment>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        hashline_edit::hashline_edit_impl(self, path, segments, plugin_id).await
    }

    async fn execute_bash(
        &self,
        command: &str,
        cwd: Option<&str>,
        plugin_id: &str,
        argv: Option<&[String]>,
        timeout_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        bash::execute_bash_impl(self, command, cwd, plugin_id, argv, timeout_ms).await
    }

    async fn require_user_confirmation(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        confirm::require_user_confirmation_impl(self, operation, preview, plugin_id).await
    }
}
