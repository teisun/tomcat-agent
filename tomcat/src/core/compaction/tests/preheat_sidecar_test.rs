use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::core::compaction::preheat::{Preheat, PreheatOutcome};
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::user_message_sidecar_path;
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::DefaultEventBus;
use crate::infra::ScopedEventEmitter;

struct SummaryProvider;

#[async_trait]
impl LlmProvider for SummaryProvider {
    fn provider_name(&self) -> &str {
        "preheat-sidecar"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("## Goal\npreheat summary"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        unreachable!("preheat compaction only uses non-streaming chat")
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[tokio::test]
async fn preheat_summary_materializes_and_points_to_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("preheat.jsonl");
    write_header(
        &transcript,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "preheat-sidecar".to_string(),
            timestamp: "2026-08-10T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    let mut user = ChatMessage::user("pre-boundary Normal user input");
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("assistant reply");
    assistant.msg_id = Some("a1".to_string());
    for message in [&user, &assistant] {
        append_entry(
            &transcript,
            &TranscriptEntry::Message(MessageEntry {
                id: message.msg_id.clone(),
                parent_id: None,
                timestamp: "2026-08-10T00:00:01.000Z".to_string(),
                message: serde_json::json!({
                    "role": if message.role == crate::core::llm::ChatMessageRole::User { "user" } else { "assistant" },
                    "content": message.text_content().unwrap(),
                }),
            }),
        )
        .unwrap();
    }

    let emitter = Arc::new(ScopedEventEmitter::new(
        Arc::new(DefaultEventBus::new()),
        "preheat-sidecar",
    ));
    let mut preheat = Preheat::new();
    assert!(preheat.try_start(
        0.95,
        &[user, assistant],
        &transcript,
        None,
        Arc::new(SummaryProvider),
        None,
        &ContextConfig::default(),
        emitter,
        None,
    ));

    let result = match preheat.await_result(Duration::from_secs(5)).await {
        PreheatOutcome::Completed(result) => result,
        _ => panic!("preheat must produce a summary"),
    };
    let sidecar_path = user_message_sidecar_path(&transcript);
    assert!(sidecar_path.is_file());
    assert!(result
        .summary_text
        .contains(&sidecar_path.display().to_string()));
    assert!(std::fs::read_to_string(sidecar_path)
        .unwrap()
        .contains("pre-boundary Normal user input"));
}

#[tokio::test]
async fn deferred_preheat_leaves_transcript_unchanged_until_the_guard_applies_it() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("deferred-preheat.jsonl");
    write_header(
        &transcript,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "deferred-preheat".to_string(),
            timestamp: "2026-09-09T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    let mut user = ChatMessage::user("input");
    user.msg_id = Some("u1".to_string());
    let mut assistant = ChatMessage::assistant("output");
    assistant.msg_id = Some("a1".to_string());
    let mut preheat = Preheat::new();
    assert!(preheat.try_start_deferred(
        0.95,
        &[user, assistant],
        &transcript,
        None,
        Arc::new(SummaryProvider),
        None,
        &ContextConfig::default(),
        Arc::new(ScopedEventEmitter::new(
            Arc::new(DefaultEventBus::new()),
            "deferred-preheat",
        )),
        None,
    ));

    let result = match preheat.await_result(Duration::from_secs(5)).await {
        PreheatOutcome::Completed(result) => result,
        _ => panic!("deferred preheat must produce a summary"),
    };
    assert!(
        result.transcript_compaction_entry_id.is_none(),
        "only the foreground guard may materialize a deferred preheat boundary"
    );
    assert!(
        read_entries_tail(&transcript, 16)
            .unwrap()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::BranchSummary(_))),
        "background preheat must not race later tool-result appends by rewriting the transcript"
    );
}
