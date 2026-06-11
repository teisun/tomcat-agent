use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::scope::{
    fnv1a_hex, project_root, resolve_session_mode, session_key_for, session_key_for_agent,
    SessionMode,
};

#[test]
fn claw_ignores_cwd_and_stays_on_default_scope() {
    let key_a = session_key_for(SessionMode::Claw, Path::new("/tmp/a"));
    let key_b = session_key_for(SessionMode::Claw, Path::new("/tmp/b"));
    assert_eq!(key_a, "agent:main:main");
    assert_eq!(key_b, "agent:main:main");
}

#[test]
fn code_uses_same_key_for_repo_root_and_subdir() {
    let repo = temp_git_repo("scope_same_repo");
    let nested = repo.join("src").join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    let root_key = session_key_for(SessionMode::Code, &repo);
    let nested_key = session_key_for(SessionMode::Code, &nested);
    assert_eq!(root_key, nested_key);
}

#[test]
fn code_uses_different_keys_for_different_repos() {
    let repo_a = temp_git_repo("scope_repo_a");
    let repo_b = temp_git_repo("scope_repo_b");
    let key_a = session_key_for(SessionMode::Code, &repo_a);
    let key_b = session_key_for(SessionMode::Code, &repo_b);
    assert_ne!(key_a, key_b);
}

#[test]
fn code_falls_back_to_cwd_for_non_git_dirs() {
    let base = tempfile::tempdir().unwrap();
    let dir_a = base.path().join("plain-a");
    let dir_b = base.path().join("plain-b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let key_a = session_key_for(SessionMode::Code, &dir_a);
    let key_b = session_key_for(SessionMode::Code, &dir_b);
    assert_ne!(key_a, key_b);
    assert_eq!(project_root(&dir_a), std::fs::canonicalize(&dir_a).unwrap());
}

#[test]
fn session_key_includes_agent_id_for_non_default_agents() {
    let key = session_key_for_agent("reviewer", SessionMode::Claw, Path::new("/tmp"));
    assert_eq!(key, "agent:reviewer:main");
}

#[test]
fn fnv1a_hex_matches_known_value() {
    assert_eq!(fnv1a_hex(b"hello"), "a430d84680aabd0b");
}

#[test]
fn resolve_session_mode_prefers_env_override() {
    let mode = resolve_session_mode("code", Some("claw")).unwrap();
    assert_eq!(mode, SessionMode::Claw);
}

fn temp_git_repo(label: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "tomcat_scope_{}_{}_{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init"]);
    std::fs::write(repo.join("README.md"), "# scope test\n").unwrap();
    run_git(&repo, &["add", "README.md"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Tomcat Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
    repo
}

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "git {:?} failed in {}",
        args,
        repo.display()
    );
}
