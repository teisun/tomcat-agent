use serde::{Deserialize, Serialize};

/// 工具子系统配置：每个内建工具的可调上限聚合在此表，避免 `LlmConfig` / `PrimitiveConfig`
/// 等已有结构再被工具相关字段污染（与 `docs/architecture/tools/read.md` §3.4 对齐）。
///
/// **设计口径**（与 `read.md` §3.4 一致）：
/// - 仅放「磁盘资源 / 安全相关」的硬上限；
/// - **不**放可由 LLM 通过 schema 字段直接控制的开关（如 `line_numbers` / `hashline`），
///   避免管理员侧静默改变模型上下文。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub read: ToolsReadConfig,
    #[serde(default)]
    pub write: ToolsWriteConfig,
    #[serde(default)]
    pub bash: ToolsBashConfig,
}

/// `[tools.read]` 子表：当前仅含 `max_bytes`。
///
/// `max_bytes` 是 **read 工具文本路径的「裸读字节上限」**：
/// - 当模型**未传** `offset` / `limit` 时，先在 `std::fs::metadata().len()` 阶段
///   与该值比对，超限直接返结构化错误，**不**触发任何 `read_to_*`；
/// - 当模型传入 `offset` / `limit`（即明确分窗）时，**不**触发该上限——
///   合理 dump / 大日志可被分窗读取（详见 `read.md` §2.5 决策图）。
///
/// 默认 25 MiB（介于 cc-fork 的 256 KiB 与 pi_agent_rust 的 100 MiB 之间，
/// 兼顾「合理 dump 文件」与「防爆 ctx」），可通过
/// `tomcat.config.toml [tools.read] max_bytes = ...` 或环境变量
/// `TOMCAT__TOOLS__READ__MAX_BYTES` 覆盖。图片 / PDF inline 上限由
/// `core::llm::types` 集中管理，**不**进 config。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsReadConfig {
    #[serde(default = "default_tools_read_max_bytes")]
    pub max_bytes: u64,
}

/// 25 MiB；read.md §2.5 决策表 R6 #2「自设」入选值。
pub const DEFAULT_TOOLS_READ_MAX_BYTES: u64 = 25 * 1024 * 1024;

fn default_tools_read_max_bytes() -> u64 {
    DEFAULT_TOOLS_READ_MAX_BYTES
}

impl Default for ToolsReadConfig {
    fn default() -> Self {
        Self {
            max_bytes: default_tools_read_max_bytes(),
        }
    }
}

/// `[tools.write]` 子表：当前仅含 `normalize_crlf`（PR-G）。
///
/// 与 `read.md` § 工具子系统配置一致的设计口径：仅放「磁盘 / 安全相关」全局开关；
/// `normalize_crlf` 控制 [`crate::core::tools::primitive::executor::write_edit::write_file_impl`]
/// 写入字节前是否将 `\r\n` 折叠为 `\n`（与 [write.md](../../../docs/architecture/tools/write.md)
/// §3.3 / §8 一致）。**默认 `true`**：跨平台仓库统一收 `\n`，行为与
/// pi-mono / cc-fork-01 同档。
///
/// **schema 决策（write.md §4.1）**：**不**新增 per-call `normalize_line_endings?` 字段，
/// 避免 schema 多一维让 LLM 混淆；用户可通过 `tomcat.config.toml [tools.write] normalize_crlf = false`
/// 或环境变量 `TOMCAT__TOOLS__WRITE__NORMALIZE_CRLF=false` 关掉。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsWriteConfig {
    #[serde(default = "default_tools_write_normalize_crlf")]
    pub normalize_crlf: bool,
}

/// 默认开启 LF 规范化（write.md §3.3 / §8 决策表）。
pub const DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF: bool = true;

fn default_tools_write_normalize_crlf() -> bool {
    DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF
}

impl Default for ToolsWriteConfig {
    fn default() -> Self {
        Self {
            normalize_crlf: default_tools_write_normalize_crlf(),
        }
    }
}

/// `[tools.bash]` 子表（T2-P0-016 PR-E / `bash.md` §8）。
///
/// 与 `read` / `write` 同口径，仅放「磁盘资源 / 安全相关」全局开关，**不**放可由 LLM
/// 直接通过 schema 字段控制的开关。
///
/// - `timeout_ms`：默认墙钟超时（毫秒），由 [`crate::core::tools::primitive::executor::bash`]
///   传给 `tokio::time::timeout(..., child.wait())`。配置默认 [`DEFAULT_TOOLS_BASH_TIMEOUT_MS`]
///   = 120_000（2 分钟，对齐 bash.md §2.4.3 / §6.2）；模型可在 schema `timeout_ms` 字段
///   显式上调，但 [`crate::core::agent_loop::tool_exec`] 会以
///   [`MAX_TOOLS_BASH_TIMEOUT_MS`] = 600_000（10 分钟）封顶。
/// - `max_output_chars`：单次 bash 调用 stdout / stderr **合并后字符数**上限（与
///   bash.md §8 / cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT=30_000` 同档）。超限走
///   `EndTruncatingAccumulator` 风格头尾保留 + 整段落盘
///   `~/.tomcat/agents/<id>/tool-results/...`，调用回执带 `truncated=true` /
///   `persisted_output_path`（详见 bash.md §2.4.3）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsBashConfig {
    #[serde(default = "default_tools_bash_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_tools_bash_max_output_chars")]
    pub max_output_chars: usize,
}

/// 默认 bash 墙钟超时：120_000 ms（2 分钟）。bash.md §2.4.3 / §6.2 / §9.2 钉死。
pub const DEFAULT_TOOLS_BASH_TIMEOUT_MS: u64 = 120_000;

/// bash 墙钟超时上限：600_000 ms（10 分钟）。schema `timeout_ms` 大于此值会被 `tool_exec`
/// 在解析阶段 clamp（避免把任何模型可见上限漂移到 catalog schema 之外）。
///
/// **Phase-E.1**：仅声明；**Phase-E.2** 在 [`crate::core::agent_loop::tool_exec`] 与
/// [`crate::core::tools::primitive::executor::bash`] 中作为 clamp 使用。
#[allow(dead_code)]
pub const MAX_TOOLS_BASH_TIMEOUT_MS: u64 = 600_000;

/// 默认 bash 输出字符上限：30_000（cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT` 同档）。
pub const DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 30_000;

/// bash 输出字符上限的硬上限：150_000（cc-fork-01 `BASH_MAX_OUTPUT_UPPER_LIMIT` 同档），
/// 用于 [`ToolsBashConfig`] 反序列化后的越界保护（可在 [`crate::infra::config::load`] 校验）。
///
/// **Phase-E.1**：仅声明；**Phase-E.3** 由 `output_accum` 在合并流时作为硬上限引用。
#[allow(dead_code)]
pub const MAX_TOOLS_BASH_MAX_OUTPUT_CHARS: usize = 150_000;

fn default_tools_bash_timeout_ms() -> u64 {
    DEFAULT_TOOLS_BASH_TIMEOUT_MS
}

fn default_tools_bash_max_output_chars() -> usize {
    DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
}

impl Default for ToolsBashConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_tools_bash_timeout_ms(),
            max_output_chars: default_tools_bash_max_output_chars(),
        }
    }
}
