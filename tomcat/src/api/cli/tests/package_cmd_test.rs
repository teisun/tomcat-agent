use std::path::Path;

use super::super::*;
use super::mocks::test_config;
use crate::core::load_package_registry;

struct CurrentDirGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
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

fn write_plugin(dir: &Path, id: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": id,
            "name": id,
            "version": "1.0.0",
            "description": "cli test plugin",
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": []
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("main.js"), "export default 1;\n").unwrap();
}

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: cli skill\n---\n# Body\n"),
    )
    .unwrap();
}

fn write_package_manifest(root: &Path) {
    std::fs::write(
        root.join("package.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "cli-package",
            "version": "1.0.0",
            "tomcat": {
                "plugins": ["plugins/cli-plugin"],
                "skills": ["skills/cli-skill"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn run_install_scope_package_writes_layer_registries() {
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let cfg = test_config(work.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    write_plugin(&source.path().join("plugins/cli-plugin"), "cli-plugin");
    write_skill(&source.path().join("skills/cli-skill"), "cli-skill");
    write_package_manifest(source.path());

    run_install(
        source.path().to_string_lossy().to_string(),
        Some(PackageVisibilityArg::Scope),
        None,
        false,
        &cfg,
    )
    .expect("cli install should succeed");

    let scope_paths = crate::core::package::resolve_layer_paths(
        &cfg,
        crate::core::package::PackageVisibility::Scope,
        Some(workspace.path()),
    )
    .unwrap();
    assert!(scope_paths.plugins_dir.join("cli-plugin").is_dir());
    assert!(scope_paths.skills_dir.join("cli-skill").is_dir());
    assert_eq!(
        load_package_registry(&scope_paths.package_registry_path)
            .packages
            .len(),
        1
    );
    assert_eq!(
        load_package_registry(&scope_paths.package_registry_path).schema,
        crate::core::package::PACKAGE_REGISTRY_SCHEMA_V1
    );
    assert_eq!(
        load_plugin_registry(&scope_paths.plugin_registry_path)
            .plugins
            .len(),
        1
    );
}

#[test]
fn run_install_without_visibility_defaults_to_scope_in_noninteractive_mode() {
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let cfg = test_config(work.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    write_skill(source.path(), "commit");
    run_install(
        source.path().to_string_lossy().to_string(),
        None,
        None,
        false,
        &cfg,
    )
    .expect("noninteractive install should default to scope");

    let scope_paths = crate::core::package::resolve_layer_paths(
        &cfg,
        crate::core::package::PackageVisibility::Scope,
        Some(workspace.path()),
    )
    .unwrap();
    assert_eq!(
        load_package_registry(&scope_paths.package_registry_path)
            .packages
            .len(),
        1
    );
}

#[test]
fn run_uninstall_removes_scope_installed_package() {
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let cfg = test_config(work.path());
    crate::ensure_work_dir_structure(&cfg).unwrap();

    write_skill(source.path(), "cleanup");
    run_install(
        source.path().to_string_lossy().to_string(),
        Some(PackageVisibilityArg::Scope),
        None,
        false,
        &cfg,
    )
    .unwrap();
    run_uninstall(
        "cleanup".to_string(),
        Some(PackageVisibilityArg::Scope),
        None,
        &cfg,
    )
    .unwrap();

    let scope_paths = crate::core::package::resolve_layer_paths(
        &cfg,
        crate::core::package::PackageVisibility::Scope,
        Some(workspace.path()),
    )
    .unwrap();
    assert!(load_package_registry(&scope_paths.package_registry_path)
        .packages
        .is_empty());
}
