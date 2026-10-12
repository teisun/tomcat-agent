mod common;
use common::serve::*;
use serde_json::{json, Value};
use serial_test::serial;
use std::{fs, time::Duration};
const WAIT: Duration = Duration::from_secs(30);

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(
        &json!({"type":"control_request","requestId":"init","subtype":"initialize","payload":{}}),
    );
    let frames = child.recv_until(WAIT, |v| {
        v["type"] == "control_response" && v["requestId"] == "init"
    });
    frames.last().unwrap()["payload"]["sessionId"]
        .as_str()
        .unwrap()
        .into()
}
fn call(name: &str, args: Value, id: &str) -> ScriptedResponse {
    response(vec![
        sse_tool_call(id, name, &args.to_string()),
        sse_finish("tool_calls"),
        sse_done(),
    ])
}
fn answer(text: &str) -> ScriptedResponse {
    response(vec![sse_delta(text), sse_finish("stop"), sse_done()])
}

#[test]
#[serial]
fn svg_history_reference_round_trips_through_rewind_with_typed_rendition() {
    use base64::Engine as _;
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        answer("first"),
        answer("second"),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let models_path = fx.home_path.join(".tomcat/models.toml");
    let models = fs::read_to_string(&models_path).unwrap();
    fs::write(
        &models_path,
        models.replace(
            "vision = false, files = false, tools = true",
            "vision = true, files = false, tools = true",
        ),
    )
    .unwrap();
    let mut child = spawn_serve_child(&fx);
    let session = initialize(&mut child);
    let svg = base64::engine::general_purpose::STANDARD.encode(b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><rect width=\"1\" height=\"1\"/></svg>");
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    child.send_value(&json!({"type":"ingest_attachment","id":"svg","sessionId":session,"attachment":{
        "kind":"image","mimeType":"image/svg+xml","filename":"icon.svg","dataBase64":svg,"providerBase64":png,"providerMimeType":"image/png"
    }}));
    let ingest = child.recv_until(WAIT, |v| v["id"] == "svg").pop().unwrap();
    assert_eq!(ingest["success"], true, "{ingest:?}");
    let attachment = json!({"kind":"image","blobSha":ingest["payload"]["blobSha"],"providerSha":ingest["payload"]["providerSha"],"filename":"icon.svg","mimeType":"image/svg+xml"});
    child.send_value(&json!({"type":"prompt","id":"first","sessionId":session,"text":"inspect icon","params":{"userMessageId":"svg-user","attachments":[attachment]}}));
    child.recv_until(WAIT, |v| v["type"] == "agent_idle");
    child.send_value(&json!({"type":"get_messages","id":"history","sessionId":session,"params":{"attachmentMode":"reference","limit":100}}));
    let history = child
        .recv_until(WAIT, |v| v["id"] == "history")
        .pop()
        .unwrap();
    let user = history["payload"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "svg-user")
        .unwrap();
    let part = user["message"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["type"] == "input_image_ref")
        .unwrap();
    assert_eq!(part["providerSha"], ingest["payload"]["providerSha"]);
    assert!(part.get("provider_sha").is_none());
    let returned_attachment = json!({"kind":"image","blobSha":part["blobSha"],"providerSha":part["providerSha"],"filename":part["filename"],"mimeType":part["mime_type"]});
    child.send_value(&json!({"type":"rewind_and_resend","id":"edit","sessionId":session,"messageId":"svg-user","files":"keep","message":{"text":"inspect edited icon","attachments":[returned_attachment]}}));
    let ack = child.recv_until(WAIT, |v| v["id"] == "edit").pop().unwrap();
    assert_eq!(ack["success"], true, "{ack:?}");
    child.recv_until(WAIT, |v| v["type"] == "agent_end");
    child.recv_until(WAIT, |v| v["type"] == "agent_idle");
    child.send_value(
        &json!({"type":"get_messages","id":"after","sessionId":session,"params":{"limit":100}}),
    );
    let after = child
        .recv_until(WAIT, |v| v["id"] == "after")
        .pop()
        .unwrap();
    assert!(!after.to_string().contains("submitted_attachments"));
    let output = child.wait_for_exit(WAIT);
    assert!(output.status.success());
    let transcript = fs::read_to_string(
        fx.home_path
            .join(".tomcat/agents/main/sessions")
            .join(format!("{session}.jsonl")),
    )
    .unwrap();
    let active_user = transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|v| v["message"]["role"] == "user" && v["message"]["superseded"] != true)
        .unwrap();
    assert!(active_user["message"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part["provider_sha"] == ingest["payload"]["providerSha"]));
    let requests = captured_non_title_requests(&server);
    assert_eq!(
        requests.len(),
        2,
        "requests={requests:?}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    for request in &requests {
        assert!(extract_json_body(request)
            .to_string()
            .contains("data:image/png;base64,"));
    }
}

#[test]
#[serial]
fn rewind_rechecks_git_head_after_a_successful_preview() {
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        call("read", json!({"path":"a.txt"}), "read-a"),
        call(
            "write",
            json!({"path":"a.txt","content":"AI change","overwrite":true}),
            "write-a",
        ),
        answer("done"),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&fx.workspace)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "baseline",
    ]);
    fs::write(fx.workspace.join("a.txt"), "original").unwrap();
    let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
    let mut config = tomcat::load_config_toml_file(&config_path).unwrap();
    config.primitive.auto_confirm = true;
    config.workspace.workspace_roots = vec![fx.workspace.to_string_lossy().into_owned()];
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let mut child = spawn_serve_child(&fx);
    let session = initialize(&mut child);
    child.send_value(&json!({"type":"prompt","id":"original","sessionId":session,"text":"edit a","params":{"userMessageId":"git-user"}}));
    child.recv_until(WAIT, |v| v["type"] == "agent_idle");
    child.send_value(
        &json!({"type":"preview_rewind","id":"before","sessionId":session,"messageId":"git-user"}),
    );
    assert_eq!(
        child
            .recv_until(WAIT, |v| v["id"] == "before")
            .last()
            .unwrap()["payload"]["revertAvailable"],
        true
    );
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "HEAD changed after preview",
    ]);
    let path = fx
        .home_path
        .join(".tomcat/agents/main/sessions")
        .join(format!("{session}.jsonl"));
    let before = fs::read(&path).unwrap();
    child.send_value(
        &json!({"type":"preview_rewind","id":"after","sessionId":session,"messageId":"git-user"}),
    );
    assert_eq!(
        child
            .recv_until(WAIT, |v| v["id"] == "after")
            .last()
            .unwrap()["payload"]["revertReason"],
        "git_head_moved"
    );
    child.send_value(&json!({"type":"rewind_and_resend","id":"revert","sessionId":session,"messageId":"git-user","files":"revert","message":{"text":"new"}}));
    let reply = child
        .recv_until(WAIT, |v| v["id"] == "revert")
        .pop()
        .unwrap();
    assert_eq!(reply["error"], "revert_unavailable");
    assert_eq!(reply["payload"]["revertReason"], "git_head_moved");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(fx.workspace.join("a.txt")).unwrap(),
        "AI change"
    );
    assert_eq!(captured_non_title_requests(&server).len(), 3);
    assert!(child.wait_for_exit(WAIT).status.success());
}

#[test]
#[serial]
fn rewind_and_resend_real_files_payload_and_restart() {
    for (files, running) in [("keep", false), ("revert", false), ("revert", true)] {
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
            call("read", json!({"path":"a.txt"}), "read-a"),
            call(
                "write",
                json!({"path":"a.txt","content":"AI change","overwrite":true}),
                "write-a",
            ),
            call(
                "write",
                json!({"path":"b.txt","content":"AI created","overwrite":false}),
                "write-b",
            ),
            call(
                "bash",
                json!({"command":"printf terminal > c.txt","cwd":".","foreground_wait_ms":8000,"run_in_background":false}),
                "bash-c",
            ),
            if running {
                call(
                    "ask_question",
                    json!({"questions":[{"id":"wait","prompt":"Wait here?","options":[{"id":"yes","label":"Yes","recommended":true},{"id":"no","label":"No","recommended":false}]}]}),
                    "pending-question",
                )
            } else {
                answer("DISCARDED_ANSWER")
            },
            answer("replacement answer"),
        ]);
        let fx = setup_serve_fixture(&server.base_url);
        let config_path = fx.home_path.join(".tomcat/tomcat.config.toml");
        let mut config = tomcat::load_config_toml_file(&config_path).unwrap();
        config.primitive.auto_confirm = true;
        config.workspace.workspace_roots = vec![fx.workspace.to_string_lossy().into_owned()];
        fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
        fs::write(fx.workspace.join("a.txt"), b"original bytes").unwrap();
        fs::write(fx.workspace.join("d.txt"), b"human only").unwrap();
        let mut child = spawn_serve_child(&fx);
        let session = initialize(&mut child);
        child.send_value(&json!({"type":"prompt","id":"original","sessionId":session,"text":"DISCARDED_USER","params":{"userMessageId":"original-user"}}));
        let frames = child.recv_until(WAIT, |v| {
            if running {
                v["type"] == "control_request" && v["subtype"] == "ask_question"
            } else {
                v["type"] == "agent_idle"
            }
        });
        assert_eq!(
            fs::read_to_string(fx.workspace.join("a.txt")).unwrap(),
            "AI change",
            "{frames:?}"
        );
        assert!(fx.workspace.join("b.txt").exists(), "{frames:?}");
        fs::write(fx.workspace.join("a.txt"), b"manual after AI").unwrap();
        child.send_value(&json!({"type":"preview_rewind","id":"preview","sessionId":session,"messageId":"original-user"}));
        let preview = child
            .recv_until(WAIT, |v| v["id"] == "preview")
            .pop()
            .unwrap();
        assert_eq!(preview["payload"]["revertAvailable"], true, "{preview:?}");
        assert_eq!(
            preview["payload"]["revertPaths"].as_array().unwrap().len(),
            2,
            "preview={preview:?}; stderr={}",
            child.stderr()
        );
        child.send_value(&json!({"type":"rewind_and_resend","id":"edit","sessionId":session,"messageId":"original-user","files":files,"message":{"text":"replacement user"}}));
        let ack = child.recv_until(WAIT, |v| v["id"] == "edit");
        assert_eq!(ack.last().unwrap()["success"], true, "{ack:?}");
        let _frames = child.recv_until(WAIT, |v| v["type"] == "agent_idle");
        assert_eq!(
            fs::read_to_string(fx.workspace.join("a.txt")).unwrap(),
            if files == "revert" {
                "original bytes"
            } else {
                "manual after AI"
            }
        );
        assert_eq!(fx.workspace.join("b.txt").exists(), files == "keep");
        assert_eq!(
            fs::read_to_string(fx.workspace.join("c.txt")).unwrap(),
            "terminal"
        );
        assert_eq!(
            fs::read_to_string(fx.workspace.join("d.txt")).unwrap(),
            "human only"
        );
        let requests = captured_non_title_requests(&server);
        let payload = extract_json_body(requests.last().unwrap());
        let messages = payload["messages"].to_string();
        assert!(!messages.contains("DISCARDED"), "{messages}");
        assert_eq!(
            messages.matches("replacement user").count(),
            1,
            "{messages}"
        );
        child.send_value(&json!({"type":"rewind_and_resend","id":"again","sessionId":session,"messageId":"original-user","files":files,"message":{"text":"must not send"}}));
        assert_eq!(
            child
                .recv_until(WAIT, |v| v["id"] == "again")
                .last()
                .unwrap()["error"],
            "rewind_target_stale"
        );
        assert!(child.wait_for_exit(WAIT).status.success());
        let mut reopened = spawn_serve_child(&fx);
        let _ = initialize(&mut reopened);
        reopened.send_value(&json!({"type":"get_messages","id":"history","sessionId":session,"params":{"limit":100}}));
        let history = reopened
            .recv_until(WAIT, |v| v["id"] == "history")
            .pop()
            .unwrap();
        let live = history["payload"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["type"] == "message" && entry["message"]["superseded"] != true)
            .map(|entry| entry["message"].to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!live.contains("DISCARDED"));
        assert!(live.contains("replacement user"));
    }
}
