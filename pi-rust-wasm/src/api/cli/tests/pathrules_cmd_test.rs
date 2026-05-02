//! # `pi pathrules ...` 子命令测试（PR-10）
//!
//! 与 `workspace_cmd` 同款：用 `with_pi_config_in_home` 把 `HOME` 指向临时目录后再执行：
//!
//! - `add → list` 路径出现在 `[user]` 段。
//! - 重复 `add` 是 noop（共享 helper 内部 dedupe）。
//! - 不存在路径只警告，不报错。
//! - `list` 始终渲染三段：`[builtin]` / `[user]` / `[session]`。

use super::super::*;
use super::mocks::{test_config, with_pi_config_in_home};
use crate::load_config_toml_file;

#[test]
fn run_pathrules_add_then_user_section_contains_path() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let r = run_pathrules(
            PathRulesSub::Add {
                path: target_path.clone(),
                mode: "readonly".to_string(),
            },
            &cfg,
        );
        assert!(r.is_ok(), "add should succeed: {:?}", r);

        // 重新加载 TOML：path_rules 已落盘。
        let cfg_path = crate::normalize_path(DEFAULT_CONFIG_PATH).unwrap();
        let file_cfg = load_config_toml_file(&cfg_path).unwrap();
        assert!(
            file_cfg.primitive.path_rules.iter().any(|r| r
                .path
                .contains(target.path().file_name().unwrap().to_str().unwrap())),
            "新追加的 path_rule 应在 file_cfg.primitive.path_rules 中"
        );

        // List 不报错（输出由 println 给 stdout，集成测试粒度暂只断言 ok）。
        let r = run_pathrules(PathRulesSub::List, &file_cfg);
        assert!(r.is_ok());
    });
}

#[test]
fn run_pathrules_add_dedupes_on_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap().to_string();

        let _ = run_pathrules(
            PathRulesSub::Add {
                path: target_path.clone(),
                mode: "deny".to_string(),
            },
            &cfg,
        );
        let _ = run_pathrules(
            PathRulesSub::Add {
                path: target_path.clone(),
                mode: "deny".to_string(),
            },
            &cfg,
        );

        let cfg_path = crate::normalize_path(DEFAULT_CONFIG_PATH).unwrap();
        let file_cfg = load_config_toml_file(&cfg_path).unwrap();
        let count = file_cfg
            .primitive
            .path_rules
            .iter()
            .filter(|r| {
                r.path
                    .contains(target.path().file_name().unwrap().to_str().unwrap())
            })
            .count();
        assert_eq!(count, 1, "重复 add 应只保留 1 条");
    });
}

#[test]
fn run_pathrules_add_unknown_mode_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();
        let r = run_pathrules(
            PathRulesSub::Add {
                path: "/tmp/foo".to_string(),
                mode: "allow".to_string(),
            },
            &cfg,
        );
        assert!(r.is_err());
    });
}

#[test]
fn run_pathrules_add_nonexistent_path_warns_but_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();
        let r = run_pathrules(
            PathRulesSub::Add {
                path: "/__definitely_not_exist__/secrets".to_string(),
                mode: "deny".to_string(),
            },
            &cfg,
        );
        // path_rules 允许针对未来出现的路径；不存在仅警告，不报错。
        assert!(r.is_ok(), "expected ok, got {:?}", r);
    });
}

#[test]
fn run_pathrules_list_works_with_no_user_rules() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    with_pi_config_in_home(dir.path(), || {
        crate::ensure_work_dir_structure(&cfg).unwrap();
        let r = run_pathrules(PathRulesSub::List, &cfg);
        assert!(r.is_ok());
    });
}
