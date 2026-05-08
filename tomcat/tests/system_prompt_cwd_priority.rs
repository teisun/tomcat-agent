//! E2E-PROMPT-025-offline：system prompt 中 agent_workspace_dir 是“当前目录”的唯一来源。

use tomcat::core::llm::system_prompt::{
    build_system_prompt_with_state, WorkspaceContext, WorkspaceRootDescriptor, WorkspaceState,
};

#[test]
fn system_prompt_names_three_directories_and_keeps_state_as_permission_list() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_workspace_dir = tmp.path().join("project");
    let agent_definition_dir = tmp.path().join(".tomcat").join("workspace-main");
    let agent_trail_dir = tmp.path().join(".tomcat").join("agents").join("main");
    std::fs::create_dir_all(&agent_workspace_dir).unwrap();
    std::fs::create_dir_all(&agent_definition_dir).unwrap();
    std::fs::create_dir_all(&agent_trail_dir).unwrap();

    let context = WorkspaceContext {
        agent_workspace_dir: agent_workspace_dir.to_string_lossy().to_string(),
        agent_definition_dir: agent_definition_dir.to_string_lossy().to_string(),
        agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
    };
    let state = WorkspaceState {
        read_write: vec![WorkspaceRootDescriptor {
            path: agent_definition_dir.to_string_lossy().to_string(),
            label: "agent_definition_dir".into(),
            alias: None,
            description: None,
        }],
        read_only: vec![WorkspaceRootDescriptor {
            path: agent_trail_dir.to_string_lossy().to_string(),
            label: "agent_trail_dir".into(),
            alias: None,
            description: None,
        }],
        path_rules: vec![],
    };

    let prompt = build_system_prompt_with_state(context, state);

    assert!(prompt.contains("Agent workspace directory (agent_workspace_dir):"));
    assert!(prompt.contains("current directory"));
    assert!(prompt.contains("this project"));
    assert!(prompt.contains("relative paths"));
    assert!(prompt.contains("NOT automatically authorized"));
    assert!(prompt.contains("Agent definition directory (agent_definition_dir):"));
    assert!(prompt.contains("Permission: read/write"));
    assert!(prompt.contains("Do NOT treat it as the user's current directory"));
    assert!(prompt.contains("Agent trail directory (agent_trail_dir):"));
    assert!(prompt.contains("Permission: read-only"));
    assert!(!prompt.contains("## Current Working Directory"));
    assert!(!prompt.contains("Agent runtime trail:"));
    assert!(!prompt.contains("cwd_snapshot"));
    assert!(!prompt.contains("agent_workspace_definition"));
    assert!(prompt.contains("[agent_definition_dir]"));
    assert!(!prompt.contains("[agent_workspace_dir]"));
}
