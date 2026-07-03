use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::llm::{ChatMessage, ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::core::session::manager::SessionManager;
use crate::core::session::TranscriptEntry;
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::{wire, DefaultEventBus, EventContext};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

type CapturedIds = Arc<Mutex<Vec<String>>>;
type AssistantIdCapture = (CapturedIds, CapturedIds);

fn create_session_manager() -> (tempfile::TempDir, SessionManager) {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    (dir, mgr)
}

fn capture_assistant_ids(
    event_bus: &Arc<DefaultEventBus>,
) -> AssistantIdCapture {
    let message_start_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let turn_end_ids = Arc::new(Mutex::new(Vec::<String>::new()));

    let message_start_ids_cb = Arc::clone(&message_start_ids);
    event_bus.on(
        wire::WIRE_MESSAGE_START,
        Box::new(move |ctx: EventContext| {
            if let Some(id) = ctx
                .payload
                .get("assistantMessageId")
                .and_then(|value| value.as_str())
            {
                message_start_ids_cb.lock().unwrap().push(id.to_string());
            }
            Ok(())
        }),
    );

    let turn_end_ids_cb = Arc::clone(&turn_end_ids);
    event_bus.on(
        wire::WIRE_TURN_END,
        Box::new(move |ctx: EventContext| {
            if let Some(id) = ctx
                .payload
                .get("assistantMessageId")
                .and_then(|value| value.as_str())
            {
                turn_end_ids_cb.lock().unwrap().push(id.to_string());
            }
            Ok(())
        }),
    );

    (message_start_ids, turn_end_ids)
}

fn assistant_entry_ids(mgr: &SessionManager) -> Vec<String> {
    mgr.get_entries(16)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message)
                if message.message.get("role").and_then(|value| value.as_str())
                    == Some("assistant") =>
            {
                message.id
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn text_only_turn_reuses_stream_id_for_transcript_and_turn_end() {
    let (_dir, mgr) = create_session_manager();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "Hello stable id".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let (message_start_ids, turn_end_ids) = capture_assistant_ids(&event_bus);
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-text-only-id".to_string(),
        message_append_sink: Some(Arc::new(mgr.clone())),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new());

    let result = agent.run(vec![ChatMessage::user("hi")]).await.unwrap();
    assert_eq!(result.final_text, "Hello stable id");
    assert!(
        agent.pending_assistant_entry_id().is_none(),
        "text-only turn finished after persistence and should clear pending id"
    );

    let message_start_ids = message_start_ids.lock().unwrap().clone();
    let turn_end_ids = turn_end_ids.lock().unwrap().clone();
    let transcript_ids = assistant_entry_ids(&mgr);

    assert_eq!(message_start_ids.len(), 1);
    assert_eq!(turn_end_ids, message_start_ids);
    assert_eq!(transcript_ids, message_start_ids);
}

#[tokio::test]
async fn multi_turn_tool_loop_mints_distinct_ids_per_assistant_message() {
    let (_dir, mgr) = create_session_manager();
    let stream_tool = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(r#"{"path":"/tmp/demo.txt"}"#.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let (message_start_ids, turn_end_ids) = capture_assistant_ids(&event_bus);
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-multi-turn-id".to_string(),
        message_append_sink: Some(Arc::new(mgr.clone())),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new());

    let result = agent.run(vec![ChatMessage::user("use a tool")]).await.unwrap();
    assert_eq!(result.final_text, "done");
    assert!(
        agent.pending_assistant_entry_id().is_none(),
        "multi-turn loop should clear pending id after the last assistant settles"
    );

    let message_start_ids = message_start_ids.lock().unwrap().clone();
    let turn_end_ids = turn_end_ids.lock().unwrap().clone();
    let transcript_ids = assistant_entry_ids(&mgr);

    assert_eq!(message_start_ids.len(), 2, "tool loop should emit two assistant messages");
    assert_eq!(turn_end_ids, message_start_ids);
    assert_eq!(transcript_ids, message_start_ids);
    assert_ne!(
        message_start_ids[0], message_start_ids[1],
        "each run_chat_stream turn must mint a fresh assistantMessageId"
    );
}

#[tokio::test]
async fn no_sink_mode_still_emits_stable_assistant_ids() {
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "ephemeral".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let (message_start_ids, turn_end_ids) = capture_assistant_ids(&event_bus);
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-no-sink-id".to_string(),
        ..Default::default()
    };
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new());

    let result = agent.run(vec![ChatMessage::user("hi")]).await.unwrap();
    assert_eq!(result.final_text, "ephemeral");
    assert!(
        agent.pending_assistant_entry_id().is_none(),
        "no-sink mode should still clear pending id after the turn closes"
    );

    let message_start_ids = message_start_ids.lock().unwrap().clone();
    let turn_end_ids = turn_end_ids.lock().unwrap().clone();
    assert_eq!(message_start_ids.len(), 1);
    assert_eq!(turn_end_ids, message_start_ids);
}

#[tokio::test]
async fn interrupted_partial_stream_persists_the_same_pre_minted_id() {
    use tokio_stream::wrappers::ReceiverStream;

    struct StreamingLlm {
        rx: Mutex<Option<tokio::sync::mpsc::Receiver<Result<StreamEvent, AppError>>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for StreamingLlm {
        fn provider_name(&self) -> &str {
            "streaming_mock"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Err(AppError::Llm("unused".into()))
        }

        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            let rx = self
                .rx
                .lock()
                .unwrap()
                .take()
                .expect("chat_stream called twice");
            Ok(Box::new(ReceiverStream::new(rx)))
        }

        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    let (_dir, mgr) = create_session_manager();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent, AppError>>(16);
    let llm = Arc::new(StreamingLlm {
        rx: Mutex::new(Some(rx)),
    });
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let (message_start_ids, turn_end_ids) = capture_assistant_ids(&event_bus);
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-interrupt-id".to_string(),
        message_append_sink: Some(Arc::new(mgr.clone())),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, cancel.clone());

    tokio::spawn(async move {
        for index in 0..200 {
            if tx
                .send(Ok(StreamEvent::ContentDelta {
                    delta: format!("chunk-{index} "),
                }))
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let cancel_bg = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        cancel_bg.cancel();
    });

    let outcome = agent.run(vec![ChatMessage::user("stream")]).await;
    let result = match outcome {
        AgentRunOutcome::Interrupted(result) => result,
        other => panic!("expected Interrupted, got {other:?}"),
    };

    assert!(
        result.final_text.contains("chunk-"),
        "interrupt path should keep the streamed partial text"
    );
    assert!(
        agent.pending_assistant_entry_id().is_none(),
        "interrupt path should clear pending id after persisting the partial assistant"
    );

    let message_start_ids = message_start_ids.lock().unwrap().clone();
    let turn_end_ids = turn_end_ids.lock().unwrap().clone();
    let transcript_ids = assistant_entry_ids(&mgr);

    assert_eq!(message_start_ids.len(), 1);
    assert!(
        turn_end_ids.is_empty(),
        "interrupt path should not emit turn_end after an aborted stream"
    );
    assert_eq!(transcript_ids, message_start_ids);
}
