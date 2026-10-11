use super::to_model_name;

#[test]
fn short_names_remain_readable() {
    assert_eq!(
        to_model_name("playwright", "browser_take_screenshot"),
        "mcp__playwright__browser_take_screenshot"
    );
}

#[test]
fn long_names_keep_a_readable_head_and_unique_hash() {
    let name = to_model_name(
        "playwright",
        "navigate_and_wait_for_network_idle_then_capture_full_screenshot",
    );
    assert!(name.starts_with("mcp__playwright__navigate_and_wait"));
    assert_eq!(name.len(), 64);
    assert_ne!(
        name,
        to_model_name(
            "playwright",
            "navigate_and_wait_for_network_idle_then_capture_viewport_screenshot"
        )
    );
}

#[test]
fn underscore_in_server_and_tool_names_is_preserved() {
    assert_eq!(
        to_model_name("browser_tools", "take_screenshot"),
        "mcp__browser_tools__take_screenshot"
    );
}

#[test]
fn lossy_names_and_component_boundaries_remain_distinct() {
    for (left, right) in [
        (("source", "a.b"), ("source", "a_b")),
        (("a.b", "tool"), ("a_b", "tool")),
        (("a__b", "c"), ("a", "b__c")),
        (("a_", "b"), ("a", "_b")),
        (("source", "工具"), ("source", "__")),
        (("", "tool"), ("unnamed", "tool")),
    ] {
        let left_name = to_model_name(left.0, left.1);
        let right_name = to_model_name(right.0, right.1);
        assert_ne!(left_name, right_name, "{left:?} vs {right:?}");
        for name in [left_name, right_name] {
            assert!(name.len() <= 64);
            assert!(name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)));
        }
    }
}
