use std::collections::BTreeMap;

use serde_json::{json, Value};
use tokio_stream::{Stream, StreamExt};

use super::{ChatMessage, ChatResponse, ChatResponseChoice, StreamEvent, TokenUsage};
use crate::infra::error::AppError;

/// Collect a provider-normalized stream without exposing partial results. The adapter validates
/// the terminal marker and reports interrupted streams as errors; propagate those errors even
/// if earlier deltas contained visible text.
pub(crate) async fn collect_streaming_response(
    mut stream: impl Stream<Item = Result<StreamEvent, AppError>> + Unpin,
) -> Result<ChatResponse, AppError> {
    let mut text = String::new();
    let mut finish_reason = None;
    let mut usage = None;
    let mut tool_calls = BTreeMap::<u32, Value>::new();

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::ContentDelta { delta } => text.push_str(&delta),
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let call = tool_calls.entry(index).or_insert_with(|| {
                    json!({ "id": "", "type": "function", "function": {"name": "", "arguments": ""} })
                });
                if let Some(id) = id {
                    call["id"] = Value::String(id);
                }
                if let Some(name) = name {
                    call["function"]["name"] = Value::String(name);
                }
                if let Some(delta) = arguments_delta {
                    if let Value::String(arguments) = &mut call["function"]["arguments"] {
                        arguments.push_str(&delta);
                    }
                }
            }
            StreamEvent::FinishReason { reason } => finish_reason = Some(reason),
            StreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
                cache_read_tokens,
                cache_write_tokens,
                total_tokens,
                reasoning_tokens,
                text_tokens,
            } => {
                usage = Some(TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    total_tokens,
                    reasoning_tokens,
                    text_tokens,
                });
            }
            StreamEvent::LlmError {
                reason, message, ..
            } => {
                return Err(AppError::Llm(format!("{reason}: {message}")));
            }
            StreamEvent::Thinking { .. }
            | StreamEvent::ReasoningSnapshot { .. }
            | StreamEvent::LlmNotice { .. } => {}
        }
    }

    let mut message = ChatMessage::assistant(text);
    if !tool_calls.is_empty() {
        message.tool_calls = Some(tool_calls.into_values().collect());
    }
    message.finish_reason = finish_reason.clone();
    message.usage = usage.clone();
    Ok(ChatResponse {
        id: None,
        choices: vec![ChatResponseChoice {
            index: 0,
            message,
            finish_reason,
        }],
        usage,
    })
}
