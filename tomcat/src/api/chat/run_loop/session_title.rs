//! 首条 user 消息后异步 utility 模型生成 session 标题。

use std::sync::Arc;

use crate::core::llm::{ChatMessage, ChatMessageRole, LlmProvider, PromptCacheKeyFamily};
use crate::core::session::manager::{is_rule_derived_title, SessionManager};
use crate::core::summary::generate_session_title_with_cache_key_and_output_limit;
use crate::infra::events::wire;
use crate::infra::ScopedEventEmitter;

pub(crate) fn maybe_emit_rule_session_title(
    session: &SessionManager,
    appended_messages: &[ChatMessage],
    emitter: &Arc<ScopedEventEmitter>,
) {
    for message in appended_messages {
        if message.role != ChatMessageRole::User {
            continue;
        }
        let Some(text) = message.first_text() else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let session_key = session.current_session_key().to_string();
        let Ok(Some(current_entry)) = session.get_session(&session_key) else {
            break;
        };
        let current_title = current_entry.title.as_deref().unwrap_or("");
        if !current_title.is_empty() && !is_rule_derived_title(current_title, &text) {
            break;
        }
        emit_session_title_updated(
            emitter.as_ref(),
            &crate::core::session::manager::derive_title_from_user_message(&text),
        );
        break;
    }
}

pub(crate) fn maybe_spawn_semantic_session_title(
    session: &SessionManager,
    appended_messages: &[ChatMessage],
    title_provider: Arc<dyn LlmProvider>,
    title_model: String,
    title_output_limit: Option<u32>,
    emitter: Arc<ScopedEventEmitter>,
    session_id: String,
) {
    for message in appended_messages {
        if message.role != ChatMessageRole::User {
            continue;
        }
        let Some(text) = message.first_text() else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let session_key = session.current_session_key().to_string();
        let rule_title = crate::core::session::manager::derive_title_from_user_message(&text);
        let Ok(Some(current_entry)) = session.get_session(&session_key) else {
            break;
        };
        let current_title = current_entry.title.as_deref().unwrap_or("");
        if !current_title.is_empty() && !is_rule_derived_title(current_title, &text) {
            break;
        }
        let user_text = text;
        let session = session.clone();
        let cache_key = PromptCacheKeyFamily::Title.key_for(&session_id);
        tokio::spawn(async move {
            let Ok(generated) = generate_session_title_with_cache_key_and_output_limit(
                &user_text,
                title_provider.as_ref(),
                &title_model,
                cache_key.as_deref(),
                title_output_limit,
            )
            .await
            else {
                return;
            };
            let Ok(Some(entry)) = session.get_session(&session_key) else {
                return;
            };
            let current = entry.title.as_deref().unwrap_or("");
            if !current.is_empty() && !is_rule_derived_title(current, &user_text) {
                return;
            }
            if let Err(error) = session.update_session(&session_key, |entry| {
                if entry.title.is_none()
                    || entry
                        .title
                        .as_ref()
                        .is_some_and(|existing| is_rule_derived_title(existing, &user_text))
                {
                    entry.title = Some(generated.clone());
                }
            }) {
                tracing::warn!(error = %error, "async session title update failed");
                return;
            }
            if current == generated || (current.is_empty() && generated == rule_title) {
                return;
            }
            emit_session_title_updated(emitter.as_ref(), &generated);
        });
        break;
    }
}

fn emit_session_title_updated(emitter: &ScopedEventEmitter, title: &str) {
    let payload = serde_json::json!({
        "type": wire::WIRE_SESSION_TITLE_UPDATED,
        "title": title,
    });
    let _ = emitter.emit_payload(wire::WIRE_SESSION_TITLE_UPDATED, payload);
}
