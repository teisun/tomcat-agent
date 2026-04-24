use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::primitives::PrimitiveExecutor;
use crate::core::session::manager::ContextState;
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
    /// Agent 工作目录（Layer 0 落盘路径根）。空字符串时 Layer 0 降级截断。
    pub work_dir: String,
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
            work_dir: String::new(),
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
