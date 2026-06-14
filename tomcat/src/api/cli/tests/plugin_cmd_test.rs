//! # `tomcat plugin ...` 子命令 + 注册表持久化
//!
//! 覆盖：
//!
//! - `load_plugin_registry` / `save_plugin_registry` 的 round-trip 与
//!   损坏文件回退到空注册表的容错。
//! - `run_plugin` 在 `list` / `load` / `info` / `unload` / `enable` /
//!   `disable` 五种子命令下、对不存在路径或 id 返回 `Ok` 不抛错。

use super::super::*;
use super::mocks::test_config;

struct CurrentDirGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

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
fn run_plugin_unload_removes_registered_entry_even_without_live_manager_state() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    let reg_path = crate::resolve_plugins_dir(&cfg)
        .unwrap()
        .join("registry.json");
    save_plugin_registry(
        &reg_path,
        &PluginRegistryFile {
            plugins: vec![PluginRegistryEntry {
                id: "registered-only-plugin".to_string(),
                path: "/tmp/registered-only-plugin".to_string(),
                enabled: true,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .expect("seed registry");

    let r = run_plugin(
        PluginSub::Unload {
            id: "registered-only-plugin".to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());

    let registry = load_plugin_registry(&reg_path);
    assert!(
        registry.plugins.is_empty(),
        "unload should clear registry-only entries even when no live PluginManager state exists"
    );
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

#[test]
fn run_plugin_load_defaults_to_allow_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    let plugin_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        plugin_dir.path().join("plugin.json"),
        r#"{
  "id": "perm-allow-plugin",
  "name": "Perm Allow Plugin",
  "version": "0.1.0",
  "description": "test",
  "author": "tests",
  "main": "main.js",
  "requiredPermissions": ["read", "bash"],
  "requiredApiVersion": "1.0",
  "tags": []
}"#,
    )
    .unwrap();
    std::fs::write(plugin_dir.path().join("main.js"), "1 + 1;").unwrap();

    let r = run_plugin(
        PluginSub::Load {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());

    let registry = load_plugin_registry(
        &crate::resolve_plugins_dir(&cfg)
            .unwrap()
            .join("registry.json"),
    );
    assert!(
        registry
            .plugins
            .iter()
            .any(|entry| entry.id == "perm-allow-plugin"),
        "默认放行权限后应成功写入注册表"
    );
}

#[test]
fn plugin_disable_targets_highest_priority_registry_layer() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    let scope_registry_path = workspace.path().join(".tomcat/plugins/registry.json");
    let global_registry_path = crate::resolve_plugins_dir(&cfg)
        .unwrap()
        .join("registry.json");
    save_plugin_registry(
        &scope_registry_path,
        &PluginRegistryFile {
            plugins: vec![PluginRegistryEntry {
                id: "dup-plugin".to_string(),
                path: workspace
                    .path()
                    .join(".tomcat/plugins/dup-plugin")
                    .display()
                    .to_string(),
                enabled: true,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .unwrap();
    save_plugin_registry(
        &global_registry_path,
        &PluginRegistryFile {
            plugins: vec![PluginRegistryEntry {
                id: "dup-plugin".to_string(),
                path: dir.path().join("plugins/dup-plugin").display().to_string(),
                enabled: true,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .unwrap();

    run_plugin(
        PluginSub::Disable {
            id: "dup-plugin".to_string(),
        },
        &cfg,
    )
    .unwrap();

    let scope_registry = load_plugin_registry(&scope_registry_path);
    let global_registry = load_plugin_registry(&global_registry_path);
    assert!(!scope_registry.plugins[0].enabled);
    assert!(global_registry.plugins[0].enabled);
}

#[test]
fn plugin_unload_removes_scope_entry_before_global_entry() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    let scope_registry_path = workspace.path().join(".tomcat/plugins/registry.json");
    let global_registry_path = crate::resolve_plugins_dir(&cfg)
        .unwrap()
        .join("registry.json");
    save_plugin_registry(
        &scope_registry_path,
        &PluginRegistryFile {
            plugins: vec![PluginRegistryEntry {
                id: "dup-plugin".to_string(),
                path: workspace
                    .path()
                    .join(".tomcat/plugins/dup-plugin")
                    .display()
                    .to_string(),
                enabled: true,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .unwrap();
    save_plugin_registry(
        &global_registry_path,
        &PluginRegistryFile {
            plugins: vec![PluginRegistryEntry {
                id: "dup-plugin".to_string(),
                path: dir.path().join("plugins/dup-plugin").display().to_string(),
                enabled: true,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .unwrap();

    run_plugin(
        PluginSub::Unload {
            id: "dup-plugin".to_string(),
        },
        &cfg,
    )
    .unwrap();

    assert!(load_plugin_registry(&scope_registry_path)
        .plugins
        .is_empty());
    assert_eq!(load_plugin_registry(&global_registry_path).plugins.len(), 1);
}
