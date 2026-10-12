use super::*;
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;

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
async fn dispatch_command_returns_error_frame_and_keeps_loop_alive_after_handler_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    state.registry.remove(&slot.session_id);

    dispatch_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("get-state-missing".to_string()),
            session_id: None,
        },
    )
    .await
    .unwrap();

    let after_error = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("get-state-missing")
    })
    .await;
    let error_response = after_error
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("get-state-missing")
        })
        .expect("error response");
    assert_eq!(error_response["success"].as_bool(), Some(false));
    assert_eq!(error_response["error"].as_str(), Some("unknown_session"));

    dispatch_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("new-session-after-error".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();

    let after_success = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("new-session-after-error")
    })
    .await;
    let success_response = after_success
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("new-session-after-error")
        })
        .expect("success response");
    assert_eq!(success_response["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn interrupt_bypasses_a_backlogged_normal_command_queue() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let (normal_tx, mut normal_rx) = tokio::sync::mpsc::unbounded_channel();

    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::GetMessages {
            id: Some("heavy-history".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(128),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let queued = normal_rx.try_recv().expect("普通命令必须进入 FIFO worker");
    normal_tx.send(queued).unwrap();

    let started = std::time::Instant::now();
    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::Interrupt {
            id: Some("stop-now".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "Stop 不得等待前面的重命令"
    );
    let frames = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("stop-now")
    })
    .await;
    assert!(frames.iter().any(|line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("stop-now")
            && line.get("success").and_then(serde_json::Value::as_bool) == Some(true)
    }));
}

#[tokio::test]
#[serial(env_lock)]
async fn interrupt_cancels_the_target_session_while_three_heavy_commands_are_queued() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let (normal_tx, mut normal_rx) = tokio::sync::mpsc::unbounded_channel();
    let first_command_started = Arc::new(tokio::sync::Notify::new());
    let started = Arc::clone(&first_command_started);
    let worker_state = Arc::clone(&state);
    let worker = tokio::spawn(async move {
        let mut first = true;
        while let Some(command) = normal_rx.recv().await {
            if first {
                first = false;
                started.notify_one();
            }
            // Model a genuinely expensive normal-lane request. Keeping the worker asleep
            // makes the test prove that interrupt does not merely jump an idle receiver.
            tokio::time::sleep(Duration::from_secs(1)).await;
            dispatch_command(Arc::clone(&worker_state), command)
                .await
                .unwrap();
        }
    });

    for index in 0..3 {
        route_parsed_command(
            Arc::clone(&state),
            &normal_tx,
            ServeCommand::GetMessages {
                id: Some(format!("heavy-history-{index}")),
                session_id: Some(slot.session_id.clone()),
                params: GetMessagesParams::default(),
            },
        )
        .await
        .unwrap();
    }
    tokio::time::timeout(Duration::from_millis(100), first_command_started.notified())
        .await
        .expect("the first heavy command must occupy the FIFO worker");

    let started = std::time::Instant::now();
    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::Interrupt {
            id: Some("cancel-through-backlog".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "interrupt must not wait for the three queued one-second commands"
    );
    assert!(
        slot.ctx.session_runtime.cancel_token.lock().is_cancelled(),
        "interrupt must cancel the target session, not merely acknowledge the request"
    );
    worker.abort();
    let _ = worker.await;
}

#[tokio::test]
#[serial(env_lock)]
async fn normal_commands_keep_prompt_get_messages_get_state_fifo_order() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let (normal_tx, mut normal_rx) = tokio::sync::mpsc::unbounded_channel::<ServeCommand>();
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let worker = tokio::spawn(async move {
        while let Some(command) = normal_rx.recv().await {
            observed_tx.send(command.wire_type().to_string()).unwrap();
        }
    });

    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::Prompt {
            id: Some("prompt-first".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "first".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::GetMessages {
            id: Some("messages-second".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams::default(),
        },
    )
    .await
    .unwrap();
    route_parsed_command(
        Arc::clone(&state),
        &normal_tx,
        ServeCommand::GetState {
            id: Some("state-third".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let actual = [
        observed_rx.recv().await.unwrap(),
        observed_rx.recv().await.unwrap(),
        observed_rx.recv().await.unwrap(),
    ];
    assert_eq!(actual, ["prompt", "get_messages", "get_state"]);
    drop(normal_tx);
    worker.await.unwrap();
}
