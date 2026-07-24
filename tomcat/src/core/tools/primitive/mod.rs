//! # Primitive tool executor
//!
//! Default implementation for read/write/edit/bash/list-dir primitives.

pub mod bash_task;
mod diff;
mod executor;
mod registry_builder;
#[cfg(test)]
mod tests;
mod types;

pub use bash_task::{
    BackgroundTaskLifecycleEvent, BashOutputStream, BashRuntimePreview, BashTaskId, BashTaskInfo,
    BashTaskOutputChunk, BashTaskOutputEvent, BashTaskRegistry, BashTaskStatus, BashTaskTicket,
};
#[allow(unused_imports)]
pub(crate) use executor::compute_line_hash;
#[cfg(test)]
pub(crate) use executor::simulate_failed_commit_with_backup_for_test;
pub use executor::DefaultPrimitiveExecutor;
pub(crate) use registry_builder::build_bash_task_registry;
pub use types::{
    BashExecutionState, BashNextAction, BashResult, DiffTag, DirEntry, EditFileResult,
    EditOperation, EditOperationType, FileDiffLine, HashlineOp, HashlineSegment, PrimitiveExecutor,
    PrimitiveOperation, ReadBinaryResult, ReadResult, ReadTextResult, SearchFileCount,
    SearchFileMatch, SearchFilesArgs, SearchFilesOutput, SearchFilesOutputMode, SearchFilesQuery,
    SearchFilesResultMode, SearchFilesStats, SearchFilesTarget, WriteFileResult,
};

/// T2-P0-017 PR-D：`replace_all` 信号 sentinel。
///
/// `tool_exec` 把 `replace_all=true` 段的 `old_content` 加上此前缀；
/// `write_edit::edit_file_impl` 在分段解析时识别并剥离。
/// 选用 `\u{0000}` 边界字符是因为：UTF-8 文本 `read_file_utf8` 阶段就会拒掉
/// 含 NUL 的二进制文件，因此 sentinel 不会与合法用户输入冲突。
///
/// 决策动机（详见 [edit.md §2.4.3](../../../../docs/architecture/tools/edit.md)
/// 与计划文件 `t2-p0-017_edit_工具_*.plan.md` Phase1 决策 6）：
/// 保留 `PrimitiveExecutor::edit_file` trait 方法签名不动，避免牵动 dispatcher
/// extension / 多个 mock / 集成测试一起改名。`replace_all` 信号通过此 marker
/// 在调用对内传递，外部 trait 完全感知不到。
pub(crate) const EDIT_REPLACE_ALL_MARKER: &str = "\u{0000}__PI_EDIT_REPLACE_ALL__\u{0000}";
