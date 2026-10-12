use serde_json::Value;

use super::super::catalog::all_tools_with_policy;
use crate::core::tools::contract::catalog::render_core_identity_tool_lines_with_policy;

fn names(values: &[Value]) -> std::collections::BTreeSet<String> {
    values
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn load_skill_can_be_hidden_by_policy() {
    let with_skill = names(&all_tools_with_policy(true));
    let without_skill = names(&all_tools_with_policy(false));
    assert!(with_skill.contains("load_skill"));
    assert!(!without_skill.contains("load_skill"));
}

#[test]
fn system_tool_lines_and_tools_array_list_the_same_tools() {
    for allow_load_skill in [false, true] {
        let from_system = render_core_identity_tool_lines_with_policy(allow_load_skill)
            .lines()
            .map(|line| {
                line.strip_prefix("- ")
                    .and_then(|line| line.split_once(':'))
                    .map(|(name, _)| name.to_string())
                    .expect("tool line has '- name: description' shape")
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(from_system, names(&all_tools_with_policy(allow_load_skill)));
    }
}
