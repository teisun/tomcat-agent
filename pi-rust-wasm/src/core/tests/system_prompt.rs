use super::super::system_prompt::*;

#[test]
fn build_system_prompt_contains_tools_and_workspace() {
    let prompt = build_system_prompt("/home/user/workspace");
    assert!(prompt.contains("read_file"));
    assert!(prompt.contains("write_file"));
    assert!(prompt.contains("edit_file"));
    assert!(prompt.contains("execute_bash"));
    assert!(prompt.contains("list_dir"));
    assert!(prompt.contains("/home/user/workspace"));
    assert!(prompt.contains("coding assistant"));
}

#[test]
fn build_system_prompt_contains_current_time() {
    let prompt = build_system_prompt("/tmp");
    assert!(prompt.contains("Current date and time:"));
    assert!(prompt.contains("Agent workspace definition:"));
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
        fn render(&self, _: &str) -> String {
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
        fn render(&self, _: &str) -> String {
            "LOW".to_string()
        }
        fn priority(&self) -> u32 {
            999
        }
    }

    let mut builder = SystemPromptBuilder::new();
    builder.register(Box::new(LowPriority));
    builder.register(Box::new(HighPriority));
    let output = builder.build("/tmp");
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
        fn render(&self, _: &str) -> String {
            "CUSTOM_CONTENT".to_string()
        }
    }

    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(CustomSection));
    let output = builder.build("/tmp");
    assert!(output.contains("CUSTOM_CONTENT"));
}

// ── WorkspaceStateSection（plan §8 / PR-8） ──────────────────────────────────

fn fixture_state() -> WorkspaceState {
    WorkspaceState {
        cwd: String::new(),
        read_write: vec![
            WorkspaceRootDescriptor {
                path: "/Users/yan/proj".into(),
                label: "agent_workspace_definition".into(),
                alias: None,
                description: None,
            },
            WorkspaceRootDescriptor {
                path: "/Users/yan/scratch".into(),
                label: "extra_root".into(),
                alias: Some("scratch".into()),
                description: Some("用户附加根".into()),
            },
            WorkspaceRootDescriptor {
                path: "/tmp/dropped".into(),
                label: "dragged_path".into(),
                alias: None,
                description: None,
            },
        ],
        read_only: vec![WorkspaceRootDescriptor {
            path: "/Users/yan/.pi_/agents/main/sessions".into(),
            label: "agent_workspace_trail".into(),
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
        agent_data_dir: Some("/Users/yan/.pi_/agents/main".into()),
    }
}

#[test]
fn workspace_state_section_renders_read_write() {
    let s = WorkspaceStateSection::new(fixture_state()).render("/tmp");
    assert!(s.contains("Workspace State"));
    assert!(s.contains("/Users/yan/proj"));
    assert!(s.contains("[agent_workspace_definition]"));
    assert!(s.contains("/Users/yan/scratch"));
    assert!(s.contains("alias=scratch"));
    assert!(s.contains("desc=\"用户附加根\""));
    assert!(s.contains("/tmp/dropped"));
    assert!(s.contains("[dragged_path]"));
}

#[test]
fn workspace_state_section_renders_read_only_and_agent_dir() {
    let s = WorkspaceStateSection::new(fixture_state()).render("/tmp");
    assert!(s.contains("READ (but NOT write)"));
    assert!(s.contains("/Users/yan/.pi_/agents/main/sessions"));
    assert!(s.contains("[agent_workspace_trail]"));
    assert!(s.contains("Agent runtime trail"));
    assert!(s.contains("/Users/yan/.pi_/agents/main"));
}

#[test]
fn workspace_state_section_renders_path_rules_with_builtin_tag() {
    let s = WorkspaceStateSection::new(fixture_state()).render("/tmp");
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
    let s = WorkspaceStateSection::new(fixture_state()).render("/tmp");
    assert!(s.contains("config_get"));
    assert!(s.contains("config_set"));
    assert!(s.contains("DO NOT write to ~/.pi_/pi.config.toml"));
}

#[test]
fn workspace_state_section_handles_empty_state() {
    let st = WorkspaceState {
        cwd: String::new(),
        read_write: vec![],
        read_only: vec![],
        path_rules: vec![],
        agent_data_dir: None,
    };
    let s = WorkspaceStateSection::new(st).render("/tmp");
    assert!(s.contains("no read/write directories"));
    // 没有 read_only / path_rules / agent_data_dir 行
    assert!(!s.contains("READ (but NOT write)"));
    assert!(!s.contains("Path rules in effect:"));
    assert!(!s.contains("Agent data dir"));
    // 但 config_get 工具引导仍要保留
    assert!(s.contains("config_get"));
}

#[test]
fn build_system_prompt_with_state_includes_workspace_state() {
    let prompt = build_system_prompt_with_state("/home/user/workspace", fixture_state());
    assert!(prompt.contains("Workspace State"));
    assert!(prompt.contains("/Users/yan/proj"));
    // 默认 4 个 section + 新加的 1 个，仍包含工具说明
    assert!(prompt.contains("read_file"));
    assert!(prompt.contains("Current date and time"));
}

#[test]
fn workspace_state_priority_between_paged_reading_and_workspace_context() {
    let prompt = build_system_prompt_with_state("/tmp", fixture_state());
    // priority 顺序：core(10) < tool(20) < paged(30) < workspace_state(150) < workspace_ctx(200)
    let paged_pos = prompt.find("Tool result persisted").expect("paged section");
    let state_pos = prompt.find("Workspace State").expect("state section");
    let ctx_pos = prompt
        .find("Current date and time:")
        .expect("workspace ctx");
    assert!(paged_pos < state_pos, "state 应在 paged 之后");
    assert!(state_pos < ctx_pos, "state 应在 workspace ctx 之前");
}

// ── cwd 注入 system prompt（hotfix §A.0） ────────────────────────────────────

#[test]
fn workspace_state_section_renders_cwd_section_when_present() {
    let mut st = fixture_state();
    st.cwd = "/Users/yan/scratch/sub".into();
    let s = WorkspaceStateSection::new(st).render("/tmp");
    assert!(s.contains("## Current Working Directory"));
    assert!(s.contains("`/Users/yan/scratch/sub`"));
    assert!(
        s.contains("NOT yet authorized"),
        "cwd 不在 read_write 时应说明需授权"
    );
}

#[test]
fn workspace_state_section_renders_cwd_writable_when_in_rw_list() {
    let mut st = fixture_state();
    st.cwd = "/Users/yan/proj".into();
    let s = WorkspaceStateSection::new(st).render("/tmp");
    assert!(s.contains("## Current Working Directory"));
    assert!(
        s.contains("currently writable"),
        "cwd 在 read_write 时应给 LLM 明确授权信号"
    );
    assert!(!s.contains("NOT yet authorized"));
}

#[test]
fn workspace_state_section_renders_cwd_readonly_when_in_ro_list() {
    let mut st = fixture_state();
    st.cwd = "/Users/yan/.pi_/agents/main/sessions".into();
    let s = WorkspaceStateSection::new(st).render("/tmp");
    assert!(s.contains("## Current Working Directory"));
    assert!(s.contains("currently read-only"));
}

#[test]
fn workspace_state_section_skips_cwd_when_empty() {
    let st = fixture_state();
    let s = WorkspaceStateSection::new(st).render("/tmp");
    assert!(!s.contains("## Current Working Directory"));
}

#[test]
fn workspace_state_section_cwd_appears_before_workspace_state() {
    let mut st = fixture_state();
    st.cwd = "/some/cwd/path".into();
    let s = WorkspaceStateSection::new(st).render("/tmp");
    let cwd_pos = s.find("## Current Working Directory").expect("cwd section");
    let ws_pos = s.find("## Workspace State").expect("workspace state");
    assert!(cwd_pos < ws_pos, "cwd 段必须在 workspace state 之前");
}

#[test]
fn workspace_state_section_skips_duplicate_agent_data_dir() {
    // 当 agent_data_dir 已经在 read_only 列表中时，不重复输出独立行。
    let mut st = fixture_state();
    let dup = "/Users/yan/.pi_/agents/main/agent";
    st.read_only.push(WorkspaceRootDescriptor {
        path: dup.to_string(),
        label: "agent_data_dir".into(),
        alias: None,
        description: None,
    });
    let s = WorkspaceStateSection::new(st).render("/tmp");
    let occurrences = s.matches(dup).count();
    assert_eq!(occurrences, 1, "agent_data_dir 路径只应出现一次");
}
