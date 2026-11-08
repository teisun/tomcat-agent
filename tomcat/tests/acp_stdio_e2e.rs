mod common;

use common::serve::{
    response, spawn_scripted_openai_stream_server_with_auto_title, spawn_serve_command, sse_delta,
    sse_done, sse_finish, sse_tool_call, ServeChild,
};
use common::shared_serve::ServiceFixture;
use serde_json::{json, Value};
#[cfg(unix)]
use std::path::Path;
use std::{process::Stdio, time::Duration};

use common::acp::{close, initialize, new_session, ok, prompt, request, spawn, WAIT};

#[path = "support/acp_skills.rs"]
mod skills;
fn text(frames: &[Value], kind: &str) -> String {
    frames
        .iter()
        .filter(|v| v["params"]["update"]["sessionUpdate"] == kind)
        .filter_map(|v| v["params"]["update"]["content"]["text"].as_str())
        .collect()
}

#[test]
fn acp_startup_config_applies_before_new_load_and_resets_per_controller() {
    fn value<'a>(options: &'a Value, id: &str) -> &'a Value {
        &options
            .as_array()
            .unwrap()
            .iter()
            .find(|option| option["id"] == id)
            .unwrap()["currentValue"]
    }
    fn contract(options: &Value) {
        assert_eq!(options.as_array().unwrap().len(), 5);
        for option in options.as_array().unwrap() {
            assert_eq!(option["type"], "select");
            assert!(option["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|choice| choice["value"] == option["currentValue"]));
            assert!(option["name"].is_string());
        }
    }
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut command = fx.command();
    command.args([
        "acp",
        "--mode=plan",
        "--plan-review=auto",
        "--question-policy=recommended",
        "--plan-start=auto",
    ]);
    let mut host = spawn_serve_command(command);
    initialize(&mut host);
    let created = request(
        &mut host,
        json!("create"),
        "session/new",
        json!({"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    let mut observed = created.clone();
    let options = &ok(&created)["configOptions"];
    contract(options);
    for (id, expected) in [
        ("agent_mode", "plan"),
        ("plan_review", "auto"),
        ("question_policy", "recommended"),
        ("plan_start", "auto"),
    ] {
        assert_eq!(value(options, id), expected);
    }
    let session = ok(&created)["sessionId"].as_str().unwrap().to_owned();
    let loaded = request(
        &mut host,
        json!("same"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    assert_eq!(ok(&loaded)["configOptions"], *options);
    observed.extend(loaded);
    let second = request(
        &mut host,
        json!("second"),
        "session/new",
        json!({"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    let second_id = ok(&second)["sessionId"].as_str().unwrap().to_owned();
    observed.extend(second);
    for (id, expected) in [
        ("agent_mode", "chat"),
        ("plan_review", "skip"),
        ("question_policy", "manual"),
        ("plan_start", "ask"),
    ] {
        let changed = request(
            &mut host,
            json!(format!("second-{id}")),
            "session/set_config_option",
            json!({"sessionId":second_id,"configId":id,"value":expected}),
        );
        contract(&ok(&changed)["configOptions"]);
        assert_eq!(value(&ok(&changed)["configOptions"], id), expected);
        observed.extend(changed);
    }
    // Reading through a no-op setter must not reset the first session's policies.
    let unchanged = request(
        &mut host,
        json!("first-unchanged"),
        "session/set_config_option",
        json!({"sessionId":session,"configId":"agent_mode","value":"plan"}),
    );
    assert_eq!(ok(&unchanged)["configOptions"], *options);
    observed.extend(unchanged);
    for (id, invalid) in [
        ("agent_mode", "exec"),
        ("plan_review", "false"),
        ("question_policy", "auto"),
        ("plan_start", "skip"),
        ("unknown", "ask"),
    ] {
        let frames = request(
            &mut host,
            json!("invalid"),
            "session/set_config_option",
            json!({"sessionId":session,"configId":id,"value":invalid}),
        );
        assert_eq!(frames.last().unwrap()["error"]["code"], -32602);
        observed.extend(frames);
    }
    let output = host.wait_for_exit(Duration::from_secs(65));
    assert!(output.status.success(), "{output:?}");
    observed.extend(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()),
    );
    let mut next = spawn(&fx);
    initialize(&mut next);
    let loaded = request(
        &mut next,
        json!("load"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    );
    let options = &ok(&loaded)["configOptions"];
    observed.extend(loaded.iter().cloned());
    contract(options);
    for (id, expected) in [
        ("agent_mode", "plan"),
        ("plan_review", "ask"),
        ("question_policy", "manual"),
        ("plan_start", "ask"),
    ] {
        assert_eq!(value(options, id), expected);
    }
    let changed = request(
        &mut next,
        json!("chat"),
        "session/set_config_option",
        json!({"sessionId":session,"configId":"agent_mode","value":"chat"}),
    );
    contract(&ok(&changed)["configOptions"]);
    assert_eq!(value(&ok(&changed)["configOptions"], "agent_mode"), "chat");
    observed.extend(changed);
    let output = next.wait_for_exit(Duration::from_secs(65));
    assert!(output.status.success(), "{output:?}");
    observed.extend(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()),
    );
    assert!(observed.iter().all(|frame| frame["jsonrpc"] == "2.0"));
    assert!(
        observed
            .iter()
            .all(|frame| { frame["params"]["update"]["sessionUpdate"] != "config_option_update" }),
        "client configuration responses already carry the current options: {observed:?}"
    );
    assert!(
        server.captured_requests().is_empty(),
        "configuration must never call a model"
    );
}

#[test]
fn acp_agent_plan_execution_notifies_mode_change_once() {
    let tool = |id: &str, name: &str, arguments: Value| {
        response(vec![
            sse_tool_call(id, name, &arguments.to_string()),
            sse_finish("tool_calls"),
            sse_done(),
        ])
    };
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        tool(
            "create",
            "create_plan",
            json!({
                "goal":"Mode notification fixture", "draft":"Complete one disposable step.",
                "todos":[{"id":"step", "content":"Complete fixture", "status":"pending"}]
            }),
        ),
        tool("start", "start_plan", json!({})),
        tool(
            "finish",
            "update_plan",
            json!({"ops":[{
                "kind":"set_status", "id":"step", "status":"completed", "evidence":["fixture complete"]
            }]}),
        ),
        response(vec![sse_delta("finished"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut command = fx.command();
    command.args([
        "acp",
        "--mode=plan",
        "--plan-review=skip",
        "--plan-start=auto",
    ]);
    let mut host = spawn_serve_command(command);
    initialize(&mut host);
    let mut observed = request(
        &mut host,
        json!("new"),
        "session/new",
        json!({"cwd":fx.0.workspace, "mcpServers":[]}),
    );
    let session = ok(&observed)["sessionId"].as_str().unwrap().to_owned();
    prompt(
        &mut host,
        "execute",
        &session,
        "Write the plan and explicitly implement it to completion.",
    );
    let completed = host.recv_until(WAIT, |frame| frame["id"] == "execute");
    assert_eq!(ok(&completed)["stopReason"], "end_turn");
    assert_eq!(text(&completed, "agent_message_chunk"), "finished");
    observed.extend(completed);
    let notified =
        |frame: &Value| frame["params"]["update"]["sessionUpdate"] == "config_option_update";
    if !observed.iter().any(notified) {
        observed.extend(host.recv_until(WAIT, notified));
    }
    let output = host.wait_for_exit(Duration::from_secs(65));
    assert!(output.status.success(), "{output:?}");
    observed.extend(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()),
    );
    let updates = observed
        .iter()
        .filter(|frame| notified(frame))
        .collect::<Vec<_>>();
    assert_eq!(
        updates.len(),
        1,
        "exactly one Agent-initiated mode change: {observed:?}"
    );
    assert_eq!(updates[0]["params"]["sessionId"], session);
    let options = updates[0]["params"]["update"]["configOptions"]
        .as_array()
        .unwrap();
    assert_eq!(options.len(), 5);
    assert_eq!(
        options
            .iter()
            .find(|option| option["id"] == "agent_mode")
            .unwrap()["currentValue"],
        "chat"
    );
    for option in options {
        assert_eq!(option["type"], "select");
        assert!(option["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|choice| choice["value"] == option["currentValue"]));
    }
    let plans = std::fs::read_dir(fx.0.home_path.join(".tomcat/plans"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".plan.md"))
        .collect::<Vec<_>>();
    assert_eq!(plans.len(), 1);
    assert!(std::fs::read_to_string(&plans[0])
        .unwrap()
        .contains("state: completed"));
}

#[test]
fn acp_questions_use_explicit_recommendation_policy_and_record_source() {
    let question = json!({"questions":[{"id":"choice","prompt":"Which option?","options":[
        {"id":"a","label":"A","recommended":true}, {"id":"b","label":"B"}
    ]}]});
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call("stray-question", "ask_question", &question.to_string()),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![sse_delta("continued"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut command = fx.command();
    command.args(["acp", "--question-policy=recommended"]);
    let mut host = spawn_serve_command(command);
    initialize(&mut host);
    let session = new_session(&mut host, &fx.0.workspace);
    prompt(
        &mut host,
        "question",
        &session,
        "ask a question using the fixture",
    );
    let frames = host.recv_until(WAIT, |v| v["id"] == "question");
    assert_eq!(ok(&frames)["stopReason"], "end_turn");
    let reply = text(&frames, "agent_message_chunk");
    assert_eq!(reply, "continued");
    assert!(!reply.contains("cannot collect"));
    close(host);

    let bodies: Vec<Value> = server
        .captured_requests()
        .iter()
        .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
        .filter(|body: &Value| body["model"] == "gpt-5.4")
        .collect();
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    assert!(bodies[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"] == "ask_question"));
    assert!(
        bodies[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("policy_recommended")
                        && text.contains("\"option_ids\":[\"a\"]"))),
        "the model must receive the marked policy answer: {}",
        bodies[1]
    );
}

#[tokio::test]
async fn official_client_selects_a_model_and_receives_a_real_stream() {
    use agent_client_protocol::{schema::v1 as acp, schema::ProtocolVersion, ByteStreams, Client};
    use std::sync::{Arc, Mutex};
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("hello from model"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = ServiceFixture::new(&server.base_url);
    let added = fx
        .command()
        .args([
            "model",
            "add",
            "acp-alternate",
            "--api",
            "openai",
            "--provider",
            "openai",
            "--base-url",
            &server.base_url,
            "--model-name",
            "alternate-wire",
            "--tools",
        ])
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    let mut command = tokio::process::Command::from(fx.command());
    let mut child = command
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = ByteStreams::new(
        child.stdin.take().unwrap().compat_write(),
        child.stdout.take().unwrap().compat(),
    );
    let updates = Arc::new(Mutex::new(Vec::new()));
    let collected = updates.clone();
    let cwd = fx.0.workspace.clone();
    tokio::time::timeout(
        WAIT,
        Client
            .builder()
            .on_receive_notification(
                async move |update: acp::SessionNotification, _| {
                    collected
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(update).unwrap());
                    Ok::<(), acp::Error>(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(transport, async move |cx| {
                let init = cx
                    .send_request(acp::InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert!(init.agent_capabilities.load_session);
                let created = cx
                    .send_request(acp::NewSessionRequest::new(cwd))
                    .block_task()
                    .await?;
                let model: acp::SetSessionConfigOptionRequest = serde_json::from_value(json!({
                    "sessionId":created.session_id, "configId":"model", "value":"acp-alternate"
                }))
                .unwrap();
                let changed = cx.send_request(model).block_task().await?;
                assert_eq!(
                    serde_json::to_value(changed).unwrap()["configOptions"][0]["currentValue"],
                    "acp-alternate"
                );
                let done = cx
                    .send_request(acp::PromptRequest::new(
                        created.session_id,
                        vec!["hello".into()],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(done.stop_reason, acp::StopReason::EndTurn);
                Ok::<(), acp::Error>(())
            }),
    )
    .await
    .expect("official client deadline")
    .unwrap();
    let output = tokio::time::timeout(WAIT, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let streamed: String = updates
        .lock()
        .unwrap()
        .iter()
        .filter(|v| v["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|v| v["update"]["content"]["text"].as_str())
        .collect();
    assert_eq!(streamed, "hello from model");
    let requests: Vec<Value> = common::serve::captured_non_title_requests(&server)
        .iter()
        .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    let selected = requests
        .iter()
        .find(|request| request["model"] == "alternate-wire")
        .expect("selected model must reach the provider");
    let cwd = tomcat::normalize_path(fx.0.workspace.to_str().unwrap()).unwrap();
    // Compare decoded text, not JSON-escaped Windows backslashes or 8.3 aliases.
    assert!(
        selected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains(cwd.to_str().unwrap()))
            }),
        "session cwd {cwd:?} must reach the model: {selected}"
    );
    assert_eq!(
        fx.action("status")["running"],
        true,
        "closing a forwarder must not stop the service"
    );
}

async fn receive_raw(
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    id: &str,
) -> Vec<Value> {
    use tokio::io::AsyncBufReadExt;
    tokio::time::timeout(WAIT, async {
        let mut frames = Vec::new();
        loop {
            let mut line = String::new();
            assert!(
                stdout.read_line(&mut line).await.unwrap() > 0,
                "unexpected ACP EOF"
            );
            let frame: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(frame["jsonrpc"], "2.0");
            let finished = frame["id"] == id;
            frames.push(frame);
            if finished {
                return frames;
            }
        }
    })
    .await
    .expect("ACP reply deadline")
}

#[tokio::test]
async fn slow_host_still_receives_the_complete_reply() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

    let chunks: Vec<_> = (0..500)
        .map(|i| format!("{i:04}{}", "x".repeat(2044)))
        .collect();
    let expected = chunks.concat();
    let mut parts: Vec<_> = chunks.iter().map(|chunk| sse_delta(chunk)).collect();
    parts.extend([sse_finish("stop"), sse_done()]);
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(parts)]);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.serve.delta_coalesce_ms = 0;
        cfg.serve.max_buffered_frames = 4;
    });
    // ServeChild reads stdout on a background thread; use a raw pipe so the pause
    // below actually blocks the host transport rather than just its assertions.
    let mut child = tokio::process::Command::from(fx.command())
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    for (id, method, params) in [
        (
            "init",
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{}}),
        ),
        (
            "new",
            "session/new",
            json!({"cwd":fx.0.workspace,"mcpServers":[]}),
        ),
    ] {
        stdin
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let frames = receive_raw(&mut stdout, id).await;
        let reply = ok(&frames);
        if id == "new" {
            stdin.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":"slow","method":"session/prompt","params":{
                "sessionId":reply["sessionId"],"prompt":[{"type":"text","text":"emit the large fixture"}]
            }})).as_bytes()).await.unwrap();
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let frames = receive_raw(&mut stdout, "slow").await;
    assert_eq!(ok(&frames)["stopReason"], "end_turn");
    let received = text(&frames, "agent_message_chunk");
    assert!(!received.contains("Tomcat transport notice"));
    assert!(
        received == expected,
        "reply differs: received {} of {} bytes",
        received.len(),
        expected.len()
    );
    drop(stdin);
    let mut remaining = Vec::new();
    tokio::time::timeout(WAIT, stdout.read_to_end(&mut remaining))
        .await
        .unwrap()
        .unwrap();
    assert!(tokio::time::timeout(WAIT, child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn cancel_stops_background_work_before_a_backpressured_host_resumes_reading() {
    use common::shared_serve::NativeClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

    let background = |name: &str| {
        response(vec![
            sse_tool_call(
                name,
                "bash",
                &json!({"command":background_writer_command(name), "run_in_background":true})
                    .to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ])
    };
    let chunks: Vec<_> = (0..500)
        .map(|i| format!("{i:04}{}", "x".repeat(2044)))
        .collect();
    let marker = "all queued before cancellation";
    let expected = chunks.concat() + marker;
    let mut parts: Vec<_> = chunks.iter().map(|chunk| sse_delta(chunk)).collect();
    parts.extend([sse_delta(marker), sse_finish("stop"), sse_done()]);
    let server = common::serve::spawn_paused_openai_stream_server(
        vec![
            background("other"),
            response(vec![
                sse_delta("other is working"),
                sse_finish("stop"),
                sse_done(),
            ]),
            background("job"),
            response(parts),
        ],
        chunks.len() + 1,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.serve.delta_coalesce_ms = 0;
        cfg.serve.max_buffered_frames = 4096;
    });
    let info = fx.action("start");
    // This task-running native connection must supply its own Git/Node search path.
    // Keep its home isolated and omit unrelated host variables and secrets.
    let mut environment = json!({"HOME":fx.0.home_path,"USERPROFILE":fx.0.home_path});
    for key in ["PATH", "PATHEXT", "SYSTEMROOT"] {
        if let Ok(value) = std::env::var(key) {
            environment[key] = value.into();
        }
    }
    let (mut observer, hello) =
        NativeClient::open_with_environment(&info, env!("CARGO_PKG_VERSION"), environment).await;
    assert!(hello.get("error").is_none(), "{hello}");
    assert_eq!(hello["instanceId"], info["instanceId"]);
    observer.send(json!({"type":"control_request","requestId":"init","subtype":"initialize","payload":{}})).await;
    observer
        .until(|v| v["type"] == "control_response" && v["requestId"] == "init")
        .await;
    let other = observer.new_session(&fx.0.workspace).await;
    let accepted = observer.request(json!({"type":"prompt","id":"other-work","sessionId":other,"text":"start the other writer"})).await;
    assert_eq!(accepted["success"], true);
    let other_frames = observer
        .until(|v| v["assistantMessageEvent"]["delta"] == "other is working")
        .await;

    // Unlike ServeChild, no background reader drains this real host pipe.
    let mut child = tokio::process::Command::from(fx.command())
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut session = Value::Null;
    for (id, method, params) in [
        (
            "init",
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{}}),
        ),
        (
            "new",
            "session/new",
            json!({"cwd":fx.0.workspace,"mcpServers":[]}),
        ),
    ] {
        stdin
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let frames = receive_raw(&mut stdout, id).await;
        if id == "new" {
            session = ok(&frames)["sessionId"].clone();
        }
    }
    let opened = observer
        .request(json!({"type":"switch_session","id":"observe","sessionId":session}))
        .await;
    assert_eq!(opened["success"], true);
    assert_eq!(opened["payload"]["access"], "observe");
    stdin.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":"work","method":"session/prompt","params":{
        "sessionId":session,"prompt":[{"type":"text","text":"fill stdout and keep writing"}]
    }})).as_bytes()).await.unwrap();
    // Independent native observation proves the megabyte was queued before cancel;
    // the ACP host still has not consumed a single prompt output frame.
    let job_frames = observer
        .until(|v| v["sessionId"] == session && v["assistantMessageEvent"]["delta"] == marker)
        .await;
    let pid = |name: &str| {
        std::fs::read_to_string(fx.0.workspace.join(format!("{name}.pid")))
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
    };
    tokio::time::timeout(WAIT, async {
        while pid("job").is_none() || pid("other").is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        // Diagnose via already-consumed native frames, never drain the ACP pipe.
        for frame in other_frames.iter().chain(&job_frames) {
            if frame["type"] == "tool_execution_end" {
                eprintln!("writer tool result: {frame}");
            }
        }
        panic!(
            "both real writers started: {error}; job PID={:?}, other PID={:?}",
            pid("job"),
            pid("other")
        );
    });
    let job_pid = pid("job").unwrap();
    let other_pid = pid("other").unwrap();
    assert!(fixture_process_alive(job_pid) && fixture_process_alive(other_pid));
    stdin
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while fixture_process_alive(job_pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("cancel must stop the shell while host stdout is still blocked");
    let ticks = |name: &str| {
        std::fs::metadata(fx.0.workspace.join(format!("{name}.ticks")))
            .unwrap()
            .len()
    };
    let before = (ticks("job"), ticks("other"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ticks("job"), before.0);
    assert!(fixture_process_alive(other_pid) && ticks("other") > before.1);

    let frames = receive_raw(&mut stdout, "work").await;
    let terminals: Vec<_> = frames.iter().filter(|v| v["id"] == "work").collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["result"]["stopReason"], "cancelled");
    assert_eq!(text(&frames, "agent_message_chunk"), expected);

    let terminal = frames.iter().position(|v| v["id"] == "work").unwrap();
    assert!(frames[terminal + 1..]
        .iter()
        .all(|v| v["method"] != "session/update"));
    let closed = observer
        .request(json!({"type":"close_session","id":"stop-other","sessionId":other}))
        .await;
    assert_eq!(closed["success"], true);
    drop(stdin);
    let mut remaining = Vec::new();
    tokio::time::timeout(WAIT, stdout.read_to_end(&mut remaining))
        .await
        .unwrap()
        .unwrap();
    assert!(
        remaining.is_empty(),
        "no second terminal or late update after cancellation"
    );
    assert!(tokio::time::timeout(WAIT, child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}

#[tokio::test]
async fn acp_hello_is_gated_by_protocol_not_cli_version() {
    use common::shared_serve::NativeClient;
    use tokio::io::BufReader;
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let info = fx.action("start");
    for protocol_version in [1, 99] {
        let endpoint = info["endpoint"].as_str().unwrap();
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(endpoint).await.unwrap();
        #[cfg(windows)]
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(endpoint)
            .unwrap();
        let mut client = NativeClient(BufReader::new(Box::new(stream)));
        client
            .send(
                json!({"service":"tomcat","protocolVersion":protocol_version,
            "clientVersion":"0.0.0-other","protocol":"acp"}),
            )
            .await;
        let hello = client.next().await;
        if protocol_version == 99 {
            assert_eq!(hello["error"], "local_protocol_mismatch");
            continue;
        }
        assert!(hello.get("error").is_none(), "{hello}");
        assert_eq!(hello["instanceId"], info["instanceId"]);
        let initialized = client
            .request(json!({"jsonrpc":"2.0","id":"init","method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{}}}))
            .await;
        assert!(initialized.get("error").is_none(), "{initialized}");
        assert_eq!(initialized["result"]["protocolVersion"], 1);
        let created = client
            .request(json!({"jsonrpc":"2.0","id":"new","method":"session/new",
            "params":{"cwd":fx.0.workspace,"mcpServers":[]}}))
            .await;
        assert!(created.get("error").is_none(), "{created}");
        assert!(created["result"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
    }
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
}

#[cfg(unix)]
#[test]
fn connection_environment_reaches_bash_and_is_replaced_on_live_session_takeover() {
    use std::os::unix::fs::PermissionsExt;
    let mut script = Vec::new();
    for file in ["a.env", "b.env", "taken-over.env"] {
        let command = format!("printf '%s|%s|%s|%s|' \"$HOST_VALUE\" \"${{STARTER_ONLY-unset}}\" \"${{OPENAI_API_KEY-unset}}\" \"$TOMCAT_AGENT_ACTIVE\" > {file}; fixture-host-command >> {file}");
        script.push(response(vec![
            sse_tool_call(file, "bash", &json!({"command":command}).to_string()),
            sse_finish("tool_calls"),
            sse_done(),
        ]));
        script.push(response(vec![
            sse_delta("done"),
            sse_finish("stop"),
            sse_done(),
        ]));
    }
    let server = spawn_scripted_openai_stream_server_with_auto_title(script);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.primitive.auto_confirm = true;
        cfg.workspace.workspace_roots = vec![fx.0.workspace.to_string_lossy().into_owned()];
    });
    let started = common::shared_serve::output_json(
        fx.command()
            .env("STARTER_ONLY", "starter-secret")
            .env("HOST_VALUE", "starter")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let start_host = |marker: &str| {
        let bin = fx.0.workspace.join(format!("bin-{marker}"));
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("fixture-host-command");
        std::fs::write(&executable, format!("#!/bin/sh\nprintf '{marker}'\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths([bin, "/usr/bin".into(), "/bin".into()]).unwrap();
        let mut command = fx.command();
        command
            .arg("acp")
            .env("HOST_VALUE", marker)
            .env("PATH", path)
            .env_remove("OPENAI_API_KEY");
        spawn_serve_command(command)
    };
    let mut a = start_host("A");
    let mut b = start_host("B");
    initialize(&mut a);
    initialize(&mut b);
    let sa = new_session(&mut a, &fx.0.workspace);
    let sb = new_session(&mut b, &fx.0.workspace);
    for (client, session, id) in [(&mut a, &sa, "a"), (&mut b, &sb, "b")] {
        prompt(client, id, session, "write the environment fixture");
        assert_eq!(
            ok(&client.recv_until(WAIT, |v| v["id"] == id))["stopReason"],
            "end_turn"
        );
    }
    assert_eq!(
        std::fs::read_to_string(fx.0.workspace.join("a.env")).unwrap(),
        "A|unset|unset|1|A"
    );
    assert_eq!(
        std::fs::read_to_string(fx.0.workspace.join("b.env")).unwrap(),
        "B|unset|unset|1|B"
    );
    close(a);
    ok(&request(
        &mut b,
        json!("takeover"),
        "session/load",
        json!({"sessionId":sa,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    prompt(
        &mut b,
        "continued",
        &sa,
        "write again using the new controller",
    );
    assert_eq!(
        ok(&b.recv_until(WAIT, |v| v["id"] == "continued"))["stopReason"],
        "end_turn"
    );
    assert_eq!(
        std::fs::read_to_string(fx.0.workspace.join("taken-over.env")).unwrap(),
        "B|unset|unset|1|B"
    );
    close(b);
    assert_eq!(fx.action("status")["instanceId"], started["instanceId"]);
}

#[test]
fn acp_proxy_environment_cannot_override_saved_proxy_even_after_session_load() {
    let reply = |message: &str| response(vec![sse_delta(message), sse_finish("stop"), sse_done()]);
    let starter = spawn_scripted_openai_stream_server_with_auto_title(vec![reply("wrong starter")]);
    let proxy = spawn_scripted_openai_stream_server_with_auto_title(vec![
        reply("configured first"),
        reply("configured second"),
        reply("configured after load"),
    ]);
    let fx = ServiceFixture::new("http://tomcat-proxy-fixture.invalid");
    fx.configure(|cfg| cfg.llm.proxy = Some(proxy.base_url.clone()));
    let started = common::shared_serve::output_json(
        fx.command()
            .env("HTTP_PROXY", &starter.base_url)
            .env("NO_PROXY", "*")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let spawn_with_proxy = |proxy: &str| {
        let mut command = fx.command();
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            command.env_remove(key);
        }
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            command.env(key, proxy);
        }
        // This would bypass even llm.proxy under the removed session policy.
        command.env("NO_PROXY", "*").arg("acp");
        spawn_serve_command(command)
    };
    let mut a = spawn_with_proxy("http://invalid-host-a:bad-port");
    let mut b = spawn_with_proxy("http://invalid-host-b:bad-port");
    initialize(&mut a);
    initialize(&mut b);
    let sa = new_session(&mut a, &fx.0.workspace);
    let sb = new_session(&mut b, &fx.0.workspace);
    for (client, session, id, expected) in [
        (&mut a, &sa, "proxy-first", "configured first"),
        (&mut b, &sb, "proxy-second", "configured second"),
    ] {
        prompt(client, id, session, id);
        let frames = client.recv_until(WAIT, |v| v["id"] == id);
        assert_eq!(ok(&frames)["stopReason"], "end_turn");
        assert_eq!(text(&frames, "agent_message_chunk"), expected);
    }
    close(a);
    ok(&request(
        &mut b,
        json!("load"),
        "session/load",
        json!({"sessionId":sa,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    prompt(&mut b, "continued-proxy", &sa, "proxy-after-load");
    let frames = b.recv_until(WAIT, |v| v["id"] == "continued-proxy");
    assert_eq!(ok(&frames)["stopReason"], "end_turn");
    assert_eq!(
        text(&frames, "agent_message_chunk"),
        "configured after load"
    );
    close(b);
    assert!(
        starter.captured_requests().is_empty(),
        "starter proxy must not receive requests"
    );
    let requests = proxy.captured_requests();
    for marker in ["proxy-first", "proxy-second", "proxy-after-load"] {
        assert!(
            requests.iter().any(|request| request
                .starts_with("POST http://tomcat-proxy-fixture.invalid/")
                && request.contains(marker)),
            "configured proxy did not receive {marker}"
        );
    }
    assert_eq!(fx.action("status")["instanceId"], started["instanceId"]);
}

#[cfg(unix)]
fn wait_mcp_record(path: &Path) -> Value {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return value;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "MCP did not write {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn private_mcp(fx: &ServiceFixture, label: &str, extra: &[&str]) -> Value {
    let node = std::process::Command::new("node")
        .args(["-p", "process.execPath"])
        .output()
        .unwrap();
    assert!(node.status.success());
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/fake_stdio_server.mjs");
    let mut args = vec![
        fixture.to_string_lossy().into_owned(),
        "--environment-record".into(),
        fx.0.workspace
            .join(format!("{label}.env.json"))
            .to_string_lossy()
            .into_owned(),
        "--events".into(),
        fx.0.workspace
            .join(format!("{label}.events"))
            .to_string_lossy()
            .into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    json!({"name":"private","command":String::from_utf8(node.stdout).unwrap().trim(),"args":args,
        "env":[{"name":"TASK_VALUE","value":label},{"name":"UNREAD_SECRET","value":"only-in-private-mcp-config"}]})
}

#[cfg(unix)]
#[test]
fn session_mcp_same_cwd_is_private_uses_controller_environment_and_stops_before_handoff() {
    let mut script = Vec::new();
    for label in ["A", "B", "C"] {
        script.push(response(vec![
            sse_tool_call(
                label,
                "tool_call",
                &json!({"name":"mcp__private__capture","arguments":{"label":label}}).to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]));
        script.push(response(vec![
            sse_delta("done"),
            sse_finish("stop"),
            sse_done(),
        ]));
    }
    let server = spawn_scripted_openai_stream_server_with_auto_title(script);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| cfg.primitive.auto_confirm = true);
    let global_config = fx.0.home_path.join(".tomcat/mcp.json");
    let saved_config = std::fs::read(&global_config).unwrap();
    let started = common::shared_serve::output_json(
        fx.command()
            .env("STARTER_ONLY", "starter-secret")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let start_host = |label: &str| {
        let paths = std::env::join_paths(
            std::iter::once(fx.0.workspace.join(format!("bin-{label}")))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = fx.command();
        command
            .arg("acp")
            .env("PATH", paths)
            .env("TASK_VALUE", "not-inherited")
            .env_remove("STARTER_ONLY");
        spawn_serve_command(command)
    };
    let mut a = start_host("A");
    let mut b = start_host("B");
    initialize(&mut a);
    initialize(&mut b);
    let mut sessions = Vec::new();
    let mut pids = Vec::new();
    for (client, label) in [(&mut a, "A"), (&mut b, "B")] {
        let frames = request(
            client,
            json!("new"),
            "session/new",
            json!({"cwd":fx.0.workspace,"mcpServers":[private_mcp(&fx, label, &[])]}),
        );
        let id = ok(&frames)["sessionId"].as_str().unwrap().to_owned();
        let record = wait_mcp_record(&fx.0.workspace.join(format!("{label}.env.json")));
        assert_eq!(record["task"], label);
        assert_eq!(record["starter"], Value::Null);
        assert!(record["path"]
            .as_str()
            .unwrap()
            .contains(&format!("bin-{label}")));
        assert_eq!(
            Path::new(record["cwd"].as_str().unwrap()),
            dunce::canonicalize(&fx.0.workspace).unwrap()
        );
        pids.push(record["pid"].as_i64().unwrap() as libc::pid_t);
        prompt(client, label, &id, "call the private MCP fixture");
        assert_eq!(
            ok(&client.recv_until(WAIT, |v| v["id"] == label))["stopReason"],
            "end_turn"
        );
        let events =
            std::fs::read_to_string(fx.0.workspace.join(format!("{label}.events"))).unwrap();
        assert!(events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "completed" && event["label"] == label
        }));
        sessions.push(id);
    }
    assert_ne!(pids[0], pids[1]);
    close(a);
    assert_ne!(
        unsafe { libc::kill(pids[0], 0) },
        0,
        "EOF must join private MCP shutdown"
    );
    assert_eq!(
        unsafe { libc::kill(pids[1], 0) },
        0,
        "other session's MCP must survive"
    );
    ok(&request(
        &mut b,
        json!("takeover"),
        "session/load",
        json!({"sessionId":sessions[0],"cwd":fx.0.workspace,"mcpServers":[private_mcp(&fx, "C", &[])]}),
    ));
    let record = wait_mcp_record(&fx.0.workspace.join("C.env.json"));
    assert_eq!(record["task"], "C");
    assert!(record["path"].as_str().unwrap().contains("bin-B"));
    let pid_c = record["pid"].as_i64().unwrap() as libc::pid_t;
    prompt(&mut b, "C", &sessions[0], "call after handoff");
    assert_eq!(
        ok(&b.recv_until(WAIT, |v| v["id"] == "C"))["stopReason"],
        "end_turn"
    );
    let events = std::fs::read_to_string(fx.0.workspace.join("C.events")).unwrap();
    assert!(events.contains("\"label\":\"C\""));
    assert!(!serde_json::to_string(&server.captured_requests())
        .unwrap()
        .contains("only-in-private-mcp-config"));
    assert_eq!(
        std::fs::read(&global_config).unwrap(),
        saved_config,
        "private setup must not change saved global config"
    );
    close(b);
    for pid in [pids[1], pid_c] {
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
    }
    assert_eq!(fx.action("status")["instanceId"], started["instanceId"]);
}

#[cfg(unix)]
#[test]
fn session_mcp_pending_startup_and_idle_replacement_are_reaped_without_stopping_service() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let mut client = spawn(&fx);
    initialize(&mut client);
    let frames = request(
        &mut client,
        json!("new"),
        "session/new",
        json!({"cwd":fx.0.workspace,"mcpServers":[private_mcp(&fx, "pending", &["--hang-startup"])]}),
    );
    let id = ok(&frames)["sessionId"].as_str().unwrap().to_owned();
    let first = wait_mcp_record(&fx.0.workspace.join("pending.env.json"))["pid"]
        .as_i64()
        .unwrap() as libc::pid_t;
    ok(&request(
        &mut client,
        json!("replace"),
        "session/load",
        json!({"sessionId":id,"cwd":fx.0.workspace,"mcpServers":[private_mcp(&fx, "replacement", &["--hang-startup"])]}),
    ));
    assert_ne!(
        unsafe { libc::kill(first, 0) },
        0,
        "load must wait for the retired startup process"
    );
    let second = wait_mcp_record(&fx.0.workspace.join("replacement.env.json"))["pid"]
        .as_i64()
        .unwrap() as libc::pid_t;
    // Even a never-ready MCP must be reaped before the forwarder acknowledges EOF.
    close(client);
    assert_ne!(unsafe { libc::kill(second, 0) }, 0);
    assert_eq!(fx.action("status")["running"], true);
}

#[cfg(unix)]
#[test]
fn session_mcp_cancel_joins_running_server_and_next_prompt_restarts_it() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let gate = fx.0.workspace.join("unreleased-gate");
    let script = std::sync::Mutex::new(
        vec![
            response(vec![
                sse_tool_call(
                    "held",
                    "tool_call",
                    &json!({"name":"mcp__private__capture",
            "arguments":{"label":"held","gateFile":gate,"progressMs":20}})
                    .to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![
                sse_tool_call(
                    "again",
                    "tool_call",
                    &json!({"name":"mcp__private__capture",
            "arguments":{"label":"again"}})
                    .to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![sse_delta("done"), sse_finish("stop"), sse_done()]),
        ]
        .into_iter(),
    );
    // Cancellation can also schedule a non-streaming thinking-title request.
    // Keep auxiliary requests separate from the three main-model responses.
    let server = common::serve::spawn_routed_openai_stream_server(move |request| {
        if request["stream"] == false {
            response(vec![common::serve::ScriptedPart {
                delay_ms: 0,
                body: json!({"choices":[{"message":{"role":"assistant",
                    "content":"Used private MCP"},"finish_reason":"stop"}]})
                .to_string(),
            }])
        } else {
            script
                .lock()
                .unwrap()
                .next()
                .expect("unexpected main-model request")
        }
    });
    // Point the saved fixture model at the local scripted provider before starting serve.
    let model_path = fx.0.home_path.join(".tomcat/models.toml");
    let models = std::fs::read_to_string(&model_path)
        .unwrap()
        .replace("http://127.0.0.1:1", &server.base_url);
    std::fs::write(model_path, models).unwrap();
    fx.configure(|cfg| cfg.primitive.auto_confirm = true);
    let mut client = spawn(&fx);
    initialize(&mut client);
    let frames = request(
        &mut client,
        json!("new"),
        "session/new",
        json!({"cwd":fx.0.workspace,"mcpServers":[private_mcp(&fx, "cancel", &[])]}),
    );
    let original_options = ok(&frames)["configOptions"].clone();
    let session = ok(&frames)["sessionId"].as_str().unwrap().to_owned();
    let record_path = fx.0.workspace.join("cancel.env.json");
    let pid = wait_mcp_record(&record_path)["pid"].as_i64().unwrap() as libc::pid_t;
    prompt(
        &mut client,
        "held",
        &session,
        "start the controlled MCP call",
    );
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let events =
            std::fs::read_to_string(fx.0.workspace.join("cancel.events")).unwrap_or_default();
        if events
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .any(|e| e["kind"] == "started" && e["label"] == "held")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "MCP call not started: {events}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for (id, value) in [
        ("agent_mode", "plan"),
        ("plan_review", "auto"),
        ("question_policy", "recommended"),
        ("plan_start", "auto"),
    ] {
        let rejected = request(
            &mut client,
            json!(format!("busy-{id}")),
            "session/set_config_option",
            json!({"sessionId":session,"configId":id,"value":value}),
        );
        assert!(
            rejected.last().unwrap()["error"]["message"]
                .as_str()
                .is_some_and(|message| message.to_ascii_lowercase().contains("busy")),
            "{id} must reject, not queue or cancel the running MCP call: {rejected:?}"
        );
    }
    client.send_value(
        &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    );
    assert_eq!(
        ok(&client.recv_until(WAIT, |v| v["id"] == "held"))["stopReason"],
        "cancelled"
    );
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "cancelled is not an early acknowledgement"
    );
    let unchanged = request(
        &mut client,
        json!("configuration-after-busy"),
        "session/set_config_option",
        json!({"sessionId":session,"configId":"model","value":original_options[0]["currentValue"]}),
    );
    assert_eq!(ok(&unchanged)["configOptions"], original_options);
    prompt(
        &mut client,
        "again",
        &session,
        "use the same private MCP again",
    );
    let resumed = client.recv_until(WAIT, |v| v["id"] == "again");
    assert_eq!(ok(&resumed)["stopReason"], "end_turn");
    let restarted = wait_mcp_record(&record_path)["pid"].as_i64().unwrap() as libc::pid_t;
    assert_ne!(pid, restarted);
    assert_eq!(unsafe { libc::kill(restarted, 0) }, 0);
    close(client);
    assert_ne!(unsafe { libc::kill(restarted, 0) }, 0);
}

#[cfg(feature = "test-streamable-http-server")]
#[tokio::test]
async fn session_mcp_streamable_http_uses_host_headers_without_exposing_them() {
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call(
                "http",
                "tool_call",
                &json!({"name":"mcp__web__echo",
            "arguments":{"message":"HTTP_OK"}})
                .to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![sse_delta("done"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| cfg.primitive.auto_confirm = true);
    let address_file = fx.0.workspace.join("http-address");
    let mut http = tokio::process::Command::new(env!("CARGO_BIN_EXE_test_streamable_http_server"))
        .args(["--port", "0"])
        .env("MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE", &address_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let address = tokio::time::timeout(WAIT, async {
        loop {
            assert!(http.try_wait().unwrap().is_none());
            if let Ok(raw) = std::fs::read_to_string(&address_file) {
                if let Ok(address) = raw.trim().parse::<std::net::SocketAddr>() {
                    break address;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut client = spawn(&fx);
    initialize(&mut client);
    let frames = request(
        &mut client,
        json!("new"),
        "session/new",
        json!({"cwd":fx.0.workspace,"mcpServers":[{"type":"http","name":"web",
            "url":format!("http://{address}/mcp"),"headers":[{"name":"Authorization","value":"Bearer fake-access-token"}]}]}),
    );
    let session = ok(&frames)["sessionId"].as_str().unwrap().to_owned();
    prompt(
        &mut client,
        "http",
        &session,
        "call the private HTTP source",
    );
    assert_eq!(
        ok(&client.recv_until(WAIT, |v| v["id"] == "http"))["stopReason"],
        "end_turn"
    );
    let captured = serde_json::to_string(&server.captured_requests()).unwrap();
    assert!(
        captured.contains("fake echo: HTTP_OK"),
        "HTTP MCP must actually execute: {captured}"
    );
    assert!(!captured.contains("fake-access-token"));
    ok(&request(
        &mut client,
        json!("clear"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    close(client);
    assert!(
        http.try_wait().unwrap().is_none(),
        "closing a transport does not kill its remote server"
    );
    http.kill().await.unwrap();
    http.wait().await.unwrap();
}

#[test]
fn protocol_errors_load_ownership_and_saved_history() {
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_delta("first reply"),
            sse_finish("stop"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("second reply"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut first = spawn(&fx);
    let premature = request(
        &mut first,
        json!(0),
        "session/new",
        json!({"cwd":fx.0.workspace, "mcpServers":[]}),
    );
    assert!(premature.last().unwrap()["error"].is_object());
    initialize(&mut first);
    let bad = request(
        &mut first,
        json!(2),
        "session/new",
        json!({"cwd":"relative", "mcpServers":[]}),
    );
    assert!(bad.last().unwrap()["error"].is_object());
    let unsupported = request(&mut first, json!(3), "new_session", json!({}));
    assert_eq!(unsupported.last().unwrap()["error"]["code"], -32601);
    let session = new_session(&mut first, &fx.0.workspace);
    prompt(&mut first, "turn", &session, "first input");
    let frames = first.recv_until(WAIT, |v| v["id"] == "turn");
    assert_eq!(ok(&frames)["stopReason"], "end_turn");
    assert_eq!(text(&frames, "agent_message_chunk"), "first reply");
    let mut next = spawn(&fx);
    initialize(&mut next);
    let load = json!({"sessionId":session, "cwd":fx.0.workspace, "mcpServers":[]});
    let occupied = request(&mut next, json!("occupied"), "session/load", load.clone());
    assert!(occupied.last().unwrap()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("session_in_use"));
    close(first);
    let loaded = request(&mut next, json!("loaded"), "session/load", load);
    ok(&loaded);
    assert!(text(&loaded, "user_message_chunk").contains("first input"));
    assert!(text(&loaded, "agent_message_chunk").contains("first reply"));
    prompt(&mut next, "next", &session, "continue");
    let frames = next.recv_until(WAIT, |v| v["id"] == "next");
    assert_eq!(ok(&frames)["stopReason"], "end_turn");
    assert_eq!(text(&frames, "agent_message_chunk"), "second reply");
    close(next);
}

#[test]
fn saved_false_uses_the_real_permission_gate_for_allow_deny_and_unknown_options() {
    for choice in [
        "allow_once",
        "allow_and_persist_root",
        "deny",
        "unoffered-option",
    ] {
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
            response(vec![
                sse_tool_call(
                    "write-one",
                    "write",
                    &json!({"path":"gated.txt", "content":"approved"}).to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![
                sse_delta("permission handled"),
                sse_finish("stop"),
                sse_done(),
            ]),
        ]);
        let fx = ServiceFixture::new(&server.base_url);
        fx.configure(|cfg| {
            cfg.primitive.auto_confirm = false;
            cfg.workspace.workspace_roots.clear();
        });
        fx.action("start");
        fx.action("restart");
        let mut client = spawn(&fx);
        initialize(&mut client);
        let session = new_session(&mut client, &fx.0.workspace);
        prompt(&mut client, "gated", &session, "write the file");
        let waiting = client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
        let permission = waiting.last().unwrap();
        assert_eq!(permission["params"]["sessionId"], session);
        assert!(
            !fx.0.workspace.join("gated.txt").exists(),
            "no side effect before approval"
        );
        client.send_value(&json!({"jsonrpc":"2.0", "id":permission["id"], "result":{"outcome":{"outcome":"selected","optionId":choice,"root":fx.0.home_path}}}));
        let done = client.recv_until(WAIT, |v| v["id"] == "gated");
        ok(&done);
        assert_eq!(
            fx.0.workspace.join("gated.txt").exists(),
            matches!(choice, "allow_once" | "allow_and_persist_root")
        );
        let cfg = tomcat::load_config_toml_file(&fx.0.home_path.join(".tomcat/tomcat.config.toml"))
            .unwrap();
        assert!(
            !cfg.primitive.auto_confirm,
            "ACP must not override a saved false"
        );
        if choice == "allow_and_persist_root" {
            assert_eq!(
                cfg.workspace.workspace_roots,
                vec![
                    permission["params"]["toolCall"]["rawInput"]["suggestedRoot"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                ]
            );
        } else {
            assert!(cfg.workspace.workspace_roots.is_empty());
        }
        close(client);
        let mut reloaded = spawn(&fx);
        initialize(&mut reloaded);
        let history = request(
            &mut reloaded,
            json!("history"),
            "session/load",
            json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
        );
        ok(&history);
        assert!(history
            .iter()
            .any(|v| v["params"]["update"]["sessionUpdate"] == "tool_call"
                && v["params"]["update"]["toolCallId"] == "write-one"));
        let expected_status = if matches!(choice, "allow_once" | "allow_and_persist_root") {
            "completed"
        } else {
            "failed"
        };
        assert!(
            history.iter().any(
                |v| v["params"]["update"]["sessionUpdate"] == "tool_call_update"
                    && v["params"]["update"]["toolCallId"] == "write-one"
                    && v["params"]["update"]["status"] == expected_status
            ),
            "{history:?}"
        );
        close(reloaded);
    }
}

#[test]
fn cancel_pending_permission_and_ignore_a_late_approval() {
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call(
                "pending",
                "write",
                &json!({"path":"never.txt","content":"forbidden"}).to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_tool_call(
                "later",
                "write",
                &json!({"path":"later.txt","content":"forbidden"}).to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![sse_delta("continued"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.primitive.auto_confirm = false;
        cfg.workspace.workspace_roots.clear();
    });
    let mut client = spawn(&fx);
    initialize(&mut client);
    let session = new_session(&mut client, &fx.0.workspace);
    prompt(&mut client, "pending", &session, "write");
    let frames = client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
    let permission_id = frames.last().unwrap()["id"].clone();
    client.send_value(
        &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    );
    let cancelled = client.recv_until(WAIT, |v| v["id"] == "pending");
    assert_eq!(ok(&cancelled)["stopReason"], "cancelled");
    prompt(&mut client, "next", &session, "try another file");
    let waiting = client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
    let current_permission = waiting.last().unwrap()["id"].clone();
    assert_ne!(current_permission, permission_id);
    client.send_value(&json!({"jsonrpc":"2.0","id":permission_id,"result":{"outcome":{"outcome":"selected","optionId":"allow_and_persist_root"}}}));
    client.send_value(&json!({"jsonrpc":"2.0","id":current_permission,"result":{"outcome":{"outcome":"selected","optionId":"deny"}}}));
    let next = client.recv_until(WAIT, |v| v["id"] == "next");
    assert_eq!(ok(&next)["stopReason"], "end_turn");
    assert!(!fx.0.workspace.join("never.txt").exists());
    assert!(!fx.0.workspace.join("later.txt").exists());
    let cfg =
        tomcat::load_config_toml_file(&fx.0.home_path.join(".tomcat/tomcat.config.toml")).unwrap();
    assert!(
        cfg.workspace.workspace_roots.is_empty(),
        "a late approval must not save a root either"
    );
    close(client);
}

#[test]
fn bash_command_approval_precedes_effects_and_respects_denial() {
    for choice in ["allow_once", "deny"] {
        let command = "printf accepted > command-marker.txt";
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
            response(vec![
                sse_tool_call(
                    "bash-gated",
                    "bash",
                    &json!({"command":command}).to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![sse_delta("done"), sse_finish("stop"), sse_done()]),
        ]);
        let fx = ServiceFixture::new(&server.base_url);
        fx.configure(|cfg| {
            cfg.primitive.auto_confirm = false;
            cfg.workspace.workspace_roots = vec![fx.0.workspace.to_string_lossy().into_owned()];
            cfg.primitive.bash_approval_required.push("^printf".into());
        });
        let mut client = spawn(&fx);
        initialize(&mut client);
        let session = new_session(&mut client, &fx.0.workspace);
        prompt(&mut client, "bash", &session, "run command");
        let waiting = client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
        let permission = waiting.last().unwrap();
        assert!(!fx.0.workspace.join("command-marker.txt").exists());
        assert!(permission.to_string().contains(command));
        client.send_value(&json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":choice}}}));
        ok(&client.recv_until(WAIT, |v| v["id"] == "bash"));
        assert_eq!(
            fx.0.workspace.join("command-marker.txt").exists(),
            choice == "allow_once"
        );
        close(client);
    }
}

#[test]
fn existing_root_grants_are_kept_and_auto_confirm_does_not_bypass_denies() {
    for denied in [false, true] {
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
            response(vec![
                sse_tool_call(
                    "write",
                    "write",
                    &json!({"path":"checked.txt","content":"allowed"}).to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![sse_delta("done"), sse_finish("stop"), sse_done()]),
        ]);
        let fx = ServiceFixture::new(&server.base_url);
        fx.configure(|cfg| {
            cfg.primitive.auto_confirm = denied;
            cfg.workspace.workspace_roots = vec![fx.0.workspace.to_string_lossy().into_owned()];
            if denied {
                cfg.primitive.path_rules.push(
                    serde_json::from_value(
                        json!({"path":fx.0.workspace.join("checked.txt"),"mode":"deny"}),
                    )
                    .unwrap(),
                );
            }
        });
        let mut client = spawn(&fx);
        initialize(&mut client);
        let session = new_session(&mut client, &fx.0.workspace);
        prompt(&mut client, "write", &session, "write");
        let frames = client.recv_until(WAIT, |v| v["id"] == "write");
        ok(&frames);
        assert!(frames
            .iter()
            .all(|v| v["method"] != "session/request_permission"));
        assert_eq!(fx.0.workspace.join("checked.txt").exists(), !denied);
        close(client);
    }
}

#[test]
fn immediate_cancel_cannot_be_overtaken_by_a_queued_prompt() {
    let mut delayed = sse_delta("too late");
    delayed.delay_ms = 500;
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            delayed,
            sse_finish("stop"),
            sse_done()
        ]);
        8
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut client = spawn(&fx);
    initialize(&mut client);
    let session = new_session(&mut client, &fx.0.workspace);
    for id in 10..14 {
        let prompt = json!({"jsonrpc":"2.0", "id":id, "method":"session/prompt", "params":{
            "sessionId":session, "prompt":[{"type":"text", "text":"cancel immediately"}]
        }});
        let cancel =
            json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}});
        client.send_raw(&format!("{prompt}\n{cancel}"));
        let done = client.recv_until(WAIT, |v| v["id"] == id);
        assert_eq!(ok(&done)["stopReason"], "cancelled", "{done:?}");
    }
    close(client);
}

#[test]
fn permission_wait_ends_on_disconnect_and_service_stop_without_writing() {
    for stop_service in [false, true] {
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
            sse_tool_call(
                "pending",
                "write",
                &json!({"path":"never.txt","content":"forbidden"}).to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ])]);
        let fx = ServiceFixture::new(&server.base_url);
        fx.configure(|cfg| {
            cfg.primitive.auto_confirm = false;
            cfg.workspace.workspace_roots.clear();
        });
        let mut client = spawn(&fx);
        initialize(&mut client);
        let session = new_session(&mut client, &fx.0.workspace);
        prompt(&mut client, "pending", &session, "write");
        client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
        if stop_service {
            assert_eq!(fx.action("stop")["stopped"], true);
            let output = client.wait_for_exit(WAIT);
            assert!(
                !output.status.success(),
                "unexpected service loss is not a confirmed graceful exit"
            );
            assert_eq!(fx.action("status")["running"], false);
        } else {
            close(client);
            assert_eq!(fx.action("status")["running"], true);
        }
        assert!(!fx.0.workspace.join("never.txt").exists());
    }
}

#[test]
fn model_failure_returns_an_rpc_error_not_success() {
    let failure = response(vec![common::serve::ScriptedPart {
        delay_ms: 0,
        body: "data: {\"error\":{\"message\":\"injected provider failure\",\"type\":\"invalid_request_error\",\"code\":\"invalid_api_key\"}}\n\n".into(),
    }]);
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![failure; 8]);
    let fx = ServiceFixture::new(&server.base_url);
    // This checks the ACP terminal error contract, not the default retry budget.
    // Keep the scripted server and WAIT bounded even when the default increases.
    fx.configure(|cfg| cfg.llm.agent_max_attempts = 1);
    let mut client = spawn(&fx);
    initialize(&mut client);
    let session = new_session(&mut client, &fx.0.workspace);
    prompt(&mut client, "failed", &session, "fail");
    let frames = client.recv_until(WAIT, |v| v["id"] == "failed");
    let reply = frames.last().unwrap();
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{reply}"
    );
    assert!(
        reply.get("result").is_none(),
        "failure must not be end_turn: {reply}"
    );
    close(client);
}

#[test]
fn stdin_eof_waits_for_actual_background_shell_exit() {
    background_turn_shutdown("eof");
}

#[test]
fn session_cancel_waits_for_background_shell_exit_before_terminal() {
    background_turn_shutdown("cancel");
}

#[test]
fn killed_forwarder_cancels_work_but_keeps_the_service_alive() {
    background_turn_shutdown("kill");
}

fn fixture_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal zero only queries the PID written by the owned fixture.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{
            ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        // SAFETY: request only synchronization rights for our known native Windows PID.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32),
                "{error}"
            );
            return false; // Gone, not permission-denied or another hidden failure.
        }
        // SAFETY: the fresh handle has one owner and stays live through the query.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => false,
            WAIT_TIMEOUT => true,
            other => panic!(
                "unexpected process wait result {other}: {}",
                std::io::Error::last_os_error()
            ),
        }
    }
}

fn background_writer_command(name: &str) -> String {
    // MSYS $$ is not a native Windows PID; Node reports the actual child PID.
    if cfg!(windows) {
        format!(
            r#"node -e 'const fs=require("node:fs");fs.writeFileSync("{name}.pid",String(process.pid));fs.writeFileSync("{name}.ticks","t");setInterval(()=>fs.appendFileSync("{name}.ticks","t"),50)'"#
        )
    } else {
        format!("echo $$ > {name}.pid; while :; do printf t >> {name}.ticks; sleep 0.05; done")
    }
}

fn background_turn_shutdown(how: &str) {
    let mut delayed = sse_delta("still working");
    delayed.delay_ms = 10_000;
    let command = background_writer_command("job");
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![
            sse_tool_call(
                "background",
                "bash",
                &json!({"command":command, "run_in_background":true}).to_string(),
            ),
            sse_finish("tool_calls"),
            sse_done(),
        ]),
        response(vec![
            sse_delta("working"),
            delayed,
            sse_finish("stop"),
            sse_done(),
        ]),
        response(vec![sse_delta("resumed"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let mut client = spawn(&fx);
    initialize(&mut client);
    let instance = fx.action("status")["instanceId"].clone();
    let session = new_session(&mut client, &fx.0.workspace);
    prompt(&mut client, "work", &session, "start work");
    client.recv_until(WAIT, |v| {
        v["params"]["update"]["content"]["text"] == "working"
    });
    let deadline = std::time::Instant::now() + WAIT;
    while !std::fs::metadata(fx.0.workspace.join("job.ticks")).is_ok_and(|meta| meta.len() > 0) {
        assert!(
            std::time::Instant::now() < deadline,
            "background subprocess did not start writing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: u32 = std::fs::read_to_string(fx.0.workspace.join("job.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        fixture_process_alive(pid),
        "fixture must actually be alive before cancellation"
    );
    match how {
        "cancel" => {
            client.send_value(&json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}}));
            let cancelled = client.recv_until(WAIT, |v| v["id"] == "work");
            assert_eq!(ok(&cancelled)["stopReason"], "cancelled");
            // Check before closing stdin; EOF must not mask a broken session/cancel.
            assert!(
                !fixture_process_alive(pid),
                "cancelled must mean the shell has stopped"
            );
            close(client);
        }
        "kill" => {
            client.kill_process(); // Abrupt OS termination without first closing stdin.
            assert!(!client.wait_for_exit(WAIT).status.success());
            let deadline = std::time::Instant::now() + WAIT;
            while fixture_process_alive(pid) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "orphaned shell after forwarder crash"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        "eof" => close(client),
        _ => unreachable!(),
    }
    assert!(
        !fixture_process_alive(pid),
        "forwarder exit must follow actual subprocess exit"
    );
    let before = std::fs::metadata(fx.0.workspace.join("job.ticks"))
        .unwrap()
        .len();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        std::fs::metadata(fx.0.workspace.join("job.ticks"))
            .unwrap()
            .len(),
        before
    );
    assert_eq!(fx.action("status")["instanceId"], instance);
    let mut next = spawn(&fx);
    initialize(&mut next);
    ok(&request(
        &mut next,
        json!("load"),
        "session/load",
        json!({"sessionId":session,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    prompt(&mut next, "resume", &session, "continue");
    let done = next.recv_until(WAIT, |v| v["id"] == "resume");
    assert_eq!(ok(&done)["stopReason"], "end_turn");
    close(next);
}
