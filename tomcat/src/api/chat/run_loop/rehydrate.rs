use tracing::warn;

use crate::api::chat::ChatContext;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::init_context_state;
use crate::infra::error::AppError;

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
            .session
            .current_transcript_path()
            .ok()
            .flatten()
            .unwrap_or_default(),
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
        "(本轮已落盘进度已保留；可直接继续输入新消息)\n"
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

    let preserved_session_obs = context_state.session_obs.clone();
    let preserved_live = context_state.live.clone();
    match init_context_state(&ctx.session, context_config, system_text) {
        Ok(fresh) => {
            *context_state = fresh;
        }
        Err(rehydrate_error) => {
            warn!(
                error = %rehydrate_error,
                original_error = %error,
                "append_message_chain recovery rehydrate failed; falling back to empty context state"
            );
            let mut fallback = make_fallback_context_state(ctx, system_text, context_config);
            fallback.session_obs = preserved_session_obs;
            fallback.live = preserved_live;
            *context_state = fallback;
        }
    }
    true
}
