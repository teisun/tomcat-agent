//! E2E-EXEC-024：bash assignment RHS 必须进入 PermissionGate 路径预检。

use std::sync::Arc;

use pi_wasm::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};
use pi_wasm::{
    AllowAllConfirmation, AppError, DefaultPrimitiveExecutor, PrimitiveConfig, PrimitiveExecutor,
    TracingAuditRecorder,
};

fn make_executor(
    agent_definition_dir: &std::path::Path,
    denied: &std::path::Path,
) -> DefaultPrimitiveExecutor {
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: agent_definition_dir.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                denied.to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    );

    DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        gate.into_arc(),
    )
}

#[tokio::test]
async fn bash_assignment_rhs_denied_in_all_supported_positions() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_def_dir = tmp.path().join("workspace-temp");
    let denied_dir = tmp.path().join("deny-target");
    let denied = denied_dir.join("foo");
    std::fs::create_dir_all(&agent_def_dir).unwrap();
    std::fs::create_dir_all(&denied_dir).unwrap();
    std::fs::write(&denied, "secret").unwrap();

    let exec = make_executor(&agent_def_dir, &denied);
    for command in [
        format!("stat -c %s p={}", denied.display()),
        format!("p={} ls -la \"$p\"", denied.display()),
        format!("p={}; ls -la \"$p\"", denied.display()),
    ] {
        let err = exec
            .execute_bash(&command, None, "__test__", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::Permission(_)),
            "{command} should be denied, got {err:?}"
        );
        assert!(
            err.to_string()
                .contains(&denied.to_string_lossy().to_string()),
            "error should mention denied RHS path: {err}"
        );
    }
}
