use std::path::PathBuf;

use tempfile::TempDir;

use super::super::append::{
    append_path_rule_to_disk, append_workspace_entry_to_disk, append_workspace_root_to_disk,
};
use super::super::load::load_config_toml_file;
use super::super::types::WorkspaceEntry;
use crate::core::permission::{PathRule, PathRuleMode};
use serial_test::serial;

fn empty_config_file(dir: &TempDir) -> PathBuf {
    let p = dir.path().join("tomcat.config.toml");
    std::fs::write(
        &p,
        "[agent]\nid='main'\nworkspace='/tmp'\n\n[storage]\nwork_dir='/tmp'\n\n[llm]\ndefault_model='gpt-5.4'\n\n[workspace]\nworkspace_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true",
    )
    .unwrap();
    p
}

#[test]
fn append_extra_root_appends_once() {
    let dir = TempDir::new().unwrap();
    let p = empty_config_file(&dir);
    let extra = dir.path().join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    let s = extra.to_string_lossy().into_owned();
    append_workspace_root_to_disk(&p, s.clone()).unwrap();
    append_workspace_root_to_disk(&p, s.clone()).unwrap();
    let cfg = load_config_toml_file(&p).unwrap();
    assert_eq!(cfg.workspace.workspace_roots, vec![s]);
}

#[test]
fn append_path_rule_dedupes() {
    let dir = TempDir::new().unwrap();
    let p = empty_config_file(&dir);
    let rule = PathRule {
        path: "~/.foo".to_string(),
        mode: PathRuleMode::Deny,
    };
    append_path_rule_to_disk(&p, rule.clone()).unwrap();
    append_path_rule_to_disk(&p, rule).unwrap();
    let cfg = load_config_toml_file(&p).unwrap();
    assert_eq!(cfg.primitive.path_rules.len(), 1);
}

#[test]
fn append_workspace_entry_dedupes_by_path() {
    let dir = TempDir::new().unwrap();
    let p = empty_config_file(&dir);
    let entry = WorkspaceEntry {
        path: "/tmp/proj".into(),
        alias: Some("proj".into()),
        description: None,
    };
    append_workspace_entry_to_disk(&p, entry.clone()).unwrap();
    append_workspace_entry_to_disk(&p, entry).unwrap();
    let cfg = load_config_toml_file(&p).unwrap();
    assert_eq!(cfg.workspace.entries.len(), 1);
}

#[test]
#[serial(env_lock)]
fn append_workspace_root_does_not_persist_env_merged_values() {
    struct EnvGuard(Option<String>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("TOMCAT__LOG__LEVEL", v),
                None => std::env::remove_var("TOMCAT__LOG__LEVEL"),
            }
        }
    }

    let dir = TempDir::new().unwrap();
    let p = empty_config_file(&dir);
    let extra = dir.path().join("extra_env_guard");
    std::fs::create_dir_all(&extra).unwrap();
    let _guard = EnvGuard(std::env::var("TOMCAT__LOG__LEVEL").ok());
    std::env::set_var("TOMCAT__LOG__LEVEL", "trace");

    append_workspace_root_to_disk(&p, extra.to_string_lossy().into_owned()).unwrap();

    let cfg = load_config_toml_file(&p).unwrap();
    assert_eq!(
        cfg.log.level, "warn",
        "append helper should write back disk config instead of env-merged log.level"
    );
}
