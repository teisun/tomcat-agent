use std::borrow::Cow;

use crate::core::llm::multimodal::{
    UNSUPPORTED_FILE_INPUT_PLACEHOLDER, UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER,
};
use crate::core::llm::{
    degrade_unsupported_multimodal, Capabilities, ChatMessage, ChatMessageContent,
    ChatMessageContentPart, ContextReference,
};

#[test]
fn degrade_unsupported_multimodal_returns_borrowed_when_all_parts_supported() {
    let messages = vec![ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("before "),
        ChatMessageContentPart::reference(ContextReference::file("docs/guide.md", "guide.md")),
    ])];
    let capabilities = Capabilities {
        vision: true,
        files: true,
        tools: true,
        reasoning: true,
        web_search: false,
    };

    let degraded = degrade_unsupported_multimodal(&messages, &capabilities);
    assert!(matches!(degraded, Cow::Borrowed(_)));
}

#[test]
fn degrade_unsupported_multimodal_replaces_only_missing_capabilities_and_preserves_order() {
    let messages = vec![ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("before "),
        ChatMessageContentPart::image_file_id("file-image").unwrap(),
        ChatMessageContentPart::reference(ContextReference::file("docs/guide.md", "guide.md")),
        ChatMessageContentPart::file_file_id("file-pdf", Some("guide.pdf".to_string())).unwrap(),
        ChatMessageContentPart::text(" after"),
    ])];
    let capabilities = Capabilities {
        vision: true,
        files: false,
        tools: true,
        reasoning: true,
        web_search: false,
    };

    let degraded = degrade_unsupported_multimodal(&messages, &capabilities);
    let degraded = degraded.into_owned();
    assert_eq!(degraded.len(), 1);
    let parts = match &degraded[0].content {
        Some(ChatMessageContent::Parts(parts)) => parts,
        other => panic!("expected parts content, got {other:?}"),
    };

    assert_eq!(parts.len(), 5);
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "before "
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputImage { .. }
    ));
    assert!(matches!(
        &parts[2],
        ChatMessageContentPart::InputReference { reference }
            if reference.path == "docs/guide.md"
    ));
    assert!(matches!(
        &parts[3],
        ChatMessageContentPart::InputText { text } if text == UNSUPPORTED_FILE_INPUT_PLACEHOLDER
    ));
    assert!(matches!(
        &parts[4],
        ChatMessageContentPart::InputText { text } if text == " after"
    ));
}

#[test]
fn degrade_unsupported_multimodal_replaces_images_and_files_with_placeholders() {
    let messages = vec![ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::image_file_id("file-image").unwrap(),
        ChatMessageContentPart::file_file_id("file-pdf", Some("guide.pdf".to_string())).unwrap(),
    ])];

    let degraded = degrade_unsupported_multimodal(&messages, &Capabilities::default()).into_owned();
    let parts = match &degraded[0].content {
        Some(ChatMessageContent::Parts(parts)) => parts,
        other => panic!("expected parts content, got {other:?}"),
    };
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputText { text } if text == UNSUPPORTED_FILE_INPUT_PLACEHOLDER
    ));
}
