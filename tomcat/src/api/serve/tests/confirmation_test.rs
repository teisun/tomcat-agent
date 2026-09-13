use std::path::PathBuf;
use std::time::Duration;

use crate::api::serve::confirmation::ServeConfirmationBridge;
use crate::api::serve::test_support::{read_ndjson_lines, spawn_buffered_writer};
use crate::api::serve::types::ControlFrame;
use crate::core::tools::contract::confirmation::ConfirmDecision;
use crate::core::tools::primitive::PrimitiveOperation;
use crate::ServeConfig;

async fn confirmation_request_id(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
) -> String {
    for _ in 0..50 {
        if let Some(request_id) = read_ndjson_lines(buffer).iter().find_map(|line| {
            (line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
                && line.get("subtype").and_then(serde_json::Value::as_str) == Some("confirmation"))
            .then(|| line.get("requestId")?.as_str().map(str::to_owned))
            .flatten()
        }) {
            return request_id;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("confirmation control request was not emitted");
}

#[tokio::test]
async fn serve_confirmation_rejects_wrong_session_and_round_trips_scoped_decision() {
    let (writer, buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeConfirmationBridge::new(writer);
    let provider = bridge.provider_for_session("confirmation-session");
    let request = tokio::spawn(async move {
        provider
            .confirm_decision(
                PrimitiveOperation::Read,
                "Read package source\n路径: /tmp/source/SKILL.md",
                "package_install_source",
                Some(PathBuf::from("/tmp/source")),
            )
            .await
    });
    let request_id = confirmation_request_id(&buffer).await;

    assert!(!bridge
        .handle_control_response(&ControlFrame::response(
            request_id.clone(),
            Some("another-session".to_string()),
            serde_json::json!({ "decision": "allow_once" }),
        ))
        .unwrap());
    assert!(bridge
        .handle_control_response(&ControlFrame::response(
            request_id,
            Some("confirmation-session".to_string()),
            serde_json::json!({
                "decision": "allow_and_persist_root",
                "root": "/tmp/source"
            }),
        ))
        .unwrap());
    assert_eq!(
        request.await.unwrap().unwrap(),
        ConfirmDecision::AllowAndPersistRoot {
            root: PathBuf::from("/tmp/source")
        }
    );
}

#[tokio::test]
async fn serve_confirmation_rejects_missing_session_without_consuming_request() {
    let (writer, buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeConfirmationBridge::new(writer);
    let provider = bridge.provider_for_session("confirmation-session");
    let request = tokio::spawn(async move {
        provider
            .confirm(
                PrimitiveOperation::Read,
                "Read package source",
                "package_install_source",
            )
            .await
    });
    let request_id = confirmation_request_id(&buffer).await;

    assert!(!bridge
        .handle_control_response(&ControlFrame::response(
            request_id.clone(),
            None,
            serde_json::json!({ "decision": "allow_once" }),
        ))
        .unwrap());
    assert!(bridge
        .handle_control_response(&ControlFrame::response(
            request_id,
            Some("confirmation-session".to_string()),
            serde_json::json!({ "decision": "allow_once" }),
        ))
        .unwrap());
    assert!(request.await.unwrap().unwrap());
}

#[tokio::test]
async fn serve_confirmation_denies_persisted_root_that_differs_from_request() {
    let (writer, buffer) = spawn_buffered_writer(&ServeConfig::default());
    let bridge = ServeConfirmationBridge::new(writer);
    let provider = bridge.provider_for_session("confirmation-session");
    let request = tokio::spawn(async move {
        provider
            .confirm_decision(
                PrimitiveOperation::Read,
                "Read package source",
                "package_install_source",
                Some(PathBuf::from("/tmp/source")),
            )
            .await
    });
    let request_id = confirmation_request_id(&buffer).await;

    assert!(bridge
        .handle_control_response(&ControlFrame::response(
            request_id,
            Some("confirmation-session".to_string()),
            serde_json::json!({
                "decision": "allow_and_persist_root",
                "root": "/tmp/other-root"
            }),
        ))
        .unwrap());
    assert_eq!(request.await.unwrap().unwrap(), ConfirmDecision::Deny);
}
