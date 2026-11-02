mod common;

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use common::serve::{
    response, setup_serve_fixture, spawn_scripted_openai_stream_server_with_auto_title,
    spawn_serve_child, sse_delta, sse_done, sse_finish, sse_tool_call, try_extract_json_body,
    ScriptedOpenAiServer, ServeChild,
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

const ASK_QUESTION_ARGS: &str = r#"{"questions":[{"id":"q1","prompt":"Pick one","options":[{"id":"a","label":"A","recommended":true},{"id":"b","label":"B"}]}]}"#;

fn is_scripted_followup_delta(value: &serde_json::Value) -> bool {
    matches!(
        value
            .get("assistantMessageEvent")
            .and_then(|v| v.get("delta"))
            .and_then(|v| v.as_str()),
        Some("after approval" | "second session kept running")
    )
}

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "init-1",
        "subtype": "initialize",
        "payload": {}
    }));
    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_response")
            && value.get("requestId").and_then(|v| v.as_str()) == Some("init-1")
    });
    frames.last().expect("initialize response")["payload"]["sessionId"]
        .as_str()
        .expect("default session")
        .to_string()
}

fn new_session(child: &mut ServeChild, request_id: &str) -> String {
    child.send_value(&json!({
        "type": "new_session",
        "id": request_id,
        "params": {}
    }));
    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("id").and_then(|v| v.as_str()) == Some(request_id)
    });
    frames.last().expect("new_session response")["payload"]["sessionId"]
        .as_str()
        .expect("new session id")
        .to_string()
}

fn ask_question_server() -> common::serve::ScriptedOpenAiServer {
    spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("after approval"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ])
}

fn streamed_request_count(server: &ScriptedOpenAiServer) -> usize {
    server
        .captured_requests()
        .into_iter()
        .filter_map(|request| try_extract_json_body(&request))
        .filter(|body| body["stream"].as_bool() == Some(true))
        .count()
}

fn answer(
    child: &mut ServeChild,
    control: &serde_json::Value,
    session: &str,
    question: &str,
    option: &str,
) {
    child.send_value(&json!({"type":"control_response", "requestId":control["requestId"], "sessionId":session,
        "payload":{"answers":[{"question_id":question,"option_ids":[option],"custom_text":null,"skipped":false,"picked_recommended":true}],"outcome":"answered","cancelled":false}}));
}

fn wait_question(child: &ServeChild, question: &str) -> serde_json::Value {
    child
        .recv_until(WAIT_TIMEOUT, |v| {
            v["type"] == "control_request"
                && v["subtype"] == "ask_question"
                && v["payload"]["questions"][0]["id"] == question
        })
        .last()
        .unwrap()
        .clone()
}

#[test]
#[serial]
fn serve_start_plan_writes_asks_and_continues_in_the_same_turn() {
    execution_workflow(false, false);
}

#[test]
#[serial]
fn serve_stop_during_execution_question_parks_plan_and_rejects_late_answer() {
    execution_workflow(true, false);
}

#[test]
#[serial]
fn serve_stop_during_recovered_execution_question_parks_plan() {
    execution_workflow(true, true);
}

fn execution_workflow(stop: bool, restart: bool) {
    let fx = setup_serve_fixture("http://127.0.0.1:1");
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call("start-1", "start_plan", r#"{"plan_id":"vertical"}"#),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_tool_call(
                "write-1",
                "write",
                &json!({"path":fx.workspace.join("before-answer.txt"),"content":"before"})
                    .to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_tool_call("ask-1", "ask_question", ASK_QUESTION_ARGS),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_tool_call(
                "write-2",
                "write",
                &json!({"path":fx.workspace.join("after-answer.txt"),"content":"after"})
                    .to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_tool_call(
                "complete-1",
                "update_plan",
                r#"{"ops":[{"kind":"set_status","id":"step","status":"completed","evidence":["files written and user answered"]}]}"#,
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("execution finished"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let models_path = fx.home_path.join(".tomcat/models.toml");
    let models = std::fs::read_to_string(&models_path).unwrap();
    // Only the main model: the fixture's utility model must stay unreachable so
    // fire-and-forget turn summaries cannot consume scripted turns.
    std::fs::write(
        &models_path,
        models.replacen("http://127.0.0.1:1", &server.base_url, 1),
    )
    .unwrap();
    let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let mut config = tomcat::load_config_toml_file(&config_path).unwrap();
    config
        .workspace
        .workspace_roots
        .push(fx.workspace.to_string_lossy().to_string());
    std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let path = fx.home_path.join(".tomcat/plans/vertical.plan.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "---\nplan_id: vertical\ngoal: Execute in one turn\nstate: planning\ncreated_at: 2026-01-01T00:00:00Z\nschema_version: 1\ntodos:\n- id: step\n  content: Write files and ask a question\n  status: pending\n---\n## Plan\nExercise the existing execution loop.\n";
    std::fs::write(&path, original).unwrap();
    let mut child = spawn_serve_child(&fx);
    let session = initialize(&mut child);
    child.send_value(
        &json!({"type":"set_plan_mode", "id":"enter-plan", "sessionId":session, "action":"enter"}),
    );
    let entered = child.recv_until(WAIT_TIMEOUT, |v| v["id"] == "enter-plan");
    assert_eq!(entered.last().unwrap()["success"], true);
    child.send_value(&json!({"type":"prompt", "id":"implement", "sessionId":session, "text":"Implement the vertical plan now.", "params":{}}));
    let confirmation = wait_question(&child, "plan-start");
    assert_eq!(confirmation["payload"]["toolCallId"], "start-1");
    assert_eq!(
        confirmation["payload"]["questions"][0]["allowCustom"],
        false
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    answer(&mut child, &confirmation, &session, "plan-start", "start");
    let mut question = wait_question(&child, "q1");
    assert_eq!(question["payload"]["toolCallId"], "ask-1");
    assert_eq!(
        std::fs::read_to_string(fx.workspace.join("before-answer.txt")).unwrap(),
        "before"
    );
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("state: executing"));
    let requests: Vec<_> = server
        .captured_requests()
        .iter()
        .filter_map(|r| try_extract_json_body(r))
        .filter(|v| v["stream"] == true)
        .collect();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        serde_json::to_vec(&requests[0]["tools"]).unwrap(),
        serde_json::to_vec(&requests[1]["tools"]).unwrap()
    );
    let before = requests[0]["messages"].as_array().unwrap();
    let after = requests[1]["messages"].as_array().unwrap();
    // The existing Chat Completions adapter promotes runtime tail into system
    // (openai_provider_test::transport_messages_promotes_ephemeral_tail_to_leading_system_instruction).
    // Compare the stable instruction snapshot separately, not the mutable wire envelope.
    let first_system = before[0]["content"].as_str().unwrap();
    let next_system = after[0]["content"].as_str().unwrap();
    let first_offset = first_system
        .find("<system_reminder kind=\"planner\">")
        .unwrap();
    let next_offset = next_system
        .find("<system_reminder kind=\"executor\"")
        .unwrap();
    assert!(
        first_system[..first_offset] == next_system[..next_offset],
        "stable system instruction snapshot changed"
    );
    let first_tail = &first_system[first_offset..];
    let next_tail = &next_system[next_offset..];
    assert_eq!(
        first_tail
            .split_once("<system_reminder kind=\"workspace_state\">")
            .unwrap()
            .1,
        next_tail
            .split_once("<system_reminder kind=\"workspace_state\">")
            .unwrap()
            .1,
        "workspace permissions must not change when starting"
    );
    assert_eq!(
        before.iter().filter(|m| m["role"] == "user").count(),
        after.iter().filter(|m| m["role"] == "user").count(),
        "start must not inject a synthetic user message"
    );
    assert_eq!(
        serde_json::to_vec(&before[1..]).unwrap(),
        serde_json::to_vec(&after[1..before.len()]).unwrap(),
        "history prefix must remain byte-identical"
    );
    assert!(first_tail.contains("kind=\"planner\""), "{first_tail}");
    assert!(first_tail.contains("name=\"write-plans\""), "{first_tail}");
    assert!(!next_tail.contains("write-plans"), "{next_tail}");
    assert!(!next_tail.contains("kind=\"planner\""));
    assert_eq!(
        next_tail.matches("kind=\"executor\"").count(),
        1,
        "{next_tail}"
    );
    if restart {
        let old_route = question["requestId"].clone();
        drop(child);
        child = spawn_serve_child(&fx);
        assert_eq!(initialize(&mut child), session);
        question = wait_question(&child, "q1");
        assert_ne!(question["requestId"], old_route);
        assert_eq!(question["payload"]["toolCallId"], "ask-1");
    }
    if stop {
        child.send_value(&json!({"type":"interrupt", "id":"stop-execution", "sessionId":session}));
        let frames = child.recv_until(WAIT_TIMEOUT, |v| v["type"] == "agent_idle");
        assert_eq!(
            frames.last().unwrap()["outcome"],
            "interrupted",
            "{frames:?}"
        );
        assert!(
            frames.iter().any(|v| v["type"] == "control_cancel"
                && v["requestId"] == question["requestId"]
                && v["payload"]["outcome"] == "interrupted"),
            "{frames:?}"
        );
        if !restart {
            // Rehydration settles before entering AgentLoop, so its shared terminal
            // contract is agent_idle(interrupted), not AgentLoop's intermediate event.
            assert!(
                frames.iter().any(|v| v["type"] == "agent_interrupted"),
                "{frames:?}"
            );
        }
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("state: pending"));
        child.send_value(&json!({"type":"get_messages", "id":"before-late", "sessionId":session}));
        let history = child
            .recv_until(WAIT_TIMEOUT, |v| v["id"] == "before-late")
            .last()
            .unwrap()["payload"]
            .clone();
        if restart {
            let settled = history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|entry| entry["message"]["tool_call_id"] == "ask-1")
                .expect("recovered question must have a durable result");
            let content = settled["message"]["content"].as_str().unwrap();
            // GetMessages includes superseded placeholders. The latest result must
            // be terminal, whether the panel or the normal Stop finalizer wins.
            assert!(
                content == "[interrupted]"
                    || serde_json::from_str::<serde_json::Value>(content)
                        .is_ok_and(|result| result["outcome"] == "interrupted"
                            && result["cancelled"] == true),
                "unsettled recovered result: {settled}"
            );
        }
        answer(&mut child, &question, &session, "q1", "a");
        child.send_value(&json!({"type":"get_messages", "id":"after-late", "sessionId":session}));
        let frames = child.recv_until(WAIT_TIMEOUT, |v| v["id"] == "after-late");
        assert_eq!(
            frames.last().unwrap()["payload"],
            history,
            "late answers must not replace the interrupted result or append history"
        );
        assert_eq!(streamed_request_count(&server), 3);
        assert!(!fx.workspace.join("after-answer.txt").exists());
    } else {
        answer(&mut child, &question, &session, "q1", "a");
        let frames = child.recv_until(WAIT_TIMEOUT, |v| v["type"] == "agent_idle");
        assert!(
            frames
                .iter()
                .any(|v| v["assistantMessageEvent"]["delta"] == "execution finished"),
            "{frames:?}"
        );
        assert_eq!(
            std::fs::read_to_string(fx.workspace.join("after-answer.txt")).unwrap(),
            "after"
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("state: completed"));
        assert_eq!(streamed_request_count(&server), 6);
    }
}

#[test]
#[serial]
fn serve_ask_question_roundtrip_resumes_turn() {
    common::setup_logging();
    let server = ask_question_server();
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-1",
        "sessionId": session_id.clone(),
        "text": "ask me a question",
        "params": {}
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = frames.last().expect("control request");
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    let pending_frames = child.recv_available(Duration::from_millis(750));
    assert!(
        !pending_frames.iter().any(|value| {
            matches!(
                value.get("type").and_then(|v| v.as_str()),
                Some("agent_end" | "tool_execution_end" | "control_cancel")
            )
        }),
        "unanswered ask_question must stay live without a deadline: {pending_frames:?}"
    );

    child.send_value(&json!({
        "type": "control_response",
        "requestId": request_id,
        "sessionId": session_id,
        "payload": {
            "answers": [{
                "question_id": "q1",
                "option_ids": ["a"],
                "custom_text": null,
                "skipped": false,
                "picked_recommended": true
            }],
            "cancelled": false,
            "outcome": "answered"
        }
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value
                    .get("assistantMessageEvent")
                    .and_then(|v| v.get("delta"))
                    .and_then(|v| v.as_str())
                    == Some("after approval")
        }),
        "expected final delta after approval, got {frames:?}"
    );
    assert_eq!(streamed_request_count(&server), 2);
}

#[test]
#[serial]
fn serve_restart_rearms_unanswered_ask_question_with_a_new_request_id() {
    common::setup_logging();
    let server = ask_question_server();
    let fx = setup_serve_fixture(&server.base_url);

    let (session_id, initial_ask_request_id) = {
        let mut child = spawn_serve_child(&fx);
        let session_id = initialize(&mut child);
        child.send_value(&json!({
            "type": "prompt",
            "id": "ask-restart-1",
            "sessionId": session_id,
            "text": "ask me, then simulate a server restart",
            "params": {}
        }));
        let frames = child.recv_until(WAIT_TIMEOUT, |value| {
            value.get("type").and_then(|v| v.as_str()) == Some("control_request")
                && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
        });
        assert!(
            frames.iter().any(|value| {
                value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_start")
                    && value.get("toolCallId").and_then(|v| v.as_str()) == Some("call_1")
            }),
            "ask_question must be durably started before restarting: {frames:?}"
        );
        let control = frames
            .iter()
            .find(|value| {
                value.get("type").and_then(|v| v.as_str()) == Some("control_request")
                    && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
            })
            .expect("initial ask_question control request");
        (
            session_id,
            control["payload"]["requestId"]
                .as_str()
                .expect("initial ask_question response route")
                .to_string(),
        )
    };

    let mut resumed = spawn_serve_child(&fx);
    let resumed_session_id = initialize(&mut resumed);
    assert_eq!(resumed_session_id, session_id);
    let frames = resumed.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let rearmed = frames
        .iter()
        .find(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("control_request")
                && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
        })
        .expect("re-armed ask_question control request");
    assert_eq!(
        rearmed["payload"]["toolCallId"].as_str(),
        Some("call_1"),
        "restart must retain durable question identity: {rearmed:?}"
    );
    assert_ne!(
        rearmed["payload"]["requestId"].as_str(),
        Some(initial_ask_request_id.as_str()),
        "restart must create a new connection-scoped response route"
    );
    assert!(
        rearmed["payload"]["requestId"]
            .as_str()
            .is_some_and(|request_id| !request_id.is_empty()),
        "restart must issue a live response route: {rearmed:?}"
    );
}

#[test]
#[serial]
fn serve_ask_question_cancel_roundtrip_does_not_hang() {
    common::setup_logging();
    let server = ask_question_server();
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-2",
        "sessionId": session_id.clone(),
        "text": "ask then cancel",
        "params": {}
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = frames.last().expect("control request");
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    child.send_value(&json!({
        "type": "control_cancel",
        "requestId": request_id,
        "sessionId": session_id,
        "payload": {}
    }));

    let frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
    });
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_end")
                && value.get("toolName").and_then(|v| v.as_str()) == Some("ask_question")
                && value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .is_some_and(|result| {
                        result.contains("\"cancelled\":true")
                            && result.contains("\"outcome\":\"interrupted\"")
                    })
        }),
        "expected interrupted ask_question result before settling, got {frames:?}"
    );
    assert!(
        frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
                && value.get("error").is_some_and(|v| v.is_null())
        }),
        "expected cancel path to settle cleanly, got {frames:?}"
    );
}

#[test]
#[serial]
fn serve_ask_question_routes_by_session() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("second session kept running"),
            sse_finish("stop"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("after approval"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_a = initialize(&mut child);
    let session_b = new_session(&mut child, "new-2");

    child.send_value(&json!({
        "type": "prompt",
        "id": "ask-route-1",
        "sessionId": session_a.clone(),
        "text": "ask session a",
        "params": {}
    }));

    let ask_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    let control = ask_frames.last().expect("control request");
    assert_eq!(control["sessionId"].as_str(), Some(session_a.as_str()));
    let request_id = control["requestId"]
        .as_str()
        .expect("ask_question request id");

    child.send_value(&json!({
        "type": "prompt",
        "id": "session-b-1",
        "sessionId": session_b.clone(),
        "text": "run in session b",
        "params": {}
    }));
    let session_b_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
    });
    assert!(
        session_b_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
                && is_scripted_followup_delta(value)
        }),
        "expected session b to continue independently, got {session_b_frames:?}"
    );
    assert!(
        !session_b_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("control_request")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_b.as_str())
        }),
        "ask_question control should stay on session a: {session_b_frames:?}"
    );

    child.send_value(&json!({
        "type": "control_response",
        "requestId": request_id,
        "sessionId": session_a.clone(),
        "payload": {
            "answers": [{
                "question_id": "q1",
                "option_ids": ["a"],
                "custom_text": null,
                "skipped": false,
                "picked_recommended": true
            }],
            "cancelled": false,
            "outcome": "answered"
        }
    }));

    let session_a_frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
    });
    assert!(
        session_a_frames.iter().any(|value| {
            value.get("type").and_then(|v| v.as_str()) == Some("message_update")
                && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_a.as_str())
                && is_scripted_followup_delta(value)
        }),
        "expected session a to resume after approval, got {session_a_frames:?}"
    );
}

#[test]
#[serial]
fn serve_interrupt_emits_agent_interrupted_and_tool_execution_end() {
    common::setup_logging();
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_tool_call("call_1", "ask_question", ASK_QUESTION_ARGS),
        sse_finish("tool_calls"),
        sse_done(),
    ])]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut child = spawn_serve_child(&fx);
    let session_id = initialize(&mut child);

    child.send_value(&json!({
        "type": "prompt",
        "id": "interrupt-ask-1",
        "sessionId": session_id.clone(),
        "text": "ask then interrupt",
        "params": {}
    }));

    let mut frames = child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("control_request")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("ask_question")
    });
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_start")
            && value.get("toolCallId").and_then(|v| v.as_str()) == Some("call_1")
    }));

    child.send_value(&json!({
        "type": "interrupt",
        "id": "interrupt-ask-ack",
        "sessionId": session_id.clone()
    }));
    frames.extend(child.recv_until(WAIT_TIMEOUT, |value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));

    assert!(frames.iter().any(|value| {
        value.get("id").and_then(|v| v.as_str()) == Some("interrupt-ask-ack")
            && value.get("success").and_then(|v| v.as_bool()) == Some(true)
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_interrupted")
            && value.get("sessionId").and_then(|v| v.as_str()) == Some(session_id.as_str())
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("tool_execution_end")
            && value.get("toolCallId").and_then(|v| v.as_str()) == Some("call_1")
    }));
    assert!(frames.iter().any(|value| {
        value.get("type").and_then(|v| v.as_str()) == Some("agent_end")
            && value.get("error").and_then(|v| v.as_str()) == Some("interrupted")
    }));
}
