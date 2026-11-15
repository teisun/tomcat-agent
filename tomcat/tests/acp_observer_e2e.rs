mod common;

use common::acp::{self, WAIT};
use common::serve::{
    captured_non_title_requests, response, spawn_paused_openai_stream_server, sse_done, sse_finish,
    ScriptedPart,
};
use common::shared_serve::{NativeClient, ServiceFixture};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn delta(fields: Value) -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: format!("data: {}\n\n", json!({"choices":[{"delta":fields}]})),
    }
}

async fn window(info: &Value, cwd: &Path) -> NativeClient {
    let mut client = NativeClient::connect(info).await;
    client.new_session(cwd).await;
    client
}

async fn watch(client: &mut NativeClient, session: &str, access: &str) -> Vec<Value> {
    client
        .send(json!({"type":"switch_session","id":"watch","sessionId":session}))
        .await;
    let frames = client.until(|v| v["id"] == "watch").await;
    let reply = frames.last().unwrap();
    assert_eq!(reply["success"], true, "{frames:?}");
    assert_eq!(reply["payload"]["access"], access);
    assert_eq!(reply["payload"]["origin"]["kind"], "acp");
    frames
}

fn snapshot(frames: &[Value]) -> &Value {
    frames
        .iter()
        .find(|v| v["type"] == "message_update" && v["assistantMessageEvent"].is_null())
        .expect("initial full message must precede the switch reply")
}

fn live_text(frames: &[Value], message_id: &str, channel: &str) -> String {
    let mut text = String::new();
    for frame in frames
        .iter()
        .filter(|v| v["assistantMessageId"] == message_id)
    {
        if frame["type"] != "message_update" {
            continue;
        }
        if frame["assistantMessageEvent"].is_null() {
            text = frame["message"][if channel == "content_delta" {
                "content"
            } else {
                "thinking_text"
            }]
            .as_str()
            .unwrap()
            .to_owned();
        } else if frame["assistantMessageEvent"]["kind"] == channel {
            text.push_str(frame["assistantMessageEvent"]["delta"].as_str().unwrap());
        }
    }
    text
}

#[tokio::test]
async fn acp_effort_is_session_local_durable_and_reaches_the_real_request() {
    fn option(result: &Value) -> &Value {
        result["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == "effort")
            .unwrap()
    }
    fn set(
        host: &mut common::serve::ServeChild,
        session: &str,
        config: &str,
        value: &str,
    ) -> Value {
        let frames = acp::request(
            host,
            json!(format!("set-{session}-{config}-{value}")),
            "session/set_config_option",
            json!({"sessionId":session,"configId":config,"value":value}),
        );
        acp::ok(&frames).clone()
    }
    let server = common::serve::spawn_scripted_openai_stream_server_with_auto_title(
        (0..3)
            .map(|_| {
                response(vec![
                    delta(json!({"content":"effort done"})),
                    sse_finish("stop"),
                    sse_done(),
                ])
            })
            .collect(),
    );
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| cfg.llm.thinking.level = "medium".into());
    let models_path = fx.0.home_path.join(".tomcat/models.toml");
    let models = std::fs::read_to_string(&models_path).unwrap();
    std::fs::write(
        &models_path,
        models.replace(
            "id = \"utility-flash\"",
            "id = \"utility-flash\"\nsupported_reasoning_levels = []",
        ),
    )
    .unwrap();
    let mut host = acp::spawn(&fx);
    acp::initialize(&mut host);
    let a = acp::new_session(&mut host, &fx.0.workspace);
    let b = acp::new_session(&mut host, &fx.0.workspace);
    let prefs_path = fx.0.home_path.join(".tomcat/model-thinking.json");
    let prefs_before = std::fs::read(&prefs_path).ok();
    let selected = set(&mut host, &a, "effort", "high");
    assert_eq!(option(&selected)["category"], "thought_level");
    assert_eq!(option(&selected)["currentValue"], "high");
    assert_eq!(
        option(&set(&mut host, &b, "effort", "low"))["currentValue"],
        "low"
    );
    assert_eq!(
        option(&set(&mut host, &a, "model", "gpt-5.4"))["currentValue"],
        "high",
        "same effective model must preserve the override even when it was implicit"
    );
    let invalid = acp::request(
        &mut host,
        json!("invalid-effort"),
        "session/set_config_option",
        json!({"sessionId":a,"configId":"effort","value":"turbo"}),
    );
    assert!(invalid.last().unwrap().get("error").is_some());

    let mut foreign = acp::spawn(&fx);
    acp::initialize(&mut foreign);
    let denied = acp::request(
        &mut foreign,
        json!("foreign-effort"),
        "session/set_config_option",
        json!({"sessionId":a,"configId":"effort","value":"low"}),
    );
    assert!(denied.last().unwrap().get("error").is_some(), "{denied:?}");
    acp::close(foreign);

    let sessions_dir = fx.0.home_path.join(".tomcat/agents/main/sessions");
    let lock = sessions_dir.join("sessions.json.lock");
    let backup = lock.with_extension("backup");
    std::fs::rename(&lock, &backup).unwrap();
    std::fs::create_dir(&lock).unwrap();
    let failed = acp::request(
        &mut host,
        json!("save-failed"),
        "session/set_config_option",
        json!({"sessionId":b,"configId":"effort","value":"high"}),
    );
    assert!(failed.last().unwrap().get("error").is_some(), "{failed:?}");
    std::fs::remove_dir(&lock).unwrap();
    std::fs::rename(&backup, &lock).unwrap();

    let info = fx.action("status");
    let mut observer = window(&info, &fx.0.workspace).await;
    for (session, level) in [(&a, "high"), (&b, "low")] {
        watch(&mut observer, session, "observe").await;
        let state = observer
            .request(
                json!({"type":"get_state","id":format!("state-{session}"),"sessionId":session}),
            )
            .await;
        assert_eq!(state["payload"]["thinkingLevel"], level);
        acp::prompt(&mut host, level, session, "check the selected effort");
        acp::ok(&host.recv_until(WAIT, |v| v["id"] == level));
    }
    assert_eq!(
        std::fs::read(&prefs_path).ok(),
        prefs_before,
        "ACP must not write model defaults"
    );
    drop(observer);
    acp::close(host);
    fx.action("stop");
    let mut host = acp::spawn(&fx);
    acp::initialize(&mut host);
    for (session, level) in [(&a, "high"), (&b, "low")] {
        let restored = acp::request(
            &mut host,
            json!(format!("restore-{session}")),
            "session/load",
            json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
        );
        assert_eq!(option(acp::ok(&restored))["currentValue"], level);
    }
    assert_eq!(
        option(&set(&mut host, &a, "effort", "default"))["currentValue"],
        "default"
    );
    acp::prompt(&mut host, "default-request", &a, "check inherited effort");
    acp::ok(&host.recv_until(WAIT, |v| v["id"] == "default-request"));
    let changed = set(&mut host, &b, "model", "utility-flash");
    assert_eq!(option(&changed)["currentValue"], "default");
    assert_eq!(option(&changed)["options"].as_array().unwrap().len(), 1);
    set(&mut host, &b, "effort", "default");
    assert_eq!(std::fs::read(&prefs_path).ok(), prefs_before);
    acp::close(host);
    let requests: Vec<Value> = captured_non_title_requests(&server)
        .iter()
        .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["reasoning_effort"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["high", "low", "medium"]
    );
}

#[tokio::test]
async fn live_acp_observation_reconnects_without_gaps_and_foreign_project_never_receives_it() {
    // The provider cannot finish until the test has opened/reopened the view.
    let release = Arc::new(AtomicBool::new(false));
    let server = spawn_paused_openai_stream_server(
        vec![response(vec![
            delta(json!({"reasoning_content":"先检查 "})),
            delta(json!({"content":"你好，\n"})),
            delta(json!({"reasoning_content":"再验证"})),
            delta(json!({"content":"世界 👋"})),
            sse_finish("stop"),
            sse_done(),
        ])],
        2,
        release.clone(),
    );
    let fx = ServiceFixture::new(&server.base_url);
    let mut host = acp::spawn(&fx);
    acp::initialize(&mut host);
    let session = acp::new_session(&mut host, &fx.0.workspace);
    acp::prompt(&mut host, "run", &session, "stream-one");
    let mut host_frames = host.recv_until(WAIT, |v| {
        v["params"]["update"]["content"]["text"] == "你好，\n"
    });
    let info = fx.action("status");
    let mut observer = window(&info, &fx.0.workspace).await;
    let list = observer
        .request(json!({"type":"list_sessions","id":"list","scope":"disk"}))
        .await;
    assert!(list["payload"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["sessionId"] == session));
    let first = watch(&mut observer, &session, "observe").await;
    assert_eq!(snapshot(&first)["message"]["content"], "你好，\n");
    assert_eq!(snapshot(&first)["message"]["thinking_text"], "先检查 ");
    let message_id = snapshot(&first)["assistantMessageId"]
        .as_str()
        .unwrap()
        .to_owned();
    let state = observer
        .request(json!({"type":"get_state","id":"state"}))
        .await;
    assert_eq!(state["payload"]["busy"], true);
    let history = observer
        .request(json!({"type":"get_messages","id":"history"}))
        .await;
    assert!(history["payload"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["message"]["content"] == "stream-one"));
    assert!(
        !history["payload"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == message_id),
        "unfinished message must not be invented in history"
    );

    let other = tempfile::tempdir().unwrap();
    let mut foreign = window(&info, other.path()).await;
    let list = foreign
        .request(json!({"type":"list_sessions","id":"foreign-list","scope":"disk"}))
        .await;
    assert!(!list["payload"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["sessionId"] == session));
    foreign
        .send(json!({"type":"switch_session","id":"forbidden","sessionId":session}))
        .await;
    let rejected = foreign.until(|v| v["id"] == "forbidden").await;
    assert_eq!(rejected.last().unwrap()["error"], "unknown_session");
    assert!(!rejected.iter().any(|v| v["type"] == "message_update"));
    assert_eq!(
        foreign
            .request(json!({"type":"get_messages","id":"forbidden-read","sessionId":session}))
            .await["error"],
        "unknown_session"
    );

    // Closing a subscribed tab and then its transport must not cancel the host.
    assert_eq!(
        observer
            .request(json!({"type":"close_session","id":"close-view","sessionId":session}))
            .await["success"],
        true
    );
    drop(observer);
    let mut observer = window(&info, &fx.0.workspace).await;
    let mut frames = watch(&mut observer, &session, "observe").await;
    assert_eq!(snapshot(&frames)["assistantMessageId"], message_id);
    assert_eq!(snapshot(&frames)["message"]["content"], "你好，\n");
    assert_eq!(
        observer
            .request(json!({"type":"get_state","id":"reconnected-state"}))
            .await["payload"]["busy"],
        true
    );
    observer
        .request(json!({"type":"get_messages","id":"reconnected-history"}))
        .await;
    release.store(true, Ordering::Release);
    frames.extend(
        observer
            .until(|v| v["type"] == "agent_idle" && v["sessionId"] == session)
            .await,
    );
    host_frames.extend(host.recv_until(WAIT, |v| v["id"] == "run"));
    assert_eq!(acp::ok(&host_frames)["stopReason"], "end_turn");
    assert_eq!(
        live_text(&frames, &message_id, "content_delta"),
        "你好，\n世界 👋"
    );
    assert_eq!(
        live_text(&frames, &message_id, "thinking_delta"),
        "先检查 再验证"
    );
    let history = observer
        .request(json!({"type":"get_messages","id":"finished-history"}))
        .await;
    let rows: Vec<_> = history["payload"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] == message_id)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["message"]["content"], "你好，\n世界 👋");
    foreign
        .send(json!({"type":"get_state","id":"foreign-fence"}))
        .await;
    let foreign_frames = foreign.until(|v| v["id"] == "foreign-fence").await;
    assert!(!foreign_frames
        .iter()
        .any(|v| v["sessionId"] == session && v["type"] != "response"));

    let mut late = window(&info, &fx.0.workspace).await;
    let finished = watch(&mut late, &session, "observe").await;
    assert_eq!(snapshot(&finished)["message"]["content"], "你好，\n世界 👋");
    assert!(finished
        .iter()
        .any(|v| v["type"] == "message_end" && v["assistantMessageId"] == message_id));
    assert_eq!(
        captured_non_title_requests(&server).len(),
        1,
        "watching must not call the model again"
    );
    acp::close(host);
    let mut returning_host = acp::spawn(&fx);
    acp::initialize(&mut returning_host);
    let loaded = acp::request(
        &mut returning_host,
        json!("load"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    acp::ok(&loaded);
    acp::close(returning_host);
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
}

#[tokio::test]
async fn native_steers_and_stops_real_acp_then_both_clients_relay_the_same_session() {
    let release = Arc::new(AtomicBool::new(false));
    let done = |text: &str| {
        response(vec![
            delta(json!({"content":text})),
            sse_finish("stop"),
            sse_done(),
        ])
    };
    let server = spawn_paused_openai_stream_server(
        vec![
            response(vec![
                delta(json!({"content":"working"})),
                common::serve::sse_tool_call(
                    "todo",
                    "todos",
                    r#"{"ops":[{"kind":"upsert","id":"check","content":"Check input"}]}"#,
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            done("heard the steering"),
            done("cancel this turn"),
            done("native continuation"),
            response(vec![
                ScriptedPart {
                    delay_ms: 0,
                    body: String::new(),
                },
                delta(json!({"content":"returned to ACP"})),
                sse_finish("stop"),
                sse_done(),
            ]),
        ],
        1,
        release.clone(),
    );
    let fx = ServiceFixture::new(&server.base_url);
    let mut host = acp::spawn(&fx);
    acp::initialize(&mut host);
    let session = acp::new_session(&mut host, &fx.0.workspace);
    acp::prompt(&mut host, "steering", &session, "work on the task");
    host.recv_until(WAIT, |v| {
        v["params"]["update"]["content"]["text"] == "working"
    });
    let info = fx.action("status");
    let mut native = window(&info, &fx.0.workspace).await;
    watch(&mut native, &session, "observe").await;
    let reply = native
        .request(json!({"type":"steer", "id":"steer", "sessionId":session,
        "text":"test before implementing", "params":{"onlyIfRunning":true}}))
        .await;
    assert_eq!(reply["payload"]["queued"], true, "{reply}");
    release.store(true, Ordering::Release);
    let frames = native.until(|v| v["type"] == "agent_idle").await;
    assert_eq!(frames.last().unwrap()["outcome"], "completed");
    assert!(!frames.iter().any(|v| v["type"] == "session_access"));
    let completed = host.recv_until(WAIT, |v| v["id"] == "steering");
    assert_eq!(acp::ok(&completed)["stopReason"], "end_turn");
    assert!(completed
        .iter()
        .any(|v| v["params"]["update"]["content"]["text"] == "heard the steering"));
    let requests = captured_non_title_requests(&server);
    let next_request: Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(
        next_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "test before implementing"),
        "{requests:?}"
    );

    release.store(false, Ordering::Release);
    acp::prompt(&mut host, "cancel", &session, "stop this task");
    host.recv_until(WAIT, |v| {
        v["params"]["update"]["content"]["text"] == "cancel this turn"
    });
    assert_eq!(
        native
            .request(json!({"type":"interrupt","id":"stop","sessionId":session}))
            .await["success"],
        true
    );
    let frames = native.until(|v| v["type"] == "agent_idle").await;
    assert_eq!(frames.last().unwrap()["outcome"], "interrupted");
    let cancelled = host.recv_until(WAIT, |v| v["id"] == "cancel");
    assert_eq!(acp::ok(&cancelled)["stopReason"], "cancelled");
    release.store(true, Ordering::Release);
    acp::close(host);

    // Native takes over without reopening or changing the session/selection.
    native.send(json!({"type":"prompt","id":"continue","sessionId":session,"text":"continue from VS Code"})).await;
    let mut frames = native.until(|v| v["id"] == "continue").await;
    assert_eq!(frames.last().unwrap()["success"], true, "{frames:?}");
    frames.extend(native.until(|v| v["type"] == "agent_idle").await);
    assert!(frames
        .iter()
        .any(|v| v["type"] == "session_access" && v["access"] == "control"));
    assert!(
        frames.iter().any(|v| v["type"] == "message_update"
            && v["assistantMessageEvent"]["delta"] == "native continuation"),
        "{frames:?}"
    );
    assert_eq!(frames.last().unwrap()["outcome"], "completed");

    let mut returned = acp::spawn(&fx);
    acp::initialize(&mut returned);
    let history = acp::request(
        &mut returned,
        json!("load"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    acp::ok(&history);
    assert!(history.iter().any(
        |v| v["params"]["update"]["sessionUpdate"] == "user_message_chunk"
            && v["params"]["update"]["content"]["text"] == "continue from VS Code"
    ));
    release.store(false, Ordering::Release);
    acp::prompt(&mut returned, "return", &session, "continue in ACP again");
    // The view stays on the same session: no switch or get_messages repairs it.
    let mut frames = native.until(|v| v["type"] == "agent_start").await;
    let user_events: Vec<_> = frames
        .iter()
        .filter(|v| v["type"] == "session.user_message")
        .collect();
    assert_eq!(user_events.len(), 1, "{frames:?}");
    assert_eq!(user_events[0]["sessionId"], session);
    assert_eq!(
        user_events[0]["message"]["content"],
        "continue in ACP again"
    );
    assert!(!user_events[0]["userMessageId"].as_str().unwrap().is_empty());
    assert!(!frames.iter().any(|v| v["type"] == "message_update"));
    release.store(true, Ordering::Release);
    let returned_frames = returned.recv_until(WAIT, |v| v["id"] == "return");
    assert_eq!(acp::ok(&returned_frames)["stopReason"], "end_turn");
    assert!(
        !returned_frames
            .iter()
            .any(|v| v["params"]["update"]["sessionUpdate"] == "user_message_chunk"),
        "live input must not echo to ACP"
    );
    frames.extend(native.until(|v| v["type"] == "agent_idle").await);
    assert!(frames
        .iter()
        .any(|v| v["type"] == "session_access" && v["access"] == "observe"));
    assert!(
        frames.iter().any(|v| v["type"] == "message_update"
            && v["assistantMessageEvent"]["delta"] == "returned to ACP"),
        "{frames:?}"
    );
    acp::close(returned);
}

#[tokio::test]
async fn controller_eof_cancels_a_paused_stream_and_replay_reports_it_ended() {
    let release = Arc::new(AtomicBool::new(false));
    let server = spawn_paused_openai_stream_server(
        vec![response(vec![
            delta(json!({"content":"partial "})),
            delta(json!({"content":"must-not-arrive"})),
            sse_finish("stop"),
            sse_done(),
        ])],
        1,
        release.clone(),
    );
    let fx = ServiceFixture::new(&server.base_url);
    let mut host = acp::spawn(&fx);
    acp::initialize(&mut host);
    let session = acp::new_session(&mut host, &fx.0.workspace);
    acp::prompt(&mut host, "run", &session, "cancel-me");
    host.recv_until(WAIT, |v| {
        v["params"]["update"]["content"]["text"] == "partial "
    });
    let info = fx.action("status");
    let mut observer = window(&info, &fx.0.workspace).await;
    let first = watch(&mut observer, &session, "observe").await;
    let message_id = snapshot(&first)["assistantMessageId"]
        .as_str()
        .unwrap()
        .to_owned();
    acp::close(host); // EOF acknowledgement comes only after the turn is stopped.
    observer
        .until(|v| v["type"] == "agent_idle" && v["sessionId"] == session)
        .await;
    release.store(true, Ordering::Release);
    let state = observer
        .request(json!({"type":"get_state","id":"cancelled-state"}))
        .await;
    assert_eq!(state["payload"]["busy"], false);
    assert_eq!(state["payload"]["interrupted"], true);
    let mut late = window(&info, &fx.0.workspace).await;
    let frames = watch(&mut late, &session, "control").await;
    assert_eq!(snapshot(&frames)["message"]["content"], "partial ");
    assert!(frames
        .iter()
        .any(|v| v["type"] == "message_end" && v["assistantMessageId"] == message_id));
    assert_eq!(captured_non_title_requests(&server).len(), 1);
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
}
