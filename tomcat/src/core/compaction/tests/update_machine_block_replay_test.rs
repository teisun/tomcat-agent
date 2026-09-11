use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_stream::Stream;

use crate::core::compaction::preheat::{generate_summary, messages_to_text};
use crate::core::llm::{
    ChatMessage, ChatMessageRole, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider,
    StreamEvent,
};
use crate::infra::error::AppError;

const OLD_MACHINE_BLOCKS: &str = "<control_state>\nmode: plan\n</control_state>\n\n<verbatim_user_messages>\n[1] stale user detail\n</verbatim_user_messages>\n\n## Goal\nmodel body remains";

struct CaptureProvider {
    request: Arc<Mutex<Option<ChatRequest>>>,
}

#[async_trait]
impl LlmProvider for CaptureProvider {
    fn provider_name(&self) -> &str {
        "update-machine-block-capture"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        *self.request.lock().unwrap() = Some(request);
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("## Goal\nnew model body"),
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

#[test]
fn messages_to_text_strips_machine_blocks_from_previous_summary_only() {
    let batch = messages_to_text(&[
        ChatMessage::compaction_summary(OLD_MACHINE_BLOCKS, "old-summary"),
        ChatMessage::user("new tail input"),
    ]);

    assert!(batch.contains("[Previous Summary]\n## Goal\nmodel body remains"));
    assert!(batch.contains("[User] new tail input"));
    assert!(!batch.contains("<control_state>"));
    assert!(!batch.contains("<verbatim_user_messages>"));
    assert!(!batch.contains("stale user detail"));
}

#[tokio::test]
async fn update_request_strips_old_machine_blocks_from_both_prompt_channels() {
    let request = Arc::new(Mutex::new(None));
    let provider = CaptureProvider {
        request: Arc::clone(&request),
    };
    let snapshot = vec![
        ChatMessage::compaction_summary(OLD_MACHINE_BLOCKS, "old-summary"),
        ChatMessage::user("new tail input"),
    ];

    generate_summary(
        &snapshot,
        Some(OLD_MACHINE_BLOCKS),
        &provider,
        "gpt-5.4",
        None,
        None,
    )
    .await
    .unwrap();

    let request = request.lock().unwrap().clone().unwrap();
    let system = request
        .messages
        .iter()
        .find(|message| message.role == ChatMessageRole::System)
        .and_then(ChatMessage::text_content)
        .unwrap();
    let user_batch = request
        .messages
        .iter()
        .find(|message| message.role == ChatMessageRole::User)
        .and_then(ChatMessage::text_content)
        .unwrap();
    for channel in [system, user_batch] {
        assert!(channel.contains("## Goal\nmodel body remains"));
        assert!(!channel.contains("<control_state>"));
        assert!(!channel.contains("<verbatim_user_messages>"));
        assert!(!channel.contains("stale user detail"));
    }
    assert!(
        request.tools.is_none(),
        "UPDATE compaction must still disable tools"
    );
}
