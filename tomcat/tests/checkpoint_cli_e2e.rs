mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tomcat::core::session::read_entries_tail;
use tomcat::{
    load_config_toml_file, resolve_agent_trail_dir, resolve_sessions_dir, CheckpointKind,
    CheckpointRecordRequest, CheckpointStore, SessionManager, ShadowGitStore, TranscriptEntry,
};
use tracing::{info, info_span};

#[allow(deprecated)]
fn cmd() -> Command {
    let mut c = Command::cargo_bin("tomcat").expect("binary tomcat should exist");
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

struct Fixture {
    _home: tempfile::TempDir,
    home_path: PathBuf,
    workdir: PathBuf,
    session: SessionManager,
    session_id: String,
    store: ShadowGitStore,
}

fn setup_fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();
    let workdir = home.path().join("workspace");
    fs::create_dir_all(&workdir).unwrap();

    cmd()
        .args(["init"])
        .env("HOME", &home_path)
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let config_path = home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).expect("config should load");
    common::apply_deepseek_app_config(&mut cfg);
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize deepseek test config"),
    )
    .expect("persist deepseek test config");
    cfg.storage.work_dir = Some(home_path.join(".tomcat").to_string_lossy().to_string());
    let sessions_dir = resolve_sessions_dir(&cfg).unwrap();
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &workdir);
    let session = SessionManager::new_scoped(sessions_dir, session_key.clone());
    let session_id = session
        .create_session(&session_key, None)
        .unwrap()
        .session_id;
    let store = ShadowGitStore::new(resolve_agent_trail_dir(&cfg).unwrap(), workdir.clone());

    Fixture {
        _home: home,
        home_path,
        workdir,
        session,
        session_id,
        store,
    }
}

fn write_session_plugin_fixture(workspace: &Path, plugin_id: &str, activation: &str) {
    let plugin_dir = workspace.join(".tomcat").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "events": ["session_start"],
        "activation": activation
    });
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize plugin manifest"),
    )
    .expect("write plugin manifest");
    fs::write(
        plugin_dir.join("main.js"),
        r#"
pi.on("session_start", function () {});
__pi_start_event_loop();
"#,
    )
    .expect("write plugin main");
}

fn record_checkpoint(
    store: &dyn CheckpointStore,
    session_id: &str,
    turn_id: &str,
    kind: CheckpointKind,
    message_anchor: Option<String>,
) -> String {
    store
        .record(CheckpointRecordRequest {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            kind,
            message_anchor,
            notes: None,
        })
        .unwrap()
        .to_string()
}

#[cfg(unix)]
fn spawn_slow_openai_stream_server() -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stage = Arc::new(AtomicUsize::new(0));
    let stage_clone = Arc::clone(&stage);
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            stage_clone.store(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
            let first = "data: {\"choices\":[{\"delta\":{\"content\":\"partial from mock\"}}]}\n\n";
            let finish = "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n";
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(first.as_bytes());
            let _ = stream.flush();
            stage_clone.store(2, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(5));
            let _ = stream.write_all(finish.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{}", addr), stage, handle)
}

#[cfg(unix)]
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(unix)]
fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut expected_total_len: Option<usize> = None;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if expected_total_len.is_none() {
                    if let Some(header_end) = find_header_end(&buf) {
                        let headers = String::from_utf8_lossy(&buf[..header_end]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                if name.eq_ignore_ascii_case("content-length") {
                                    value.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        expected_total_len = Some(header_end + 4 + content_length);
                    }
                }
                if let Some(total_len) = expected_total_len {
                    if buf.len() >= total_len {
                        break;
                    }
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(unix)]
fn extract_last_task_id_from_request(request: &str) -> Option<String> {
    let (_, body) = request.split_once("\r\n\r\n")?;
    let payload: serde_json::Value = serde_json::from_str(body).ok()?;
    let messages = payload.get("messages")?.as_array()?;
    for message in messages.iter().rev() {
        if message.get("role").and_then(|v| v.as_str()) != Some("tool") {
            continue;
        }
        let content = message.get("content").and_then(|v| v.as_str())?;
        let tool_payload: serde_json::Value = serde_json::from_str(content).ok()?;
        if let Some(task_id) = tool_payload.get("taskId").and_then(|v| v.as_str()) {
            return Some(task_id.to_string());
        }
    }
    None
}

#[cfg(unix)]
fn write_sse_chunk(stream: &mut std::net::TcpStream, chunk: serde_json::Value) {
    let headers =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
    let body = format!("data: {}\n\n", chunk);
    let _ = stream.write_all(headers.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

#[cfg(unix)]
fn spawn_tool_then_text_openai_stream_server(
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let stage = Arc::new(AtomicUsize::new(0));
    let stage_clone = Arc::clone(&stage);
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut emitted_background_tool = false;
        let mut emitted_wait_tool = false;
        while stage_clone.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let request = read_http_request(&mut stream);
                    if !emitted_background_tool {
                        let tool_turn = serde_json::json!({
                            "choices": [{
                                "delta": {
                                    "tool_calls": [{
                                        "index": 0,
                                        "id": "call_bg",
                                        "function": {
                                            "name": "bash",
                                            "arguments": "{\"command\":\"sleep 5\",\"run_in_background\":true}"
                                        }
                                    }]
                                },
                                "finish_reason": "tool_calls"
                            }]
                        });
                        write_sse_chunk(&mut stream, tool_turn);
                        emitted_background_tool = true;
                        stage_clone.store(1, Ordering::SeqCst);
                        continue;
                    }

                    if !emitted_wait_tool {
                        if let Some(task_id) = extract_last_task_id_from_request(&request) {
                            let tool_turn = serde_json::json!({
                                "choices": [{
                                    "delta": {
                                        "tool_calls": [{
                                            "index": 0,
                                            "id": "call_wait",
                                            "function": {
                                                "name": "task_output",
                                                "arguments": format!(
                                                    "{{\"task_id\":\"{}\",\"since\":0,\"block\":true,\"timeout_ms\":30000}}",
                                                    task_id
                                                )
                                            }
                                        }]
                                    },
                                    "finish_reason": "tool_calls"
                                }]
                            });
                            write_sse_chunk(&mut stream, tool_turn);
                            emitted_wait_tool = true;
                            stage_clone.store(2, Ordering::SeqCst);
                        } else {
                            let tool_turn = serde_json::json!({
                                "choices": [{
                                    "delta": {
                                        "tool_calls": [{
                                            "index": 0,
                                            "id": "call_bg",
                                            "function": {
                                                "name": "bash",
                                                "arguments": "{\"command\":\"sleep 5\",\"run_in_background\":true}"
                                            }
                                        }]
                                    },
                                    "finish_reason": "tool_calls"
                                }]
                            });
                            write_sse_chunk(&mut stream, tool_turn);
                        }
                        continue;
                    }

                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
                    let reply =
                        "data: {\"choices\":[{\"delta\":{\"content\":\"RECOVERED_E2E\"}}]}\n\n";
                    let finish = "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n";
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(reply.as_bytes());
                    let _ = stream.write_all(finish.as_bytes());
                    let _ = stream.flush();
                    stage_clone.store(3, Ordering::SeqCst);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://{}", addr), stage, handle)
}

#[cfg(unix)]
fn wait_for_stage(stage: &Arc<AtomicUsize>, target: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if stage.load(Ordering::SeqCst) >= target {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        stage.load(Ordering::SeqCst) >= target,
        "server stage did not reach {target} within {:?}",
        timeout
    );
}

#[cfg(unix)]
fn wait_for_child_output(
    mut child: std::process::Child,
    timeout: Duration,
) -> std::process::Output {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(_status) = child.try_wait().unwrap() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("child did not exit within {:?}", timeout);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn test_resume_after_interrupt() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_resume_after_interrupt").entered();
    let fx = setup_fixture();

    let m1 = fx
        .session
        .append_message(json!({"role":"user","content":"hello"}))
        .unwrap();
    let m2 = fx
        .session
        .append_message(json!({"role":"assistant","content":"partial reply"}))
        .unwrap();
    fs::write(fx.workdir.join("note.txt"), "interrupt-good").unwrap();
    let checkpoint_id = record_checkpoint(
        &fx.store,
        &fx.session_id,
        &format!("{m1}::{m2}"),
        CheckpointKind::Interrupt,
        Some(m2),
    );

    info!("Act: tomcat chat --resume + local /ckpt list");
    let assert = cmd()
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin("/ckpt list\n")
        .assert();

    assert
        .success()
        .stdout(predicate::str::contains("恢复会话"))
        .stdout(predicate::str::contains(&checkpoint_id));
}

#[test]
fn test_slash_restore_recovers_after_bad_edit() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_slash_restore_recovers_after_bad_edit").entered();
    let fx = setup_fixture();

    let _m1 = fx
        .session
        .append_message(json!({"role":"user","content":"q1"}))
        .unwrap();
    let m2 = fx
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();
    fs::write(fx.workdir.join("note.txt"), "good").unwrap();
    let checkpoint_id = record_checkpoint(
        &fx.store,
        &fx.session_id,
        "turn-1",
        CheckpointKind::TurnEnd,
        Some(m2.clone()),
    );

    let m3 = fx
        .session
        .append_message(json!({"role":"user","content":"q2"}))
        .unwrap();
    let m4 = fx
        .session
        .append_message(json!({"role":"assistant","content":"a2"}))
        .unwrap();
    fs::write(fx.workdir.join("note.txt"), "bad").unwrap();

    info!("Act: tomcat chat + local /restore");
    let assert = cmd()
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin(format!("/restore {checkpoint_id}\n"))
        .assert();

    assert
        .success()
        .stdout(predicate::str::contains("已恢复 checkpoint"));
    assert_eq!(
        fs::read_to_string(fx.workdir.join("note.txt")).unwrap(),
        "good"
    );

    let transcript_path = fx
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    let entries = read_entries_tail(&transcript_path, 16).unwrap();
    let superseded_ids: Vec<String> = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(me)
                if me
                    .message
                    .get("superseded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false) =>
            {
                me.id.clone()
            }
            _ => None,
        })
        .collect();
    assert!(superseded_ids.contains(&m3));
    assert!(superseded_ids.contains(&m4));
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Custom(custom)
                if custom.extra.get("customType").and_then(|v| v.as_str()) == Some("checkpoint.restore")
        )),
        "应追加 checkpoint.restore custom 条目"
    );
}

#[test]
fn test_pre_rollback_only_before_turn_end_restore() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_pre_rollback_only_before_turn_end_restore").entered();
    let fx = setup_fixture();

    let assistant_id = fx
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();
    fs::write(fx.workdir.join("note.txt"), "good").unwrap();
    let turn_end_ckpt = record_checkpoint(
        &fx.store,
        &fx.session_id,
        "turn-end-1",
        CheckpointKind::TurnEnd,
        Some(assistant_id),
    );

    fs::write(fx.workdir.join("note.txt"), "broken-after-turn-end").unwrap();
    cmd()
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin(format!("/restore {turn_end_ckpt}\n"))
        .assert()
        .success();

    let after_turn_end = fx
        .store
        .list(&fx.session_id, Default::default())
        .expect("checkpoint list after turn-end restore");
    let pre_rollback_count_after_turn_end = after_turn_end
        .iter()
        .filter(|meta| {
            matches!(
                &meta.kind,
                CheckpointKind::Manual { label } if label.starts_with("pre-rollback to")
            )
        })
        .count();
    assert_eq!(
        pre_rollback_count_after_turn_end, 1,
        "TurnEnd restore 前应额外记录一次 pre-rollback 手动 checkpoint"
    );

    fs::write(fx.workdir.join("note.txt"), "manual-good").unwrap();
    let manual_ckpt = record_checkpoint(
        &fx.store,
        &fx.session_id,
        "manual-1",
        CheckpointKind::Manual {
            label: "manual save".to_string(),
        },
        None,
    );
    fs::write(fx.workdir.join("note.txt"), "manual-bad").unwrap();
    cmd()
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin(format!("/restore {manual_ckpt}\n"))
        .assert()
        .success();

    let after_manual = fx
        .store
        .list(&fx.session_id, Default::default())
        .expect("checkpoint list after manual restore");
    let pre_rollback_count_after_manual = after_manual
        .iter()
        .filter(|meta| {
            matches!(
                &meta.kind,
                CheckpointKind::Manual { label } if label.starts_with("pre-rollback to")
            )
        })
        .count();
    assert_eq!(
        pre_rollback_count_after_manual, pre_rollback_count_after_turn_end,
        "Manual restore 不应新增 pre-rollback checkpoint"
    );
}

#[test]
fn test_idle_readline_eof_exits_without_interrupt_ckpt() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_idle_readline_eof_exits_without_interrupt_ckpt").entered();
    let fx = setup_fixture();

    cmd()
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin("")
        .assert()
        .success()
        .stdout(predicate::str::contains("再见！"));

    let checkpoints = fx
        .store
        .list(&fx.session_id, Default::default())
        .expect("checkpoint list after idle EOF");
    assert!(
        checkpoints.is_empty(),
        "阻塞在 readline 时 Ctrl+D / EOF 应直接退出，不写 Interrupt checkpoint"
    );
}

#[test]
fn test_idle_readline_eof_with_loaded_lazy_plugin_avoids_cleanup_warning() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_idle_readline_eof_with_loaded_lazy_plugin_avoids_cleanup_warning")
        .entered();
    let fx = setup_fixture();
    let config_path = fx.home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).expect("config should load");
    cfg.plugin.auto_load = vec![
        "session-eof-plugin".to_string(),
        "lazy-eof-plugin".to_string(),
    ];
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize config with plugin auto-load"),
    )
    .expect("persist config with plugin auto-load");
    write_session_plugin_fixture(&fx.workdir, "session-eof-plugin", "session");
    write_session_plugin_fixture(&fx.workdir, "lazy-eof-plugin", "lazy");

    cmd()
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .write_stdin("")
        .assert()
        .success()
        .stdout(predicate::str::contains("再见！"))
        .stderr(predicate::str::contains("[cleanup_instance] no event_sender").not());
}

#[cfg(unix)]
#[test]
/// [E2E-CLI-076] 运行中挂断后，partial transcript 与 Interrupt checkpoint 都要保留下来。
fn test_hangup_during_run_leaves_interrupt_ckpt() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_hangup_during_run_leaves_interrupt_ckpt").entered();
    let fx = setup_fixture();
    let (base_url, stage, handle) = spawn_slow_openai_stream_server();

    // T2-P0-010 后路由改为 model-first：catalog 命中的 model 用 entry.base_url，
    // 旧的 `[llm].api_base` 覆盖对内置 model 不再生效（见 llm-multi-llm-productization §4.2.0
    // 与 resolver::tests::catalog_route_ignores_legacy_api_base_override）。因此把 mock
    // endpoint 通过 `models.toml` 自定义 model 声明出来，再用 default_model 选中它。
    let models_toml = fx.home_path.join(".tomcat").join("models.toml");
    fs::write(
        &models_toml,
        format!(
            r#"[[models]]
id = "mock-local"
api = "openai"
provider = "openai"
api_key_env = "{api_key_env}"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = false }}
"#,
            api_key_env = common::DEEPSEEK_TEST_API_KEY_ENV,
        ),
    )
    .unwrap();

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin!("tomcat"))
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("chat child should start");

    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin.write_all(b"say hi\n").unwrap();
    stdin.flush().unwrap();

    wait_for_stage(&stage, 2, Duration::from_secs(3));
    // stage=2 仅表示 mock server 已把首段 delta 写到 socket；给客户端一点时间消费，
    // 避免 SIGHUP 抢在 content_buf 吃到 partial 之前，导致用例退化成“建连后立刻中断”。
    std::thread::sleep(Duration::from_millis(250));
    // 运行中 SIGHUP 等价软中断；随后关闭 stdin 让进程在回到 prompt 后自然退出。
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    drop(stdin);

    let output = wait_for_child_output(child, Duration::from_secs(10));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "运行中挂断后子进程应在 soft interrupt + EOF 下正常退出，stderr={stderr}"
    );
    assert!(
        stderr.contains("^C 已中断（partial 已保存）"),
        "挂断应走 Interrupted 持久化路径，stderr={stderr}"
    );

    let transcript_path = fx
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    let entries = read_entries_tail(&transcript_path, 16).unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("assistant")
                    && me
                        .message
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .contains("partial from mock")
        )),
        "挂断后 partial assistant 应已落盘到 transcript"
    );

    let checkpoints = fx
        .store
        .list(&fx.session_id, Default::default())
        .expect("checkpoint list after hangup");
    assert!(
        checkpoints
            .iter()
            .any(|meta| matches!(meta.kind, CheckpointKind::Interrupt)),
        "运行中挂断后应留下 Interrupt checkpoint"
    );

    handle.join().unwrap();
}

#[cfg(unix)]
#[test]
/// [E2E-CLI-077] 工具执行中挂断后，同一 chat 子进程继续输入应自动恢复而不是 append_message_chain 退出。
fn test_hangup_during_tool_run_allows_same_process_followup() {
    if !git_available() {
        return;
    }
    common::setup_logging();
    let _span = info_span!("test_hangup_during_tool_run_allows_same_process_followup").entered();
    let fx = setup_fixture();
    let (base_url, stage, handle) = spawn_tool_then_text_openai_stream_server();

    let models_toml = fx.home_path.join(".tomcat").join("models.toml");
    fs::write(
        &models_toml,
        format!(
            r#"[[models]]
id = "mock-local"
api = "openai"
provider = "openai"
api_key_env = "{api_key_env}"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = false }}
"#,
            api_key_env = common::DEEPSEEK_TEST_API_KEY_ENV,
        ),
    )
    .unwrap();

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin!("tomcat"))
        .current_dir(&fx.workdir)
        .arg("code")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("chat child should start");

    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin.write_all(b"run slow tool\n").unwrap();
    stdin.flush().unwrap();

    wait_for_stage(&stage, 2, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(250));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    std::thread::sleep(Duration::from_millis(250));
    stdin.write_all(b"continue after interrupt\n").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let output = wait_for_child_output(child, Duration::from_secs(30));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "同一子进程应在软中断后继续处理下一条输入并正常退出，stderr={stderr}"
    );
    assert!(
        !stderr.contains("append_message_chain"),
        "第二轮不应因 append_message_chain 退出，stderr={stderr}"
    );
    assert!(
        stage.load(Ordering::SeqCst) >= 3,
        "第二轮请求应命中 mock server，actual stage={}",
        stage.load(Ordering::SeqCst)
    );
    assert!(
        stdout.contains("RECOVERED_E2E"),
        "第二轮应继续完成回复，stdout={stdout}"
    );

    let transcript_path = fx
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    let entries = read_entries_tail(&transcript_path, 32).unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                    && me.message.get("tool_call_id").and_then(|v| v.as_str()) == Some("call_wait")
                    && me.message.get("content").and_then(|v| v.as_str()) == Some("[interrupted]")
        )),
        "第一轮被打断的工具调用应补 `[interrupted]` 到 transcript"
    );

    let interrupted_idx = entries
        .iter()
        .position(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("tool")
                    && me.message.get("tool_call_id").and_then(|v| v.as_str()) == Some("call_wait")
                    && me.message.get("content").and_then(|v| v.as_str()) == Some("[interrupted]")
        ))
        .expect("should find interrupted tool result");
    let user_idx = entries
        .iter()
        .position(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("user")
                    && me.message.get("content").and_then(|v| v.as_str()) == Some("continue after interrupt")
        ))
        .expect("should find second user input");
    let assistant_idx = entries
        .iter()
        .position(|entry| matches!(
            entry,
            TranscriptEntry::Message(me)
                if me.message.get("role").and_then(|v| v.as_str()) == Some("assistant")
                    && me.message.get("content").and_then(|v| v.as_str()) == Some("RECOVERED_E2E")
        ))
        .expect("should find recovered assistant reply");
    assert!(
        interrupted_idx < user_idx && user_idx < assistant_idx,
        "第二轮输入与回复应位于 `[interrupted]` 之后；entries={entries:?}"
    );

    handle.join().unwrap();
}
