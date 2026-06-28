use serde::{Deserialize, Serialize};

use super::core::default_true;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServeTransport {
    #[default]
    Stdio,
    Ws,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServeConfig {
    #[serde(default)]
    pub transport: ServeTransport,
    #[serde(default = "default_serve_max_sessions")]
    pub max_sessions: usize,
    // TODO(next): implement idle ChatContext eviction before advertising this as live behavior.
    #[serde(default)]
    pub session_idle_unload_ms: u32,
    #[serde(default = "default_serve_delta_coalesce_ms")]
    pub delta_coalesce_ms: u32,
    #[serde(default = "default_serve_max_buffered_frames")]
    pub max_buffered_frames: usize,
    #[serde(default)]
    pub schema_out_dir: Option<String>,
}

fn default_serve_max_sessions() -> usize {
    crate::core::agent_registry::MAX_CONCURRENT_AGENTS as usize
}

fn default_serve_delta_coalesce_ms() -> u32 {
    25
}

fn default_serve_max_buffered_frames() -> usize {
    64
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            transport: ServeTransport::default(),
            max_sessions: default_serve_max_sessions(),
            session_idle_unload_ms: 0,
            delta_coalesce_ms: default_serve_delta_coalesce_ms(),
            max_buffered_frames: default_serve_max_buffered_frames(),
            schema_out_dir: None,
        }
    }
}

/// `[plan]` 子表（T2-P1-002 PR-PLA/PLB）：PLAN 模式运行时全局参数。
///
/// 设计口径：仅放「锁等待 / 自动 checkpoint 开关」这类**运行时与磁盘资源**相关的全局
/// 参数；reviewer 子流程参数独立到 [`ReviewerConfig`]，避免单表字段膨胀。
///
/// 详见 `docs/architecture/plan-runtime.md`（PR-PLB / PR-PLE）。
///
/// env 覆盖：
/// - `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS` → `lock_timeout_ms`
/// - `TOMCAT_PLAN_AUTO_CHECKPOINT_ON_BUILD` → `auto_checkpoint_on_build`
/// - `TOMCAT_PLAN_MAX_REVIEW_ROUNDS` → `max_review_rounds`
///
/// `verify_gate` 暂不提供 env 覆盖：只走 `[plan].verify_gate = "soft" | "gate"`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanConfig {
    /// `~/.tomcat/plans/*.plan.md` advisory lock 等待上限（毫秒）。默认 2000。
    #[serde(default = "default_plan_lock_timeout_ms")]
    pub lock_timeout_ms: u64,
    /// `/plan build` 时是否自动打 `CheckpointKind::Manual{label:"plan_build:<id>"}`。默认 false。
    #[serde(default)]
    pub auto_checkpoint_on_build: bool,
    /// 单 plan 累计 review 软上限：超过时仅 transcript warning，**不**阻 `create_plan` / `/plan build`。
    /// 默认 1（仅首次 create_plan 必跑；后续 update_plan 不再触发，由调用方控制）。
    #[serde(default = "default_plan_max_review_rounds")]
    pub max_review_rounds: u32,
    /// EXEC 完成前 code reviewer 的最大尝试轮次。默认 1；0 表示直接跳过 code review。
    #[serde(default = "default_plan_max_code_review_rounds")]
    pub max_code_review_rounds: u32,
    /// Verifier gate 模式：`soft`（默认，FAIL 仅 advisory）或 `gate`（FAIL 阻止 completed）。
    #[serde(default = "default_plan_verify_gate")]
    pub verify_gate: String,
}

fn default_plan_lock_timeout_ms() -> u64 {
    2000
}

fn default_plan_max_review_rounds() -> u32 {
    1
}

fn default_plan_max_code_review_rounds() -> u32 {
    1
}

fn default_plan_verify_gate() -> String {
    "soft".to_string()
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            lock_timeout_ms: default_plan_lock_timeout_ms(),
            auto_checkpoint_on_build: false,
            max_review_rounds: default_plan_max_review_rounds(),
            max_code_review_rounds: default_plan_max_code_review_rounds(),
            verify_gate: default_plan_verify_gate(),
        }
    }
}

/// `[ask_question]` 子表：`ask_question` 工具运行时参数。
///
/// env 覆盖：
/// - `TOMCAT_ASK_QUESTION_TIMEOUT_MS` → `timeout_ms`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AskQuestionConfig {
    /// 一次 ask_question 调用等待用户输入的墙钟超时（毫秒）。0 表示无超时。默认 300_000 ms = 5 min。
    #[serde(default = "default_ask_question_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_ask_question_timeout_ms() -> u64 {
    0
}

impl Default for AskQuestionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: default_ask_question_timeout_ms(),
        }
    }
}

/// `[todos]` 子表：session-local todos 持久化与生命周期参数（GAP-N12 / G3）。
///
/// env 覆盖：
/// - `TOMCAT_TODOS_AUTO_NEW_TODOS_ON_REPLACE_AFTER_TERMINAL` → `auto_new_todos_on_replace_after_terminal`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TodosConfig {
    /// 当上一个 active todos 进入 terminal（全 completed / cancelled）后，下一次 todos replace 调用是否自动
    /// 视为「new todos」开启新文件（而不是 in-place 覆盖原文件）。默认 true。
    #[serde(default = "default_true")]
    pub auto_new_todos_on_replace_after_terminal: bool,
}

impl Default for TodosConfig {
    fn default() -> Self {
        Self {
            auto_new_todos_on_replace_after_terminal: true,
        }
    }
}

/// `[reviewer]` 子表（T2-P1-004 RV-A/B/E）：reviewer 内联子 Agent 派发参数。
///
/// reviewer 子 Agent 走 `AgentRegistry::spawn_subagent_internal`，**不**进 LLM catalog；
/// 本表仅控制：子 loop 最大轮次、（可选）覆盖父 model。
///
/// **改稿权 (`allow_review_edit`) 已固定为 `true`**——实现层硬编码，不再提供配置项。
/// 历史变更：移除 `TOMCAT_REVIEWER_DEFAULT_ALLOW_EDIT` / `[reviewer].default_allow_edit`。
///
/// env 覆盖（plan §P0.5 / reviewer §11）：
/// - `TOMCAT_REVIEWER_MAX_TURNS` → `max_turns`（默认 64）
/// - `TOMCAT_REVIEWER_MODEL` → `model_override`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewerConfig {
    /// reviewer 子 loop 最大 LLM reasoning 轮次（映射到 `AgentLoopConfig.max_tool_rounds`）。
    /// 默认 64；transcript 落 `reviewer_turns_used/limit/stop_reason` 便于调参。
    #[serde(default = "default_reviewer_max_turns")]
    pub max_turns: u32,
    /// 显式覆盖 reviewer 子 Agent 使用的 LLM 模型；`None` 时继承父 Agent。
    #[serde(default)]
    pub model_override: Option<String>,
}

fn default_reviewer_max_turns() -> u32 {
    64
}

impl Default for ReviewerConfig {
    fn default() -> Self {
        Self {
            max_turns: default_reviewer_max_turns(),
            model_override: None,
        }
    }
}
