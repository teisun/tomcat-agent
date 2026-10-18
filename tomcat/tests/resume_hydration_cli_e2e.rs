mod common;

use assert_cmd::Command;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tomcat::{
    init_context_state, load_config_toml_file, resolve_agent_trail_dir, resolve_sessions_dir,
    ContextConfig, SessionManager, TranscriptEntry,
};

#[allow(deprecated)]
fn cmd() -> Command {
    let mut c = Command::cargo_bin("tomcat").expect("binary tomcat should exist");
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c.env("TOMCAT__UI__LANGUAGE", "en");
    c
}

fn apply_idatatlas_env(command: &mut Command) {
    let model = common::idatatlas_test_model();
    command
        .env(common::IDATATLAS_TEST_API_KEY_ENV, "dummy-key")
        .env("TOMCAT__LLM__DEFAULT_MODEL", &model)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", &model);
}

struct Fixture {
    _home: tempfile::TempDir,
    home_path: PathBuf,
    workdir: PathBuf,
    session: SessionManager,
}

fn setup_fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();
    let workdir = home.path().join("workspace");
    std::fs::create_dir_all(&workdir).unwrap();

    cmd()
        .args(["init"])
        .env("HOME", &home_path)
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let config_path = home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).expect("config should load");
    cfg.storage.work_dir = Some(home_path.join(".tomcat").to_string_lossy().to_string());
    common::apply_idatatlas_app_config(&mut cfg);
    // This fixture asserts the primary hydrated request. Keep the title scene from
    // consuming the main-flow local mock-server responses first.
    cfg.llm.title_model = Some("test-title-disabled".to_string());
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize idatatlas test config"),
    )
    .expect("persist idatatlas test config");
    let sessions_dir = resolve_sessions_dir(&cfg).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &workdir);
    let session = SessionManager::new_scoped(sessions_dir, session_key.clone());
    session.create_session(&session_key, None).unwrap();
    session
        .update_session(&session_key, |entry| {
            entry.title = Some("Resume fixture".to_string());
        })
        .expect("seed fixed resume fixture title");
    let _ = resolve_agent_trail_dir(&cfg).unwrap();

    Fixture {
        _home: home,
        home_path,
        workdir,
        session,
    }
}

fn append_json_line(path: &std::path::Path, value: serde_json::Value) {
    tomcat::core::session::append_line(path, &value.to_string()).unwrap();
}

fn sidecar_path(transcript_path: &std::path::Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap();
    transcript_path.with_file_name(format!("{stem}.resume-index.json"))
}

fn write_mock_models_toml(home_path: &std::path::Path, base_url: &str) {
    let models_toml = home_path.join(".tomcat").join("models.toml");
    std::fs::write(
        &models_toml,
        format!(
            r#"[[models]]
id = "mock-local"
api = "openai"
provider = "openai"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"
capabilities = {{ vision = false, files = false, tools = true, reasoning = false }}
"#
        ),
    )
    .unwrap();
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
}

fn parse_content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    })
}

fn spawn_capturing_openai_stream_server(
    replies: Vec<serde_json::Value>,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock llm server");
    let addr = listener.local_addr().expect("local addr");
    listener.set_nonblocking(true).expect("nonblocking accept");
    let captured_body = Arc::new(Mutex::new(Vec::new()));
    let captured_body_clone = Arc::clone(&captured_body);
    let handle = std::thread::spawn(move || {
        for reply in replies {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "CLI did not send the expected model request"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept model request: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking request reads");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .expect("set read timeout");

            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            let mut header_end = None;
            let mut content_len = None;
            loop {
                let read = stream.read(&mut chunk).expect("read request bytes");
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..read]);
                if header_end.is_none() {
                    if let Some(end) = find_header_end(&raw) {
                        header_end = Some(end);
                        let headers = String::from_utf8_lossy(&raw[..end]);
                        content_len = parse_content_length(&headers);
                    }
                }
                if let (Some(end), Some(len)) = (header_end, content_len) {
                    if raw.len() >= end + len {
                        break;
                    }
                }
            }

            let body = if let (Some(end), Some(len)) = (header_end, content_len) {
                String::from_utf8_lossy(&raw[end..end + len]).to_string()
            } else {
                String::new()
            };
            captured_body_clone.lock().unwrap().push(body);

            let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
            let finish_reason = if reply.get("tool_calls").is_some() {
                "tool_calls"
            } else {
                "stop"
            };
            let first = format!(
                "data: {}\n\n",
                serde_json::json!({"choices": [{"delta": reply}]})
            );
            let finish = format!(
                "data: {}\n\n",
                serde_json::json!({"choices": [{"finish_reason": finish_reason}]})
            );
            stream.write_all(headers.as_bytes()).expect("write headers");
            stream.write_all(first.as_bytes()).expect("write delta");
            stream.write_all(finish.as_bytes()).expect("write finish");
            stream.flush().expect("flush response");
        }
    });
    (format!("http://{addr}"), captured_body, handle)
}

fn trace_field(stderr: &str, key: &str) -> Option<String> {
    stderr
        .lines()
        .find(|line| line.starts_with("TOMCAT_RESUME_TRACE "))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|part| part.strip_prefix(&format!("{key}=")).map(str::to_string))
        })
}

#[test]
fn resume_cli_cold_start_trace_is_bounded_with_sidecar() {
    common::setup_logging();
    let fx = setup_fixture();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
    for idx in 0..10_000usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("old-turn-{idx}") }
            }),
        );
    }
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_cli",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": 10000,
            "isBoundary": true,
        }),
    );
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("recent_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("recent-turn-{idx}") }
            }),
        );
    }
    let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();
    let file_len = std::fs::metadata(&transcript_path).unwrap().len();

    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("TOMCAT_RESUME_TRACE", "1")
        .write_stdin("");
    apply_idatatlas_env(&mut command);
    let output = command.output().expect("code --resume should run");
    assert!(
        output.status.success(),
        "code --resume should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TOMCAT_RESUME_TRACE mode=Tail"),
        "stderr should contain tail trace, got: {stderr}"
    );
    let bytes_scanned: u64 = trace_field(&stderr, "bytes_scanned")
        .expect("trace bytes_scanned")
        .parse()
        .unwrap();
    assert!(
        bytes_scanned < file_len / 4,
        "cold resume should scan bounded tail bytes: scanned={bytes_scanned} file_len={file_len}"
    );
}

#[test]
fn resume_cli_plan_fastpath_reports_sidecar_plan_source() {
    common::setup_logging();
    let fx = setup_fixture();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
    let plan_path = transcript_path.with_extension("plan.md");
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "custom",
            "id": "plan_evt_1",
            "timestamp": "2025-01-01T00:00:00.000Z",
            "event": tomcat::infra::wire::WIRE_PLAN_BUILD,
            "plan_id": "plan_cli",
            "path": plan_path.to_string_lossy(),
            "state": "executing",
        }),
    );
    for idx in 0..5_001usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("turn-{idx}") }
            }),
        );
    }
    let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();

    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("TOMCAT_RESUME_TRACE", "1")
        .write_stdin("");
    apply_idatatlas_env(&mut command);
    let output = command.output().expect("code --resume should run");
    assert!(
        output.status.success(),
        "code --resume should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plan_source=sidecar"),
        "plan fast path should come from sidecar, got: {stderr}"
    );
}

#[test]
fn resume_cli_corrupt_index_rebuilds_on_startup() {
    common::setup_logging();
    let fx = setup_fixture();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
    for idx in 0..3_000usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("turn-{idx}") }
            }),
        );
    }
    let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();
    let sidecar = sidecar_path(&transcript_path);
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    json["schema_version"] = serde_json::json!(999);
    std::fs::write(&sidecar, serde_json::to_vec_pretty(&json).unwrap()).unwrap();

    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("TOMCAT_RESUME_TRACE", "1")
        .write_stdin("");
    apply_idatatlas_env(&mut command);
    let output = command.output().expect("code --resume should run");
    assert!(
        output.status.success(),
        "code --resume should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fallback=rebuild"),
        "corrupt sidecar should rebuild, got: {stderr}"
    );
    let repaired: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    assert_eq!(
        repaired["schema_version"],
        serde_json::json!(tomcat::core::session::RESUME_INDEX_SCHEMA_VERSION)
    );
}

#[test]
fn resume_cli_heals_dangling_tool_call_tail_without_llm() {
    common::setup_logging();
    let fx = setup_fixture();
    fx.session
        .append_message(serde_json::json!({"role":"user","content":"继续"}))
        .unwrap();
    fx.session
        .append_message(serde_json::json!({
            "role":"assistant",
            "content":"call tool",
            "tool_calls":[
                {
                    "id":"call_1",
                    "type":"function",
                    "function":{"name":"read","arguments":"{}"}
                }
            ]
        }))
        .unwrap();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();

    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .write_stdin("");
    apply_idatatlas_env(&mut command);
    let output = command.output().expect("code --resume should run");
    assert!(
        output.status.success(),
        "code --resume should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let entries = tomcat::core::session::read_entries_tail(&transcript_path, 8).unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                    && me.message.get("tool_call_id").and_then(|v| v.as_str()) == Some("call_1")
                    && me.message.get("content").and_then(|v| v.as_str())
                        == Some(tomcat::core::session::UNKNOWN_RESTART_TOOL_RESULT_TEXT)
        )),
        "resume CLI must require verification for a dangling read: read-only does not prove it never ran"
    );
}

#[test]
fn resume_cli_large_session_restores_recent_context_in_request_body() {
    common::setup_logging();
    let fx = setup_fixture();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
    for idx in 0..1_100usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("u_old_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("old-turn-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("a_old_{idx}"),
                "timestamp": "2025-01-01T00:00:02.000Z",
                "message": { "role": "assistant", "content": format!("old-answer-{idx}") }
            }),
        );
    }
    append_json_line(
        &transcript_path,
        serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_cli_body",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": 2200,
            "isBoundary": true,
        }),
    );
    for idx in 0..12usize {
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("u_recent_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("recent-turn-{idx}") }
            }),
        );
        append_json_line(
            &transcript_path,
            serde_json::json!({
                "type": "message",
                "id": format!("a_recent_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "assistant", "content": format!("recent-answer-{idx}") }
            }),
        );
    }
    let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();

    let (base_url, captured_body, server_handle) = spawn_capturing_openai_stream_server(vec![
        serde_json::json!({"content": "RESUME_BODY_OK"}),
    ]);
    write_mock_models_toml(&fx.home_path, &base_url);

    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("OPENAI_API_KEY", "stub")
        .env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .write_stdin("continue\n");
    apply_idatatlas_env(&mut command);
    command.env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local");
    let output = command.output().expect("code --resume should run");
    assert!(
        output.status.success(),
        "code --resume should succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&tomcat::infra::tr("terminal.resumed", &[("key", "")])),
        "stdout should mention resume flow: {stdout}"
    );

    server_handle.join().unwrap();
    let request_body = captured_body
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("mock server should capture upstream request body");
    let request_json: serde_json::Value =
        serde_json::from_str(&request_body).expect("request body should be valid JSON");
    let messages_json = request_json["messages"].to_string();
    assert!(
        messages_json.contains("recent-turn-11") && messages_json.contains("recent-answer-11"),
        "upstream request should include recent hydrated context: {messages_json}"
    );
    assert!(
        !messages_json.contains("old-turn-0") && !messages_json.contains("old-answer-1099"),
        "upstream request should exclude boundary-pruned history: {messages_json}"
    );
}

#[test]
fn resume_cli_build_uses_persisted_binding_and_consumes_kickoff() {
    use tomcat::core::plan_runtime::{file_store, PlanRuntime};

    common::setup_logging();
    let fx = setup_fixture();
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &fx.workdir);
    let session_id = fx.session.current_session_id().unwrap().unwrap();
    let plan_path = fx.workdir.join("resume-bound.plan.md");
    let plan = file_store::PlanFile {
        frontmatter: serde_json::from_value(serde_json::json!({
            "plan_id": "resume-bound", "goal": "resume the bound plan",
            "state": "pending", "created_at": chrono::Utc::now().to_rfc3339(),
            "schema_version": file_store::PLAN_FILE_SCHEMA_VERSION,
            "todos": [{"id": "t1", "content": "complete resumed work", "status": "pending"}]
        }))
        .unwrap(),
        body: "## Plan\nComplete the resumed work.\n".into(),
    };
    file_store::write_plan(&plan_path, &plan, 2_000).unwrap();
    let shadow_path = fx.home_path.join(".tomcat/plans/resume-bound.plan.md");
    std::fs::create_dir_all(shadow_path.parent().unwrap()).unwrap();
    file_store::write_plan(&shadow_path, &plan, 2_000).unwrap();
    let shadow_before = std::fs::read(&shadow_path).unwrap();
    // The CLI child expands the persisted `~/...` against its HOME, so persist under the same
    // one: on Windows the temp dir sits inside the real profile and would become `~/AppData/...`.
    let real_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &fx.home_path);
    // Persist via real runtime events and the real session writer, not bind_for_test.
    {
        let runtime = PlanRuntime::new(session_key);
        let transcript = fx.session.clone();
        runtime.attach_transcript_appender(Arc::new(move |event| {
            transcript
                .append_custom_entry(event)
                .expect("persist runtime plan event");
            Ok(())
        }));
        runtime
            .build_plan(&plan_path.to_string_lossy(), Some(session_id.clone()))
            .unwrap();
        runtime.park_executing_plan().unwrap();
    }
    match real_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
    assert_eq!(
        file_store::read_plan(&plan_path).unwrap().frontmatter.state,
        file_store::PlanFileState::Pending
    );
    let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).unwrap();
    cfg.workspace
        .workspace_roots
        .push(fx.workdir.to_string_lossy().into_owned());
    std::fs::write(&config_path, toml::to_string_pretty(&cfg).unwrap()).unwrap();
    let (base_url, requests, server) = spawn_capturing_openai_stream_server(vec![
        serde_json::json!({"role": "assistant", "tool_calls": [{
            "index": 0, "id": "finish-resumed-plan", "type": "function",
            "function": {"name": "update_plan", "arguments": serde_json::json!({
                "ops": [{"kind": "set_status", "id": "t1", "status": "completed", "evidence": ["test:resumed-kickoff"]}]
            }).to_string()}
        }]}),
        serde_json::json!({"content": "RESUMED_PLAN_DONE"}),
    ]);
    write_mock_models_toml(&fx.home_path, &base_url);
    let output = cmd()
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("OPENAI_API_KEY", "stub")
        .env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local")
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", "mock-local")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .timeout(Duration::from_secs(40))
        .write_stdin("/plan build\n")
        .output()
        .expect("run resumed CLI");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        server.join().is_ok(),
        "CLI model requests={}, stdout={} stderr={}",
        requests.lock().unwrap().len(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "one native-tool roundtrip, no extra model turn"
    );
    let first: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
    assert!(
        first["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "user"
                && message["content"].as_str().is_some_and(|text| {
                    text.contains("start building")
                        && text.contains(plan_path.to_string_lossy().as_ref())
                })),
        "the real CLI loop must consume its build continuation, not merely return it"
    );
    assert_eq!(
        std::fs::read(&shadow_path).unwrap(),
        shadow_before,
        "resuming an external plan must not mutate the default-directory plan with the same ID"
    );
    let completed = file_store::read_plan(&plan_path).unwrap();
    assert_eq!(
        completed.frontmatter.state,
        file_store::PlanFileState::Completed
    );
    assert_eq!(
        completed.frontmatter.todos[0].status,
        file_store::TodoStatus::Completed
    );
    assert_eq!(
        completed.frontmatter.session_id.as_deref(),
        Some(session_id.as_str())
    );
    assert!(
        fx.session
            .get_entries(32)
            .unwrap()
            .iter()
            .any(|entry| matches!(entry,
        TranscriptEntry::Message(message) if message.message["role"] == "assistant"
            && message.message["content"] == "RESUMED_PLAN_DONE")),
        "the resumed session must persist the successful final model reply"
    );
}
