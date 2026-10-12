use super::*;

#[tokio::test]
async fn failed_capture_releases_the_completed_foreground_task() {
    use crate::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};
    use crate::core::AllowAllConfirmation;
    use crate::infra::{PrimitiveConfig, TracingAuditRecorder};
    let dir = tempfile::tempdir().unwrap();
    let executor = DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        DefaultPermissionGate::new(
            GateConfig {
                agent_definition_dir: dir.path().to_path_buf(),
                workspace_roots: vec![],
                agent_trail_readonly_dirs: vec![],
                user_path_rules: vec![],
                user_bash_forbidden: vec![],
                user_bash_approval: vec![],
                auto_confirm: true,
            },
            SessionGrants::new(),
        )
        .into_arc(),
    );
    let registry = Arc::new(BashTaskRegistry::new(dir.path().to_path_buf()));
    let ticket = registry
        .spawn_tracked("echo drained".into(), None, false)
        .await
        .unwrap();
    registry.wait_for_finish(&ticket.task_id).await.unwrap();
    let (sender, captured) = tokio::sync::oneshot::channel();
    sender
        .send(Err(std::io::Error::other("capture fixture failed")))
        .unwrap();
    let result = finalize_foreground_result(
        &executor,
        &registry,
        &ticket.task_id,
        &ticket.log_path,
        Instant::now(),
        false,
        captured,
    )
    .await;
    assert!(
        matches!(result, Err(AppError::Io(_))),
        "capture failure must remain visible"
    );
    assert!(
        registry.get_info(&ticket.task_id).is_none(),
        "an errored foreground receipt has no background owner"
    );
    assert!(
        Path::new(&ticket.log_path).exists(),
        "keep the diagnostic log"
    );
}

#[test]
fn foreground_wait_clamps_to_contract() {
    struct Values;
    assert_eq!(crate::infra::MIN_TOOLS_BASH_FOREGROUND_WAIT_MS, 8_000);
    assert_eq!(crate::infra::MAX_TOOLS_BASH_FOREGROUND_WAIT_MS, 16_000);
    let _ = Values;
}
