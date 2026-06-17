//! # `tomcat plugin ...` 子命令 + 注册表持久化
//!
//! 覆盖：
//!
//! - `load_plugin_registry` / `save_plugin_registry` 的 round-trip 与
//!   损坏文件返回显式错误。
//! - `run_plugin` 在 `list` / `load` / `info` / `unload` / `enable` /
//!   `disable` 五种子命令下、对不存在路径或 id 返回 `Ok` 不抛错。

use super::super::*;
use super::mocks::test_config;
use crate::api::cli::plugin_cmd::render_plugin_list_output;

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

fn buildable_plugin_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("create plugin dir");
    std::fs::write(
        dir.path().join("plugin.json"),
        r#"{
  "id": "buildable-plugin",
  "name": "Buildable Plugin",
  "version": "0.1.0",
  "description": "test",
  "author": "tests",
  "main": "main.js",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": [],
  "functions": [
    {
      "point": "web_search.backend",
      "function": "webSearchBackend"
    }
  ]
}"#,
    )
    .expect("write plugin.json");
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("backends")).expect("create src layout");
    std::fs::write(src.join("config.js"), "var READY = true;\n").expect("write config");
    std::fs::write(
        src.join("shared.js"),
        "function buildResult() { return { backend: 'fixture', hits: [], warnings: READY ? [] : ['bad'] }; }\n",
    )
    .expect("write shared");
    std::fs::write(
        src.join("index.js"),
        "pi.registerFunction('webSearchBackend', function () { return buildResult(); });\n",
    )
    .expect("write index");
    dir
}

#[test]
fn plugin_registry_load_save_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");

    let reg = load_plugin_registry(&path).unwrap();
    assert!(reg.plugins.is_empty());

    let mut reg = PluginRegistryFile::default();
    reg.plugins.push(PluginRegistryEntry {
        id: "test-plugin".to_string(),
        path: "/some/path".to_string(),
        enabled: true,
        loaded_at: "2026-01-01T00:00:00Z".to_string(),
    });
    save_plugin_registry(&path, &reg).unwrap();

    let loaded = load_plugin_registry(&path).unwrap();
    assert_eq!(loaded.plugins.len(), 1);
    assert_eq!(loaded.plugins[0].id, "test-plugin");
    assert!(loaded.plugins[0].enabled);
}

#[test]
fn plugin_registry_corrupt_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    std::fs::write(&path, "not valid json {{{").unwrap();

    let error = load_plugin_registry(&path).unwrap_err().to_string();
    assert!(error.contains("registry 损坏"), "unexpected error: {error}");
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

    let registry = load_plugin_registry(&reg_path).unwrap();
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
    )
    .unwrap();
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

    let scope_registry = load_plugin_registry(&scope_registry_path).unwrap();
    let global_registry = load_plugin_registry(&global_registry_path).unwrap();
    assert!(!scope_registry.plugins[0].enabled);
    assert!(global_registry.plugins[0].enabled);
}

#[test]
fn plugin_list_renders_visible_and_shadowed_layered_entries() {
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
                enabled: false,
                loaded_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        },
    )
    .unwrap();

    let output = render_plugin_list_output(&cfg).unwrap();
    assert!(output.contains("dup-plugin"));
    assert!(output.contains("scope"));
    assert!(output.contains("visible"));
    assert!(output.contains("shadowed:"));
    assert!(output.contains("dup-plugin @ global"));
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
        .unwrap()
        .plugins
        .is_empty());
    assert_eq!(
        load_plugin_registry(&global_registry_path)
            .unwrap()
            .plugins
            .len(),
        1
    );
}

#[test]
fn run_plugin_build_writes_main_js() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let plugin_dir = buildable_plugin_dir();

    let r = run_plugin(
        PluginSub::Build {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());

    let output = std::fs::read_to_string(plugin_dir.path().join("main.js")).expect("read main.js");
    assert!(output.contains("Generated by `tomcat plugin build`"));
    assert!(output.contains("webSearchBackend"));
}

#[test]
fn run_plugin_build_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let plugin_dir = buildable_plugin_dir();

    run_plugin(
        PluginSub::Build {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    )
    .unwrap();
    let first = std::fs::read_to_string(plugin_dir.path().join("main.js")).expect("first main.js");

    run_plugin(
        PluginSub::Build {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    )
    .unwrap();
    let second =
        std::fs::read_to_string(plugin_dir.path().join("main.js")).expect("second main.js");

    assert_eq!(first, second);
}

#[test]
fn run_plugin_build_missing_src_returns_ok_with_message() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        plugin_dir.path().join("plugin.json"),
        r#"{
  "id": "missing-src-plugin",
  "name": "Missing Src Plugin",
  "version": "0.1.0",
  "description": "test",
  "author": "tests",
  "main": "main.js",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": []
}"#,
    )
    .unwrap();

    let r = run_plugin(
        PluginSub::Build {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
    assert!(!plugin_dir.path().join("main.js").exists());
}

#[test]
fn run_plugin_build_then_load_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();
    let plugin_dir = buildable_plugin_dir();

    run_plugin(
        PluginSub::Build {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    )
    .unwrap();

    let r = run_plugin(
        PluginSub::Load {
            path: plugin_dir.path().to_string_lossy().to_string(),
        },
        &cfg,
    );
    assert!(r.is_ok());
}
