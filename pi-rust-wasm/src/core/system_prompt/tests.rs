use super::*;

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
    assert!(prompt.contains("Current working directory:"));
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
