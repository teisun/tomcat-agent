//! # `pi plugin ...` 子命令 + 注册表持久化
//!
//! 覆盖：
//!
//! - `load_plugin_registry` / `save_plugin_registry` 的 round-trip 与
//!   损坏文件回退到空注册表的容错。
//! - `run_plugin` 在 `list` / `load` / `info` / `unload` / `enable` /
//!   `disable` 五种子命令下、对不存在路径或 id 返回 `Ok` 不抛错。

use super::super::*;
use super::mocks::test_config;

#[test]
fn plugin_registry_load_save_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");

    let reg = load_plugin_registry(&path);
    assert!(reg.plugins.is_empty());

    let mut reg = PluginRegistryFile::default();
    reg.plugins.push(PluginRegistryEntry {
        id: "test-plugin".to_string(),
        path: "/some/path".to_string(),
        enabled: true,
        loaded_at: "2026-01-01T00:00:00Z".to_string(),
    });
    save_plugin_registry(&path, &reg).unwrap();

    let loaded = load_plugin_registry(&path);
    assert_eq!(loaded.plugins.len(), 1);
    assert_eq!(loaded.plugins[0].id, "test-plugin");
    assert!(loaded.plugins[0].enabled);
}

#[test]
fn plugin_registry_corrupt_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    std::fs::write(&path, "not valid json {{{").unwrap();

    let reg = load_plugin_registry(&path);
    assert!(reg.plugins.is_empty());
}

#[test]
fn run_plugin_list_returns_ok_with_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(PluginSub::List, &cfg);
    assert!(r.is_ok());
}

#[test]
fn run_plugin_load_nonexistent_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Load {
            path: "/nonexistent/path/to/plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_info_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Info {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_unload_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Unload {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_enable_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Enable {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}

#[test]
fn run_plugin_disable_not_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let r = run_plugin(
        PluginSub::Disable {
            id: "nonexistent-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}
