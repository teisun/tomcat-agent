use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{
    ChatMessage, ChatMessageRole, ChatRequest, ChatResponse, LlmProvider, StreamEvent,
};
use crate::core::tools::primitive::BashTaskRegistry;
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

use super::mocks::MockPrimitiveExecutor;

struct HungTaskBoundedProvider {
    requests: StdMutex<Vec<ChatRequest>>,
    task_id: StdMutex<Option<String>>,
}

impl HungTaskBoundedProvider {
    fn new() -> Self {
        Self {
            requests: StdMutex::new(Vec::new()),
            task_id: StdMutex::new(None),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn task_id(&self) -> Option<String> {
        self.task_id.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for HungTaskBoundedProvider {
    fn provider_name(&self) -> &str {
        "hung_task_bounded_mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let step = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(req.clone());
            requests.len() - 1
        };
        let events = match step {
            0 => single_tool_call_stream(
                "call-bash",
                "bash",
                r#"{"command":"printf HUNG_TIMEOUT_SNAPSHOT; sleep 30","run_in_background":true}"#,
            ),
            1 => {
                let task_id = extract_background_task_id(&req)?;
                *self.task_id.lock().unwrap() = Some(task_id.clone());
                single_tool_call_stream(
                    "call-task-output-1",
                    "task_output",
                    &format!(
                        r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":150}}"#,
                        task_id
                    ),
                )
            }
            2 => {
                assert_timeout_snapshot_present(&req)?;
                vec![
                    Ok(StreamEvent::ContentDelta {
                        delta: "HUNG_WAIT_STOPPED_OK".to_string(),
                    }),
                    Ok(StreamEvent::FinishReason {
                        reason: "stop".to_string(),
                    }),
                ]
            }
            _ => {
                return Err(AppError::Llm(format!(
                    "hung task mock should stop after 3 requests, got step={step}"
                )));
            }
        };


        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

fn single_tool_call_stream(
    call_id: &str,
    name: &str,
    arguments: &str,
) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(call_id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(arguments.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

fn extract_background_task_id(req: &ChatRequest) -> Result<String, AppError> {
    for message in req.messages.iter().rev() {
        if message.role != ChatMessageRole::Tool {
            continue;
        }
        let Some(content) = message.text_content() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        if let Some(task_id) = value.get("taskId").and_then(|v| v.as_str()) {
            return Ok(task_id.to_string());
        }
    }
    Err(AppError::Llm(
        "expected prior bash tool result containing taskId".to_string(),
    ))
}

fn assert_timeout_snapshot_present(req: &ChatRequest) -> Result<(), AppError> {
    for message in req.messages.iter().rev() {
        if message.role != ChatMessageRole::Tool {
            continue;
        }
        let Some(content) = message.text_content() else {
            continue;
        };
        if content.contains("\"wakeReason\":\"timeout\"")
            && content.contains("HUNG_TIMEOUT_SNAPSHOT")
            && content.contains("\"finished\":false")
        {
            return Ok(());
        }
    }
    Err(AppError::Llm(
        "expected timeout tool result containing recent output snapshot".to_string(),
    ))
}

#[tokio::test]
async fn run_hung_background_task_timeout_snapshot_keeps_turn_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let llm = Arc::new(HungTaskBoundedProvider::new());
    let event_bus = Arc::new(DefaultEventBus::new());
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(
        llm.clone(),
        Arc::new(MockPrimitiveExecutor),
        event_bus,
        AgentLoopConfig {
            model: "gpt-4".to_string(),
            session_id: "s-hung-timeout-bounded".to_string(),
            ..Default::default()
        },
        abort,
    )
    .with_bash_task_registry(registry.clone());

    let result = loop_
        .run(vec![ChatMessage::user(
            "start one hung background task and stop polling after the first timeout snapshot",
        )])
        .await
        .unwrap();

    assert!(
        result.final_text.contains("HUNG_WAIT_STOPPED_OK"),
        "timeout snapshot 后应立即停止轮询，实际 final_text={:?}",
        result.final_text
    );
    assert_eq!(llm.request_count(), 3, "应严格限制为 3 次 LLM 请求");

    if let Some(task_id) = llm.task_id() {
        let _ = registry.stop(&task_id).await;
    }
}
