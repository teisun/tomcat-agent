use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio_stream::Stream;

use crate::core::compaction::preheat::{generate_summary_with_output_limit, SummaryRequestOptions};
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::session::transcript::{
    append_entry, mark_user_message_entry_superseded_by_id, write_header, MessageEntry,
    SessionHeader, TranscriptEntry,
};
use crate::core::session::user_message_sidecar::user_message_sidecar_path;
use crate::infra::error::AppError;

struct RetryDuringSummaryProvider {
    transcript: PathBuf,
    appended: Mutex<bool>,
}

#[async_trait]
impl LlmProvider for RetryDuringSummaryProvider {
    fn provider_name(&self) -> &str {
        "preheat-sidecar-freshness"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        if !*self.appended.lock().unwrap() {
            mark_user_message_entry_superseded_by_id(&self.transcript, "u1")?;
            append_entry(
                &self.transcript,
                &TranscriptEntry::Message(MessageEntry {
                    id: Some("u2".to_string()),
                    parent_id: None,
                    timestamp: "2026-08-10T00:00:02.000Z".to_string(),
                    message: serde_json::json!({
                        "role":"user",
                        "kind":"normal",
                        "content":"retry copy arrived while LLM awaited"
                    }),
                }),
            )?;
            *self.appended.lock().unwrap() = true;
        }
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("## Goal\nsummary after retry"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        unreachable!("compaction uses non-streaming chat")
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[tokio::test]
async fn sidecar_is_rebuilt_after_llm_await_before_machine_block_is_rendered() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("preheat-freshness.jsonl");
    write_header(
        &transcript,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "preheat-freshness".to_string(),
            timestamp: "2026-08-10T00:00:00.000Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();
    append_entry(
        &transcript,
        &TranscriptEntry::Message(MessageEntry {
            id: Some("u1".to_string()),
            parent_id: None,
            timestamp: "2026-08-10T00:00:01.000Z".to_string(),
            message: serde_json::json!({
                "role":"user",
                "kind":"normal",
                "content":"superseded original input"
            }),
        }),
    )
    .unwrap();

    let summary = generate_summary_with_output_limit(
        &[ChatMessage::user("superseded original input")],
        None,
        &RetryDuringSummaryProvider {
            transcript: transcript.clone(),
            appended: Mutex::new(false),
        },
        "gpt-5.4",
        None,
        SummaryRequestOptions {
            transcript_path: Some(&transcript),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let sidecar_path = user_message_sidecar_path(&transcript);
    let sidecar = std::fs::read_to_string(&sidecar_path).unwrap();
    assert!(summary.contains(&sidecar_path.display().to_string()));
    assert!(sidecar.contains("retry copy arrived while LLM awaited"));
    assert!(
        !sidecar.contains("superseded original input"),
        "摘要机器区指向 sidecar 前必须反映 await 期间发生的 supersede"
    );
}
