//! # AgentLoop 公共构造器、访问器与事件发射辅助
//!
//! 本文件存放 [`AgentLoop`] 的构造器（`new` / `new_with_steering_queue`）、
//! 用户面控制访问器（`steer` / `follow_up` / `abort` / `cancel_token`）、
//! 上下文存档读写（`set_context_state` / `take_context_state`）以及
//! `pub(super)` 级的事件发射工具（`emit_event` / `emit_extension_event` /
//! `emit_context_metrics`）和 abort 错误构造器（`make_aborted`）。
//!
//! ## 为什么独立成文件（而非合入 `types.rs`）
//!
//! [PLAN_SPEC §A](../../../docs/openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
//! 要求每个 `.rs` 业务文件 ≤ 300 行。`types.rs` 已含枚举 / 结构体定义 + Outcome
//! 三件套（`OverflowTrimStats` / `StreamOutcome` / `DispatchOutcome`）241 行；
//! 再追加访问器与 emit 辅助（合计 ~120 行）会超阈值。计划风险表的备选方案即
//! 抽 `accessors.rs`：types.rs 仅保留"纯类型 / 常量"，本文件聚合"impl 行为"。
//!
//! ## 与 `run.rs` / `stream_handler.rs` / `tool_dispatcher.rs` 的关系
//!
//! - `emit_event` / `emit_extension_event` / `emit_context_metrics` 设为
//!   `pub(super)`：被 `error_classifier.rs` / `stream_handler.rs` /
//!   `tool_dispatcher.rs` / `turn_finalize.rs` 通过 `agent.emit_*(...)` 调用。
//! - `make_aborted` 设为 `pub(super)` 且签名为 `&self`（仅读 `start_idx`），
//!   解除"`&agent.primitive` 共享借用 + `&mut agent` 可变借用"在 select! 内
//!   的冲突，是 Phase 2.3 抽 `tool_dispatcher` 的前置必做项。

use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::session::manager::ContextState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::infra::event_bus::{EventBus, EventContext};
use crate::infra::events::{AgentEvent, ExtensionEvent};

use super::types::{AgentLoop, AgentLoopConfig, BackgroundCompletionRoutes, LoopError};

impl AgentLoop {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config_backend: None,
            bash_task_registry: None,
            web_fetch_runtime: None,
            web_search_runtime: None,
            config,
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            completion_routes: None,
            cancel_token,
            context_state: None,
            block_tool_calls: false,
            reasoning_turn_budget_exhausted: false,
            start_idx: 0,
            context_tail_start: 0,
        }
    }

    /// 注入 `config_get` / `config_set` 工具后端（PR-7）；返回 `self` 形成 builder 链。
    /// `chat::ChatContext::from_config` 在创建 `AgentLoop` 后立即调用此方法。
    pub fn with_config_backend(
        mut self,
        backend: super::config_backend::SharedConfigBackend,
    ) -> Self {
        self.config_backend = Some(backend);
        self
    }

    /// T2-P0-016 PR-I：注入 bash 后台任务注册表，启用
    /// `bash run_in_background=true` / `task_output` / `task_stop` / `task_list`
    /// 四件套；不调用此方法时上述工具命中返回「未启用」错误，同步 `bash` 不受影响。
    pub fn with_bash_task_registry(
        mut self,
        registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    ) -> Self {
        self.bash_task_registry = Some(registry);
        self
    }

    /// T2-P1-013：注入 `web_fetch` 会话级 runtime。
    pub fn with_web_fetch_runtime(
        mut self,
        runtime: Arc<crate::core::tools::web_fetch::WebFetchRuntime>,
    ) -> Self {
        self.web_fetch_runtime = Some(runtime);
        self
    }

    /// T2-P1-012：注入 `web_search` 会话级 runtime。
    pub fn with_web_search_runtime(
        mut self,
        runtime: Arc<crate::core::tools::web_search::WebSearchRuntime>,
    ) -> Self {
        self.web_search_runtime = Some(runtime);
        self
    }

    /// P1：注入 `ChatContext` 持有的 session 级共享 `follow_up_queue`。
    ///
    /// 不调用此方法时保持原有"单次 AgentLoop 私有 queue"语义（向后兼容
    /// `agent_loop_tests.rs` 的 FollowUp 单测）；调用后：
    /// - host lifecycle subscriber 推入的 synthetic notification 会落到这份共享 queue；
    /// - 同一 `run_chat_turn` 内 conversation loop 在每个 attempt 成功后 drain
    ///   该 queue 进入下一次 reasoning loop，让后台完成事件能在同一 turn 内被消费；
    /// - 跨 turn 时，host between-turns drain 会接管未消费的纸条。
    pub fn with_shared_follow_up_queue(mut self, queue: Arc<Mutex<Vec<ChatMessage>>>) -> Self {
        self.follow_up_queue = queue;
        self
    }

    /// P1：注入 `ChatContext` 持有的 session 级 `completion_routes` 路由表。
    ///
    /// `task_output(block=true)` 路径据此做 claim-on-entry，并在 wake 时按
    /// `Finished / NewOutput / Timeout / Cancel` 四态维护 entry；详见
    /// [`super::types::CompletionRoute`] 与 docs。
    pub fn with_completion_routes(mut self, routes: BackgroundCompletionRoutes) -> Self {
        self.completion_routes = Some(routes);
        self
    }

    /// P1：typed follow-up 入口。`follow_up(String)` 内部仍 `ChatMessage::user`
    /// 包装；当调用方需要直接塞已经构造好的 `ChatMessage`（例如 host 构造 synthetic
    /// notification 时希望保留某种 metadata）时使用本方法。当前 P1 实际只用
    /// `ChatMessage::user`，但保留 typed 入口便于后续演进。
    pub fn follow_up_message(&self, msg: ChatMessage) {
        self.follow_up_queue.lock().push(msg);
    }

    /// 测试用：注入 steering_queue，便于 mock 在工具执行中推入 steering 消息。
    #[cfg(test)]
    pub fn new_with_steering_queue(
        llm: Arc<dyn LlmProvider>,
        primitive: Arc<dyn PrimitiveExecutor>,
        event_bus: Arc<dyn EventBus>,
        config: AgentLoopConfig,
        cancel_token: CancellationToken,
        steering_queue: Arc<Mutex<Vec<ChatMessage>>>,
    ) -> Self {
        Self {
            llm,
            primitive,
            event_bus,
            config_backend: None,
            bash_task_registry: None,
            web_fetch_runtime: None,
            web_search_runtime: None,
            config,
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            completion_routes: None,
            steering_queue,
            cancel_token,
            context_state: None,
            block_tool_calls: false,
            reasoning_turn_budget_exhausted: false,
            start_idx: 0,
            context_tail_start: 0,
        }
    }

    pub fn steer(&self, msg: String) {
        self.steering_queue.lock().push(ChatMessage::steering(msg));
    }

    pub fn follow_up(&self, msg: String) {
        self.follow_up_queue.lock().push(ChatMessage::user(msg));
    }

    /// 触发本次 `run` 的取消。幂等且不可逆——调用方需在下一回合前
    /// `new(...)` 时传入新的 `CancellationToken`。
    pub fn abort(&self) {
        self.cancel_token.cancel();
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub fn set_context_state(&mut self, state: Option<ContextState>) {
        self.context_state = state;
    }

    pub fn take_context_state(&mut self) -> Option<ContextState> {
        self.context_state.take()
    }

    pub(super) fn persist_message_if_needed(
        &self,
        msg: &mut ChatMessage,
    ) -> Result<(), crate::infra::error::AppError> {
        let Some(ref sink) = self.config.message_append_sink else {
            return Ok(());
        };
        if msg.msg_id.is_some() {
            return Ok(());
        }
        let row_id = sink.append_message(serde_json::to_value(&*msg)?)?;
        msg.msg_id = Some(row_id);
        Ok(())
    }

    pub(super) fn push_message(
        &self,
        messages: &mut Vec<ChatMessage>,
        mut msg: ChatMessage,
    ) -> Result<(), crate::infra::error::AppError> {
        self.persist_message_if_needed(&mut msg)?;
        messages.push(msg);
        Ok(())
    }

    pub(super) fn sync_persisted_messages_into_context(&mut self, messages: &[ChatMessage]) {
        let Some(ref mut ctx_state) = self.context_state else {
            return;
        };
        for msg in messages {
            let Some(msg_id) = msg.msg_id.as_deref() else {
                continue;
            };
            let exists = ctx_state
                .messages
                .iter()
                .any(|existing| existing.msg_id.as_deref() == Some(msg_id));
            if !exists {
                ctx_state.messages.push(msg.clone());
            }
        }
    }

    /// 刷新实时 token 指标并发射 `ContextMetricsUpdate` 事件（仅当 `context_state`
    /// 存在时）。先用 `&mut ctx_state` 把 `live` 字段刷新一次，再用 `&ctx_state`
    /// 拿快照构造事件——分段借用避免 `emit_event` 的 `&self` 与 `ctx_state`
    /// 的可变借用冲突。
    pub(super) fn emit_context_metrics(&mut self) {
        if let Some(ref mut ctx_state) = self.context_state {
            let input_tokens_used = ctx_state.estimated_token_count();
            let context_utilization_ratio = ctx_state.usage_ratio();
            let preheat_in_progress = ctx_state.preheat.is_warmup_task_active();
            let preheat_result_pending = ctx_state.preheat.preheat_result_pending();
            ctx_state.live.input_tokens_used = input_tokens_used;
            ctx_state.live.context_utilization_ratio = context_utilization_ratio;
            ctx_state.live.preheat_in_progress = preheat_in_progress;
            ctx_state.live.preheat_result_pending = preheat_result_pending && !preheat_in_progress;
        }
        if let Some(ref ctx_state) = self.context_state {
            self.emit_event(AgentEvent::ContextMetricsUpdate {
                input_tokens_used: ctx_state.live.input_tokens_used,
                context_utilization_ratio: ctx_state.live.context_utilization_ratio,
                compaction_count: ctx_state.session_obs.compaction_count,
                compaction_tokens_freed: ctx_state.session_obs.compaction_tokens_freed,
                total_tool_result_bytes_persisted: ctx_state
                    .session_obs
                    .tool_result_chars_persisted,
                preheat_in_progress: ctx_state.live.preheat_in_progress,
                preheat_result_pending: ctx_state.live.preheat_result_pending,
            });
        }
    }

    /// 序列化 `AgentEvent` 为 wire `serde_json::Value`，从 `type` 字段抽事件名，
    /// 通过 `EventBus::emit_sync` 同步派发。`emit_sync` 错误被吞掉（事件总线
    /// 失败不应阻塞主流程）。
    pub(super) fn emit_event(&self, event: AgentEvent) {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let event_name = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ctx = EventContext::new(event_name.clone(), payload);
        let _ = self.event_bus.emit_sync(&event_name, ctx);
    }

    /// `ExtensionEvent`（ToolCall / ToolResult 等）走与 `emit_event` 完全相同的
    /// wire 协议；分两个方法仅为类型签名清晰，运行时行为一致。
    pub(super) fn emit_extension_event(&self, event: ExtensionEvent) {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let event_name = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ctx = EventContext::new(event_name.clone(), payload);
        let _ = self.event_bus.emit_sync(&event_name, ctx);
    }

    /// 构造 `LoopError::Aborted`：
    ///
    /// - `partial_text` 是本轮 assistant 流**已收到**的 delta 拼接（包含将要作为
    ///   partial assistant 写入 messages 的文本）；
    /// - `partial_messages` 取 `messages[start_idx..]`——这是本轮新增的全部消息，
    ///   既包含中断前已完成的 tool_result，也包含即将作为 partial 写入的
    ///   assistant 消息（调用方在进入本函数前已 `push` 到 messages）。
    ///
    /// 签名为 `&self`（仅读 `start_idx`），允许在 `tokio::select!` 内 `&agent.primitive`
    /// 共享借用之后立即调用，无需先 drop primitive 借用。
    pub(super) fn make_aborted(&self, messages: &[ChatMessage], partial_text: String) -> LoopError {
        LoopError::Aborted {
            partial_text,
            partial_messages: messages[self.start_idx..].to_vec(),
        }
    }
}
