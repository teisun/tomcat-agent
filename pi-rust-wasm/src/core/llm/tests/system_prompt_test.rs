use super::super::system_prompt::*;

#[test]
fn build_system_prompt_contains_tools_and_workspace() {
    let prompt = build_system_prompt("/home/user/workspace");
    assert!(prompt.contains("read"));
    assert!(!prompt.contains("read_file"));
    assert!(prompt.contains("write"));
    assert!(!prompt.contains("write_file"));
    assert!(prompt.contains("edit"));
    assert!(!prompt.contains("edit_file"));
    assert!(prompt.contains("execute_bash"));
    assert!(prompt.contains("list_dir"));
    assert!(prompt.contains("search_files"));
    assert!(prompt.contains("config_get"));
    assert!(prompt.contains("config_set"));
    assert!(prompt.contains("/home/user/workspace"));
    assert!(prompt.contains("coding assistant"));
}

#[test]
fn build_system_prompt_contains_current_time() {
    let prompt = build_system_prompt("/tmp");
    assert!(prompt.contains("Current date and time:"));
    assert!(prompt.contains("Agent workspace directory"));
}

#[test]
fn build_system_prompt_contains_anti_hallucination_constraint() {
    let prompt = build_system_prompt("/tmp");
    assert!(
        prompt.contains("Only claim you can access"),
        "system prompt 应包含防幻觉约束"
    );
}

#[test]
fn build_system_prompt_prefers_search_files_over_bash_search() {
    let prompt = build_system_prompt("/tmp");
    assert!(
        prompt.contains("prefer it over execute_bash with grep/find/ls -R"),
        "system prompt 应引导模型优先使用 search_files，而不是 bash 搜索"
    );
}

#[test]
fn build_system_prompt_contains_paged_reading_guide() {
    let prompt = build_system_prompt("/tmp");
    assert!(
        prompt.contains("Tool result persisted"),
        "system prompt should contain paged reading guide"
    );
}

#[test]
fn builder_sections_ordered_by_priority() {
    struct HighPriority;
    impl SystemPromptSection for HighPriority {
        fn section_name(&self) -> &str {
            "high"
        }
        fn render(&self, _: &WorkspaceContext) -> String {
            "HIGH".to_string()
        }
        fn priority(&self) -> u32 {
            1
        }
    }
    struct LowPriority;
    impl SystemPromptSection for LowPriority {
        fn section_name(&self) -> &str {
            "low"
        }
        fn render(&self, _: &WorkspaceContext) -> String {
            "LOW".to_string()
        }
        fn priority(&self) -> u32 {
            999
        }
    }

    let mut builder = SystemPromptBuilder::new();
    builder.register(Box::new(LowPriority));
    builder.register(Box::new(HighPriority));
    let output = builder.build(&fixture_context());
    let high_pos = output.find("HIGH").unwrap();
    let low_pos = output.find("LOW").unwrap();
    assert!(high_pos < low_pos, "HIGH should come before LOW");
}

#[test]
fn custom_section_appears_in_output() {
    struct CustomSection;
    impl SystemPromptSection for CustomSection {
        fn section_name(&self) -> &str {
            "custom"
        }
        fn render(&self, _: &WorkspaceContext) -> String {
            "CUSTOM_CONTENT".to_string()
        }
    }

    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(CustomSection));
    let output = builder.build(&fixture_context());
    assert!(output.contains("CUSTOM_CONTENT"));
}

// ── WorkspaceStateSection（plan §8 / PR-8） ──────────────────────────────────

fn fixture_state() -> WorkspaceState {
    WorkspaceState {
        read_write: vec![
            WorkspaceRootDescriptor {
                path: "/Users/yan/.pi_/workspace-main".into(),
                label: "agent_definition_dir".into(),
                alias: None,
                description: None,
            },
            WorkspaceRootDescriptor {
                path: "/Users/yan/scratch".into(),
                label: "agent_workspace_root".into(),
                alias: Some("scratch".into()),
                description: Some("用户附加根".into()),
            },
        ],
        read_only: vec![WorkspaceRootDescriptor {
            path: "/Users/yan/.pi_/agents/main/sessions".into(),
            label: "agent_trail_dir".into(),
            alias: None,
            description: None,
        }],
        path_rules: vec![
            PathRuleSummary {
                path: "/etc".into(),
                mode: "deny".into(),
                builtin: true,
            },
            PathRuleSummary {
                path: "/Users/yan/secrets".into(),
                mode: "deny".into(),
                builtin: false,
            },
            PathRuleSummary {
                path: "/Users/yan/refs".into(),
                mode: "readonly".into(),
                builtin: false,
            },
        ],
    }
}

fn fixture_context() -> WorkspaceContext {
    WorkspaceContext {
        agent_workspace_dir: "/Users/yan/proj".into(),
        agent_definition_dir: "/Users/yan/.pi_/workspace-main".into(),
        agent_trail_dir: "/Users/yan/.pi_/agents/main".into(),
    }
}

#[test]
fn workspace_state_section_renders_read_write() {
    let s = WorkspaceStateSection::new(fixture_state()).render(&fixture_context());
    assert!(s.contains("Workspace State"));
    assert!(s.contains("/Users/yan/.pi_/workspace-main"));
    assert!(s.contains("[agent_definition_dir]"));
    assert!(!s.contains("[agent_workspace_dir]"));
    assert!(s.contains("/Users/yan/scratch"));
    assert!(s.contains("alias=scratch"));
    assert!(s.contains("desc=\"用户附加根\""));
    assert!(!s.contains("[dragged_path]"));
}

#[test]
fn workspace_state_section_renders_read_only_and_agent_dir() {
    let s = WorkspaceStateSection::new(fixture_state()).render(&fixture_context());
    assert!(s.contains("READ (but NOT write)"));
    assert!(s.contains("/Users/yan/.pi_/agents/main/sessions"));
    assert!(s.contains("[agent_trail_dir]"));
    assert!(!s.contains("Agent runtime trail"));
}

#[test]
fn workspace_state_section_renders_path_rules_with_builtin_tag() {
    let s = WorkspaceStateSection::new(fixture_state()).render(&fixture_context());
    assert!(s.contains("Path rules in effect:"));
    let deny_line = s
        .lines()
        .find(|l| l.contains("deny:"))
        .expect("should have deny line");
    assert!(deny_line.contains("/etc [builtin]"));
    assert!(deny_line.contains("/Users/yan/secrets"));
    // 用户自定义不带 [builtin]
    assert!(!deny_line.contains("/Users/yan/secrets [builtin]"));
    let ro_line = s
        .lines()
        .find(|l| l.contains("readonly:"))
        .expect("should have readonly line");
    assert!(ro_line.contains("/Users/yan/refs"));
}

#[test]
fn workspace_state_section_mentions_config_tools() {
    let s = WorkspaceStateSection::new(fixture_state()).render(&fixture_context());
    assert!(s.contains("config_get"));
    assert!(s.contains("config_set"));
    assert!(s.contains("DO NOT write to ~/.pi_/pi.config.toml"));
}

#[test]
fn workspace_state_section_handles_empty_state() {
    let st = WorkspaceState {
        read_write: vec![],
        read_only: vec![],
        path_rules: vec![],
    };
    let s = WorkspaceStateSection::new(st).render(&fixture_context());
    assert!(s.contains("no read/write directories"));
    // 没有 read_only / path_rules 行
    assert!(!s.contains("READ (but NOT write)"));
    assert!(!s.contains("Path rules in effect:"));
    assert!(!s.contains("Agent trail dir"));
    // 但 config_get 工具引导仍要保留
    assert!(s.contains("config_get"));
}

#[test]
fn build_system_prompt_with_state_includes_workspace_state() {
    let prompt = build_system_prompt_with_state(fixture_context(), fixture_state());
    assert!(prompt.contains("Workspace State"));
    assert!(prompt.contains("/Users/yan/.pi_/workspace-main"));
    assert!(prompt.contains("Agent workspace directory (agent_workspace_dir): /Users/yan/proj"));
    // 默认 4 个 section + 新加的 1 个，仍包含工具说明
    assert!(prompt.contains("read"));
    assert!(!prompt.contains("read_file"));
    assert!(prompt.contains("edit"));
    assert!(!prompt.contains("edit_file"));
    assert!(prompt.contains("Current date and time"));
}

#[test]
fn workspace_state_priority_between_paged_reading_and_workspace_context() {
    let prompt = build_system_prompt_with_state(fixture_context(), fixture_state());
    // priority 顺序：core(10) < tool(20) < paged(30) < workspace_state(150) < workspace_ctx(200)
    let paged_pos = prompt.find("Tool result persisted").expect("paged section");
    let state_pos = prompt.find("Workspace State").expect("state section");
    let ctx_pos = prompt
        .find("Current date and time:")
        .expect("workspace ctx");
    assert!(paged_pos < state_pos, "state 应在 paged 之后");
    assert!(state_pos < ctx_pos, "state 应在 workspace ctx 之前");
}

#[test]
fn workspace_context_section_describes_three_directories() {
    let prompt = build_system_prompt_with_state(fixture_context(), fixture_state());
    assert!(prompt.contains("Agent workspace directory (agent_workspace_dir): /Users/yan/proj"));
    assert!(prompt.contains("current directory"));
    assert!(prompt.contains("NOT automatically authorized"));
    assert!(prompt.contains("Agent definition directory (agent_definition_dir):"));
    assert!(prompt.contains("Permission: read/write"));
    assert!(prompt.contains("Agent trail directory (agent_trail_dir):"));
    assert!(prompt.contains("Permission: read-only"));
    assert!(prompt.contains("Do NOT treat it as the user's current directory"));
}
