use super::{code_diff_content_fingerprint, collect_git_changed_files, is_code_path};

#[test]
fn user_visible_markup_and_styles_trigger_code_review() {
    for path in [
        "gui/src/styles.css",
        "web/app.scss",
        "web/theme.less",
        "web/index.html",
        "WEB/COMPONENT.CSS",
    ] {
        assert!(is_code_path(path), "{path} must trigger the code gate");
    }
}

#[test]
fn non_code_assets_do_not_trigger_code_review() {
    for path in ["README.md", "docs/guide.txt", "assets/logo.png"] {
        assert!(!is_code_path(path), "{path} must not trigger the code gate");
    }
}

#[tokio::test]
async fn changed_files_include_tracked_diff_and_untracked_files() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    std::fs::write(root.join("tracked.rs"), "fn before() {}\n").expect("seed tracked");
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(init.success());
    let add = std::process::Command::new("git")
        .args(["add", "tracked.rs"])
        .current_dir(root)
        .status()
        .expect("git add");
    assert!(add.success());
    let commit = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=tomcat-test",
            "-c",
            "user.email=tomcat-test@example.invalid",
            "commit",
            "-qm",
            "seed",
        ])
        .current_dir(root)
        .status()
        .expect("git commit");
    assert!(commit.success());
    std::fs::write(root.join("tracked.rs"), "fn after() {}\n").expect("modify tracked");
    std::fs::write(root.join("untracked.rs"), "fn fresh() {}\n").expect("add untracked");
    assert_eq!(
        collect_git_changed_files(root).await,
        vec!["tracked.rs".to_string(), "untracked.rs".to_string()]
    );
}

#[tokio::test]
async fn code_diff_fingerprint_changes_for_code_not_non_code_or_noop_rewrites() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    std::fs::write(root.join("tracked.rs"), "fn before() {}\n").expect("seed tracked");
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("git init")
        .success());
    assert!(std::process::Command::new("git")
        .args(["add", "tracked.rs"])
        .current_dir(root)
        .status()
        .expect("git add")
        .success());
    assert!(std::process::Command::new("git")
        .args([
            "-c",
            "user.name=tomcat-test",
            "-c",
            "user.email=tomcat-test@example.invalid",
            "commit",
            "-qm",
            "seed"
        ])
        .current_dir(root)
        .status()
        .expect("git commit")
        .success());
    std::fs::write(root.join("tracked.rs"), "fn first_dirty_version() {}\n")
        .expect("modify tracked");
    let first = code_diff_content_fingerprint(root)
        .await
        .expect("fingerprint");
    std::fs::write(root.join("tracked.rs"), "fn second_dirty_version() {}\n")
        .expect("modify already dirty file");
    let second = code_diff_content_fingerprint(root)
        .await
        .expect("fingerprint");
    assert_ne!(
        first, second,
        "an edit to an already-dirty code file is meaningful progress"
    );
    std::fs::write(root.join("README.md"), "non-code change\n").expect("write non-code");
    let after_non_code = code_diff_content_fingerprint(root)
        .await
        .expect("fingerprint");
    assert_eq!(
        second, after_non_code,
        "non-code paths must not reset the completion-guard progress signal"
    );
    std::fs::write(root.join("tracked.rs"), "fn second_dirty_version() {}\n")
        .expect("rewrite identical content");
    assert_eq!(
        after_non_code,
        code_diff_content_fingerprint(root)
            .await
            .expect("fingerprint"),
        "a no-op rewrite must not reset the completion-guard progress signal"
    );
}

#[tokio::test]
async fn changed_files_are_relative_to_a_nested_workspace_root() {
    let repo = tempfile::tempdir().expect("repo");
    let root = repo.path();
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).expect("nested");
    std::fs::write(nested.join("tracked.rs"), "fn before() {}\n").expect("seed");
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("git init")
        .success());
    assert!(std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .expect("git add")
        .success());
    assert!(std::process::Command::new("git")
        .args([
            "-c",
            "user.name=tomcat-test",
            "-c",
            "user.email=tomcat-test@example.invalid",
            "commit",
            "-qm",
            "seed"
        ])
        .current_dir(root)
        .status()
        .expect("git commit")
        .success());
    std::fs::write(nested.join("tracked.rs"), "fn after() {}\n").expect("modify");
    assert_eq!(
        collect_git_changed_files(&nested).await,
        vec!["tracked.rs".to_string()]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn git_paths_preserve_unicode_whitespace_and_literal_pathspecs() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let tracked = [
        "界面.rs",
        " leading.rs",
        "line\nbreak.rs",
        ":(glob)ghost.rs",
    ];
    let untracked = [" 新增.ts", "new\nline.ts", "quote\"name.js"];
    for name in tracked {
        std::fs::write(root.join(name), "before").unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=tomcat-test",
            "-c",
            "user.email=tomcat-test@example.invalid",
            "commit",
            "-qm",
            "seed",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    for name in tracked.into_iter().chain(untracked) {
        std::fs::write(root.join(name), "first").unwrap();
    }
    let mut expected = tracked
        .into_iter()
        .chain(untracked)
        .map(String::from)
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(collect_git_changed_files(root).await, expected);
    let mut previous = code_diff_content_fingerprint(root)
        .await
        .expect("initial fingerprint");
    for name in tracked.into_iter().chain(untracked) {
        std::fs::write(root.join(name), "second").unwrap();
        let next = code_diff_content_fingerprint(root)
            .await
            .expect("updated fingerprint");
        assert_ne!(previous, next, "content update was missed for {name:?}");
        previous = next;
    }
}
