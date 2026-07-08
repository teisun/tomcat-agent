use super::*;
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;

fn write_session_plugin_fixture(workspace: &std::path::Path, plugin_id: &str) {
    let plugin_dir = workspace.join(".tomcat").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let manifest = serde_json::json!({
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
        "activation": "session"
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

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_initialize_control_request_sets_ready_state() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "init-1".to_string(),
            subtype: "initialize".to_string(),
            session_id: None,
            payload: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(state.initialized.load(Ordering::SeqCst));

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_response")
        })
        .unwrap();
    assert_eq!(
        response
            .get("requestId")
            .and_then(serde_json::Value::as_str),
        Some("init-1")
    );
    let payload = response.get("payload").unwrap();
    assert_eq!(
        payload
            .get("protocolVersion")
            .and_then(serde_json::Value::as_i64),
        Some(1)
    );
    let capabilities = payload["capabilities"]
        .as_array()
        .expect("capabilities array");
    let capability_names = capabilities
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    for expected in [
        "prompt",
        "steer",
        "follow_up",
        "get_state",
        "set_plan_mode",
        "set_model",
        "set_thinking_level",
        "list_models",
        "upsert_model",
        "remove_model",
        "set_provider_key",
        "list_provider_keys",
        "new_session",
        "switch_session",
        "get_messages",
        "close_session",
        "list_sessions",
        "interrupt",
        "ask_question",
    ] {
        assert!(
            capability_names.contains(&expected),
            "missing capability {expected:?} in {capability_names:?}"
        );
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_not_initialized_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    state.initialized.store(false, Ordering::SeqCst);

    let allowed = ensure_initialized_or_error(
        &state,
        &ServeCommand::Prompt {
            id: Some("prompt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .unwrap();
    assert!(!allowed);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("prompt-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("not_initialized")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_cancels_target_session() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-1".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert!(handled);
    assert!(slot.ctx.session_runtime.cancel_token.lock().is_cancelled());

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-1"))
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_unknown_session_returns_error_response() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-missing".to_string()),
            session_id: Some("missing-session".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("interrupt-missing")
        })
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("unknown_session")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_unknown_control_subtype_returns_unknown_command_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    let handled = handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "weird-1".to_string(),
            subtype: "mystery".to_string(),
            session_id: None,
            payload: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    assert!(handled);

    let lines = wait_for_line(&buffer, |line| {
        line.get("error").and_then(serde_json::Value::as_str)
            == Some("unknown_command: control_request/mystery")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("error").and_then(serde_json::Value::as_str)
                == Some("unknown_command: control_request/mystery")
        })
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn shutdown_all_sessions_stops_live_plugin_vms_idempotently() {
    const PLUGIN_ID: &str = "serve-session-cleanup-plugin";

    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let plugin_workspace = tempfile::tempdir().expect("plugin workspace");
    write_session_plugin_fixture(plugin_workspace.path(), PLUGIN_ID);
    let plugin_dir = plugin_workspace
        .path()
        .join(".tomcat")
        .join("plugins")
        .join(PLUGIN_ID);

    let plugin_manager = slot
        .ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    plugin_manager
        .load_plugin(&plugin_dir)
        .expect("load plugin fixture");
    plugin_manager
        .enable_plugin(PLUGIN_ID)
        .expect("enable plugin fixture");
    plugin_manager
        .start_session_vm(&slot.session_id, PLUGIN_ID)
        .await
        .expect("start session vm");

    let instance_id = format!("{}/{}", slot.session_id, PLUGIN_ID);
    assert!(
        plugin_manager.has_session_vm(&slot.session_id, PLUGIN_ID),
        "fixture should have a live session VM before shutdown"
    );
    assert!(
        slot.ctx
            .scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_some(),
        "session VM should register an event sender before shutdown"
    );

    shutdown_all_sessions(Arc::clone(&state))
        .await
        .expect("shutdown all sessions");
    shutdown_all_sessions(Arc::clone(&state))
        .await
        .expect("shutdown all sessions again");

    assert!(
        !plugin_manager.has_session_vm(&slot.session_id, PLUGIN_ID),
        "shutdown should release session VMs"
    );
    assert!(
        slot.ctx
            .scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_none(),
        "shutdown should clear plugin event senders"
    );
}
