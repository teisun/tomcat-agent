//! 首条 user 消息后异步 utility 模型生成 session 标题。

use std::sync::Arc;

use crate::core::llm::{ChatMessage, ChatMessageRole, LlmProvider};
use crate::core::session::manager::{is_rule_derived_title, SessionManager};
use crate::core::summary::generate_session_title;
use crate::infra::events::wire;
use crate::infra::ScopedEventEmitter;

pub(crate) fn maybe_spawn_semantic_session_title(
    session: &SessionManager,
    appended_messages: &[(ChatMessage, bool)],
    title_provider: Arc<dyn LlmProvider>,
    title_model: String,
    emitter: Arc<ScopedEventEmitter>,
    _session_id: String,
) {
    for (message, _) in appended_messages {
        if message.role != ChatMessageRole::User {
            continue;
        }
        let Some(text) = message.text_content() else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let session_key = session.current_session_key().to_string();
        let rule_title = crate::core::session::manager::derive_title_from_user_message(text);
        let user_text = text.to_string();
        let session = session.clone();
        tokio::spawn(async move {
            let Ok(generated) =
                generate_session_title(&user_text, title_provider.as_ref(), &title_model).await
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
            let payload = serde_json::json!({
                "type": wire::WIRE_SESSION_TITLE_UPDATED,
                "title": generated,
            });
            let _ = emitter.emit_payload(wire::WIRE_SESSION_TITLE_UPDATED, payload);
        });
        break;
    }
}
