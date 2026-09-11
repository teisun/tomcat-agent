//! Opt-in, real-LLM acceptance tests for the UI verification capability.
//!
//! These intentionally exercise `tomcat serve --stdio`, never the interactive
//! chat CLI. They require a vision-capable OpenAI-compatible model and are
//! excluded from default CI (`scripts/test-groups.sh` does not list this
//! binary; each case is `#[ignore]` and also requires `TOMCAT_REAL_LLM_E2E=1`):
//! `TOMCAT_REAL_LLM_E2E=1 cargo test --test ui_acceptance_real_llm_e2e -- --ignored`

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use serial_test::serial;

use common::serve::{setup_serve_fixture, spawn_serve_child, ServeChild, ServeFixture};
use tomcat::{core::skill::materialize_builtin_skills, load_config_toml_file};

const TURN_TIMEOUT: Duration = Duration::from_secs(600);
const HEALTHY_FIXTURE: &str = include_str!("fixtures/verify_skill_ui/healthy.html");
const INTERACTIVE_FIXTURE: &str = include_str!("fixtures/verify_skill_ui/interactive.html");

struct UiFixtureServer {
    base_url: String,
    intermediate_token: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl UiFixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind UI fixture server");
        listener
            .set_nonblocking(true)
            .expect("set UI fixture server nonblocking");
        let address = listener.local_addr().expect("UI fixture address");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let intermediate_token = format!(
            "verify{:08x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
                & 0xffff_ffff
        );
        let interactive_fixture =
            INTERACTIVE_FIXTURE.replace("INTERMEDIATE_TOKEN", &intermediate_token);
        let thread = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => respond_fixture(stream, &interactive_fixture),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept UI fixture request: {error}"),
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            intermediate_token,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for UiFixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn respond_fixture(mut stream: TcpStream, interactive_fixture: &str) {
    stream
        .set_nonblocking(false)
        .expect("make accepted UI fixture socket blocking");
    let mut request = [0; 1024];
    let read = stream.read(&mut request).expect("read fixture request");
    let target = std::str::from_utf8(&request[..read])
        .expect("UTF-8 request")
        .split_whitespace()
        .nth(1)
        .unwrap_or("/");
    let body = match target {
        "/healthy.html" => HEALTHY_FIXTURE,
        "/interactive.html" => interactive_fixture,
        _ => "",
    };
    let status = if body.is_empty() {
        "404 Not Found"
    } else {
        "200 OK"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write fixture response");
}

struct RealLlmTarget {
    api_key: String,
    base_url: String,
    model_id: String,
    upstream_model: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaywrightMcpMode {
    Disabled,
    Headless,
    Headed,
}

/// Opt-in headed E2E screenshots are deliberately retained for local inspection.
fn headed_e2e_screenshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tomcat crate must be inside the repository")
        .join(".agents/shots")
}

fn real_llm_target() -> RealLlmTarget {
    assert_eq!(
        std::env::var("TOMCAT_REAL_LLM_E2E").as_deref(),
        Ok("1"),
        "set TOMCAT_REAL_LLM_E2E=1 to acknowledge real-model cost"
    );
    let user_env_path = dirs::home_dir()
        .expect("locate the current user's home directory")
        .join(".tomcat/assets/.env");
    let _ = dotenvy::from_path(&user_env_path);
    let key_env = std::env::var("TOMCAT_REAL_LLM_E2E_KEY_ENV")
        .unwrap_or_else(|_| "TOMCAT_REAL_LLM_E2E_API_KEY".to_string());
    RealLlmTarget {
        api_key: std::env::var(&key_env)
            .unwrap_or_else(|_| panic!("set {key_env}, or add it to {}", user_env_path.display())),
        base_url: std::env::var("TOMCAT_REAL_LLM_E2E_BASE_URL")
            .expect("set TOMCAT_REAL_LLM_E2E_BASE_URL to an OpenAI-compatible endpoint"),
        model_id: std::env::var("TOMCAT_REAL_LLM_E2E_MODEL")
            .expect("set TOMCAT_REAL_LLM_E2E_MODEL to a vision-capable tool model"),
        upstream_model: std::env::var("TOMCAT_REAL_LLM_E2E_UPSTREAM_MODEL")
            .expect("set TOMCAT_REAL_LLM_E2E_UPSTREAM_MODEL to the provider model name"),
    }
}

fn configure_real_vision_agent(
    fixture: &ServeFixture,
    target: &RealLlmTarget,
    playwright_mcp: PlaywrightMcpMode,
) {
    let config_path = fixture.home_path.join(".tomcat/tomcat.config.toml");
    let mut config = load_config_toml_file(&config_path).expect("load generated config");
    config.llm.default_model = target.model_id.clone();
    config.context.compaction_model = target.model_id.clone();
    config.llm.title_model = None;
    config.skills.enabled = true;
    config.connector.enabled = playwright_mcp != PlaywrightMcpMode::Disabled;
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&config).expect("serialize real LLM config"),
    )
    .expect("write real LLM config");
    std::fs::write(
        fixture.home_path.join(".tomcat/models.toml"),
        format!(
            r#"[[models]]
id = "{model_id}"
model_name = "{upstream_model}"
api = "openai-responses"
provider = "openai"
base_url = "{base_url}"
api_key_env = "TOMCAT_REAL_LLM_E2E_API_KEY"
capabilities = {{ vision = true, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            model_id = target.model_id,
            upstream_model = target.upstream_model,
            base_url = target.base_url,
        ),
    )
    .expect("write real model config");
    unsafe {
        std::env::set_var("TOMCAT_REAL_LLM_E2E_API_KEY", &target.api_key);
    }

    let skill_path = materialize_builtin_skills(&config).expect("materialize verify skill");
    let scripts_dir = skill_path
        .parent()
        .expect("verify skill dir")
        .join("scripts");
    let bootstrap = Command::new("node")
        .arg("bootstrap.mjs")
        .current_dir(&scripts_dir)
        .env(
            "PLAYWRIGHT_BROWSERS_PATH",
            fixture.home_path.join(".tomcat/cache/playwright"),
        )
        .output()
        .expect("run managed Playwright bootstrap");
    assert!(
        bootstrap.status.success(),
        "managed Playwright bootstrap failed:\n{}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    if playwright_mcp != PlaywrightMcpMode::Disabled {
        let mut args = vec!["-y".to_string(), "@playwright/mcp@0.0.79".to_string()];
        if playwright_mcp == PlaywrightMcpMode::Headless {
            args.push("--headless".to_string());
        }
        if playwright_mcp == PlaywrightMcpMode::Headed {
            let output_dir = headed_e2e_screenshot_dir();
            std::fs::create_dir_all(&output_dir)
                .expect("create headed screenshot output directory");
            args.push("--output-dir".to_string());
            args.push(output_dir.to_string_lossy().to_string());
        }
        std::fs::write(
            fixture.home_path.join(".tomcat/mcp.json"),
            json!({
                "mcpServers": {
                    "playwright": {
                        "command": "npx",
                        "args": args,
                        "env": {
                            "PLAYWRIGHT_BROWSERS_PATH":
                                fixture.home_path.join(".tomcat/cache/playwright"),
                        },
                        "startupTimeoutMs": 60_000,
                    }
                }
            })
            .to_string(),
        )
        .expect("write MCP config");
    }
}

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "real-ui-init",
        "subtype": "initialize",
        "payload": {}
    }));
    child
        .recv_until(TURN_TIMEOUT, |frame| {
            frame.get("type").and_then(Value::as_str) == Some("control_response")
                && frame.get("requestId").and_then(Value::as_str) == Some("real-ui-init")
        })
        .last()
        .expect("initialize frame")["payload"]["sessionId"]
        .as_str()
        .expect("session ID")
        .to_string()
}

fn run_prompt(child: &mut ServeChild, session_id: &str, id: &str, prompt: String) -> Vec<Value> {
    child.send_value(&json!({
        "type": "prompt",
        "id": id,
        "sessionId": session_id,
        "text": prompt,
        "params": {}
    }));
    child.recv_until(TURN_TIMEOUT, |frame| {
        frame.get("type").and_then(Value::as_str) == Some("agent_idle")
            && frame.get("sessionId").and_then(Value::as_str) == Some(session_id)
    })
}

fn rendered_agent_text(frames: &[Value]) -> String {
    frames
        .iter()
        .filter_map(|frame| {
            frame
                .get("assistantMessageEvent")
                .filter(|event| event.get("kind").and_then(Value::as_str) == Some("content_delta"))
                .and_then(|event| event.get("delta"))
                .and_then(Value::as_str)
        })
        .collect::<String>()
}

fn frames_contain(frames: &[Value], expected: &str) -> bool {
    frames
        .iter()
        .any(|frame| frame.to_string().contains(expected))
}

fn tool_names(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter(|frame| frame.get("type").and_then(Value::as_str) == Some("tool_execution_start"))
        .filter_map(|frame| frame.get("toolName").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn assert_deferred_playwright_navigate_click_screenshot(frames: &[Value]) {
    let names = tool_names(frames);
    let joined = names.join(", ");
    assert!(
        names.iter().any(|name| name == "tool_search"),
        "agent must discover the deferred connector catalog; tools={joined}"
    );
    assert!(
        names.iter().any(|name| name == "tool_describe"),
        "agent must fetch deferred schemas; tools={joined}"
    );
    assert!(
        names.iter().any(|name| name == "tool_call"),
        "agent must invoke deferred MCP tools; tools={joined}"
    );
    let connector_calls = frames
        .iter()
        .filter(|frame| {
            frame.get("type").and_then(Value::as_str) == Some("tool_execution_start")
                && frame.get("toolName").and_then(Value::as_str) == Some("tool_call")
        })
        .map(Value::to_string)
        .collect::<Vec<_>>();
    let calls = connector_calls.join("\n");
    let navigate_index = connector_calls
        .iter()
        .position(|frame| frame.contains("browser_navigate"))
        .unwrap_or_else(|| panic!("agent must call browser_navigate via tool_call; calls={calls}"));
    let click_index = connector_calls
        .iter()
        .enumerate()
        .skip(navigate_index + 1)
        .find_map(|(index, frame)| frame.contains("browser_click").then_some(index))
        .unwrap_or_else(|| {
            panic!("agent must call browser_click after browser_navigate; calls={calls}")
        });
    let post_click_snapshot_index = connector_calls
        .iter()
        .enumerate()
        .skip(click_index + 1)
        .find_map(|(index, frame)| {
            (frame.contains("browser_snapshot") || frame.contains("browser_get_visible_text"))
                .then_some(index)
        })
        .unwrap_or_else(|| {
            panic!(
                "agent must obtain textual evidence after browser_click before reporting an opaque token; calls={calls}"
            )
        });
    let screenshot_index = connector_calls
        .iter()
        .enumerate()
        .skip(post_click_snapshot_index + 1)
        .find_map(|(index, frame)| frame.contains("browser_take_screenshot").then_some(index))
        .unwrap_or_else(|| {
            panic!("agent must call browser_take_screenshot after textual post-click evidence; calls={calls}")
        });

    eprintln!(
        "[real-llm-e2e] confirmed deferred Playwright MCP sequence: {}",
        connector_calls[navigate_index..=screenshot_index].join(" -> ")
    );
}

fn screenshot_events(frames: &[Value]) -> Vec<Value> {
    frames
        .iter()
        .filter(|frame| {
            frame
                .get("toolName")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains("screenshot"))
        })
        .cloned()
        .collect()
}

fn wait_for_playwright_ready(child: &mut ServeChild) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut attempt = 0;
    let mut last_payload = String::from("<no list_connectors response>");
    while std::time::Instant::now() < deadline {
        attempt += 1;
        let id = format!("playwright-status-{attempt}");
        child.send_value(&json!({ "type": "list_connectors", "id": id }));
        let frames = child.recv_until(Duration::from_secs(15), |frame| {
            frame.get("id").and_then(Value::as_str) == Some(id.as_str())
        });
        let response = frames
            .iter()
            .find(|frame| frame.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .expect("list_connectors response");
        last_payload = response["payload"].to_string();
        let ready = response["payload"]["connectors"]
            .as_array()
            .is_some_and(|connectors| {
                connectors.iter().any(|connector| {
                    connector["name"].as_str() == Some("playwright")
                        && connector["state"].as_str() == Some("connected")
                })
            });
        if ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!(
        "Playwright MCP did not become ready within 120 seconds; last list_connectors={last_payload}; serve stderr={}",
        child.stderr()
    );
}

fn any_shot_exists(fixture: &ServeFixture) -> bool {
    [
        fixture.home_path.join(".tomcat/shots"),
        fixture.workspace.join(".agents/shots"),
    ]
    .iter()
    .filter_map(|directory| std::fs::read_dir(directory).ok())
    .flat_map(|entries| entries.flatten())
    .any(|entry| {
        entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("png")
    })
}

fn session_contains_input_image(child: &mut ServeChild, session_id: &str, id: &str) -> bool {
    child.send_value(&json!({
        "type": "get_messages",
        "id": id,
        "sessionId": session_id,
        "params": { "limit": 128 }
    }));
    child
        .recv_until(Duration::from_secs(15), |frame| {
            frame.get("id").and_then(Value::as_str) == Some(id)
        })
        .iter()
        .find(|frame| frame.get("id").and_then(Value::as_str) == Some(id))
        .is_some_and(|response| {
            response["payload"]["messages"]
                .to_string()
                .contains("input_image")
        })
}

fn session_cache_read_tokens(child: &mut ServeChild, session_id: &str, id: &str) -> Vec<u64> {
    child.send_value(&json!({
        "type": "get_messages",
        "id": id,
        "sessionId": session_id,
        "params": { "limit": 128 }
    }));
    let frames = child.recv_until(Duration::from_secs(15), |frame| {
        frame.get("id").and_then(Value::as_str) == Some(id)
    });
    frames
        .iter()
        .find(|frame| frame.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|response| response["payload"]["messages"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["message"]["usage"]["cache_read_tokens"].as_u64())
        .collect()
}

#[test]
#[ignore = "requires a real vision LLM, API key, Node, and browser bootstrap"]
#[serial]
fn e2e_1_real_llm_completes_phase_1_ui_acceptance() {
    let target = real_llm_target();
    let fixture = setup_serve_fixture(&target.base_url);
    configure_real_vision_agent(&fixture, &target, PlaywrightMcpMode::Disabled);
    let page = UiFixtureServer::start();
    let mut child = spawn_serve_child(&fixture);
    let session_id = initialize(&mut child);
    let work_dir = fixture.home_path.join(".tomcat");
    let frames = run_prompt(
        &mut child,
        &session_id,
        "phase1",
        format!(
            "Use the verify skill to accept the UI at {}/healthy.html. Run the managed shot script from {}/skills/verify/scripts, write artifacts to {}/shots, read its PNG, ARIA and console evidence, then answer exactly whether the visible Save changes button is present and there are no errors. Do not claim success without tool evidence.",
            page.base_url,
            work_dir.display(),
            work_dir.display(),
        ),
    );
    let text = rendered_agent_text(&frames);
    assert!(
        frames_contain(&frames, "load_skill"),
        "agent must load verify: {frames:?}"
    );
    assert!(
        frames_contain(&frames, "shot.mjs"),
        "agent must run shot: {frames:?}"
    );
    assert!(
        frames_contain(&frames, "read"),
        "agent must read artifacts: {frames:?}"
    );
    assert!(text.contains("Save changes"), "agent verdict: {text}");
    assert!(any_shot_exists(&fixture), "PNG evidence missing");
}

#[test]
#[ignore = "requires a real vision LLM, API key, Node, and Playwright MCP"]
#[serial]
fn e2e_2_real_llm_uses_phase_2_mcp_and_receives_a_screenshot() {
    let target = real_llm_target();
    let fixture = setup_serve_fixture(&target.base_url);
    configure_real_vision_agent(&fixture, &target, PlaywrightMcpMode::Headless);
    let page = UiFixtureServer::start();
    let mut child = spawn_serve_child(&fixture);
    let session_id = initialize(&mut child);
    let cold_start = run_prompt(
        &mut child,
        &session_id,
        "mcp-cold-start",
        "Immediately call tool_search with no parameters exactly once, then reply with \
         COLD_START_CHECKED. Do not invoke a connector tool or wait for a connector to become \
         ready. This is a cold-start discovery probe."
            .to_string(),
    );
    assert!(
        tool_names(&cold_start)
            .iter()
            .any(|name| name == "tool_search"),
        "cold-start prompt must exercise the stable discovery entry; frames={cold_start:?}"
    );
    wait_for_playwright_ready(&mut child);
    let frames = run_prompt(
        &mut child,
        &session_id,
        "phase2",
        format!(
            "Load the verify skill and the connectors skill. Discover the configured Playwright connector through tool_search, fetch the needed schemas through tool_describe, then invoke it only through tool_call to open {}/interactive.html and click Show details. After the click, take a fresh browser_snapshot or browser_get_visible_text and copy the exact opaque token from that textual evidence; do not OCR or infer it from the screenshot. Then take a screenshot. The token is intentionally not in this instruction: you must use MCP tools and both textual and screenshot evidence; do not guess. For the screenshot call, omit filename and omit fullPage=true so its PNG returns to you as an MCP image.",
            page.base_url,
        ),
    );
    let text = rendered_agent_text(&frames);
    assert!(
        frames_contain(&frames, "mcp__playwright__"),
        "MCP tools were not used: {frames:?}"
    );
    assert_deferred_playwright_navigate_click_screenshot(&frames);
    assert!(
        session_contains_input_image(&mut child, &session_id, "phase2-messages"),
        "MCP screenshot did not persist as an InputImage follow-up message; tools={:?}; screenshot_events={:?}; stderr={}",
        tool_names(&frames),
        screenshot_events(&frames),
        child.stderr(),
    );
    assert!(
        text.contains(&page.intermediate_token),
        "agent verdict: {text}"
    );
    let cache_reads = session_cache_read_tokens(&mut child, &session_id, "headed-mcp-cache");
    eprintln!("[real-llm-e2e] cache_read_tokens across turns: {cache_reads:?}");
    assert!(
        cache_reads.iter().any(|tokens| *tokens > 0),
        "the cold-start + deferred MCP turn must reuse a prompt-cache prefix; cache_read_tokens={cache_reads:?}; stderr={}",
        child.stderr()
    );
    assert!(
        cache_reads.windows(2).any(|window| window[1] > window[0]),
        "at least one later model round must read a larger cached prefix as the deferred \
         workflow grows; cache_read_tokens={cache_reads:?}; stderr={}",
        child.stderr()
    );
}

#[test]
#[ignore = "requires a real vision LLM, API key, Node, browser bootstrap, and Playwright MCP"]
#[serial]
fn e2e_3_real_llm_switches_from_phase_1_to_phase_2_when_interaction_is_required() {
    let target = real_llm_target();
    let fixture = setup_serve_fixture(&target.base_url);
    configure_real_vision_agent(&fixture, &target, PlaywrightMcpMode::Headless);
    let page = UiFixtureServer::start();
    let mut child = spawn_serve_child(&fixture);
    let session_id = initialize(&mut child);
    let _warmup = run_prompt(
        &mut child,
        &session_id,
        "combined-warmup",
        "Reply with READY. Do not use any tools.".to_string(),
    );
    wait_for_playwright_ready(&mut child);
    let work_dir = fixture.home_path.join(".tomcat");
    let frames = run_prompt(
        &mut child,
        &session_id,
        "combined",
        format!(
            "First use the verify skill's deterministic managed shot workflow on {}/interactive.html and read PNG/ARIA/console evidence from {}/shots. The final answer is an opaque token which is intentionally absent from this instruction and appears only after clicking Show details. You must then load the connectors skill and use tool_search → tool_describe → tool_call to drive the configured Playwright persistent browser session: navigate, click Show details, then take a fresh browser_snapshot or browser_get_visible_text and copy the exact token from textual evidence before taking a screenshot. For the screenshot call, omit filename and omit fullPage=true so its PNG returns to you as an MCP image. Do not skip either phase, OCR, or guess.",
            page.base_url,
            work_dir.display(),
        ),
    );
    let text = rendered_agent_text(&frames);
    assert!(
        frames_contain(&frames, "shot.mjs"),
        "Phase 1 was skipped: {frames:?}"
    );
    assert!(
        frames_contain(&frames, "mcp__playwright__"),
        "Phase 2 was skipped: {frames:?}"
    );
    assert_deferred_playwright_navigate_click_screenshot(&frames);
    assert!(
        session_contains_input_image(&mut child, &session_id, "combined-messages"),
        "Phase 2 screenshot did not persist as an InputImage follow-up message; tools={:?}; screenshot_events={:?}; stderr={}",
        tool_names(&frames),
        screenshot_events(&frames),
        child.stderr(),
    );
    assert!(
        text.contains(&page.intermediate_token),
        "agent verdict: {text}"
    );
}

#[test]
#[ignore = "requires a real vision LLM, API key, Node, and a visible headed Chrome browser"]
#[serial]
fn e2e_4_real_llm_drives_a_headed_playwright_browser_through_deferred_tools() {
    let target = real_llm_target();
    let fixture = setup_serve_fixture(&target.base_url);
    configure_real_vision_agent(&fixture, &target, PlaywrightMcpMode::Headed);
    let mcp_config = std::fs::read_to_string(fixture.home_path.join(".tomcat/mcp.json"))
        .expect("read headed MCP config");
    assert!(
        !mcp_config.contains("--headless"),
        "headed E2E must omit --headless: {mcp_config}"
    );
    let screenshot_path = headed_e2e_screenshot_dir().join("headed-interaction.png");
    assert!(
        mcp_config.contains(headed_e2e_screenshot_dir().to_string_lossy().as_ref()),
        "headed E2E must configure the retained screenshot directory: {mcp_config}"
    );

    let page = UiFixtureServer::start();
    let mut child = spawn_serve_child(&fixture);
    let session_id = initialize(&mut child);
    let _warmup = run_prompt(
        &mut child,
        &session_id,
        "headed-mcp-warmup",
        "Reply with READY. Do not use any tools.".to_string(),
    );
    wait_for_playwright_ready(&mut child);
    let frames = run_prompt(
        &mut child,
        &session_id,
        "headed-mcp",
        format!(
            "Load the connectors skill, then use only the deferred connector workflow (tool_search → tool_describe → tool_call). Do not load verify, run bash, or use any managed shot script. In the visible browser window, use the configured Playwright source to open {}/interactive.html, click Show details, then obtain a fresh browser_snapshot or browser_get_visible_text and copy the exact opaque token from that textual evidence. Finally take a screenshot with filename {} and without fullPage=true. Do not OCR or guess the token.",
            page.base_url,
            screenshot_path.display(),
        ),
    );
    let text = rendered_agent_text(&frames);
    assert!(
        frames_contain(&frames, "load_skill")
            && !frames_contain(&frames, "shot.mjs")
            && !tool_names(&frames).iter().any(|name| name == "bash"),
        "headed deferred-MCP E2E must use connectors but not the verification workflow: {frames:?}"
    );
    assert_deferred_playwright_navigate_click_screenshot(&frames);
    assert!(
        screenshot_path.is_file(),
        "headed MCP screenshot was not saved to {}; tools={:?}; screenshot_events={:?}; stderr={}",
        screenshot_path.display(),
        tool_names(&frames),
        screenshot_events(&frames),
        child.stderr(),
    );
    let screenshot = std::fs::read(&screenshot_path).expect("read headed screenshot");
    assert!(
        screenshot.starts_with(b"\x89PNG\r\n\x1a\n"),
        "headed screenshot is not a PNG: {}",
        screenshot_path.display(),
    );
    assert!(
        text.contains(&page.intermediate_token),
        "agent verdict: {text}"
    );
}
