//! # Agent Loop 错误分类与 L3 上下文溢出回收
//!
//! 本模块承担两类职责（均为 `AgentLoop` 的 **Attempt Loop** 内部逻辑）：
//!
//! 1. [`classify_error`]：把 LLM 返回的 [`AppError`] 映射为
//!    [`LoopError::Retryable`] 或 [`LoopError::Fatal`]，供第二层循环决定
//!    "指数退避重试"或"立即终止"。
//! 2. [`handle_overflow_retry`]：当 Attempt Loop 捕获 `Retryable(context_overflow)`
//!    时，对 `context_state` 做一次 L3 强制截断（`force_drop_oldest_after_confirmed_overflow`）并
//!    直接截断工作消息列表，发送 `ContextOverflowTrimStart/End` 事件，更新压缩计数，
//!    供下一轮 Attempt 重试。
//!
//! 历史：原 `classify_error` 位于 `convert.rs`；L3 trim 逻辑内联在 `run.rs` 的
//! `run_attempt_loop` 中（约 90 行）。T2-P0-001 将两者聚合到本文件，一方面为 T2-P0-003
//! `ToolLoopGuard` / T2-P0-002 Compaction prompt 预留一个明确的"错误与回收"领域入口，
//! 另一方面让 `run.rs` 的 Attempt Loop 只关心"调度与收尾判定"。

use tracing::info;

use crate::core::agent_loop::current_tail_guard::collapse_to_branch_summary;
use crate::core::compaction::force_drop_oldest_after_confirmed_overflow;
use crate::core::llm::{degrade_unsupported_multimodal, Capabilities, ChatMessage};
use crate::core::session::manager::{estimate_msg_chars, estimated_tokens_from_chars};
use crate::infra::error::{
    classify_llm_failure, is_context_overflow, is_unsupported_multimodal_text, llm_http_status,
    llm_stage, llm_summary, AppError, LlmFailureKind,
};
use crate::infra::events::AgentEvent;

use super::types::{AgentLoop, LoopError, OverflowTrimStats, UnsupportedMultimodalRetryStats};

fn err_snippet(s: &str) -> String {
    s.chars().take(200).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsupportedMultimodalKind {
    Vision,
    Files,
    Both,
}

fn is_stream_terminal_error(err: &AppError) -> bool {
    matches!(err, AppError::LlmDetailed(_))
        && llm_stage(err).is_none()
        && llm_http_status(err).is_none()
}

fn unsupported_multimodal_kind_from_text(text: &str) -> Option<UnsupportedMultimodalKind> {
    if !is_unsupported_multimodal_text(text) {
        return None;
    }
    let lower = text.to_lowercase();
    let hits_vision = lower.contains("input_image")
        || lower.contains("image input")
        || lower.contains("image_url");
    let hits_files = lower.contains("input_file")
        || lower.contains("file input")
        || lower.contains("file_data")
        || lower.contains(".pdf")
        || lower.contains(" pdf");
    Some(match (hits_vision, hits_files) {
        (true, false) => UnsupportedMultimodalKind::Vision,
        (false, true) => UnsupportedMultimodalKind::Files,
        _ => UnsupportedMultimodalKind::Both,
    })
}

fn temporary_capabilities_for(kind: UnsupportedMultimodalKind) -> Capabilities {
    let mut capabilities = Capabilities {
        vision: true,
        files: true,
        ..Capabilities::default()
    };
    match kind {
        UnsupportedMultimodalKind::Vision => capabilities.vision = false,
        UnsupportedMultimodalKind::Files => capabilities.files = false,
        UnsupportedMultimodalKind::Both => {
            capabilities.vision = false;
            capabilities.files = false;
        }
    }
    capabilities
}

/// 错误分类：把 `AppError` 映射为 `LoopError::Retryable` / `LoopError::Fatal`。
///
/// 先由 `infra::error::classify_llm_failure` 归一化错误语义，再决定尝试循环策略。
/// 绝不把 HTTP 状态直接当成语义：同一个 403 可以是鉴权或余额不足，流内终局错误则
/// 根本没有 HTTP 状态码。归一化函数内固定 code > type > status > summary 的证据顺序。
pub(super) fn classify_error(err: AppError) -> LoopError {
    let s = err.to_string();
    let snippet = err_snippet(&s);
    let failure = classify_llm_failure(&err);
    let stage = llm_stage(&err);
    let branch = match failure.kind {
        LlmFailureKind::ContextOverflow => "retryable_context_overflow",
        LlmFailureKind::RateLimit => "retryable_rate_limit",
        LlmFailureKind::UpstreamTransient => "retryable_upstream_transient",
        LlmFailureKind::StreamInterrupted => "retryable_stream_interrupted",
        LlmFailureKind::EmptyResponse => "retryable_empty_response",
        LlmFailureKind::UnsupportedMultimodal => "retryable_unsupported_multimodal",
        LlmFailureKind::Billing => "fatal_billing",
        LlmFailureKind::Authentication => "fatal_authentication",
        LlmFailureKind::ContentFiltered => "fatal_content_filtered",
        LlmFailureKind::InvalidRequest => "fatal_invalid_request",
        LlmFailureKind::Unknown if is_stream_terminal_error(&err) => {
            "retryable_stream_terminal_unknown"
        }
        LlmFailureKind::Unknown => "fatal_unknown",
    };
    info!(
        target: "tomcat_chat_diag",
        phase = "classify_error",
        branch,
        failure_kind = failure.kind.as_str(),
        failure_domain = failure.domain.as_str(),
        stage = ?stage,
        http_status = ?llm_http_status(&err),
        snippet = %snippet
    );
    match failure.kind {
        LlmFailureKind::ContextOverflow
        | LlmFailureKind::RateLimit
        | LlmFailureKind::UpstreamTransient
        | LlmFailureKind::StreamInterrupted
        | LlmFailureKind::EmptyResponse
        | LlmFailureKind::UnsupportedMultimodal => LoopError::Retryable(err),
        LlmFailureKind::Unknown if is_stream_terminal_error(&err) => LoopError::Retryable(err),
        LlmFailureKind::Billing
        | LlmFailureKind::Authentication
        | LlmFailureKind::ContentFiltered
        | LlmFailureKind::InvalidRequest
        | LlmFailureKind::Unknown => LoopError::Fatal(err),
    }
}

/// L3 强制截断，仅在 `Retryable` 分支内由 Attempt Loop 调用。
///
/// ## 行为约定
///
/// - 先发 `attempt_loop_retryable` 诊断日志（含 `overflow_hit` / `context_state_some` /
///   `snippet`）——**无论是否命中 overflow 都写**，便于观测哪种路径被触发。
/// - 命中 overflow + `context_state` 存在：
///   1. 发 `ContextOverflowTrimStart { ratio: ratio_before }`
///   2. `force_drop_oldest_after_confirmed_overflow` 截断 → 累计 `compaction_tokens_freed` / `+1 compaction_count`
///   3. 若历史前缀已空，立刻折叠整个工作列表，确保重试请求发生实质缩小
///   4. 发 `ContextOverflowTrimEnd { ratio_before, ratio_after, will_retry: true, .. }`
///   5. 写诊断 `l3_trim_done`（含 `compaction_count_after`），返回 `applied: true`
/// - 命中 overflow 但 `context_state.is_none()`：
///   写诊断 `l3_skipped_no_context_state`；返回 `applied: false`、**不发**任何事件。
/// - 未命中 overflow：写诊断 `l3_skipped_not_overflow`；返回 `applied: false`、**不发**任何事件。
///
/// ## 所有权边界
///
/// - `err: &str` 仅用于 `attempt_loop_retryable` 诊断日志的 `snippet` 字段（200 字符截断）。
/// - **不**在本函数内更新 `last_err` 或判断 `attempt == max_attempts` —— 那两个决定仍由
///   `run_attempt_loop` 持有，避免 retry 控制流所有权扩散。
/// - 事件通过 `agent.emit_event(...)`（`pub(super)`）发射；时序严格保持
///   `ContextOverflowTrimStart` → （trim/collapse） → `ContextOverflowTrimEnd` 各一次。
pub(super) async fn handle_overflow_retry(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    attempt: u32,
    err: &AppError,
) -> OverflowTrimStats {
    let err_text = err.to_string();
    let overflow_hit = is_context_overflow(err);
    let context_state_some = agent.context_state.is_some();
    let err_snip = err_snippet(&err_text);
    info!(
        target: "tomcat_chat_diag",
        phase = "attempt_loop_retryable",
        attempt,
        overflow_hit,
        context_state_some,
        snippet = %err_snip
    );

    if !overflow_hit {
        info!(
            target: "tomcat_chat_diag",
            phase = "l3_skipped_not_overflow",
            attempt
        );
        return OverflowTrimStats::default();
    }

    if !context_state_some {
        info!(
            target: "tomcat_chat_diag",
            phase = "l3_skipped_no_context_state",
            attempt
        );
        return OverflowTrimStats::default();
    }

    // ratio_before 走**只读借用**——与后续 `if let Some(ref mut ctx_state)` 可变借用
    // 分段隔离，保证借用检查器满意（与原 run.rs:323-386 借用结构一致）。
    let ratio_before = agent
        .context_state
        .as_ref()
        .map(|cs| cs.usage_ratio())
        .unwrap_or(0.0);

    let before_message_count = messages.len();
    let before_chars = messages.iter().map(estimate_msg_chars).sum::<usize>();

    agent.emit_event(AgentEvent::ContextOverflowTrimStart {
        reason: "context_overflow".into(),
        ratio: ratio_before,
    });

    let mut trim_tokens = 0usize;
    let mut trim_turns = 0usize;
    let mut collapse_failed = None;
    let mut collapsed = attempt >= 2;
    if collapsed {
        // 被动 overflow 的第二次命中直接复用主动侧最强的 Collapse，不另造一套
        // “递进裁剪”。这条路径能覆盖原先 L3 永远原样保留 tail 的盲点。
        if let Err(error) = collapse_to_branch_summary(agent, messages).await {
            collapse_failed = Some(error);
        }
    } else if let Some(ref mut ctx_state) = agent.context_state {
        let (turns_removed, chars_removed) =
            force_drop_oldest_after_confirmed_overflow(ctx_state, messages, &mut agent.start_idx);
        trim_turns = turns_removed;
        trim_tokens = estimated_tokens_from_chars(chars_removed);
        if chars_removed > 0 {
            ctx_state.session_obs.compaction_tokens_freed += trim_tokens;
            ctx_state.session_obs.compaction_count =
                ctx_state.session_obs.compaction_count.saturating_add(1);
        }
    }
    if !collapsed && trim_turns == 0 {
        // No completed historical turn remains. Retrying the original payload cannot recover,
        // so collapse the current tail in this same overflow attempt.
        collapsed = true;
        if let Err(error) = collapse_to_branch_summary(agent, messages).await {
            collapse_failed = Some(error);
        }
    }
    let after_message_count = messages.len();
    let after_chars = messages.iter().map(estimate_msg_chars).sum::<usize>();
    // “裁剪成功”不能只看函数是否被调用。至少消息条数或估算字符数必须严格下降；
    // Collapse 可合法地用一条摘要替换一条超长 user 消息，因此不能要求两者同时下降。
    let applied = collapse_failed.is_none()
        && (after_message_count < before_message_count || after_chars < before_chars);

    let ratio_after = agent
        .context_state
        .as_ref()
        .map(|cs| cs.usage_ratio())
        .unwrap_or(0.0);
    agent.emit_event(AgentEvent::ContextOverflowTrimEnd {
        ratio_before,
        ratio_after,
        will_retry: applied,
        estimated_tokens_freed: trim_tokens,
        turns_removed: trim_turns,
    });

    let compaction_count_after = agent
        .context_state
        .as_ref()
        .map(|cs| cs.session_obs.compaction_count)
        .unwrap_or(0);
    info!(
        target: "tomcat_chat_diag",
        phase = "l3_trim_done",
        attempt,
        turns_removed = trim_turns,
        trim_tokens,
        route = if collapsed { "collapse" } else { "reduce" },
        before_message_count,
        after_message_count,
        before_chars,
        after_chars,
        applied,
        collapse_error = ?collapse_failed.as_ref().map(ToString::to_string),
        ratio_before,
        ratio_after,
        compaction_count_after
    );

    OverflowTrimStats {
        trim_tokens,
        trim_turns,
        ratio_before,
        ratio_after,
        applied,
    }
}

pub(super) fn handle_unsupported_multimodal_retry(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    attempt: u32,
    err: &AppError,
) -> UnsupportedMultimodalRetryStats {
    let summary = llm_summary(err).unwrap_or_else(|| err.to_string());
    let Some(kind) = unsupported_multimodal_kind_from_text(&summary) else {
        info!(
            target: "tomcat_chat_diag",
            phase = "unsupported_multimodal_retry_skipped_not_matched",
            attempt
        );
        return UnsupportedMultimodalRetryStats::default();
    };

    let degraded = degrade_unsupported_multimodal(messages, &temporary_capabilities_for(kind));
    let std::borrow::Cow::Owned(next_messages) = degraded else {
        info!(
            target: "tomcat_chat_diag",
            phase = "unsupported_multimodal_retry_skipped_no_change",
            attempt
        );
        return UnsupportedMultimodalRetryStats::default();
    };

    *messages = next_messages;
    let stats = UnsupportedMultimodalRetryStats {
        applied: true,
        degraded_vision: matches!(
            kind,
            UnsupportedMultimodalKind::Vision | UnsupportedMultimodalKind::Both
        ),
        degraded_files: matches!(
            kind,
            UnsupportedMultimodalKind::Files | UnsupportedMultimodalKind::Both
        ),
    };
    agent.emit_event(AgentEvent::LlmNotice {
        finish_reason: "unsupported_multimodal_degraded".to_string(),
        message: "本轮附件未被当前端点接受，已按纯文本发送".to_string(),
    });
    info!(
        target: "tomcat_chat_diag",
        phase = "unsupported_multimodal_retry_applied",
        attempt,
        degraded_vision = stats.degraded_vision,
        degraded_files = stats.degraded_files
    );
    stats
}
