use std::borrow::Cow;

use super::catalog::Capabilities;
use super::{ChatMessage, ChatMessageContent, ChatMessageContentPart};

pub(crate) const UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER: &str = "[图片已省略：当前模型不支持图片输入]";
pub(crate) const UNSUPPORTED_FILE_INPUT_PLACEHOLDER: &str = "[文件已省略：当前模型不支持文件输入]";

pub(crate) fn degrade_placeholder(part: &ChatMessageContentPart) -> String {
    match part {
        ChatMessageContentPart::InputImage { .. } => {
            UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER.to_string()
        }
        ChatMessageContentPart::InputFile { .. } => UNSUPPORTED_FILE_INPUT_PLACEHOLDER.to_string(),
        ChatMessageContentPart::InputText { .. }
        | ChatMessageContentPart::InputReference { .. } => String::new(),
    }
}

fn unsupported_placeholder_for_part(
    part: &ChatMessageContentPart,
    capabilities: &Capabilities,
) -> Option<String> {
    match part {
        ChatMessageContentPart::InputImage { .. } if !capabilities.vision => {
            Some(degrade_placeholder(part))
        }
        ChatMessageContentPart::InputFile { .. } if !capabilities.files => {
            Some(degrade_placeholder(part))
        }
        _ => None,
    }
}

fn degrade_message_parts(
    parts: &[ChatMessageContentPart],
    capabilities: &Capabilities,
) -> Option<Vec<ChatMessageContentPart>> {
    let mut degraded: Option<Vec<ChatMessageContentPart>> = None;
    for (idx, part) in parts.iter().enumerate() {
        if let Some(placeholder) = unsupported_placeholder_for_part(part, capabilities) {
            let next = degraded.get_or_insert_with(|| parts[..idx].to_vec());
            next.push(ChatMessageContentPart::text(placeholder));
        } else if let Some(next) = degraded.as_mut() {
            next.push(part.clone());
        }
    }
    degraded
}

pub(crate) fn degrade_unsupported_multimodal<'a>(
    messages: &'a [ChatMessage],
    capabilities: &Capabilities,
) -> Cow<'a, [ChatMessage]> {
    let mut degraded_messages: Option<Vec<ChatMessage>> = None;
    for (idx, message) in messages.iter().enumerate() {
        let Some(ChatMessageContent::Parts(parts)) = &message.content else {
            if let Some(existing) = degraded_messages.as_mut() {
                existing.push(message.clone());
            }
            continue;
        };
        let Some(next_parts) = degrade_message_parts(parts, capabilities) else {
            if let Some(existing) = degraded_messages.as_mut() {
                existing.push(message.clone());
            }
            continue;
        };

        let existing = degraded_messages.get_or_insert_with(|| messages[..idx].to_vec());
        let mut degraded_message = message.clone();
        degraded_message.content = Some(ChatMessageContent::Parts(next_parts));
        existing.push(degraded_message);
    }

    match degraded_messages {
        Some(messages) => Cow::Owned(messages),
        None => Cow::Borrowed(messages),
    }
}
