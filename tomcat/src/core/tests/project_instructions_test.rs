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
