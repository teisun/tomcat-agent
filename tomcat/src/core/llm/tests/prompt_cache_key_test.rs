use std::collections::HashSet;

use super::super::PromptCacheKeyFamily;

#[test]
fn cache_key_families_are_mutually_distinct() {
    let families = [
        PromptCacheKeyFamily::Main,
        PromptCacheKeyFamily::Subagent("plan_reviewer"),
        PromptCacheKeyFamily::Compaction,
        PromptCacheKeyFamily::Title,
        PromptCacheKeyFamily::Extension,
    ];
    let keys = families
        .into_iter()
        .map(|family| family.key_for("session-1").expect("non-empty session id"))
        .collect::<HashSet<_>>();
    assert_eq!(keys.len(), families.len());
}

#[test]
fn cache_key_is_none_without_session_id() {
    assert_eq!(PromptCacheKeyFamily::Main.key_for("  "), None);
}
