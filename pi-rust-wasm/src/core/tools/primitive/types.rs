//! # 4 原语执行引擎 Trait 与类型（与 design CODE_BLOCK_P1_006 一致）

use std::path::PathBuf;

use crate::infra::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileResult {
    pub path: String,
    pub written: bool,
    /// T2-P0-016 PR-G：实际写入磁盘的字节数（已含 LF 规范化）。
    /// 为兼容历史调用方默认 `0`；编排层可据此渲染回执。
    #[serde(default)]
    pub bytes_written: u64,
    /// T2-P0-016 PR-G：覆盖写时的 unified-style diff 摘要（相对写前快照），
    /// 新建文件场景为 `None`；同样为兼容默认 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditFileResult {
    pub path: String,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "code")]
    pub exit_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilesTarget {
    #[default]
    Content,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilesOutputMode {
    Content,
    #[default]
    FilesWithMatches,
    Count,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilesResultMode {
    ContentFiles,
    ContentLines,
    ContentCount,
    Files,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilesArgs {
    pub pattern: String,
    #[serde(default)]
    pub target: SearchFilesTarget,
    pub path: Option<String>,
    pub glob: Option<String>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    #[serde(default)]
    pub output_mode: SearchFilesOutputMode,
    pub context: Option<usize>,
    /// `None` = schema field omitted (use target default);
    /// `Some(None)` = explicit JSON null (unlimited);
    /// `Some(Some(n))` = explicit limit.
    #[serde(default)]
    pub head_limit: Option<Option<usize>>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilesQuery {
    pub pattern: String,
    pub target: SearchFilesTarget,
    pub path: String,
    pub glob: Option<String>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    pub output_mode: Option<SearchFilesOutputMode>,
    pub head_limit: Option<usize>,
    pub offset: usize,
    pub case_insensitive: bool,
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileCount {
    pub path: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilesStats {
    pub scanned_files: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilesOutput {
    pub mode: SearchFilesResultMode,
    pub query: SearchFilesQuery,
    pub files: Option<Vec<String>>,
    pub matches: Option<Vec<SearchFileMatch>>,
    pub counts: Option<Vec<SearchFileCount>>,
    pub stats: SearchFilesStats,
    pub truncated: bool,
    pub next_offset: Option<usize>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOperation {
    pub operation_type: EditOperationType,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub old_content: Option<String>,
    pub new_content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum EditOperationType {
    Replace,
    Insert,
    Delete,
}

/// 解析后的 hashline_edit 段（T2-P0-017 Phase3 / PR-M）。
///
/// `tool_exec` 在调用 `PrimitiveExecutor::hashline_edit` 之前把 JSON
/// `{ op, pos, end?, lines }` 解析成本结构。算法见 [`crate::core::tools::primitive::executor::hashline_edit`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineSegment {
    pub op: HashlineOp,
    pub start_line: u64,
    pub start_hash: String,
    pub end_line: u64,
    pub end_hash: String,
    pub lines: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashlineOp {
    Replace,
    Insert,
    Delete,
}

impl HashlineSegment {
    /// 解析 `<line_no>#<2char>` 锚点；非法格式 → 结构化错误。
    pub fn parse_anchor(s: &str, ctx_idx: usize, field: &str) -> Result<(u64, String), AppError> {
        let (line_str, hash_str) = s.split_once('#').ok_or_else(|| {
            AppError::Primitive(format!(
                "hashline_edit: edits[{}].{} 锚点格式应为 `<line>#<2char>`，实际 `{}`",
                ctx_idx, field, s
            ))
        })?;
        let line_no: u64 = line_str.trim().parse().map_err(|_| {
            AppError::Primitive(format!(
                "hashline_edit: edits[{}].{} 行号 `{}` 不是有效正整数",
                ctx_idx, field, line_str
            ))
        })?;
        if line_no == 0 {
            return Err(AppError::Primitive(format!(
                "hashline_edit: edits[{}].{} 行号必须 ≥ 1",
                ctx_idx, field
            )));
        }
        let hash = hash_str.trim().to_string();
        if hash.chars().count() != 2 {
            return Err(AppError::Primitive(format!(
                "hashline_edit: edits[{}].{} 哈希应为 2 字符，实际 `{}`",
                ctx_idx, field, hash
            )));
        }
        Ok((line_no, hash))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PrimitiveOperation {
    Read,
    Write,
    Edit,
    Bash,
}

// ─── PR-RJ（T3-a）`read` 工具输出 schema：discriminated union ───────────────

/// PR-RJ（T3-a）`read` 工具的结构化返回值，承载 4 种语义：
///
/// - `Text`：常规文本路径（含 `cat -n` 行号、截断尾注）；
/// - `Image`：路由到图片 MIME（PNG/JPEG/GIF/WebP）→ inline base64，wire 翻译时
///   注入下一条 `user` 消息的 `Parts`（详见 `read.md` §4.2 的 OpenAI tool→user 注入边界）；
/// - `Pdf`：与 `Image` 同形态，仅 MIME = `application/pdf`；
/// - `FileUnchanged`：来自 `tool_exec` 的 dedup 短路（§3.2，节省整段 base64 / 内容 token）。
///
/// 注意：`primitive::read` 实现**只**会构造 `Text` / `Image` / `Pdf`；`FileUnchanged`
/// 由 [`crate::core::agent_loop::tool_exec`] 在调 primitive 之前的 dedup 路径上构造，
/// 之所以挂在同一 enum 里是为了让 wire 翻译层（T3-c）单口处理「这次工具结果该怎么进消息」。
#[derive(Debug, Clone, PartialEq)]
pub enum ReadResult {
    Text(ReadTextResult),
    Image(ReadBinaryResult),
    Pdf(ReadBinaryResult),
    FileUnchanged { path: PathBuf },
}

/// 文本路径返回载荷。
#[derive(Debug, Clone, PartialEq)]
pub struct ReadTextResult {
    /// 已经按 `line_numbers` 渲染好、且追加过截断尾注的最终字符串；
    /// `tool_exec` 直接当作 tool message 的 string content。
    pub content: String,
    /// 本次返回窗口的 1-based 起始行号。
    pub start_line: u64,
    /// 实际返回的行数（≤ `limit`）。
    pub num_lines: u64,
    /// 是否做了截断（达到 `limit` 但文件还有剩余行）。
    pub truncated: bool,
    /// 截断时的剩余行数（`!truncated` 时为 0）。
    pub remaining_lines: u64,
}

/// 图片 / PDF / 其他二进制 inline 载荷（共结构）。
///
/// **不**在 primitive 阶段读字节 + 编 base64：T3 设计把读盘 + base64 全部委托给
/// [`crate::core::llm::types::ChatMessageContentPart::image_b64`] /
/// [`crate::core::llm::types::ChatMessageContentPart::file_b64`]（PR-RJ-0 后签名是
/// `(mime, &Path)`）。primitive 只负责「认出是图 / PDF + 大小预检 + 把元信息
/// 传出去」，让 IO 与 base64 这条路径在仓里只出现一次。
#[derive(Debug, Clone, PartialEq)]
pub struct ReadBinaryResult {
    /// e.g. `image/png` / `application/pdf`。
    pub mime: String,
    /// 文件原始字节大小（来自 `std::fs::metadata().len()`，不含 base64 膨胀）。
    pub original_size: u64,
    /// 解析后的绝对路径——T3-c 在构造 `InputImage` / `InputFile` 时直接传给
    /// 上述 helper 让其 `std::fs::read + base64`。
    pub path: PathBuf,
    /// 原文件名（不含目录），用于 `InputFile.filename` 透传 / 占位句显示。
    pub filename: String,
}

impl ReadResult {
    /// 把 enum 摊平成一段适合 tool message 的字符串：
    ///
    /// - `Text` → `content`（已含行号 / 尾注）
    /// - `Image` / `Pdf` → 占位句（真正 inline part 在 T3-c 注入下一条 user message）
    /// - `FileUnchanged` → 短句 stub（与 `read_state::FILE_UNCHANGED_STUB` 同源）
    ///
    /// 本方法是 T3-a 阶段的「兼容垫片」：T3-c 完成后 `tool_exec` 会按 variant
    /// 走不同分支，不再统一调本 helper；保留它给 `Text` 路径与回退路径使用。
    pub fn to_tool_text(&self) -> String {
        match self {
            ReadResult::Text(t) => t.content.clone(),
            ReadResult::Image(b) => format!(
                "Image saved as next user input. See vision content for details (mime={}, path={}, bytes={}).",
                b.mime,
                b.path.display(),
                b.original_size
            ),
            ReadResult::Pdf(b) => format!(
                "PDF attached as next user input. See file content for details (filename={}, path={}, bytes={}).",
                b.filename,
                b.path.display(),
                b.original_size
            ),
            ReadResult::FileUnchanged { .. } => {
                crate::core::tools::pipeline::read_state::FILE_UNCHANGED_STUB.to_string()
            }
        }
    }
}

/// 4 原语执行引擎 Trait（与 design CODE_BLOCK_P1_006 一致）。
#[async_trait]
pub trait PrimitiveExecutor: Send + Sync + 'static {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError>;

    /// PR-RB（T1）read 工具入口：分页读 + metadata 阶段大小预检 + 二进制结构化提示。
    ///
    /// 与 [`Self::read_file`] 的关系：
    /// - `read_file` 保持「读全文 → `String`」的单一语义，外部调用方（dispatcher /
    ///   hostcall / 测试 mock）不受影响；
    /// - **`read`** 是 LLM 工具 `read` 在 [`crate::core::agent_loop::tool_exec`]
    ///   里调用的入口；本 trait 默认实现回退到 `read_file`（即「无 offset/limit
    ///   的完整读」），让现有 mock / 旧 PrimitiveExecutor 实现 **零改动** 升级；
    /// - [`super::executor::DefaultPrimitiveExecutor`] 重写本方法为分块流式
    ///   单循环抽窗 + 25 MiB metadata 上限 + 二进制 hint（详见
    ///   `docs/architecture/tools/read.md` §2.1–§2.5）。
    ///
    /// 入参语义：
    /// - `offset`：1-based 起始行号；`None` 等价于 `Some(1)`；
    /// - `limit`：返回行数上限；`None` 等价于「不分窗，整文件读」（受 metadata 上限保护）。
    ///
    /// 返回 [`ReadResult`]（PR-RJ T3-a 升级为 discriminated union）。
    /// 默认实现退回到 `read_file` 整文件读 → 包成 `ReadResult::Text`，确保旧 mock /
    /// 第三方 `PrimitiveExecutor` 实现**零改动**升级；只有 [`super::executor::DefaultPrimitiveExecutor`]
    /// 真正会按 mime 走 `Image` / `Pdf` 分支（T3-b 接入）+ `hashline` 渲染（PR-RM）。
    ///
    /// `line_numbers` 与 `hashline` **互斥**（spec §3.1：`hashline` 优先），
    /// 调用方（`tool_exec`）已在入口处对参数做归一化，本 trait 默认实现忽略两者。
    async fn read(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        line_numbers: bool,
        hashline: bool,
        plugin_id: &str,
    ) -> Result<ReadResult, AppError> {
        let _ = (offset, limit, line_numbers, hashline);
        let content = self.read_file(path, plugin_id).await?;
        let line_count = content.lines().count() as u64;
        Ok(ReadResult::Text(ReadTextResult {
            content,
            start_line: 1,
            num_lines: line_count,
            truncated: false,
            remaining_lines: 0,
        }))
    }
    async fn list_dir(&self, path: &str, plugin_id: &str) -> Result<Vec<DirEntry>, AppError>;
    async fn search_files(
        &self,
        _args: SearchFilesArgs,
        _plugin_id: &str,
    ) -> Result<SearchFilesOutput, AppError> {
        Err(AppError::Primitive(
            "search_files is not implemented by this PrimitiveExecutor".to_string(),
        ))
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        plugin_id: &str,
    ) -> Result<WriteFileResult, AppError>;
    async fn edit_file(
        &self,
        path: &str,
        edits: Vec<EditOperation>,
        plugin_id: &str,
    ) -> Result<EditFileResult, AppError>;
    /// T2-P0-017 Phase3 / PR-M：行级强一致编辑。**默认实现** 返回 `Unsupported` 错误，
    /// 让 mock / 简化 executor 不必实现；生产路径由 `DefaultPrimitiveExecutor` 覆盖。
    ///
    /// 入参为已解析的 [`crate::core::tools::primitive::executor::hashline_edit::HashlineSegment`]
    /// 列表（`tool_exec` 入口处解析 JSON）。
    async fn hashline_edit(
        &self,
        _path: &str,
        _segments: Vec<HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Err(AppError::Primitive(
            "hashline_edit is not implemented by this PrimitiveExecutor".to_string(),
        ))
    }
    /// 执行 bash/进程。
    /// - `argv` 为 `None`：`command` 视为完整 shell 命令（经 `sh -c` / `cmd /C`）。
    /// - `argv` 为 `Some`：`command` 为可执行文件名，`argv` 为其参数列表（不经 shell，与 pi-mono `exec(cmd, args)` 对齐）。
    /// - `timeout_ms`（T2-P0-016 PR-E）：墙钟超时（毫秒）；`None` 时使用
    ///   `[tools.bash].timeout_ms`（默认 120_000）。`tool_exec` 入口已按
    ///   [`crate::infra::config::types::MAX_TOOLS_BASH_TIMEOUT_MS`] = 600_000 clamp，
    ///   trait 实现侧再做一次防御性 clamp。`DefaultPrimitiveExecutor` 用
    ///   `tokio::time::timeout(..., child.wait())` 包裹等待；超时分支对 `Child` 调用
    ///   `kill` 并 `wait` 收口，避免 `wait_with_output` 反模式（bash.md §2.4.3 / §6.2 / §9.2）。
    ///   旧 mock / 第三方 PrimitiveExecutor 实现可忽略此参数（默认行为不变）。
    async fn execute_bash(
        &self,
        command: &str,
        cwd: Option<&str>,
        plugin_id: &str,
        argv: Option<&[String]>,
        timeout_ms: Option<u64>,
    ) -> Result<BashResult, AppError>;
    async fn require_user_confirmation(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError>;
}
