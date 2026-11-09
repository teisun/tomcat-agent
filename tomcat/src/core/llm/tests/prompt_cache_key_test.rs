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

#[test]
fn cache_key_preserves_short_keys_and_hashes_overlong_keys() {
    let boundary_session = "s".repeat(59);
    let boundary_key = format!("{boundary_session}:main");
    assert_eq!(boundary_key.len(), 64);
    assert_eq!(
        PromptCacheKeyFamily::Main.key_for(&format!("  {boundary_session} \n")),
        Some(boundary_key)
    );
    assert_eq!(
        PromptCacheKeyFamily::Main.key_for(" session-1 ").as_deref(),
        Some("session-1:main")
    );

    let unicode_session = "会".repeat(20);
    assert!(format!("{unicode_session}:main").chars().count() <= 64);
    for session_id in ["s".repeat(60), unicode_session] {
        let key = PromptCacheKeyFamily::Main.key_for(&session_id).unwrap();
        assert_eq!(key.len(), 64);
        assert!(key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(
            PromptCacheKeyFamily::Main.key_for(&format!(" {session_id} ")),
            Some(key)
        );
    }
}

#[test]
fn cache_key_for_incident_reviewer_matches_stable_digest() {
    let session_id = "1794181249645_c1d4b501cebd5921-child-plan_reviewer-5";
    let family = PromptCacheKeyFamily::Subagent("plan_reviewer");
    assert_eq!(format!("{session_id}:subagent:plan_reviewer").len(), 75);
    assert_eq!(
        family.key_for(session_id).as_deref(),
        Some("36231c7a48f11e4f224dfde22cba07c0d50ceeebc25d6c2539a988e0f794b4bc")
    );
    assert_eq!(family.key_for(session_id), family.key_for(session_id));
}

#[test]
fn cache_key_hash_preserves_session_suffixes_and_request_families() {
    let prefix = "s".repeat(100);
    let sessions = [format!("{prefix}a"), format!("{prefix}b")];
    let long_kind = "kind".repeat(30);
    let kinds = [format!("{long_kind}a"), format!("{long_kind}b")];
    let families = [
        PromptCacheKeyFamily::Main,
        PromptCacheKeyFamily::Subagent("plan_reviewer"),
        PromptCacheKeyFamily::Subagent("verifier"),
        PromptCacheKeyFamily::Compaction,
        PromptCacheKeyFamily::Title,
        PromptCacheKeyFamily::Extension,
        PromptCacheKeyFamily::Subagent(&kinds[0]),
        PromptCacheKeyFamily::Subagent(&kinds[1]),
    ];
    let mut keys = HashSet::new();
    for session_id in &sessions {
        for family in families {
            let key = family.key_for(session_id).unwrap();
            assert_eq!(key.len(), 64);
            assert!(keys.insert(key.clone()), "distinct input lost its scope");
            assert_eq!(family.key_for(session_id), Some(key));
        }
    }
    assert_eq!(keys.len(), sessions.len() * families.len());
}
