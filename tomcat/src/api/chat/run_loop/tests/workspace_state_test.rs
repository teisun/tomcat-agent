use super::*;
use crate::core::permission::{DefaultPermissionGate, GateConfig, GrantTrigger};

#[test]
fn session_grant_changes_only_the_ephemeral_workspace_tail() {
    let grants = SessionGrants::new();
    let gate: Arc<dyn PermissionGate> = Arc::new(DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: PathBuf::from("/agent-definition"),
            workspace_roots: Vec::new(),
            agent_trail_readonly_dirs: Vec::new(),
            user_path_rules: Vec::new(),
            user_bash_forbidden: Vec::new(),
            user_bash_approval: Vec::new(),
            auto_confirm: false,
        },
        grants.clone(),
    ));
    let tail = RuntimeTailProvider {
        config: crate::AppConfig::default(),
        agent_definition_dir: PathBuf::from("/agent-definition"),
        agent_trail_dir: PathBuf::from("/agent-trail"),
        session_grants: grants.clone(),
        gate: Arc::clone(&gate),
        plan_runtime: crate::core::plan_runtime::PlanRuntime::new("test-session"),
    };
    let stable_system = crate::core::llm::system_prompt::build_system_prompt_with_skills(
        WorkspaceContext {
            agent_workspace_dir: "/agent-workspace".to_string(),
            agent_definition_dir: "/agent-definition".to_string(),
            agent_plans_dir: "/plans".to_string(),
            agent_trail_dir: "/agent-trail".to_string(),
            tool_lines: None,
        },
        None,
        None,
        400_000,
    );

    let before = tail.render_ephemeral_tail();
    gate.grant_session(
        PathBuf::from("/newly-authorized"),
        GrantTrigger::UserConfirm,
    );
    let after = tail.render_ephemeral_tail();

    assert_eq!(
        stable_system,
        crate::core::llm::system_prompt::build_system_prompt_with_skills(
            WorkspaceContext {
                agent_workspace_dir: "/agent-workspace".to_string(),
                agent_definition_dir: "/agent-definition".to_string(),
                agent_plans_dir: "/plans".to_string(),
                agent_trail_dir: "/agent-trail".to_string(),
                tool_lines: None,
            },
            None,
            None,
            400_000,
        )
    );
    assert!(!before.contains("/newly-authorized"));
    assert!(after.contains("/newly-authorized"));
    assert!(after.contains("<system_reminder kind=\"workspace_state\">"));
}
