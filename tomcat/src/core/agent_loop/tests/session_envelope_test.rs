use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig};
use crate::core::llm::{ChatMessage, StreamEvent};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventBus;
use crate::infra::{wire, DefaultEventBus, EventContext};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

#[tokio::test]
async fn run_tool_loop_keeps_session_id_consistent_between_payload_and_context() {
    let session_id = "session-guard-main";
    let stream_tool: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".to_string()),
            name: Some("write".to_string()),
            arguments_delta: Some(
                r#"{"path":"~/workspace/demo.txt","content":"hello","overwrite":false}"#
                    .to_string(),
            ),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let stream_text: Vec<Result<StreamEvent, AppError>> = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "done".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let llm = Arc::new(MockLlmProvider::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(Mutex::new(Vec::<EventContext>::new()));
    let watched = [
        wire::WIRE_AGENT_START,
        wire::WIRE_TURN_START,
        wire::WIRE_TOOL_CALL_STREAMING,
        wire::WIRE_TOOL_EXECUTION_START,
        wire::WIRE_TOOL_CALL,
        wire::WIRE_TOOL_RESULT,
        wire::WIRE_TOOL_EXECUTION_END,
        wire::WIRE_MESSAGE_START,
        wire::WIRE_MESSAGE_UPDATE,
        wire::WIRE_MESSAGE_END,
        wire::WIRE_TURN_END,
        wire::WIRE_AGENT_END,
    ];
    for event_name in watched {
        let captured_cb = Arc::clone(&captured);
        event_bus.on(
            event_name,
            Box::new(move |ctx: EventContext| {
                captured_cb.lock().unwrap().push(ctx);
                Ok(())
            }),
        );
    }
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: session_id.to_string(),
        ..Default::default()
    };
    let abort = CancellationToken::new();
    let mut loop_ = AgentLoop::new(llm, primitive, event_bus, config, abort);
    let result = loop_
        .run(vec![ChatMessage::user("write demo file")])
        .await
        .unwrap();
    assert!(result.final_text.contains("done"));

    let captured = captured.lock().unwrap().clone();
    assert!(
        !captured.is_empty(),
        "tool loop 应至少发出一批可观测事件，不能是空集"
    );

    let seen_types: HashSet<String> = captured.iter().map(|ctx| ctx.event_name.clone()).collect();
    let expected_types: HashSet<String> = watched.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        seen_types, expected_types,
        "这条 tool loop 应覆盖主链路的 session-bound 事件集合"
    );

    for ctx in &captured {
        assert_eq!(
            ctx.session_id.as_deref(),
            Some(session_id),
            "EventContext.session_id 应与 AgentLoopConfig.session_id 一致: event={}",
            ctx.event_name
        );
        assert_eq!(
            ctx.payload.get("sessionId").and_then(|v| v.as_str()),
            Some(session_id),
            "payload.sessionId 应由 ScopedEventEmitter 注入: event={}",
            ctx.event_name
        );
        assert_eq!(
            ctx.payload.get("type").and_then(|v| v.as_str()),
            Some(ctx.event_name.as_str()),
            "wire payload.type 应与订阅事件名保持一致: event={}",
            ctx.event_name
        );
    }
}
