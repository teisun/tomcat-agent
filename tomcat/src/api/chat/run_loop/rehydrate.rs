use chrono::Utc;
use regex::Regex;
use reqwest::Url;
use serde_json::Value;
use tracing::warn;

use crate::api::chat::ChatContext;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::init_context_state;
use crate::core::session::{
    classify_resumable_ask_question_result, collect_recent_chat_messages_from_tail,
    find_dangling_tail_tool_calls, is_tool_call_pending, ErrorEntry, ResumableAskQuestionResult,
    TranscriptEntry,
};
use crate::infra::error::{
    classify_llm_failure, llm_http_status, llm_source_chain, llm_stage, llm_summary, AppError,
    LlmFailureKind,
};

/// Rebuild the in-memory request context from the durable transcript with the
/// currently selected Main model's limits. All explicit history rewrites use it.
pub(crate) fn reload_context_state(
    ctx: &ChatContext,
    system_text: &str,
) -> Result<crate::core::ContextState, AppError> {
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())?;
    let call = ctx.resolve_call(crate::core::llm::LlmScene::Main, entry.as_ref())?;
    crate::core::session::manager::init_context_state_with_limits(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
        &call.limits,
    )
}

const MAX_ERROR_DETAIL_CHARS: usize = 8 * 1024;

#[derive(Debug, Clone)]
struct ResumableTailAskQuestion {
    tool_call_id: String,
    arguments: Value,
}

fn tool_call_name(tool_call: &Value) -> Option<&str> {
    tool_call.get("function")?.get("name")?.as_str()
}

fn tool_call_arguments(tool_call: &Value) -> Option<Value> {
    serde_json::from_str(tool_call.get("function")?.get("arguments")?.as_str()?).ok()
}

fn find_resumable_tail_ask_questions(entries: &[TranscriptEntry]) -> Vec<ResumableTailAskQuestion> {
    let recent = collect_recent_chat_messages_from_tail(entries);

    if let Some(dangling) = find_dangling_tail_tool_calls(&recent) {
        return dangling
            .into_iter()
            .filter_map(|tool_call| {
                crate::core::tools::contract::catalog::is_replay_safe_tool(&tool_call.name).then(
                    || {
                        Some(ResumableTailAskQuestion {
                            tool_call_id: tool_call.id,
                            arguments: tool_call.arguments?,
                        })
                    },
                )?
            })
            .collect();
    }

    let mut trailing_tools = Vec::new();
    for message in recent.iter().rev() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match role {
            "tool" => trailing_tools.push(message),
            "assistant" => {
                let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
                    return Vec::new();
                };
                if tool_calls.is_empty() || trailing_tools.len() != tool_calls.len() {
                    return Vec::new();
                }
                trailing_tools.reverse();
                if !tool_calls
                    .iter()
                    .zip(&trailing_tools)
                    .all(|(tool_call, result)| {
                        tool_call.get("id").and_then(Value::as_str)
                            == result.get("tool_call_id").and_then(Value::as_str)
                    })
                {
                    return Vec::new();
                }
                return tool_calls
                    .iter()
                    .zip(trailing_tools)
                    .filter_map(|(tool_call, result)| {
                        let content = result.get("content").and_then(Value::as_str)?;
                        let result_state = classify_resumable_ask_question_result(content)?;
                        let is_pending = result_state == ResumableAskQuestionResult::Pending;
                        (tool_call_name(tool_call).is_some_and(
                            crate::core::tools::contract::catalog::is_replay_safe_tool,
                        )
                            // `[pending]` needs the explicit active-result predicate; legacy
                            // host_disconnected has already passed through the superseded-filtered
                            // `recent` chain above and is therefore active by construction.
                            && (!is_pending
                                || is_tool_call_pending(
                                    entries,
                                    tool_call
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default(),
                                )))
                        .then(|| {
                            Some(ResumableTailAskQuestion {
                                tool_call_id: tool_call.get("id")?.as_str()?.to_string(),
                                arguments: tool_call_arguments(tool_call)?,
                            })
                        })?
                    })
                    .collect();
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

async fn execute_resumed_ask_question(
    ctx: &ChatContext,
    tool_call_id: &str,
    arguments: &Value,
) -> Result<Value, AppError> {
    let panel = ctx
        .session_runtime
        .plan_runtime
        .ask_question_panel()
        .ok_or_else(|| {
            AppError::Config("ask_question panel is unavailable during resume".to_string())
        })?;
    crate::core::tools::plan_tool::ask_question::execute_for_tool(
        &ctx.session_runtime.plan_runtime,
        panel.as_ref(),
        arguments,
        crate::core::plan_runtime::AskQuestionTermination::default(),
        Some(tool_call_id),
    )
    .await
    .map_err(|error| AppError::Tool(format!("resume ask_question failed: {error}")))
}

/// Replays restart-interrupted tail `ask_question` calls before a no-input agent turn.
///
/// The normal append path is deliberately used for genuinely dangling calls, so each result still
/// passes `append_message_chain`. Existing placeholders (`[pending]` or legacy
/// `host_disconnected`) are first superseded by `tool_call_id`, then a fresh real result is
/// appended so the transcript remains append-only.
pub(crate) fn has_resumable_tail_ask_question(
    session: &crate::core::session::SessionManager,
) -> Result<bool, AppError> {
    let entries = session.get_entries(256)?;
    Ok(!find_resumable_tail_ask_questions(&entries).is_empty())
}

pub(crate) async fn resume_tail_ask_questions(ctx: &ChatContext) -> Result<bool, AppError> {
    let session = &ctx.session_runtime.session;
    let entries = session.get_entries(256)?;
    let resumable = find_resumable_tail_ask_questions(&entries);
    if resumable.is_empty() {
        return Ok(false);
    }
    for pending in resumable {
        let result =
            execute_resumed_ask_question(ctx, &pending.tool_call_id, &pending.arguments).await?;
        session.replace_tool_result_by_tool_call_id(&pending.tool_call_id, result.to_string())?;
    }
    Ok(true)
}

/// 用户明确开始新输入时，不再让旧的 `[pending]` ask_question 占住本轮。
///
/// 这不是重跑工具：旧问题被记录为 `skipped`，随后用户的新 prompt 才会落盘。这样
/// transcript 与 provider 都能看到完整的 tool round，而不是把 `[pending]` 误当成答案。
pub(crate) fn skip_tail_ask_questions_for_new_input(ctx: &ChatContext) -> Result<bool, AppError> {
    let session = &ctx.session_runtime.session;
    let entries = session.get_entries(256)?;
    let resumable = find_resumable_tail_ask_questions(&entries);
    if resumable.is_empty() {
        return Ok(false);
    }
    let skipped = serde_json::json!({
        "outcome": "skipped",
        "cancelled": true,
        "answers": [],
    })
    .to_string();
    for pending in resumable {
        session.replace_tool_result_by_tool_call_id(&pending.tool_call_id, skipped.clone())?;
    }
    Ok(true)
}

pub(super) fn make_fallback_context_state(
    ctx: &ChatContext,
    system_text: &str,
    context_config: &crate::infra::ContextConfig,
) -> crate::core::ContextState {
    let context_budget_tokens = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
        .ok()
        .flatten()
        .and_then(|entry| {
            ctx.resolve_call(crate::core::llm::LlmScene::Main, Some(&entry))
                .ok()
        })
        .map(|call| call.limits.input_budget_tokens)
        .unwrap_or_else(|| crate::infra::config::fallback_input_budget_tokens(context_config));
    crate::core::ContextState {
        messages: Vec::new(),
        estimate_context_chars: system_text.len(),
        context_budget_chars: crate::infra::config::compute_context_budget_chars_from_tokens(
            context_budget_tokens,
        ),
        context_budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: ctx
            .session_runtime
            .session
            .current_transcript_path()
            .ok()
            .flatten()
            .unwrap_or_default(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

pub(crate) fn is_fatal_error(error: &AppError) -> bool {
    matches!(error, AppError::Config(_))
}

pub(crate) fn is_append_message_chain_invariant(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Invariant {
            stage: "append_message_chain",
            ..
        }
    )
}

pub(super) fn nonfatal_error_hint(error: &AppError) -> &'static str {
    if is_append_message_chain_invariant(error) {
        "(已尝试从磁盘重新对齐上下文；可直接继续输入新消息)\n"
    } else {
        "(已清理失败轮的悬空输入并从磁盘重新对齐上下文；可直接继续输入新消息)\n"
    }
}

fn sanitize_error_detail(raw: &str) -> String {
    let mut sanitized = raw.to_string();
    for (pattern, replacement) in [
        (
            r#"(?i)(authorization\s*[:=]\s*)(?:bearer\s+)?([^\s<>\r\n"']+)"#,
            "$1[REDACTED]",
        ),
        (r"(?i)(bearer\s+)([A-Za-z0-9._~+/=-]+)", "$1[REDACTED]"),
        (
            r#"(?i)(api[_-]?key\s*[:=]\s*["']?)([^"'\s<>\r\n]+)"#,
            "$1[REDACTED]",
        ),
        (
            r#"(?i)\b((?:set-)?cookie\s*[:=]\s*)([^<\r\n]+)"#,
            "$1[REDACTED]",
        ),
        (
            r#"(?i)(x-api-key\s*[:=]\s*["']?)([^"'\s<>\r\n]+)"#,
            "$1[REDACTED]",
        ),
        (
            r#"(?i)\b((?:passwo?rd|secret)\s*[:=]\s*["']?)([^"'\s<>\r\n]+)"#,
            "$1[REDACTED]",
        ),
        (
            r#"(?i)(token\s*[:=]\s*["']?)([^"'\s<>\r\n]+)"#,
            "$1[REDACTED]",
        ),
        (r"(?i)\bsk-[A-Za-z0-9._-]{10,}\b", "[REDACTED]"),
        (r"(?i)(basic\s+)([A-Za-z0-9+/=]{8,})", "$1[REDACTED]"),
    ] {
        sanitized = Regex::new(pattern)
            .expect("error sanitizer regex should compile")
            .replace_all(&sanitized, replacement)
            .into_owned();
    }

    let mut truncated = String::new();
    for (index, ch) in sanitized.chars().enumerate() {
        if index >= MAX_ERROR_DETAIL_CHARS {
            truncated.push_str("\n...[truncated]");
            break;
        }
        truncated.push(ch);
    }
    truncated
}

fn error_detail_text(error: &AppError) -> String {
    let mut detail = llm_summary(error).unwrap_or_else(|| error.to_string());
    let sources = llm_source_chain(error);
    if !sources.is_empty() {
        detail.push_str("\nCaused by:");
        for source in sources {
            detail.push_str("\n- ");
            detail.push_str(&source);
        }
    }
    sanitize_error_detail(&detail)
}

fn extract_request_id(text: &str) -> Option<String> {
    Regex::new(r"(?i)request-id[:=]\s*([A-Za-z0-9._:-]+)")
        .expect("request-id regex should compile")
        .captures(text)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
}

fn extract_gateway_host(text: &str) -> Option<String> {
    if let Some(capture) = Regex::new(r#"https?://[^\s<>'"]+"#)
        .expect("url regex should compile")
        .find(text)
    {
        if let Some(host) = Url::parse(capture.as_str())
            .ok()
            .and_then(|url| url.host_str().map(ToString::to_string))
        {
            return Some(host);
        }
    }
    Regex::new(r#"(?i)\bhost[:=]\s*([A-Za-z0-9._-]+)"#)
        .expect("host regex should compile")
        .captures(text)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
}

fn compact_error_fallback(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Unknown error")
        .chars()
        .take(160)
        .collect()
}

fn error_phase(error: &AppError) -> Option<String> {
    match error {
        AppError::LlmDetailed(_) => llm_stage(error)
            .map(|stage| stage.to_string())
            .or_else(|| llm_http_status(error).map(|_| "HttpStatus".to_string()))
            .or_else(|| Some("Llm".to_string())),
        AppError::Llm(_) => Some("Llm".to_string()),
        AppError::Plugin(_) => Some("Plugin".to_string()),
        AppError::Primitive(_) => Some("Primitive".to_string()),
        AppError::Event(_) => Some("Event".to_string()),
        AppError::Config(_) => Some("Config".to_string()),
        AppError::Permission(_) => Some("Permission".to_string()),
        AppError::Tool(_) => Some("Tool".to_string()),
        AppError::Io(_) => Some("Io".to_string()),
        AppError::Serialize(_) => Some("Serialize".to_string()),
        AppError::QuickJS(_) => Some("QuickJS".to_string()),
        AppError::Audit(_) => Some("Audit".to_string()),
        AppError::Internal(_) => Some("Internal".to_string()),
        AppError::Invariant { stage, .. } => Some((*stage).to_string()),
        AppError::ApplyBoundaryStale { .. } => Some("ApplyBoundaryStale".to_string()),
    }
}

pub(crate) fn render_error_message(error: &AppError) -> String {
    let detail = error_detail_text(error);
    let failure = classify_llm_failure(error);
    match failure.kind {
        LlmFailureKind::Billing => {
            return "账户余额或额度不足。充值或切换 Provider 后可重试。".to_string();
        }
        LlmFailureKind::ContextOverflow => {
            return "上下文超过当前模型限制。可用 /compact 压缩上下文，或用 /restore 回退后重试。"
                .to_string();
        }
        _ => {}
    }
    if let Some(status) = llm_http_status(error) {
        let mut parts = vec![format!("API 错误 {status}")];
        if let Some(host) = extract_gateway_host(&detail) {
            parts.push(host);
        }
        if let Some(request_id) = extract_request_id(&detail) {
            parts.push(format!("Request-Id {request_id}"));
        }
        return parts.join(" · ");
    }
    compact_error_fallback(&detail)
}

fn build_error_entry(ctx: &ChatContext, error: &AppError) -> ErrorEntry {
    let session_entry = ctx
        .session_runtime
        .session
        .current_session_entry()
        .ok()
        .flatten();
    let model = Some(ctx.effective_model(session_entry.as_ref()));
    let catalog_entry = model.as_deref().and_then(|model_id| {
        ctx.global_services
            .model_catalog
            .with_catalog(|catalog| catalog.lookup(model_id).cloned())
    });
    let detail = error_detail_text(error);
    let failure = classify_llm_failure(error);
    ErrorEntry {
        id: Some(format!("err_{}", Utc::now().timestamp_micros())),
        parent_id: None,
        timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        phase: error_phase(error),
        provider: catalog_entry.as_ref().map(|entry| entry.provider.clone()),
        model,
        api_family: catalog_entry.as_ref().map(|entry| entry.api.clone()),
        status_code: llm_http_status(error),
        request_id: extract_request_id(&detail),
        failure_kind: Some(failure.kind.as_str().to_string()),
        failure_domain: Some(failure.domain.as_str().to_string()),
        summary: render_error_message(error),
        detail,
    }
}

fn same_structured_error(a: &ErrorEntry, b: &ErrorEntry) -> bool {
    a.phase == b.phase
        && a.provider == b.provider
        && a.model == b.model
        && a.api_family == b.api_family
        && a.status_code == b.status_code
        && a.request_id == b.request_id
        && a.summary == b.summary
        && a.detail == b.detail
}

fn latest_transcript_error_matches(ctx: &ChatContext, candidate: &ErrorEntry) -> bool {
    ctx.session_runtime
        .session
        .get_entries(1)
        .ok()
        .and_then(|entries| entries.into_iter().last())
        .and_then(|entry| match entry {
            crate::core::TranscriptEntry::Error(error) => Some(error),
            _ => None,
        })
        .map(|error| same_structured_error(&error, candidate))
        .unwrap_or(false)
}

fn replace_context_state_from_transcript(
    ctx: &ChatContext,
    context_config: &crate::infra::ContextConfig,
    system_text: &str,
    original_error: &AppError,
    rehydrate_failure_message: &'static str,
    context_state: &mut crate::core::ContextState,
) {
    let preserved_session_obs = context_state.session_obs.clone();
    let preserved_live = context_state.live.clone();
    match init_context_state(&ctx.session_runtime.session, context_config, system_text) {
        Ok(fresh) => {
            *context_state = fresh;
        }
        Err(rehydrate_error) => {
            warn!(
                error = %rehydrate_error,
                original_error = %original_error,
                "{rehydrate_failure_message}"
            );
            let mut fallback = make_fallback_context_state(ctx, system_text, context_config);
            fallback.session_obs = preserved_session_obs;
            fallback.live = preserved_live;
            *context_state = fallback;
        }
    }
}

pub(crate) fn try_rehydrate_context_state_after_append_invariant(
    ctx: &ChatContext,
    context_config: &crate::infra::ContextConfig,
    system_text: &str,
    error: &AppError,
    context_state: &mut crate::core::ContextState,
) -> bool {
    if !is_append_message_chain_invariant(error) {
        return false;
    }

    replace_context_state_from_transcript(
        ctx,
        context_config,
        system_text,
        error,
        "append_message_chain recovery rehydrate failed; falling back to empty context state",
        context_state,
    );
    true
}

pub(crate) fn recover_context_state_after_failed_turn(
    ctx: &ChatContext,
    context_config: &crate::infra::ContextConfig,
    system_text: &str,
    error: &AppError,
    context_state: &mut crate::core::ContextState,
) -> bool {
    if is_append_message_chain_invariant(error) {
        return try_rehydrate_context_state_after_append_invariant(
            ctx,
            context_config,
            system_text,
            error,
            context_state,
        );
    }

    let superseded_trailing_users = match ctx
        .session_runtime
        .session
        .mark_trailing_user_messages_superseded()
    {
        Ok(changed) if changed > 0 => {
            warn!(
                changed,
                "failed turn recovery superseded trailing user messages before rehydrate"
            );
            changed
        }
        Ok(_) => 0,
        Err(mark_error) => {
            warn!(
                error = %mark_error,
                original_error = %error,
                "failed turn recovery could not supersede trailing user messages"
            );
            0
        }
    };

    let structured_error = build_error_entry(ctx, error);
    let should_append_error =
        superseded_trailing_users > 0 || !latest_transcript_error_matches(ctx, &structured_error);
    if should_append_error {
        if let Err(append_error) = ctx
            .session_runtime
            .session
            .append_error_entry(structured_error)
        {
            warn!(
                error = %append_error,
                original_error = %error,
                "failed turn recovery could not append structured error entry"
            );
        }
    }

    replace_context_state_from_transcript(
        ctx,
        context_config,
        system_text,
        error,
        "failed turn recovery rehydrate failed; falling back to empty context state",
        context_state,
    );
    true
}
