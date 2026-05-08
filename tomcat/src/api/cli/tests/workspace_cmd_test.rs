//! # `tomcat workspace ...` 子命令
//!
//! `workspace` 直接读写 `~/.tomcat/tomcat.config.toml`，因此每个用例都用
//! `with_tomcat_config_in_home` 把 `HOME` 指向临时目录后再执行：
//!
//! - `add → list → remove → list` 完整生命周期。
//! - `add` 不存在路径应当报错，重复 `add` 是 noop，`add --cwd` 写入当前目录的
//!   规范化路径。
//! - `remove` 不存在路径不报错（noop）。

use super::super::*;
use super::mocks::{test_config, with_tomcat_config_in_home};
use crate::load_config_toml_file;
use crate::resolve_workspace_roots_paths;
use serial_test::serial;

#[test]
#[serial(env_lock)]
fn run_workspace_add_list_remove() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::Remove { path: target_path }, &cfg);
        assert!(r.is_ok());

        let r = run_workspace(WorkspaceSub::List, &cfg);
        assert!(r.is_ok());
    });
}

#[test]
#[serial(env_lock)]
fn run_workspace_add_nonexistent_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some("/nonexistent/path/should/fail".to_string()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_err());
    });
}

#[test]
#[serial(env_lock)]
fn run_workspace_add_duplicate_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path.clone()),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());

        let r = run_workspace(
            WorkspaceSub::Add {
                path: Some(target_path),
                cwd: false,
            },
            &cfg,
        );
        assert!(r.is_ok());
    });
}

#[test]
#[serial(env_lock)]
fn run_workspace_add_cwd_adds_current_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let target = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(target.path()).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(target.path()).unwrap();
        let r = run_workspace(
            WorkspaceSub::Add {
                path: None,
                cwd: true,
            },
            &cfg,
        );
        std::env::set_current_dir(&prev).unwrap();
        assert!(r.is_ok());

        let cfg_path = crate::normalize_path(DEFAULT_CONFIG_PATH).unwrap();
        let file_cfg = load_config_toml_file(&cfg_path).unwrap();
        let list = resolve_workspace_roots_paths(&file_cfg).unwrap();
        assert!(list.iter().any(|p| p == &canon));
    });
}

#[test]
#[serial(env_lock)]
fn run_workspace_remove_nonexistent_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_tomcat_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();

        let r = run_workspace(
            WorkspaceSub::Remove {
                path: "/some/path".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok());
    });
}
