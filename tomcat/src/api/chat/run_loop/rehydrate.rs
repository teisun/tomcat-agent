use chrono::Utc;
use regex::Regex;
use reqwest::Url;
use tracing::warn;

use crate::api::chat::ChatContext;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::init_context_state;
use crate::core::session::ErrorEntry;
use crate::infra::error::{
    llm_http_status, llm_source_chain, llm_stage, llm_summary, AppError,
};

const MAX_ERROR_DETAIL_CHARS: usize = 8 * 1024;

pub(super) fn make_fallback_context_state(
    ctx: &ChatContext,
    system_text: &str,
    context_config: &crate::infra::ContextConfig,
) -> crate::core::ContextState {
    crate::core::ContextState {
        messages: Vec::new(),
        estimate_context_chars: system_text.len(),
        context_budget_chars: crate::infra::config::compute_context_budget_chars(context_config),
        context_budget_tokens: context_config
            .context_window
            .saturating_sub(context_config.max_output_tokens),
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
    let session_entry = ctx.session_runtime.session.current_session_entry().ok().flatten();
    let model = Some(ctx.effective_model(session_entry.as_ref()));
    let catalog_entry = model.as_deref().and_then(|model_id| {
        ctx.global_services
            .model_catalog
            .with_catalog(|catalog| catalog.lookup(model_id).cloned())
    });
    let detail = error_detail_text(error);
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
    let should_append_error = superseded_trailing_users > 0
        || !latest_transcript_error_matches(ctx, &structured_error);
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
