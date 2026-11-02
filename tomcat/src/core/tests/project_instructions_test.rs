use super::*;
use crate::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};
fn gate() -> DefaultPermissionGate {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: PathBuf::from("/unused"),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
}
fn put(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
#[test]
fn metadata_strict_boolean_and_crlf() {
    assert_eq!(
        parse_document("---\r\ndescription: a\r\nalwaysApply: true\r\n---\r\nbody").unwrap(),
        ("a".into(), true, "body")
    );
    for text in [
        "body",
        "---\nalwaysApply: false\n---\nbody",
        "---\nalwaysApply: \"true\"\n---\nbody",
    ] {
        assert!(!parse_document(text).unwrap().1);
    }
    assert!(parse_document("---\ndescription: [\n---\nbody").is_err());
}
#[test]
fn discovers_all_sources_and_duplicates_without_granting_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    assert!(matches!(
        gate.check(PrimitiveOperation::Read, root.to_str().unwrap())
            .unwrap(),
        PermissionDecision::NeedConfirm { .. }
    ));
    for path in [
        ".cursor/commands/git/review.md",
        ".agents/commands/review.md",
        ".data/commands/review.md",
    ] {
        put(
            &root,
            path,
            "---\nname: ignored\ndescription: Review\n---\nbody",
        );
    }
    put(&root, ".cursor/commands/.drafts/x.md", "hidden");
    let d = discover(&root, ".data", false, &gate);
    assert_eq!(d.files.len(), 3);
    assert_eq!(d.files[0].card.name, "git/review");
    assert_eq!(d.files[0].card.id, "command:.cursor/commands/git/review.md");
    gate.grant_path_rule(PathRule {
        path: root.join(".agents").to_string_lossy().into_owned(),
        mode: PathRuleMode::Deny,
    });
    assert_eq!(discover(&root, ".data", false, &gate).files.len(), 2);
}
#[test]
fn rules_extensions_refresh_budget_and_deny() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    for ext in ["md", "mdc"] {
        put(
            &root,
            &format!(".cursor/rules/a.{ext}"),
            "---\nalwaysApply: true\ndescription: hello\n---\nrule body",
        );
    }
    put(&root, ".agents/rules/off.md", "body");
    let d = discover(&root, ".agents", true, &gate);
    let (text, errors, count) = render_rules(&d, &root, &gate, 10000);
    assert_eq!(count, 2);
    assert!(text.contains("Description: hello"));
    assert!(errors.iter().any(|e| e.contains("alwaysApply")));
    assert_eq!(render_rules(&d, &root, &gate, 1).2, 0);
    gate.grant_path_rule(PathRule {
        path: root.join(".cursor").to_string_lossy().into_owned(),
        mode: PathRuleMode::Deny,
    });
    assert_eq!(render_rules(&d, &root, &gate, 10000).2, 0);
}
#[test]
fn byte_limit_empty_and_invalid_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    put(&root, ".agents/commands/a.md", &"a".repeat(MAX_FILE_BYTES));
    let path = root.join(".agents/commands/a.md");
    assert_eq!(
        read_body(&root, &path, &gate).unwrap().len(),
        MAX_FILE_BYTES
    );
    fs::write(&path, "a".repeat(MAX_FILE_BYTES + 1)).unwrap();
    assert!(read_body(&root, &path, &gate).is_err());
    fs::write(&path, [255]).unwrap();
    assert!(read_body(&root, &path, &gate).is_err());
    fs::write(&path, " ").unwrap();
    assert!(read_body(&root, &path, &gate).is_err());
}
#[cfg(unix)]
#[test]
fn rejects_leaf_and_ancestor_links() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    put(&root, "real/commands/x.md", "body");
    std::os::unix::fs::symlink(root.join("real"), root.join(".cursor")).unwrap();
    assert!(discover(&root, ".agents", false, &gate).files.is_empty());
    fs::create_dir_all(root.join(".agents/commands")).unwrap();
    std::os::unix::fs::symlink(
        root.join("real/commands/x.md"),
        root.join(".agents/commands/x.md"),
    )
    .unwrap();
    assert!(discover(&root, ".agents", false, &gate).files.is_empty());
}
#[test]
fn deny_is_checked_before_invalid_file_content_and_metadata_are_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    let path = root.join(".cursor/commands/secret.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [255]).unwrap();
    gate.grant_path_rule(PathRule {
        path: path.to_string_lossy().into_owned(),
        mode: PathRuleMode::Deny,
    });
    let d = discover(&root, ".agents", false, &gate);
    assert!(d.files.is_empty());
    assert!(d.diagnostics.iter().any(|e| e.contains("Deny")));
    assert!(!d.diagnostics.iter().any(|e| e.contains("UTF-8")));
}

#[test]
fn agents_md_root_to_cwd_and_worktree_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let outer = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let root = outer.join("repo");
    let cwd = root.join("packages/web/src");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    put(&outer, "AGENTS.md", "OUTSIDE_REPO");
    put(&root, "AGENTS.md", "ROOT_INSTRUCTIONS");
    put(&root, "packages/web/AGENTS.md", "PACKAGE_INSTRUCTIONS");
    put(&cwd, "AGENTS.md", "CWD_INSTRUCTIONS");
    put(&cwd, "child/AGENTS.md", "DESCENDANT_INSTRUCTIONS");
    let gate = gate();
    let discovery = discover_rules(&cwd, &cwd, ".agents", &gate);
    let (text, errors, loaded) = render_rules(&discovery, &cwd, &gate, 1);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded, 3);
    assert!(text.find("ROOT_INSTRUCTIONS").unwrap() < text.find("PACKAGE_INSTRUCTIONS").unwrap());
    assert!(text.find("PACKAGE_INSTRUCTIONS").unwrap() < text.find("CWD_INSTRUCTIONS").unwrap());
    assert!(!text.contains("OUTSIDE_REPO"));
    assert!(!text.contains("DESCENDANT_INSTRUCTIONS"));
    assert!(text.contains("### Source: packages/web/AGENTS.md"));
    assert_eq!(
        render_rules(
            &discover_rules(&root, &root, ".agents", &gate),
            &root,
            &gate,
            1
        )
        .2,
        1
    );

    let worktree = root.join("worktree");
    put(&worktree, ".git", "gitdir: ../.git/worktrees/branch");
    put(&worktree, "AGENTS.md", "WORKTREE_INSTRUCTIONS");
    let (text, errors, count) = render_rules(
        &discover_rules(&worktree, &worktree, ".agents", &gate),
        &worktree,
        &gate,
        1,
    );
    assert_eq!(count, 1);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(text.contains("WORKTREE_INSTRUCTIONS"));
    assert!(!text.contains("ROOT_INSTRUCTIONS"));
}

#[test]
fn agents_md_without_repository_reads_only_cwd_and_preserves_rule_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let cwd = root.join("sub");
    put(&root, "AGENTS.md", "PARENT_IGNORED");
    put(&cwd, "AGENTS.md", "CWD_ONLY");
    put(
        &root,
        ".agents/rules/root.md",
        "---\nalwaysApply: true\n---\nRESOURCE_ROOT_RULE",
    );
    put(
        &cwd,
        ".agents/rules/child.md",
        "---\nalwaysApply: true\n---\nUNSELECTED_RULE",
    );
    let gate = gate();
    let (text, errors, count) = render_rules(
        &discover_rules(&root, &cwd, ".agents", &gate),
        &root,
        &gate,
        10000,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(count, 2);
    assert!(text.contains("CWD_ONLY"));
    assert!(text.contains("RESOURCE_ROOT_RULE"));
    assert!(!text.contains("PARENT_IGNORED"));
    assert!(!text.contains("UNSELECTED_RULE"));
    assert!(
        discover(&root, ".agents", false, &gate).files.is_empty(),
        "AGENTS.md is not a command card"
    );
}

#[test]
fn agents_md_raw_markdown_has_independent_budget_and_no_tool_grants() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let gate = gate();
    let body = format!("---\nnot: [valid yaml\n---\n{}", "a".repeat(40000));
    put(&root, "AGENTS.md", &format!("\u{feff}{body}"));
    put(
        &root,
        ".agents/rules/a.md",
        "---\nalwaysApply: true\n---\nORDINARY_RULE",
    );
    put(
        &root,
        ".agents/rules/b.md",
        &format!("---\nalwaysApply: true\n---\n{}", "b".repeat(400)),
    );
    let ordinary = discover(&root, ".agents", true, &gate);
    let combined = discover_rules(&root, &root, ".agents", &gate);
    for budget in [1, 800, 2000, 10000] {
        let (expected, expected_errors, count) = render_rules(&ordinary, &root, &gate, budget);
        let (text, errors, loaded) = render_rules(&combined, &root, &gate, budget);
        assert_eq!(loaded, count + 1);
        assert_eq!(errors, expected_errors);
        assert!(text.contains(&body));
        assert!(!text.contains('\u{feff}'));
        if !expected.is_empty() {
            assert!(text.contains(
                expected
                    .strip_prefix("## User Custom Instructions\n\n")
                    .unwrap()
            ));
            assert!(
                text.find("### Source: AGENTS.md").unwrap()
                    < text.find("### Source: .agents/rules").unwrap()
            );
        }
    }
    assert!(matches!(
        gate.check(PrimitiveOperation::Read, root.to_str().unwrap())
            .unwrap(),
        PermissionDecision::NeedConfirm { .. }
    ));
}

#[test]
fn agents_md_single_file_limits_utf8_empty_and_deny() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let path = root.join("AGENTS.md");
    let gate = gate();
    for (bytes, expected) in [
        (vec![b'a'; MAX_FILE_BYTES], 1),
        (vec![b'a'; MAX_FILE_BYTES + 1], 0),
        (vec![255], 0),
        (b" \n\t".to_vec(), 0),
        ("\u{feff} \n".as_bytes().to_vec(), 0),
    ] {
        fs::write(&path, bytes).unwrap();
        let (text, errors, loaded) = render_rules(
            &discover_rules(&root, &root, ".agents", &gate),
            &root,
            &gate,
            1,
        );
        assert_eq!(loaded, expected, "{errors:?}");
        assert_eq!(errors.is_empty(), expected == 1);
        assert_eq!(text.is_empty(), expected == 0);
    }
    fs::write(&path, [255]).unwrap();
    gate.grant_path_rule(PathRule {
        path: path.to_string_lossy().into_owned(),
        mode: PathRuleMode::Deny,
    });
    let (_, errors, count) = render_rules(
        &discover_rules(&root, &root, ".agents", &gate),
        &root,
        &gate,
        1,
    );
    assert_eq!(count, 0);
    assert!(errors.iter().any(|e| e.contains("Deny")));
    assert!(!errors.iter().any(|e| e.contains("UTF-8")));
}

#[cfg(unix)]
#[test]
fn agents_md_symlink_containment_directory_links_and_target_deny() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let outer = crate::infra::platform::canonicalize(temp.path()).unwrap();
    let root = outer.join("repo");
    fs::create_dir_all(root.join(".git")).unwrap();
    put(&root, "CLAUDE.md", "IN_REPO_CONTENT");
    put(&outer, "outside/AGENTS.md", "OUTSIDE_CONTENT");
    let path = root.join("AGENTS.md");
    symlink("CLAUDE.md", &path).unwrap();
    let gate = gate();
    let discovery = discover_rules(&root, &root, ".agents", &gate);
    let (text, errors, loaded) = render_rules(&discovery, &root, &gate, 1);
    assert_eq!(loaded, 1);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(text.contains("IN_REPO_CONTENT"));
    gate.grant_path_rule(PathRule {
        path: root.join("CLAUDE.md").to_string_lossy().into_owned(),
        mode: PathRuleMode::Deny,
    });
    assert_eq!(render_rules(&discovery, &root, &gate, 1).2, 0);

    fs::remove_file(&path).unwrap();
    symlink("../outside/AGENTS.md", &path).unwrap();
    let gate = super::tests::gate();
    let (text, errors, loaded) = render_rules(&discovery, &root, &gate, 1);
    assert_eq!(loaded, 0);
    assert!(!errors.is_empty());
    assert!(!text.contains("OUTSIDE_CONTENT"));
    fs::remove_file(&path).unwrap();
    symlink(outer.join("outside"), root.join("linked")).unwrap();
    let linked = root.join("linked");
    let (text, errors, loaded) = render_rules(
        &discover_rules(&root, &linked, ".agents", &gate),
        &root,
        &gate,
        1,
    );
    assert_eq!(loaded, 0);
    assert!(!errors.is_empty());
    assert!(!text.contains("OUTSIDE_CONTENT"));
}

#[test]
fn sorted_candidate_truncation() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::infra::platform::canonicalize(temp.path()).unwrap();
    for n in (0..257).rev() {
        put(&root, &format!(".cursor/commands/{n:03}.md"), "body");
    }
    let d = discover(&root, ".agents", false, &gate());
    assert_eq!(d.files.len(), 256);
    assert_eq!(d.files[255].card.name, "255");
    assert!(d
        .diagnostics
        .iter()
        .any(|e| e.contains(&crate::infra::i18n::tr_in(
            crate::infra::i18n::Locale::En,
            "instructions.filesTruncated",
            &[("path", "")]
        ))));
}
