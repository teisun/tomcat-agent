//! Remote entry to exactly the same resource-command menu as terminal chat.
use super::{
    commands::resolve_slot_or_error,
    session_job::spawn_session_job,
    types::{OutFrame, ResponseFrame},
    ServeState,
};
use crate::api::chat::commands::{
    parse_shared_slash, run_shared_slash_command, shared_usage_error, SlashReply,
};
use crate::AppError;
use std::sync::Arc;

pub(super) async fn run(
    state: Arc<ServeState>,
    id: Option<String>,
    session_id: Option<String>,
    text: String,
) -> Result<(), AppError> {
    let Some(slot) = resolve_slot_or_error(&state, id.clone(), session_id).await? else {
        return Ok(());
    };
    let parsed = match parse_shared_slash(&text) {
        Some(Ok((name, args))) => match shared_usage_error(&name, &args) {
            Some(reply) => Err(reply),
            None => Ok((name, args)),
        },
        Some(Err(message)) => Err(SlashReply {
            ok: false,
            text: message,
        }),
        None => Err(SlashReply {
            ok: false,
            text: format!(
                "未知命令 {}，输入框可用：{}",
                text.split_whitespace().next().unwrap_or("（空输入）"),
                crate::api::chat::commands::SHARED_SLASH_COMMANDS
                    .iter()
                    .map(|command| format!("/{}", command.name))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }),
    };
    let (name, args) = match parsed {
        Ok(parsed) => parsed,
        Err(reply) => {
            return state.writer.send(OutFrame::Response(ResponseFrame::ok(
                id,
                Some(slot.session_id.clone()),
                Some(serde_json::to_value(reply)?),
            )))
        }
    };
    let context = Arc::clone(&slot.ctx);
    spawn_session_job(state, slot, id, async move {
        serde_json::to_value(run_shared_slash_command(&context, &name, &args).await)
            .map_err(|error| error.to_string())
    })
}
