mod common;

use common::shared_serve::{output_json, NativeClient as Client, ServiceFixture};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(20);

#[cfg(windows)]
#[tokio::test]
async fn self_install_updates_running_copy_without_restarting_until_explicit_restart() {
    use common::serve::{cargo_bin_path, isolated_serve_command_with_binary};
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let bin = fx.0.home_path.join(".local/bin");
    let installed = bin.join("tomcat.exe");
    let current = std::fs::read(cargo_bin_path()).unwrap();
    let mut stale = current.clone();
    stale.extend_from_slice(b"older-build"); // Runnable PE, distinct same-version build fixture.
    std::fs::write(&installed, stale).unwrap();
    let action = |name: &str| output_json(
        isolated_serve_command_with_binary(&fx.0.home_path, &installed)
            .current_dir(&fx.0.workspace).args(["serve", name]).output().unwrap()
    );
    let before = action("start");
    let output = fx.command().arg("self-install").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(std::fs::read(&installed).unwrap() == current);
    assert_eq!(std::fs::read_dir(&bin).unwrap().filter(|entry|
        entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".old")
    ).count(), 1);
    let unchanged = action("status"); // Never let start hide an accidental shutdown.
    assert_eq!(unchanged["running"], true);
    assert_eq!(unchanged["instanceId"], before["instanceId"]);
    assert_eq!(unchanged["pid"], before["pid"]);
    let after = action("restart");
    assert_ne!(after["instanceId"], before["instanceId"]);
    assert_ne!(after["pid"], before["pid"]);
    let mut client = Client::connect(&after).await;
    let session_id = client.new_session(&fx.0.workspace).await;
    let reply = client.request(json!({"type":"get_state","id":"after-update","sessionId":session_id})).await;
    assert_eq!(reply["success"], true, "{reply}");
}

#[cfg(windows)]
#[test]
fn service_owns_a_hidden_console_so_children_open_no_windows() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let info = fx.action("start");
    let pid = info["pid"].as_u64().unwrap();
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            "$ErrorActionPreference = 'Stop'; $consoles = @(Get-CimInstance Win32_Process -Filter \"ParentProcessId={pid} AND Name='conhost.exe'\"); @{{ consoleCount = $consoles.Count; window = (Get-Process -Id {pid}).MainWindowHandle.ToInt64() }} | ConvertTo-Json -Compress"
        ))
        .output()
        .unwrap();
    let console = output_json(output);
    assert!(console["consoleCount"].as_u64().unwrap() > 0, "{console}");
    assert_eq!(console["window"], 0, "service must have no visible window");
}

#[tokio::test]
async fn shared_service_resolves_auto_language_per_connection() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let info = output_json(fx.command()
        .env("TOMCAT_HOST_LOCALE", "zh-CN")
        .env_remove("TOMCAT__UI__LANGUAGE")
        .env("LC_ALL", "en_US.UTF-8")
        .args(["serve", "start"]).output().unwrap());
    for (environment, effective, overridden) in [
        (json!({"TOMCAT_HOST_LOCALE":"zh-CN"}), "zh-CN", false),
        (json!({"TOMCAT_HOST_LOCALE":"en"}), "en", false),
        (json!({"TOMCAT_HOST_LOCALE":"en", "TOMCAT__UI__LANGUAGE":"zh-CN"}), "zh-CN", true),
        // Missing connection keys must not borrow the starter's zh-CN host locale.
        (json!({}), "en", false),
    ] {
        let mut client = Client::open_with_environment(&info, env!("CARGO_PKG_VERSION"), environment).await;
        let hello = client.next().await;
        assert!(hello.get("error").is_none(), "{hello}");
        client.send(json!({"type":"control_request","requestId":"init","subtype":"initialize","payload":{}})).await;
        let frames = client.until(|v| v["type"] == "control_response" && v["requestId"] == "init").await;
        assert_eq!(frames.last().unwrap()["payload"]["uiPreferences"],
            json!({"language":"auto","effective":effective,"envOverride":overridden}));
        if effective == "en" {
            let error = client.request(json!({"type":"new_session","id":"bad-cwd","params":{"cwd":"relative"}})).await;
            assert_eq!(error["success"], false, "{error}");
            let catalog: Value = serde_json::from_str(include_str!("../assets/i18n/en.json")).unwrap();
            assert!(error.to_string().contains(catalog["serve.cwdAbsolute"].as_str().unwrap()), "{error}");
        }
        // Saving and switching back to auto must resolve this same connection, too.
        let saved = client.request(json!({"type":"set_ui_language","id":"save","language":"en"})).await;
        assert_eq!(saved["success"], true, "{saved}");
        assert_eq!(saved["payload"]["effective"], if overridden { "zh-CN" } else { "en" });
        let auto = client.request(json!({"type":"set_ui_language","id":"auto","language":"auto"})).await;
        assert_eq!(auto["success"], true, "{auto}");
        assert_eq!(auto["payload"]["effective"], effective);
    }
}

// Explicit OS-settings harness only: these tests never mutate global settings.
// The Windows wrapper toggles the actual registry, and records proxy requests.
#[tokio::test]
#[ignore = "run only inside the controlled system-proxy acceptance wrapper"]
async fn system_proxy_off_cannot_reach_remote_model() {
    assert_eq!(
        std::env::var("TOMCAT_SYSTEM_PROXY_ACCEPTANCE").as_deref(),
        Ok("off")
    );
    #[cfg(windows)]
    assert_system_proxy_test_job();
    let fx = ServiceFixture::new("http://tomcat-system-model.invalid:9");
    let mut client = Client::connect(&fx.action("start")).await;
    let reply = client
        .request(json!({"type":"list_remote_models","id":"off",
        "baseUrl":"http://tomcat-system-model.invalid:9","api":"openai","apiKey":"dummy-key"}))
        .await;
    assert_eq!(reply["success"], false, "{reply}");
    assert!(
        matches!(
            reply["payload"]["reason"].as_str(),
            Some("network" | "timeout")
        ),
        "{reply}"
    );
}

#[tokio::test]
#[ignore = "run only inside the controlled system-proxy acceptance wrapper"]
async fn system_proxy_routes_native_and_acp_model_and_remote_mcp() {
    use common::{
        acp,
        serve::{
            response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
            sse_finish, sse_tool_call,
        },
    };
    assert_eq!(
        std::env::var("TOMCAT_SYSTEM_PROXY_ACCEPTANCE").as_deref(),
        Ok("on")
    );
    #[cfg(windows)]
    assert_system_proxy_test_job();
    let mcp_url = std::env::var("TOMCAT_SYSTEM_PROXY_MCP_URL").unwrap();
    assert!(mcp_url.starts_with("http://tomcat-system-mcp.invalid:"));
    let server = spawn_scripted_openai_stream_server_with_auto_title((0..2).flat_map(|_| vec![
        response(vec![sse_tool_call("system-mcp", "tool_call",
            &json!({"name":"mcp__proxy__echo","arguments":{"message":"system-proxy-check"}}).to_string()),
            sse_finish("tool_calls"), sse_done()]),
        response(vec![sse_delta("system-proxy-task-complete"), sse_finish("stop"), sse_done()]),
    ]).collect());
    let remote_model = server
        .base_url
        .replace("127.0.0.1", "tomcat-system-model.invalid");
    let fx = ServiceFixture::new(&remote_model);
    fx.configure(|cfg| {
        assert!(cfg.llm.proxy.is_none());
        cfg.llm.retry_count = 0;
        cfg.llm.agent_max_attempts = 1;
    });
    std::fs::write(
        fx.0.home_path.join(".tomcat/mcp.json"),
        json!({"mcpServers":{"proxy":{
            "url":mcp_url,"auth":"bearer","headers":{"Authorization":"Bearer fake-access-token"}
        }}})
        .to_string(),
    )
    .unwrap();
    let info = fx.action("start");
    // Windows environment keys are case-insensitive: send one NO_PROXY entry,
    // not a malformed snapshot with both NO_PROXY and no_proxy.
    let environment = json!({"HTTP_PROXY":"http://native.invalid:bad-port", "HTTPS_PROXY":"http://native.invalid:bad-port",
        "ALL_PROXY":"http://native.invalid:bad-port", "NO_PROXY":"*"});
    let mut native =
        Client::open_with_environment(&info, env!("CARGO_PKG_VERSION"), environment).await;
    let hello = native.next().await;
    assert_eq!(hello["instanceId"], info["instanceId"]);
    assert!(hello.get("error").is_none(), "{hello}");
    native.send(json!({"type":"control_request","requestId":"init","subtype":"initialize","payload":{}})).await;
    native
        .until(|v| v["type"] == "control_response" && v["requestId"] == "init")
        .await;
    let session = native.new_session(&fx.0.workspace).await;
    native.send(json!({"type":"prompt","id":"os-proxy","sessionId":session,"text":"Use the remote MCP echo tool."})).await;
    let frames = native.until(|v| v["type"] == "agent_idle").await;
    assert!(
        frames
            .iter()
            .any(|v| v.to_string().contains("system-proxy-task-complete")),
        "{frames:?}"
    );

    let mut command = fx.command();
    command
        .arg("acp")
        .env("HTTP_PROXY", "http://acp.invalid:bad-port")
        .env("HTTPS_PROXY", "http://acp.invalid:bad-port")
        .env("ALL_PROXY", "http://acp.invalid:bad-port")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*");
    let mut host = common::serve::spawn_serve_command(command);
    acp::initialize(&mut host);
    let session = acp::new_session(&mut host, &fx.0.workspace);
    let frames = acp::request(
        &mut host,
        json!("os-proxy"),
        "session/prompt",
        json!({"sessionId":session,"prompt":[{"type":"text","text":"Use the remote MCP echo tool."}]}),
    );
    assert_eq!(acp::ok(&frames)["stopReason"], "end_turn");
    acp::close(host);
    let requests = server.captured_requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("fake echo: system-proxy-check"))
            .count(),
        2,
        "both real MCP results must reach their subsequent model request: {requests:?}"
    );
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
}

#[tokio::test]
async fn connection_skills_stay_out_of_native_session_catalogs_and_new_load_rescan_project() {
    use common::acp;
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    fx.configure(|cfg| cfg.skills.enabled = true);
    let host_root = fx.0.home_path.join("host-skills");
    std::fs::create_dir_all(host_root.join("host-only")).unwrap();
    std::fs::write(
        host_root.join("host-only/SKILL.md"),
        "---\nname: host-only\ndescription: HOST_PRIVATE_CATALOG_SENTINEL\n---\nprivate body\n",
    )
    .unwrap();
    let mut command = fx.command();
    command.args(["acp", "--skills-dir"]).arg(&host_root);
    let mut host = common::serve::spawn_serve_command(command);
    acp::initialize(&mut host);
    let host_session = acp::new_session(&mut host, &fx.0.workspace);
    let info = fx.action("status");
    let mut native = Client::connect(&info).await;
    let original = native.new_session(&fx.0.workspace).await;
    let catalog = native
        .request(json!({"type":"get_instruction_catalog","id":"own-catalog"}))
        .await;
    assert_eq!(catalog["success"], true, "{catalog}");
    assert!(!catalog
        .to_string()
        .contains("HOST_PRIVATE_CATALOG_SENTINEL"));

    let project_skill = fx.0.workspace.join(".agents/skills/late-native");
    std::fs::create_dir_all(&project_skill).unwrap();
    std::fs::write(
        project_skill.join("SKILL.md"),
        "---\nname: late-native\ndescription: LATE_PROJECT_CATALOG_SENTINEL\n---\nproject body\n",
    )
    .unwrap();
    native.new_session(&fx.0.workspace).await;
    let catalog = native
        .request(json!({"type":"get_instruction_catalog","id":"new-catalog"}))
        .await;
    assert!(catalog
        .to_string()
        .contains("LATE_PROJECT_CATALOG_SENTINEL"));
    assert!(!catalog
        .to_string()
        .contains("HOST_PRIVATE_CATALOG_SENTINEL"));
    let loaded = native
        .request(json!({"type":"switch_session","id":"cached","sessionId":original}))
        .await;
    assert_eq!(loaded["payload"]["access"], "control");
    let catalog = native
        .request(json!({"type":"get_instruction_catalog","id":"loaded-catalog"}))
        .await;
    assert!(catalog
        .to_string()
        .contains("LATE_PROJECT_CATALOG_SENTINEL"));
    assert!(!catalog
        .to_string()
        .contains("HOST_PRIVATE_CATALOG_SENTINEL"));

    // Looking at the host's actual session uses its view, without changing our own.
    let observed = native
        .request(json!({"type":"switch_session","id":"host","sessionId":host_session}))
        .await;
    assert_eq!(observed["payload"]["access"], "observe");
    let catalog = native
        .request(json!({"type":"get_instruction_catalog","id":"observed-catalog"}))
        .await;
    assert!(catalog
        .to_string()
        .contains("HOST_PRIVATE_CATALOG_SENTINEL"));
    native
        .request(json!({"type":"switch_session","id":"back","sessionId":original}))
        .await;
    let catalog = native
        .request(json!({"type":"get_instruction_catalog","id":"back-catalog"}))
        .await;
    assert!(!catalog
        .to_string()
        .contains("HOST_PRIVATE_CATALOG_SENTINEL"));
    acp::close(host);
}

#[tokio::test]
async fn shared_service_ignores_starter_model_config_proxy_and_keeps_saved_credentials() {
    use common::serve::{
        response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
        sse_finish,
    };
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("saved-config-response"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| cfg.serve.max_sessions = 1);
    let info = output_json(
        fx.command()
            .env("TOMCAT__LLM__DEFAULT_MODEL", "starter-only-model")
            .env("TOMCAT__SERVE__MAX_SESSIONS", "100")
            .env("OPENAI_API_KEY", "starter-only-key")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let mut owner = Client::connect(&info).await;
    let session = owner.new_session(&fx.0.workspace).await;
    owner.send(json!({"type":"prompt","id":"saved","sessionId":session,"text":"check saved configuration","params":{}})).await;
    let frames = owner.until(|v| v["type"] == "agent_idle").await;
    assert!(
        frames
            .iter()
            .any(|v| v.to_string().contains("saved-config-response")),
        "{frames:?}"
    );
    let requests = server.captured_requests();
    let request = requests.first().expect("actual model request");
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], "gpt-5.4");
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bearer dummy-key"));
    assert!(!request.contains("starter-only-key"));
    let mut other = Client::connect(&info).await;
    let full = other
        .request(json!({"type":"new_session","id":"full","params":{"cwd":fx.0.workspace}}))
        .await;
    assert_eq!(
        full["success"], false,
        "saved max_sessions must win over the starter: {full}"
    );
}

#[tokio::test]
async fn shared_service_saved_proxy_cannot_be_bypassed_by_starter_or_native_environment() {
    use common::serve::{
        response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
        sse_finish,
    };
    let proxy = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("saved-proxy-response"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    // This reserved name is intentionally unreachable without the explicit local proxy.
    let fx = ServiceFixture::new("http://tomcat-fixture.invalid");
    fx.configure(|cfg| cfg.llm.proxy = Some(proxy.base_url.clone()));
    let info = output_json(
        fx.command()
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let mut client = Client::open_with_environment(
        &info,
        env!("CARGO_PKG_VERSION"),
        json!({
            "HTTP_PROXY":"http://invalid-native-proxy:bad-port",
            "HTTPS_PROXY":"http://invalid-native-proxy:bad-port",
            "ALL_PROXY":"http://invalid-native-proxy:bad-port",
            "NO_PROXY":"*"
        }),
    )
    .await;
    let hello = client.next().await;
    assert_eq!(hello["instanceId"], info["instanceId"]);
    assert!(hello.get("error").is_none(), "{hello}");
    client.send(json!({"type":"control_request","requestId":"init","subtype":"initialize","payload":{}})).await;
    let initialized = client
        .until(|v| v["type"] == "control_response" && v["requestId"] == "init")
        .await;
    assert_eq!(initialized.last().unwrap()["payload"]["protocolVersion"], 2);
    let session = client.new_session(&fx.0.workspace).await;
    client.send(json!({"type":"prompt","id":"proxy","sessionId":session,"text":"check saved proxy","params":{}})).await;
    let frames = client.until(|v| v["type"] == "agent_idle").await;
    assert!(
        frames
            .iter()
            .any(|v| v.to_string().contains("saved-proxy-response")),
        "{frames:?}"
    );
    let requests = proxy.captured_requests();
    assert!(
        requests.iter().any(|request| request
            .lines()
            .take_while(|line| !line.is_empty())
            .any(|line| line.contains("tomcat-fixture.invalid"))),
        "proxy never received the main model request"
    );
}

#[tokio::test]
async fn shared_service_does_not_fall_back_to_a_starter_key_when_saved_key_is_missing() {
    use common::serve::{
        response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
        sse_finish,
    };
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
        sse_delta("must-not-contact-model"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let fx = ServiceFixture::new(&server.base_url);
    std::fs::write(fx.0.home_path.join(".tomcat/assets/.env"), "").unwrap();
    let info = output_json(
        fx.command()
            .env("OPENAI_API_KEY", "starter-only-key")
            .args(["serve", "start"])
            .output()
            .unwrap(),
    );
    let mut client = Client::connect(&info).await;
    let session = client.new_session(&fx.0.workspace).await;
    client.send(json!({"type":"prompt","id":"no-key","sessionId":session,"text":"must require saved credentials","params":{}})).await;
    let frames = client.until(|v| v["type"] == "agent_idle").await;
    assert!(
        frames.iter().any(|v| v["type"] == "agent_end"
            && v["error"]
                .as_str()
                .is_some_and(|error| error.contains("OPENAI_API_KEY"))),
        "{frames:?}"
    );
    assert_eq!(frames.last().unwrap()["outcome"], "failed");
    assert!(
        server.captured_requests().is_empty(),
        "no ambient credential may reach the model"
    );
}

#[test]
fn concurrent_starters_share_one_instance_and_stop_restart_are_explicit() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    assert_eq!(fx.action("status")["running"], false);
    let mut starters = Vec::new();
    for _ in 0..10 {
        starters.push(
            fx.command()
                .args(["serve", "start"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let infos: Vec<_> = starters
        .into_iter()
        .map(|child| output_json(child.wait_with_output().unwrap()))
        .collect();
    for info in &infos {
        assert_eq!(info["instanceId"], infos[0]["instanceId"]);
        assert_eq!(info["pid"], infos[0]["pid"]);
    }
    assert_eq!(fx.action("status")["instanceId"], infos[0]["instanceId"]);
    // In particular, Windows must not retain the launcher's worktree as process cwd.
    std::fs::remove_dir(&fx.0.workspace).unwrap();
    std::fs::create_dir(&fx.0.workspace).unwrap();
    let restarted = fx.action("restart");
    assert_ne!(restarted["instanceId"], infos[0]["instanceId"]);
    assert_eq!(fx.action("stop")["stopped"], true);
    assert_eq!(fx.action("status")["running"], false);
    // The unchanged lock and stale socket are intentionally reused, not deleted by a destructor.
    let replacement = fx.action("start");
    assert_ne!(replacement["instanceId"], restarted["instanceId"]);
}

#[tokio::test]
async fn native_connections_keep_request_ids_selection_and_cwd_separate() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let info = fx.action("start");
    let cwd_a = fx.0.workspace.join("a");
    let cwd_b = fx.0.workspace.join("b");
    std::fs::create_dir_all(&cwd_a).unwrap();
    std::fs::create_dir_all(&cwd_b).unwrap();
    let mut a = Client::connect(&info).await;
    let mut b = Client::connect(&info).await;
    let (sa, sb) = tokio::join!(a.new_session(&cwd_a), b.new_session(&cwd_b));
    assert_ne!(sa, sb);
    for (client, session) in [(&mut a, &sa), (&mut b, &sb)] {
        let reply = client
            .request(json!({"type":"get_state", "id":"same-id"}))
            .await;
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["sessionId"].as_str(), Some(session.as_str()));
    }
    let missing = b
        .request(json!({"type":"new_session","id":"missing", "params":{}}))
        .await;
    assert_eq!(
        missing["success"], false,
        "shared service must not use its own cwd"
    );
    let foreign = b
        .request(json!({"type":"switch_session","id":"foreign","sessionId":sa}))
        .await;
    assert_eq!(foreign["error"], "unknown_session");
    let foreign_read = b
        .request(json!({"type":"get_messages","id":"foreign-read","sessionId":sa}))
        .await;
    assert_eq!(foreign_read["error"], "unknown_session");
    let list = b
        .request(json!({"type":"list_sessions","scope":"disk","id":"own-list"}))
        .await;
    let sessions = list["payload"]["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|entry| entry["sessionId"] == sb));
    assert!(!sessions.iter().any(|entry| entry["sessionId"] == sa));
    assert_eq!(
        b.request(json!({"type":"get_state","id":"unchanged"}))
            .await["sessionId"],
        sb
    );

    let mut same_project = Client::connect(&info).await;
    same_project.new_session(&cwd_a).await;
    let observed = same_project
        .request(json!({"type":"switch_session","id":"observe","sessionId":sa}))
        .await;
    assert_eq!(observed["payload"]["access"], "observe");
    let denied = same_project
        .request(json!({"type":"interrupt","id":"deny","sessionId":sa}))
        .await;
    assert_eq!(denied["error"], "read_only_session");
    drop(same_project);
    drop(b);
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
    assert_eq!(
        a.request(json!({"type":"get_state","id":"still-here"}))
            .await["sessionId"],
        sa
    );
    let mut wrong = Client::open(&info, "0.0.0-incompatible").await;
    assert!(wrong.next().await["error"]
        .as_str()
        .unwrap()
        .contains("service_version_mismatch"));
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
}

#[tokio::test]
async fn completed_connections_do_not_exhaust_slots_and_history_can_be_reopened() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    fx.configure(|cfg| cfg.serve.max_sessions = 2);
    let info = fx.action("start");
    let mut first = None;
    let mut observer = Client::connect(&info).await;
    observer.new_session(&fx.0.workspace).await;
    for _ in 0..5 {
        let mut client = Client::connect(&info).await;
        let id = client.new_session(&fx.0.workspace).await;
        first.get_or_insert(id.clone());
        // Windows named-pipe shutdown() only flushes; it is not a half-close.
        // Close the real handle, then wait until another connection can take control.
        // A read-only list reports current ownership, not whether takeover is possible.
        drop(client);
        tokio::time::timeout(WAIT, async {
            loop {
                let reply = observer
                    .request(json!({"type":"switch_session", "sessionId":id, "id":"released"}))
                    .await;
                assert_eq!(reply["success"], true, "{reply}");
                if reply["payload"]["access"] == "control" {
                    break;
                }
                assert_eq!(reply["payload"]["access"], "observe", "{reply}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("EOF must release control before the next capacity admission");
    }
    let restored = observer.request(json!({"type":"switch_session", "id":"reopen", "sessionId":first.unwrap(), "cwd":fx.0.workspace})).await;
    assert_eq!(
        restored["success"], true,
        "eviction must preserve history: {restored}"
    );
}

#[cfg(windows)]
fn assert_system_proxy_test_job() {
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    };
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    // Reuse nextest's BREAKAWAY_OK test Job. Do not modify the runner's Job
    // or the deliberately restrictive host Job tested elsewhere in this file.
    // SAFETY: this Win32 output structure contains only zero-valid POD fields.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: null selects this process's immediate Job; the output buffer has
    // the exact Win32 structure and remains alive for the synchronous call.
    let queried = unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        queried,
        0,
        "query test Job: {}",
        std::io::Error::last_os_error()
    );
    let flags = limits.BasicLimitInformation.LimitFlags;
    println!("Windows acceptance immediate Job flags: {flags:#x}");
    assert_ne!(
        flags & JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        0,
        "run this acceptance through cargo nextest"
    );
}

#[cfg(windows)]
fn spawn_in_windows_job(
    command: std::process::Command,
    allow_breakaway: bool,
) -> (std::os::windows::io::OwnedHandle, common::serve::ServeChild) {
    use common::serve::spawn_serve_command;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    // SAFETY: null attributes/name create an anonymous, non-inheritable Job handle.
    let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!raw_job.is_null(), "{}", std::io::Error::last_os_error());
    // SAFETY: the checked handle is fresh and transferred to exactly one owner.
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
    // SAFETY: this Win32 information structure is POD and accepts zero defaults.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | if allow_breakaway {
            JOB_OBJECT_LIMIT_BREAKAWAY_OK
        } else {
            0
        };
    // SAFETY: the Job is live and the initialized buffer has the exact advertised size.
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    assert_ne!(configured, 0, "{}", std::io::Error::last_os_error());

    let mut host = spawn_serve_command(command);
    // SAFETY: the harness owns this live child, which is waiting for stdin before spawning.
    let raw_process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, host.pid()) };
    assert!(
        !raw_process.is_null(),
        "{}",
        std::io::Error::last_os_error()
    );
    // SAFETY: OpenProcess returned a new owned handle checked above.
    let process = unsafe { OwnedHandle::from_raw_handle(raw_process) };
    // SAFETY: both handles are live; the process has the required assignment rights.
    let assigned =
        unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) };
    assert_ne!(assigned, 0, "{}", std::io::Error::last_os_error());
    // A real input barrier, not a sleep-based race with Job assignment.
    host.send_raw("start");
    (job, host)
}

#[cfg(windows)]
fn starter_in_windows_job(
    fx: &ServiceFixture,
    allow_breakaway: bool,
) -> (std::os::windows::io::OwnedHandle, common::serve::ServeChild) {
    use common::serve::{cargo_bin_path, isolated_serve_command_with_binary};
    let mut command =
        isolated_serve_command_with_binary(&fx.0.home_path, Path::new("powershell.exe"));
    command
        .current_dir(&fx.0.workspace)
        .env("TOMCAT_TEST_SERVICE_BINARY", cargo_bin_path())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
$null = [Console]::ReadLine()
$result = & $env:TOMCAT_TEST_SERVICE_BINARY serve start 2>&1
$code = $LASTEXITCODE
if ($code -ne 0) {
    @{ error = ($result | Out-String) } | ConvertTo-Json -Compress
    exit $code
}
$result
Start-Sleep -Seconds 60
"#,
        ]);
    spawn_in_windows_job(command, allow_breakaway)
}

#[cfg(windows)]
fn restricted_acp_host(
    fx: &ServiceFixture,
) -> (std::os::windows::io::OwnedHandle, common::serve::ServeChild) {
    use common::serve::{cargo_bin_path, isolated_serve_command_with_binary};
    // Only a process owner/stdio relay: the Rust test drives the standard ACP API.
    // No Multica checkout, database or host-specific adapter code is needed in CI.
    let script = fx.0.workspace.join("restricted-acp-host.cjs");
    std::fs::write(&script, r#"
const { spawn } = require('node:child_process');
process.stdin.once('data', () => {
  // Do not add libuv's inner Job: inherit the actual restrictive host Job unchanged.
  const child = spawn(process.argv[2], ['acp'], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true, detached: true });
  let error = '';
  child.stderr.on('data', data => { error += data; });
  child.stdout.pipe(process.stdout);
  child.stdin.on('error', () => {}); // A failed startup can close stdin before the host finishes writing.
  process.stdin.pipe(child.stdin);
  child.once('spawn', () => console.log(JSON.stringify({ hostReady: true })));
  child.once('error', failure => { error += failure.message; });
  child.once('close', code => {
    process.stdin.unpipe(child.stdin); process.stdin.pause();
    if (code !== 0) console.log(JSON.stringify({ hostError: error }));
    process.exitCode = code ?? 1;
  });
});
"#).unwrap();
    let mut command = isolated_serve_command_with_binary(&fx.0.home_path, Path::new("node"));
    command
        .current_dir(&fx.0.workspace)
        .arg(script)
        .arg(cargo_bin_path());
    spawn_in_windows_job(command, false)
}

#[cfg(windows)]
#[tokio::test]
async fn restricted_acp_host_requires_prestart_then_completes_task_without_owning_service() {
    use common::{
        acp,
        serve::{
            response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
            sse_finish, sse_tool_call,
        },
    };
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(vec![sse_tool_call("job-proof", "write", &json!({
            "path":"job-proof.txt","content":"real tool under restricted host","overwrite":false
        }).to_string()), sse_finish("tool_calls"), sse_done()]),
        response(vec![
            sse_delta("restricted-host-task-complete"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    let (job, host) = restricted_acp_host(&fx);
    let failure = host.recv_until(WAIT, |frame| frame.get("hostError").is_some());
    assert!(
        failure.last().unwrap()["hostError"]
            .as_str()
            .unwrap()
            .contains("tomcat serve start"),
        "{failure:?}"
    );
    assert!(!host.wait_for_exit(WAIT).status.success());
    drop(job);
    assert_eq!(
        fx.action("status")["running"],
        false,
        "restricted startup must not silently escape the Job"
    );

    let before = fx.action("start"); // Outside the artificial host's restrictive Job.
    let (job, mut host) = restricted_acp_host(&fx);
    assert_eq!(host.recv_value(WAIT)["hostReady"], true);
    acp::initialize(&mut host);
    let session = acp::new_session(&mut host, &fx.0.workspace);
    let frames = acp::request(
        &mut host,
        json!("job-task"),
        "session/prompt",
        json!({"sessionId":session,"prompt":[{"type":"text","text":"Write the proof file and finish."}]}),
    );
    assert_eq!(acp::ok(&frames)["stopReason"], "end_turn");
    assert!(frames
        .iter()
        .any(|frame| frame.to_string().contains("restricted-host-task-complete")));
    assert_eq!(
        std::fs::read_to_string(fx.0.workspace.join("job-proof.txt")).unwrap(),
        "real tool under restricted host"
    );
    // Terminate the real Job with a known code: closing a Job does not promise a
    // nonzero exit code, and wait_for_exit also closes stdin. This proves the
    // owner was killed by the Job, not just gracefully ended by the helper's EOF.
    // SAFETY: this test owns the live Job and its disposable host/relay processes.
    let terminated = unsafe { TerminateJobObject(job.as_raw_handle(), 77) };
    assert_ne!(terminated, 0, "{}", std::io::Error::last_os_error());
    drop(job);
    assert_eq!(host.wait_for_exit(WAIT).status.code(), Some(77));
    let after = fx.action("status"); // A read-only status must not start a replacement service.
    assert_eq!(after["instanceId"], before["instanceId"]);
    assert_eq!(after["pid"], before["pid"]);
    let mut native = Client::connect(&after).await; // Direct IPC, no startup fallback.
    native.new_session(&fx.0.workspace).await;
    assert_eq!(
        native
            .request(json!({"type":"switch_session","id":"reopen-job","sessionId":session}))
            .await["success"],
        true
    );
    assert_eq!(
        native
            .request(json!({"type":"get_state","id":"still-usable"}))
            .await["sessionId"],
        session
    );
}

#[cfg(windows)]
#[test]
fn service_survives_closing_the_starters_kill_on_close_job() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let (job, host) = starter_in_windows_job(&fx, true);
    let info = host.recv_value(WAIT);
    assert_eq!(info["service"], "tomcat", "{info}");
    drop(job);
    host.wait_for_exit(WAIT);
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
    std::fs::remove_dir(&fx.0.workspace).unwrap();
    std::fs::create_dir(&fx.0.workspace).unwrap();
    assert_eq!(fx.action("status")["running"], true);
}

#[cfg(windows)]
#[test]
fn restricted_job_without_breakaway_fails_with_guidance() {
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let (job, host) = starter_in_windows_job(&fx, false);
    // The wrapper frames the actual CLI error before exiting, avoiding a stderr-reader race.
    let reply = host.recv_value(WAIT);
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("tomcat serve start"),
        "{reply}"
    );
    assert!(!host.wait_for_exit(WAIT).status.success());
    drop(job);
    assert_eq!(fx.action("status")["running"], false);
}

#[cfg(unix)]
#[test]
fn service_survives_killing_the_starter_process_group_and_uses_its_data_directory() {
    use common::serve::{cargo_bin_path, isolated_serve_command_with_binary, spawn_serve_command};
    use std::os::unix::process::CommandExt;
    let fx = ServiceFixture::new("http://127.0.0.1:1");
    let mut command = isolated_serve_command_with_binary(&fx.0.home_path, Path::new("/bin/sh"));
    command
        .current_dir(&fx.0.workspace)
        .process_group(0)
        .args(["-c", "\"$1\" serve start && exec sleep 60", "host"])
        .arg(cargo_bin_path());
    let host = spawn_serve_command(command);
    let info = host.recv_value(WAIT);
    let service_pid = info["pid"].as_i64().unwrap() as libc::pid_t;
    // SAFETY: host owns the fresh process group, and is not reaped until after this signal.
    assert_eq!(
        unsafe { libc::kill(-(host.pid() as libc::pid_t), libc::SIGKILL) },
        0
    );
    assert!(!host.wait_for_exit(WAIT).status.success());
    assert_eq!(fx.action("status")["instanceId"], info["instanceId"]);
    // SAFETY: read-only queries of the known service PID.
    assert_eq!(unsafe { libc::getsid(service_pid) }, service_pid);
    assert_eq!(unsafe { libc::getpgid(service_pid) }, service_pid);
    // The cross-platform lifecycle case checks deletion of the launch directory too.
    std::fs::remove_dir(&fx.0.workspace).unwrap();
    std::fs::create_dir(&fx.0.workspace).unwrap();
    assert_eq!(fx.action("status")["running"], true);
}

#[cfg(unix)]
#[tokio::test]
async fn controller_eof_waits_for_its_shell_to_stop_without_stopping_the_other_session() {
    use common::serve::{
        response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
        sse_finish, sse_tool_call,
    };
    use tokio::io::AsyncWriteExt;
    let mut script = Vec::new();
    for name in ["a", "b"] {
        let args = json!({"command": format!("echo $$ > {name}.pid; while :; do printf t >> {name}.ticks; sleep 0.05; done"), "run_in_background":true});
        script.push(response(vec![
            sse_tool_call(name, "bash", &args.to_string()),
            sse_finish("tool_calls"),
            sse_done(),
        ]));
        script.push(response(vec![
            sse_delta("scheduled"),
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
    let info = fx.action("start");
    let mut a = Client::connect(&info).await;
    let mut b = Client::connect(&info).await;
    for (client, name) in [(&mut a, "a"), (&mut b, "b")] {
        let session = client.new_session(&fx.0.workspace).await;
        client.send(json!({"type":"prompt", "id":name, "sessionId":session, "text":"start the background job", "params":{}})).await;
        client.until(|v| v["type"] == "agent_idle").await;
    }
    let pid = |name: &str| -> libc::pid_t {
        std::fs::read_to_string(fx.0.workspace.join(format!("{name}.pid")))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let a_pid = pid("a");
    let b_pid = pid("b");
    a.0.shutdown().await.unwrap();
    let mut tail = Vec::new();
    tokio::time::timeout(
        WAIT,
        tokio::io::AsyncReadExt::read_to_end(&mut a.0, &mut tail),
    )
    .await
    .unwrap()
    .unwrap();
    // SAFETY: signal 0 only queries processes whose PIDs the controlled shells wrote.
    assert_ne!(
        unsafe { libc::kill(a_pid, 0) },
        0,
        "socket EOF must acknowledge actual shell exit"
    );
    assert_eq!(unsafe { libc::kill(b_pid, 0) }, 0);
    let before = std::fs::metadata(fx.0.workspace.join("b.ticks"))
        .unwrap()
        .len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        std::fs::metadata(fx.0.workspace.join("b.ticks"))
            .unwrap()
            .len()
            > before
    );
    b.0.shutdown().await.unwrap();
    tokio::time::timeout(
        WAIT,
        tokio::io::AsyncReadExt::read_to_end(&mut b.0, &mut tail),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(unsafe { libc::kill(b_pid, 0) }, 0);
}

#[tokio::test]
async fn slow_connection_does_not_cancel_tasks() {
    use common::serve::{
        response, spawn_scripted_openai_stream_server_with_auto_title, sse_delta, sse_done,
        sse_finish,
    };
    let chunk = "x".repeat(4096);
    // 512 KiB fills the socket and 64-frame queue without triggering unrelated
    // background compaction (the 2 MiB fixture consumed the next scripted LLM reply).
    let mut parts: Vec<_> = (0..128)
        .map(|_| {
            let mut part = sse_delta(&chunk);
            part.delay_ms = 1;
            part
        })
        .collect();
    parts.extend([sse_finish("stop"), sse_done()]);
    let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
        response(parts),
        response(vec![sse_delta("next-turn"), sse_finish("stop"), sse_done()]),
    ]);
    let fx = ServiceFixture::new(&server.base_url);
    fx.configure(|cfg| {
        cfg.serve.delta_coalesce_ms = 0;
        cfg.serve.max_buffered_frames = 64;
    });
    let info = fx.action("start");
    let mut owner = Client::connect(&info).await;
    let session = owner.new_session(&fx.0.workspace).await;
    let mut observer = Client::connect(&info).await;
    observer.new_session(&fx.0.workspace).await;
    let open = observer
        .request(json!({"type":"switch_session","id":"open","sessionId":session}))
        .await;
    assert_eq!(open["payload"]["access"], "observe");
    owner.send(json!({"type":"prompt","id":"run","sessionId":session,"text":"stream the answer","params":{}})).await;
    // Do not read the controlling socket until the other connection observes completion.
    let live = observer.until(|v| v["type"] == "agent_idle").await;
    assert!(
        live.iter().any(|v| v["type"] == "message_update"),
        "{live:?}"
    );
    let slow = owner.until(|v| v["type"] == "agent_idle").await;
    assert!(
        slow.iter()
            .any(|v| v["type"] == "llm_notice" && v["finishReason"] == "backpressure"),
        "must actually exercise queue pressure, not just delayed reading"
    );
    owner.send(json!({"type":"prompt","id":"next","sessionId":session,"text":"another turn","params":{}})).await;
    let next = owner.until(|v| v["type"] == "agent_idle").await;
    assert!(
        next.iter().any(|v| v.to_string().contains("next-turn")),
        "the same controlling connection must still work; frames={:?}; llm_requests={}",
        next.iter()
            .filter(|frame| frame["type"] != "message_update")
            .collect::<Vec<_>>(),
        server.captured_requests().len()
    );
}
