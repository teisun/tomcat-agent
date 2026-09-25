//! Real Serve + routed LLM + controlled HTTP MCP. No manager futures substitute
//! for the session wire, tool gateway, result events or interruption path here.
use super::{initialize, SERVE_TIMEOUT};
use crate::common::serve::{
    response, setup_serve_fixture, spawn_routed_openai_stream_server, spawn_serve_child, sse_delta,
    sse_done, sse_finish, sse_tool_call, ServeChild,
};
use serde_json::{json, Value};
use std::{net::SocketAddr, process::Stdio, time::Duration};
use tokio::{
    process::Command,
    time::{sleep, timeout},
};

struct McpFixture {
    child: tokio::process::Child,
    http: reqwest::Client,
    url: String,
    _temp: tempfile::TempDir,
}
impl McpFixture {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let address_file = temp.path().join("address");
        let mut child = Command::new(env!("CARGO_BIN_EXE_test_streamable_http_server"))
            .args(["--port", "0", "--faults"])
            .env("MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE", &address_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let address = timeout(Duration::from_secs(30), async {
            loop {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "MCP fixture exited during startup"
                );
                if let Ok(raw) = tokio::fs::read_to_string(&address_file).await {
                    if let Ok(addr) = raw.trim().parse::<SocketAddr>() {
                        if addr.ip().is_loopback() && addr.port() != 0 {
                            break addr;
                        }
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture bound address");
        Self {
            child,
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            url: format!("http://{address}"),
            _temp: temp,
        }
    }
    async fn state(&self) -> Value {
        self.http
            .get(format!("{}/__test/state", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn control(&self, value: Value) {
        self.http
            .post(format!("{}/__test/control", self.url))
            .json(&value)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    async fn until(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        timeout(Duration::from_secs(8), async {
            loop {
                let value = self.state().await;
                if predicate(&value) {
                    return value;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("server-side MCP barrier was not reached"))
    }
    async fn stop(mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}
fn calls(state: &Value) -> &Vec<Value> {
    state["calls"].as_array().expect("fixture calls")
}
fn is_event(frame: &Value, kind: &str, session: &str) -> bool {
    frame["type"] == kind && frame["sessionId"] == session
}
fn prompt(child: &mut ServeChild, session: &str, label: &str) {
    child.send_value(&json!({"type":"prompt", "id":format!("prompt-{label}"), "sessionId":session, "text":format!("MCP-E2E:{label}"), "params":{}}));
}

async fn exercise(
    shape: &'static str,
    different_tools: bool,
    close: bool,
    fill_default_quota: bool,
) {
    let mcp = McpFixture::start().await;
    let llm = spawn_routed_openai_stream_server(move |request| {
        let messages = request["messages"].as_array().expect("OpenAI messages");
        let user = messages
            .iter()
            .rposition(|message| {
                message["role"] == "user" && message["content"].to_string().contains("MCP-E2E:")
            })
            .expect("session-specific user sentinel");
        let text = messages[user]["content"].to_string();
        let label = ["held", "fast", "survivor"]
            .into_iter()
            .find(|label| text.contains(&format!("MCP-E2E:{label}")))
            .unwrap();
        if messages[user + 1..]
            .iter()
            .any(|message| message["role"] == "tool")
        {
            return response(vec![
                sse_delta(&format!("finished-{label}")),
                sse_finish("stop"),
                sse_done(),
            ]);
        }
        let tool = if different_tools && label != "held" {
            "mcp__fixture__echo_other"
        } else {
            "mcp__fixture__echo"
        };
        let (gateway, arguments) = if fill_default_quota && label == "held" {
            (
                "tool_run_code",
                json!({"code":format!("return await Promise.all(Array.from({{length:15}}, (_, i) => callTool('mcp__fixture__echo', {{label:'held-'+i, gate:'held', response:'{shape}'}}))); ")}),
            )
        } else {
            let mut arguments = json!({"label":label, "response":shape});
            if label != "fast" {
                arguments["gate"] = json!(label);
            }
            ("tool_call", json!({"name":tool, "arguments":arguments}))
        };
        response(vec![
            sse_tool_call("same-external-id", gateway, &arguments.to_string()),
            sse_finish("tool_calls"),
            sse_done(),
        ])
    });
    let fx = setup_serve_fixture(&llm.base_url);
    let cfg_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let mut cfg = tomcat::load_config_toml_file(&cfg_path).unwrap();
    cfg.connector.enabled = true;
    cfg.primitive.auto_confirm = true;
    std::fs::write(&cfg_path, toml::to_string_pretty(&cfg).unwrap()).unwrap();
    let model_path = fx.home_path.join(".tomcat/models.toml");
    let models = std::fs::read_to_string(&model_path)
        .unwrap()
        .replace("http://127.0.0.1:1", &llm.base_url);
    std::fs::write(model_path, models).unwrap();
    let source = json!({"url":format!("{}/mcp", mcp.url), "auth":"none"});
    assert!(source.get("maxConcurrentCalls").is_none());
    assert_eq!(cfg.connector.mcp.max_concurrent_calls, 16);
    std::fs::create_dir_all(fx.workspace.join(".agents")).unwrap();
    std::fs::write(
        fx.workspace.join(".agents/mcp.json"),
        json!({"mcpServers":{"fixture":source}}).to_string(),
    )
    .unwrap();

    let mut child = spawn_serve_child(&fx);
    let initial = initialize(&mut child);
    let mut sessions = Vec::new();
    for id in ["new-A", "new-B"] {
        child.send_value(&json!({"type":"new_session", "id":id, "params":{"cwd":fx.workspace}}));
        let created = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == id);
        sessions.push(
            created.last().unwrap()["payload"]["sessionId"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let a = sessions[0].clone();
    let b = sessions[1].clone();
    child.send_value(&json!({"type":"close_session", "id":"close-initial", "sessionId":initial}));
    child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "close-initial");
    assert_ne!(a, b);
    // Use the real Workspace approval boundary, not a config-level trust bypass.
    // The default rootless session must not donate its global-only registry to A/B.
    child.send_value(&json!({"type":"list_connectors", "id":"configured", "context":{"workspaceRoot":fx.workspace}}));
    let configured = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "configured");
    let root = configured.last().unwrap()["payload"]["project"]["root"].clone();
    child.send_value(&json!({"type":"trust_project", "id":"warm-connector", "projectRoot":root}));
    let receipt = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "warm-connector");
    assert_eq!(receipt.last().unwrap()["success"], true, "{receipt:?}");
    let ready_deadline = std::time::Instant::now() + SERVE_TIMEOUT;
    loop {
        child.send_value(&json!({"type":"list_connectors", "id":"ready", "context":{"workspaceRoot":fx.workspace}}));
        let ready = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "ready");
        if ready.last().unwrap()["payload"]["connectors"]
            .as_array()
            .is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["name"] == "fixture" && row["state"] == "connected")
            })
        {
            break;
        }
        assert!(
            std::time::Instant::now() < ready_deadline,
            "connector never ready: {ready:?}"
        );
        sleep(Duration::from_millis(20)).await;
    }
    let held_count = if fill_default_quota { 15 } else { 1 };
    prompt(&mut child, &a, "held");
    mcp.until(|state| calls(state).len() == held_count).await;
    prompt(&mut child, &b, "fast");
    let overlapping = mcp
        .until(|state| {
            calls(state)
                .iter()
                .any(|call| call["label"] == "fast" && call["completed"] == true)
        })
        .await;
    assert_eq!(
        overlapping["initialized"],
        1,
        "one shared connector, not one per session; fixture={overlapping}; stderr={}",
        child.stderr()
    );
    assert!(calls(&overlapping)
        .iter()
        .filter(|call| call["label"] != "fast")
        .all(|call| call["completed"] == false && call["activeResponse"] == true));
    assert_eq!(calls(&overlapping).len(), held_count + 1);
    assert!(calls(&overlapping)
        .iter()
        .all(|call| call["session"] == calls(&overlapping)[0]["session"]));
    let ids: std::collections::HashSet<_> = calls(&overlapping)
        .iter()
        .map(|call| call["id"].to_string())
        .collect();
    assert_eq!(
        ids.len(),
        held_count + 1,
        "MCP routes by request id, never outer call id or return order"
    );
    let mut frames = child.recv_until(SERVE_TIMEOUT, |frame| is_event(frame, "agent_idle", &b));
    assert!(
        !frames
            .iter()
            .any(|frame| is_event(frame, "tool_execution_end", &a)),
        "A must still be blocked when B completes"
    );
    for session in [&a, &b] {
        assert!(
            frames
                .iter()
                .any(|frame| is_event(frame, "tool_execution_start", session)
                    && frame["toolCallId"] == "same-external-id"),
            "actual tool gateway start: {frames:?}"
        );
    }
    prompt(&mut child, &b, "survivor");
    mcp.until(|state| calls(state).iter().any(|call| call["label"] == "survivor"))
        .await;
    child.send_value(&json!({"type":if close {"close_session"} else {"interrupt"}, "id":"stop-held", "sessionId":a}));
    let stopped = child.recv_until(SERVE_TIMEOUT, |frame| frame["id"] == "stop-held");
    assert_eq!(stopped.last().unwrap()["success"], true);
    frames.extend(stopped);
    mcp.control(json!({"release":"survivor"})).await;
    frames.extend(child.recv_until(SERVE_TIMEOUT, |frame| is_event(frame, "agent_idle", &b)));
    for label in ["fast", "survivor"] {
        let result = format!("fixture result: {label}");
        assert!(
            frames
                .iter()
                .any(|frame| is_event(frame, "tool_execution_end", &b)
                    && frame["result"].to_string().contains(&result)),
            "missing B result {label}: {frames:?}"
        );
        assert!(
            !frames
                .iter()
                .any(|frame| is_event(frame, "tool_execution_end", &a)
                    && frame["result"].to_string().contains(&result)),
            "B's answer leaked into A"
        );
    }
    let done = mcp
        .until(|state| {
            calls(state)
                .iter()
                .all(|call| call["activeResponse"] == false)
        })
        .await;
    assert_eq!(
        done["initialized"], 1,
        "stopping A must not rebuild B's connection"
    );
    assert_eq!(calls(&done).len(), held_count + 2, "no implicit replay");
    assert!(calls(&done)
        .iter()
        .filter(|call| call["label"] == "fast" || call["label"] == "survivor")
        .all(|call| call["completed"] == true && call["cancellations"] == 0));
    let output = child.wait_for_exit(Duration::from_secs(8));
    assert!(
        output.status.success(),
        "Serve shutdown: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    mcp.stop().await;
    drop(llm); // joins listener and all request workers; no detached test threads
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn serve_mcp_same_tool_headers_overlap_and_interrupt_isolated() {
    exercise("headers", false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn serve_mcp_different_tools_sse_overlap_and_close_isolated() {
    exercise("sse", true, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn serve_mcp_default_sixteen_slots_span_code_and_another_session() {
    exercise("json", true, false, true).await;
}
