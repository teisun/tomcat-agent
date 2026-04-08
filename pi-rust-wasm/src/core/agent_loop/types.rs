use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::core::llm::LlmProvider;
use crate::core::primitives::PrimitiveExecutor;
use crate::core::session::manager::ContextState;
use crate::infra::config::ContextConfig;
use crate::infra::event_bus::EventBus;

// ─── 5.1 AgentMessage 与转换 ───────────────────────────────────────────────

/// 单次工具调用信息（与 OpenAI 流式 tool_calls 对应）。
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Agent 内部富类型消息；仅在调 LLM 边界转为 ChatMessage。
#[derive(Debug, Clone)]
pub enum AgentMessage {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallInfo>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    System {
        text: String,
    },
    Steering {
        text: String,
        timestamp: i64,
    },
    CompactionSummary {
        summary: String,
    },
}

// ─── 5.7 错误分类与重试 ─────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LoopError {
    Retryable(String),
    Fatal(String),
    Aborted,
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

#[derive(Debug)]
pub struct AgentRunResult {
    pub final_text: String,
    pub new_messages: Vec<AgentMessage>,
}

// ─── AgentLoop 结构体 ───────────────────────────────────────────────────────

pub struct AgentLoop {
    pub(super) llm: Arc<dyn LlmProvider>,
    pub(super) primitive: Arc<dyn PrimitiveExecutor>,
    pub(super) event_bus: Arc<dyn EventBus>,
    pub(super) config: AgentLoopConfig,
    pub(super) steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub(super) follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    pub(super) abort_signal: Arc<AtomicBool>,
    pub(super) context_state: Option<ContextState>,
    pub(super) block_tool_calls: bool,
    pub(super) start_idx: usize,
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
