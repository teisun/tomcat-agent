//! # Reasoning Loop 收束分支：text-only 回合的 timing ⑤ + TurnEnd 发射
//!
//! 当 LLM 本轮**没有产出 tool_calls**（纯文本回复）时，reasoning loop 不再继续，
//! 进入"收束分支"做四步 cleanup：
//!
//! 1. `on_message_appended(content_buf.len())` + `messages.push(assistant)`
//! 2. **Timing ⑤**：L0 cleanup → preheat.try_restart_if_pending → L2
//!    `check_after_reply`（仅 ratio ≥ 0.85）→ preheat.try_start（Idle → Running）
//! 3. 条件发射 `Layer0ContextRelease` / `AutoCompactionStart`
//! 4. `emit_context_metrics()` + `TurnEnd { tool_results: [] }`
//!
//! 历史：原嵌在 `run.rs::run_reasoning_loop` 的 `if tool_calls.is_empty()` 分支
//! 内（约 80 行）。Phase 3 抽出为本文件的自由函数后，`run_reasoning_loop` 主体
//! 只关心"取消预检 / TurnStart / Stream 调度 / Tool Dispatch"四件事，骨架更清晰。

use std::sync::Arc;

use crate::core::compaction::run_layer0_cleanup;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::estimated_tokens_from_chars;
use crate::infra::events::{AgentEvent, Message};

use super::types::AgentLoop;

/// 处理 text-only 回合的全部副作用：消息落盘、timing ⑤、收束事件发射。
///
/// **必须在 `tool_calls.is_empty()` 分支调用，且仅调用一次**——重复调用会重复
/// `on_message_appended` 计费、重复发 `TurnEnd`。
///
/// `content_buf`：本轮 delta 累积。`turn_index`：作为 `TurnEnd` 的 turn 序号。
pub(super) fn finalize_turn_after_text(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    content_buf: &str,
    turn_index: usize,
) {
    if let Some(ref mut ctx_state) = agent.context_state {
        ctx_state.on_message_appended(content_buf.len());
    }
    messages.push(ChatMessage::assistant(content_buf));

    // Timing ⑤: L0 → try_restart → check_after_reply → try_start → metrics
    let mut preheat_started: Option<(usize, f64)> = None;
    let mut layer0_release: Option<(usize, usize)> = None;
    if let Some(ref mut ctx_state) = agent.context_state {
        // Step 1: L0 cleanup
        let l0 = run_layer0_cleanup(
            ctx_state,
            &agent.config.context_config,
            std::path::Path::new(&agent.config.agent_trail_dir),
            &agent.config.session_id,
        );
        for pr in &l0.persisted {
            ctx_state.session_obs.tool_result_chars_persisted += pr.original_chars;
        }
        let persist_tok = estimated_tokens_from_chars(l0.persist_chars_freed);
        let placeholder_tok = estimated_tokens_from_chars(l0.placeholder_chars_freed);
        if persist_tok > 0 || placeholder_tok > 0 {
            ctx_state.session_obs.compaction_tokens_freed += persist_tok + placeholder_tok;
            layer0_release = Some((persist_tok, placeholder_tok));
        }

        // Step 2: restore ExhaustedPending → Running
        ctx_state.preheat.try_restart_if_pending(
            ctx_state.usage_ratio(),
            &ctx_state.messages,
            &ctx_state.transcript_path,
            Arc::clone(&agent.llm),
            &agent.config.context_config,
            Arc::clone(&agent.event_bus),
        );

        // Step 3: L2 non-blocking poll + apply boundary
        if ctx_state.usage_ratio() >= 0.85 {
            crate::core::compaction::apply::check_after_reply(ctx_state, &*agent.event_bus);
        }

        // Step 4: Idle → Running (start new preheat if conditions met)
        let ratio = ctx_state.usage_ratio();
        let turn_count = ctx_state.turn_count();
        if ctx_state.preheat.try_start(
            ratio,
            &ctx_state.messages,
            &ctx_state.transcript_path,
            Arc::clone(&agent.llm),
            &agent.config.context_config,
            Arc::clone(&agent.event_bus),
        ) {
            preheat_started = Some((turn_count, ratio));
        }
    }

    if let Some((p, ph)) = layer0_release {
        agent.emit_event(AgentEvent::Layer0ContextRelease {
            persist_tokens_freed: p,
            placeholder_tokens_freed: ph,
        });
    }
    if let Some((covered_count, ratio_before)) = preheat_started {
        agent.emit_event(AgentEvent::AutoCompactionStart {
            covered_count,
            ratio_before,
        });
    }

    agent.emit_context_metrics();
    agent.emit_event(AgentEvent::TurnEnd {
        session_id: agent.config.session_id.clone(),
        turn_index,
        message: Message(serde_json::json!({})),
        tool_results: vec![],
    });
}
