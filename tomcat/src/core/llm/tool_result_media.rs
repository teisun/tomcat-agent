//! Request-only media policy. Tool results own their attachments in durable history;
//! any synthetic user carrier exists only while projecting a provider request.
use super::{ChatMessage, ChatMessageContent, ChatMessageContentPart};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolResultMediaMode {
    Native,
    Split,
}

/// Verified by the 2026-10-08 relay matrix. Chat's native extension is not portable.
/// Keep the decision here, rather than learning modes by retrying paid requests.
pub(crate) fn tool_result_media_mode(api: &str) -> ToolResultMediaMode {
    match api {
        "anthropic-messages" | "openai-responses" => ToolResultMediaMode::Native,
        _ => ToolResultMediaMode::Split,
    }
}

pub(crate) const TOOL_MEDIA_INTRO: &str = "Attached media from tool result:";

pub(crate) fn media_parts(message: &ChatMessage) -> impl Iterator<Item = &ChatMessageContentPart> {
    let parts = match message.content.as_ref() {
        Some(ChatMessageContent::Parts(parts)) => parts.as_slice(),
        _ => &[],
    };
    parts.iter().filter(|part| {
        matches!(
            part,
            ChatMessageContentPart::InputImage { .. } | ChatMessageContentPart::InputFile { .. }
        )
    })
}

/// Split transports must not replace an attached image with an "unsupported" warning.
pub(crate) fn tool_result_text(message: &ChatMessage) -> String {
    let text = match &message.content {
        Some(ChatMessageContent::Text(text)) => text.clone(),
        Some(ChatMessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                ChatMessageContentPart::InputText { text } => Some(text.clone()),
                ChatMessageContentPart::InputReference { reference } => {
                    Some(reference.to_prompt_text())
                }
                _ => None,
            })
            .collect(),
        None => String::new(),
    };
    if text.is_empty() && media_parts(message).next().is_some() {
        "[Media attached]".into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_media_defaults_follow_verified_api_contracts() {
        assert_eq!(
            tool_result_media_mode("anthropic-messages"),
            ToolResultMediaMode::Native
        );
        assert_eq!(
            tool_result_media_mode("openai-responses"),
            ToolResultMediaMode::Native
        );
        assert_eq!(tool_result_media_mode("openai"), ToolResultMediaMode::Split);
        assert_eq!(
            tool_result_media_mode("unknown"),
            ToolResultMediaMode::Split
        );
    }
}
