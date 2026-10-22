use super::*;
use common::serve::{spawn_routed_openai_stream_server, ScriptedOpenAiServer, ScriptedPart};
use std::path::Path;

fn provider() -> ScriptedOpenAiServer {
    spawn_routed_openai_stream_server(|request| {
        if request["stream"] == false {
            return response(vec![ScriptedPart { delay_ms: 0,
                body: json!({"choices":[{"message":{"role":"assistant","content":"Checked skills"},"finish_reason":"stop"}]}).to_string() }]);
        }
        let last = request["messages"].as_array().unwrap().last().unwrap();
        if last["role"] == "user" && last["content"] == "load-host" {
            response(vec![
                sse_tool_call(
                    "load-private",
                    "load_skill",
                    &json!({"name":"host-only"}).to_string(),
                ),
                sse_finish("tool_calls"),
                sse_done(),
            ])
        } else {
            response(vec![sse_delta("done"), sse_finish("stop"), sse_done()])
        }
    })
}

fn skill(root: &Path, name: &str, description: &str, body: &str) {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .unwrap();
}

fn host(fx: &ServiceFixture, root: &Path) -> ServeChild {
    let mut command = fx.command();
    command.args(["acp", "--skills-dir"]).arg(root);
    let mut client = spawn_serve_command(command);
    initialize(&mut client);
    client
}

fn turn(client: &mut ServeChild, session: &str, input: &str) {
    prompt(client, input, session, input);
    assert_eq!(
        ok(&client.recv_until(WAIT, |v| v["id"] == input))["stopReason"],
        "end_turn"
    );
}

fn last_request(server: &ScriptedOpenAiServer) -> Value {
    server
        .captured_requests()
        .iter()
        .rev()
        .find_map(|request| {
            let body: Value = serde_json::from_str(request.split_once("\r\n\r\n")?.1).ok()?;
            (body["stream"] == true).then_some(body)
        })
        .expect("main model request")
}

fn system(server: &ScriptedOpenAiServer) -> String {
    last_request(server)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn connection_skills_rescan_new_cached_load_and_handoff_without_leaking_between_sessions() {
    let server = provider();
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.skills.enabled = true;
        cfg.primitive.auto_confirm = true;
    });
    let a_root = fx.0.home_path.join("host-a");
    let b_root = fx.0.home_path.join("host-b");
    let project = fx.0.workspace.join(".agents/skills");
    skill(&a_root, "host-only", "HOST_A_CATALOG", "HOST_A_BODY");
    skill(&b_root, "host-only", "HOST_B_CATALOG", "HOST_B_BODY");
    skill(
        &project,
        "project-only",
        "PROJECT_OLD_CATALOG",
        "project body",
    );
    let mut a = host(&fx, &a_root);
    let original = new_session(&mut a, &fx.0.workspace);
    turn(&mut a, &original, "load-host");
    assert!(system(&server).contains("HOST_A_CATALOG"));
    assert!(!system(&server).contains("HOST_B_CATALOG"));
    assert!(
        last_request(&server).to_string().contains("HOST_A_BODY"),
        "load_skill must use the same view as the prompt"
    );

    // A new slot must not reuse a directory's stale discovery result.
    skill(
        &project,
        "late-project",
        "PROJECT_LATE_CATALOG",
        "new project body",
    );
    let second = new_session(&mut a, &fx.0.workspace);
    turn(&mut a, &second, "new-slot");
    assert!(system(&server).contains("PROJECT_LATE_CATALOG"));
    // Neither a cached load nor a controller handoff may keep a stale/private view.
    skill(
        &a_root,
        "host-only",
        "HOST_A_UPDATED_CATALOG",
        "HOST_A_UPDATED_BODY",
    );
    skill(
        &project,
        "project-only",
        "PROJECT_UPDATED_CATALOG",
        "updated project body",
    );
    ok(&request(
        &mut a,
        json!("reload"),
        "session/load",
        json!({"sessionId":original,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    turn(&mut a, &original, "cached-load");
    assert!(system(&server).contains("HOST_A_UPDATED_CATALOG"));
    assert!(system(&server).contains("PROJECT_UPDATED_CATALOG"));
    assert!(!system(&server).contains("PROJECT_OLD_CATALOG"));

    let mut b = host(&fx, &b_root);
    let own_b = new_session(&mut b, &fx.0.workspace);
    turn(&mut b, &own_b, "isolated-b");
    assert!(system(&server).contains("HOST_B_CATALOG"));
    assert!(!system(&server).contains("HOST_A_UPDATED_CATALOG"));
    close(a);
    ok(&request(
        &mut b,
        json!("take"),
        "session/load",
        json!({"sessionId":original,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    turn(&mut b, &original, "takeover");
    assert!(system(&server).contains("HOST_B_CATALOG"));
    // Persisted history is intentionally not rewritten; compare the live system catalog.
    assert!(!system(&server).contains("HOST_A_UPDATED_CATALOG"));
    close(b);
    let mut plain = spawn(&fx);
    initialize(&mut plain);
    ok(&request(
        &mut plain,
        json!("plain"),
        "session/load",
        json!({"sessionId":original,"cwd":fx.0.workspace,"mcpServers":[]}),
    ));
    turn(&mut plain, &original, "no-private-root");
    assert!(!system(&server).contains("HOST_B_CATALOG"));
    assert!(!fx.0.home_path.join(".tomcat/skills/host-only").exists());
    close(plain);
}

#[test]
fn connection_skills_do_not_grant_file_access_when_confirmation_is_required() {
    let server = provider();
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.skills.enabled = true;
        cfg.primitive.auto_confirm = false;
        cfg.workspace.workspace_roots.clear();
    });
    let root = fx.0.home_path.join("private-skills");
    skill(
        &root,
        "host-only",
        "HOST_PRIVATE_CATALOG",
        "HOST_PRIVATE_PAYLOAD",
    );
    let mut client = host(&fx, &root);
    let session = new_session(&mut client, &fx.0.workspace);
    prompt(&mut client, "read", &session, "load-host");
    let frames = client.recv_until(WAIT, |v| v["method"] == "session/request_permission");
    let permission = frames.last().unwrap();
    assert!(permission.to_string().contains("SKILL.md"));
    assert!(!server
        .captured_requests()
        .join("\n")
        .contains("HOST_PRIVATE_PAYLOAD"));
    client.send_value(&json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":"deny"}}}));
    ok(&client.recv_until(WAIT, |v| v["id"] == "read"));
    assert!(!server
        .captured_requests()
        .join("\n")
        .contains("HOST_PRIVATE_PAYLOAD"));
    close(client);
}

#[test]
fn connection_skills_invalid_directory_is_rejected_before_starting_service() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let file = fx.0.home_path.join("not-a-directory");
    std::fs::write(&file, "file").unwrap();
    for directory in [
        std::path::PathBuf::from("relative"),
        fx.0.home_path.join("missing"),
        file,
    ] {
        let output = fx
            .command()
            .args(["acp", "--skills-dir"])
            .arg(directory)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("absolute readable directory"));
        assert!(output.stdout.is_empty());
        assert_eq!(fx.action("status")["running"], false);
    }
}
