use super::*;

#[tokio::test]
async fn io_receipt_waits_for_actual_drop_and_seals_unstarted_requests() {
    let active = Arc::new(CallIo::default());
    let guard = active.begin().unwrap();
    let released = {
        let active = active.clone();
        tokio::spawn(async move { active.wait_released().await })
    };
    tokio::task::yield_now().await;
    assert!(!released.is_finished());
    drop(guard);
    released.await.unwrap();
    assert!(active.begin().is_none());
    let unstarted = Arc::new(CallIo::default());
    unstarted.wait_released().await;
    assert!(unstarted.begin().is_none());
}

#[test]
fn rpc_errors_keep_the_code_without_disclosing_remote_message_or_data() {
    let failure = CallFailure::Sdk(ServiceError::McpError(rmcp::ErrorData::invalid_params(
        "SENTINEL_PRIVATE_ERROR",
        Some(serde_json::json!({"token":"SENTINEL_PRIVATE_DATA"})),
    )));
    assert!(!failure.session_expired());
    let message = failure.to_app_error("mcp__fixture__echo").to_string();
    assert!(message.contains("-32602"));
    assert!(!message.contains("SENTINEL"));
}
