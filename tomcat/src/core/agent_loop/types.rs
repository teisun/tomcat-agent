use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::session::manager::ContextState;
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::{CheckpointStore, NoopStore};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;

// ─── ToolCallInfo ─────────────────────────────────────────────────────────

/// 单次工具调用信息（与 OpenAI 流式 tool_calls 对应）。
/// Temporary type: used only during stream accumulation + tool execution;
/// stored in messages as `serde_json::Value` via `ChatMessage::tool_calls`.
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

// ─── 5.7 错误分类与重试 ─────────────────────────────────────────────────────

/// 第二层 / 第三层循环内部错误分类。
///
/// `Aborted` 是**用户中断**（Soft Interrupt）引发的主动退出：
/// 携带本回合已经累积的 partial 文本（assistant 流中断处的 `content_buf`）
/// 和至此追加进 `messages` 的全部新消息（assistant + 已完成的 tool_result），
/// 以便外层 `run()` 把它们装入 `AgentRunResult` 让 `chat_loop` 走与 `Completed`
/// 一致的持久化路径（T-004 / T-017）。
#[derive(Debug)]
pub enum LoopError {
    Retryable(String),
    Fatal(String),
    Aborted {
        partial_text: String,
        partial_messages: Vec<ChatMessage>,
    },
}

// ─── 配置与结果 ─────────────────────────────────────────────────────────────

pub struct AgentLoopConfig {
    pub max_attempts: u32,
    /// 单次 Attempt 最大工具轮次。默认 `usize::MAX`（不限制）；
    /// 上下文预算自然约束轮次。TODO: 待 tool-loop-detection 方案替代。
    pub max_tool_rounds: usize,
    pub retry_base_delay_ms: u64,
    pub model: String,
    pub session_id: String,
    pub tool_definitions: Vec<serde_json::Value>,
    pub context_config: ContextConfig,
    /// Agent 运行态轨迹目录（Layer 0 落盘路径根）。空字符串时 Layer 0 降级截断。
    pub agent_trail_dir: String,
    /// PR-RF（T2-b/c）`read` 工具的会话级 dedup / staleness 表。
    ///
    /// 默认 `Arc::new(ReadFileState::default())`（空表）；`AgentLoop` 析构时
    /// 随 `Arc` 引用计数归零自动释放（即「session 结束自动 cleanup」）。
    /// 跨 session 复用同一 `Arc` 时建议在新 session 起点显式调用
    /// [`ReadFileState::clear`]，避免上一会话的 stamp 干扰新会话的 dedup 判定。
    pub read_file_state: Arc<ReadFileState>,
    /// T2-P0-015：OpenAI Files 会话级运行时（含 client/cache/cleanup registry）。
    /// 不支持 Files 的 provider 该字段为 `None`。
    pub openai_files_runtime: Option<Arc<OpenAiFilesRuntime>>,
    /// Checkpoint 存储：turn_end / interrupt / restore 使用。
    pub checkpoint_store: Arc<dyn CheckpointStore>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_tool_rounds: usize::MAX,
            retry_base_delay_ms: 300,
            model: String::new(),
            session_id: String::new(),
            tool_definitions: Vec::new(),
            context_config: ContextConfig::default(),
            agent_trail_dir: String::new(),
            read_file_state: Arc::new(ReadFileState::default()),
            openai_files_runtime: None,
            checkpoint_store: Arc::new(NoopStore),
        }
    }
}

/// 一次 `AgentLoop::run` 的成功 / 中断共用载荷。
///
/// `Completed` 与 `Interrupted` 都产出本类型，确保 `chat_loop` 两条分支
/// 走同一条持久化路径（`append_message` + `persist_context_observability`）。
#[derive(Debug)]
pub struct AgentRunResult {
    pub final_text: String,
    pub new_messages: Vec<ChatMessage>,
}

/// `AgentLoop::run` 的三态返回：
///
/// - `Completed`：正常收敛（LLM 不再调用工具、tool_rounds 达到上限等）。
/// - `Interrupted`：用户中断（`cancel_token.cancel()`）。`result.new_messages`
///   已包含 partial assistant + 已完成的 tool_result，`result.final_text`
///   为中断处的累计文本，**允许为空**。外层按"成功"持久化。
/// - `Failed`：致命错误（401、非 overflow 400、Retry 耗尽等）。
#[derive(Debug)]
pub enum AgentRunOutcome {
    Completed(AgentRunResult),
    Interrupted(AgentRunResult),
    Failed(AppError),
}

impl AgentRunOutcome {
    /// 测试 / 调用方语法糖：取 `Completed` 载荷，其它分支 panic。
    /// 与旧 `Result<AgentRunResult, _>::unwrap()` 行为对齐，方便 `.await.unwrap()`
    /// 式测试代码无痛迁移。
    #[track_caller]
    pub fn unwrap(self) -> AgentRunResult {
        match self {
            AgentRunOutcome::Completed(r) => r,
            AgentRunOutcome::Interrupted(_) => {
                panic!("AgentRunOutcome::unwrap called on Interrupted")
            }
            AgentRunOutcome::Failed(e) => panic!("AgentRunOutcome::unwrap called on Failed: {e}"),
        }
    }

    /// 测试辅助：仅当 `Failed` 时取出 `AppError`；其它分支 panic。
    #[track_caller]
    pub fn unwrap_err(self) -> AppError {
        match self {
            AgentRunOutcome::Failed(e) => e,
            AgentRunOutcome::Completed(_) => {
                panic!("AgentRunOutcome::unwrap_err called on Completed")
            }
            AgentRunOutcome::Interrupted(_) => {
                panic!("AgentRunOutcome::unwrap_err called on Interrupted")
            }
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, AgentRunOutcome::Completed(_))
    }

    pub fn is_err(&self) -> bool {
        matches!(self, AgentRunOutcome::Failed(_))
    }

    pub fn is_interrupted(&self) -> bool {
        matches!(self, AgentRunOutcome::Interrupted(_))
    }
}

// ─── AgentLoop 结构体 ───────────────────────────────────────────────────────

pub struct AgentLoop {
    pub(super) llm: Arc<dyn LlmProvider>,
    pub(super) primitive: Arc<dyn PrimitiveExecutor>,
    pub(super) event_bus: Arc<dyn EventBus>,
    /// 可选 `config_get` / `config_set` 后端（plan §6 / PR-7）。
    ///
    /// 注入路径：`ChatContext::from_config` 在创建 `AgentLoop` 前构造
    /// `ChatConfigBackend` 并通过 [`AgentLoop::with_config_backend`] 设入；
    /// CLI / 单测路径继续传 `None`，工具命中时返回"未启用"错误（不影响
    /// 4 原语的 execute_tool 主流程）。
    pub(super) config_backend: Option<super::config_backend::SharedConfigBackend>,
    /// T2-P0-016 PR-I：bash 后台任务三件套（task_output / task_stop / task_list）
    /// 共享的注册表。注入路径：`ChatContext::from_config` 用 `agent_trail_dir/tool-results`
    /// 作 persist_dir 构造一份 `Arc<BashTaskRegistry>`，通过
    /// [`AgentLoop::with_bash_task_registry`] 设入；CLI / 单测路径继续传 `None`，
    /// `bash run_in_background=true` / `task_*` 命中时返回「未启用」错误，
    /// 同步 `bash` 路径不受影响。
    pub(super) bash_task_registry: Option<Arc<crate::core::tools::primitive::BashTaskRegistry>>,
    pub(super) config: AgentLoopConfig,
    pub(super) steering_queue: Arc<Mutex<Vec<ChatMessage>>>,
    pub(super) follow_up_queue: Arc<Mutex<Vec<ChatMessage>>>,
    /// 用户中断令牌。`cancel()` 后所有 `select!` 监听分支立即唤醒；
    /// token 是进程级的、可从任意线程调用，**一旦 cancel 不可逆**——
    /// 每回合 `chat_loop` 在 readline 读到非空输入后重建并通过
    /// `new(..., cancel_token)` 注入新 token。
    pub(super) cancel_token: CancellationToken,
    pub(super) context_state: Option<ContextState>,
    pub(super) block_tool_calls: bool,
    pub(super) start_idx: usize,
    /// 首次进入 `run()` 时 `messages` 中自该下标起（含）为**不得被 L3 rebuild 覆盖**的尾部
    ///（当前用户句 / steering + 后续 assistant/tool）。用于 overflow 后只替换 transcript 段。
    pub(super) context_tail_start: usize,
}

pub(super) fn unix_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Default)]
pub(super) struct ToolCallAccumulator {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}

// ─── L3 overflow trim 统计（error_classifier::handle_overflow_retry 返回） ───

// ─── tool_dispatcher 输出 ───────────────────────────────────────────────────

/// `tool_dispatcher::run_tool_calls` 的输出载荷：
///
/// - `tool_results`：按 `tool_calls` 顺序排列的 `Message`（供 `TurnEnd` 事件使用）；
///   包含 `block_tool_calls == true` 时注入的 blocked 占位文本。
/// - `steered == true`：本轮至少有 **1** 个 tool 执行完毕后被 steering queue 打断
///   （queue 非空 → `messages.extend(q.drain(..)) + break`）。调用方应 `continue`
///   下一轮 reasoning loop，让下一次 LLM 请求携带 steering 消息。
///
/// `#[allow(dead_code)]`：`tool_results` 字段当前通过 `_ = outcome.tool_results`
/// 读取；Phase 4 测试将按 `steered / tool_results.len()` 做断言。
#[allow(dead_code)]
pub(super) struct DispatchOutcome {
    pub(super) tool_results: Vec<crate::infra::events::Message>,
    pub(super) steered: bool,
}

// ─── stream_handler 输出 ────────────────────────────────────────────────────

/// `stream_handler::run_chat_stream` 的输出载荷：
///
/// - `content_buf`：本轮 `ContentDelta` 累积的文本（调用方可直接
///   `final_text.push_str(&content_buf)` 或当作 partial assistant 落到 messages）
/// - `tool_calls_buf`：按 `index` 对齐的 `ToolCallAccumulator` 列表（空 `name`
///   的条目由调用方 `.filter` 后再构造 `ToolCallInfo`）
/// - `aborted == true`：中途被 `cancel_token.cancel()`；此时**已经**发射
///   `MessageEnd` 事件，但**尚未** push partial assistant 到 messages 或构造
///   `LoopError::Aborted`——调用方（`run_reasoning_loop` Step 5）负责：
///   1. 若 `content_buf` 非空，`ctx_state.on_message_appended(len)` +
///      `messages.push(ChatMessage::assistant(&content_buf))` +
///      `final_text.push_str(&content_buf)`
///   2. 调用 `agent.make_aborted(messages, final_text)` 返回 `LoopError::Aborted`
///      （"谁拥有 messages 谁负责落盘"原则）
/// - `aborted == false`：正常收敛（`FinishReason` / stream 末尾）或建连前被取消
///   （后者 `content_buf` / `tool_calls_buf` 均为空）。
pub(super) struct StreamOutcome {
    pub(super) content_buf: String,
    pub(super) tool_calls_buf: Vec<ToolCallAccumulator>,
    pub(super) aborted: bool,
}

/// `handle_overflow_retry` 的结果统计：
///
/// - `applied == true`：成功执行了 L3 trim（发送了 `ContextOverflowTrimStart/End` 事件，
///   重建了 `messages`，更新了 `ctx_state.session_obs.compaction_count/tokens_freed`）
/// - `applied == false`：两种跳过场景（均**不发** `ContextOverflowTrim*` 事件，
///   仅写诊断 `info!`）——
///   * `context_state` 为 `None`（诊断日志 `phase="l3_skipped_no_context_state"`）
///   * 错误非 context overflow（诊断日志 `phase="l3_skipped_not_overflow"`）
///
/// `#[allow(dead_code)]`：当前生产代码仅 `let _stats = ...` 消费；Phase 4 单测会按
/// `applied / ratio_before / ratio_after / trim_tokens / trim_turns` 断言，届时移除。
#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub(super) struct OverflowTrimStats {
    pub(super) trim_tokens: usize,
    pub(super) trim_turns: usize,
    pub(super) ratio_before: f64,
    pub(super) ratio_after: f64,
    pub(super) applied: bool,
}
