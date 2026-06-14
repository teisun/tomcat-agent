use std::path::{Path, PathBuf};

use serde_json::json;

use crate::core::package::{
    load_package_registry, load_plugin_registry, resolve_layer_paths, PackageManager,
    PackageVisibility,
};
use crate::AppConfig;

fn test_config(work_dir: &Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg
}

fn write_plugin(dir: &Path, id: &str, version: &str, tool_name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&json!({
            "id": id,
            "name": format!("{id}-name"),
            "version": version,
            "description": format!("{id} description"),
            "author": "tests",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": [],
            "tools": [{
                "name": tool_name,
                "description": format!("{tool_name} description"),
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("main.js"), "export default 1;\n").unwrap();
    dir.to_path_buf()
}

fn write_skill(dir: &Path, name: &str, description: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# Body\n"),
    )
    .unwrap();
    dir.to_path_buf()
}

fn write_package_manifest(root: &Path, plugins: &[&str], skills: &[&str]) {
    std::fs::write(
        root.join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "combo-package",
            "version": "1.2.3",
            "description": "package description",
            "tomcat": {
                "plugins": plugins,
                "skills": skills
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn detect_bare_plugin_and_bare_skill() {
    let work_dir = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    let plugin_dir = tempfile::tempdir().unwrap();
    write_plugin(plugin_dir.path(), "demo-plugin", "0.1.0", "demo_tool");
    let detected_plugin = manager.detect_source(plugin_dir.path()).unwrap();
    assert_eq!(detected_plugin.manifest.name, "demo-plugin");
    assert_eq!(detected_plugin.manifest.version, "0.1.0");
    assert_eq!(detected_plugin.resources.len(), 1);
    assert_eq!(detected_plugin.resources[0].id, "demo-plugin");

    let skill_dir = tempfile::tempdir().unwrap();
    write_skill(skill_dir.path(), "commit", "Create a commit.");
    let detected_skill = manager.detect_source(skill_dir.path()).unwrap();
    assert_eq!(detected_skill.manifest.name, "commit");
    assert_eq!(detected_skill.manifest.version, "0.0.0");
    assert_eq!(detected_skill.resources.len(), 1);
    assert_eq!(detected_skill.resources[0].id, "commit");
}

#[test]
fn detect_package_manifest_requires_package_json_tomcat_block() {
    let work_dir = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    let pkg_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        pkg_dir.path().join("package.json"),
        serde_json::to_string_pretty(&json!({
            "name": "plain-npm",
            "version": "1.0.0"
        }))
        .unwrap(),
    )
    .unwrap();

    let error = manager
        .detect_source(pkg_dir.path().join("package.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("tomcat"), "unexpected error: {error}");
}

#[test]
fn prepare_install_rejects_same_layer_conflict_without_force() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    write_plugin(
        &workspace.path().join(".tomcat/plugins/conflict-plugin"),
        "conflict-plugin",
        "0.1.0",
        "tool_a",
    );
    let source = tempfile::tempdir().unwrap();
    write_plugin(source.path(), "conflict-plugin", "0.2.0", "tool_b");

    let error = manager
        .prepare_install(
            source.path(),
            PackageVisibility::Scope,
            Some(workspace.path()),
            false,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("同层"), "unexpected error: {error}");
}

#[test]
fn prepare_install_reports_cross_layer_shadow_warning() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    write_skill(
        &workspace.path().join(".tomcat/skills/commit"),
        "commit",
        "already here",
    );
    let source = tempfile::tempdir().unwrap();
    write_skill(source.path(), "commit", "newer");

    let prepared = manager
        .prepare_install(
            source.path(),
            PackageVisibility::Global,
            Some(workspace.path()),
            false,
        )
        .unwrap();
    assert!(
        prepared
            .warnings
            .iter()
            .any(|warning| warning.contains("更高优先级层")),
        "warnings should mention higher-priority shadowing: {:?}",
        prepared.warnings
    );
}

#[test]
fn install_scope_package_writes_layer_registries() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    let source = tempfile::tempdir().unwrap();
    write_plugin(
        &source.path().join("plugins/release-plugin"),
        "release-plugin",
        "1.0.0",
        "release_tool",
    );
    write_skill(
        &source.path().join("skills/commit"),
        "commit",
        "Create a commit.",
    );
    write_package_manifest(
        source.path(),
        &["plugins/release-plugin"],
        &["skills/commit"],
    );

    let prepared = manager
        .prepare_install(
            source.path(),
            PackageVisibility::Scope,
            Some(workspace.path()),
            false,
        )
        .unwrap();
    let outcome = manager.install(prepared).unwrap();
    assert_eq!(outcome.record.name, "combo-package");
    assert_eq!(outcome.record.resources.len(), 2);

    let scope_paths =
        resolve_layer_paths(&cfg, PackageVisibility::Scope, Some(workspace.path())).unwrap();
    assert!(scope_paths.plugins_dir.join("release-plugin").is_dir());
    assert!(scope_paths.skills_dir.join("commit").is_dir());

    let package_registry = load_package_registry(&scope_paths.package_registry_path);
    assert_eq!(package_registry.packages.len(), 1);
    assert_eq!(package_registry.packages[0].name, "combo-package");

    let plugin_registry = load_plugin_registry(&scope_paths.plugin_registry_path);
    assert_eq!(plugin_registry.plugins.len(), 1);
    assert_eq!(plugin_registry.plugins[0].id, "release-plugin");
}

#[test]
fn install_failure_rolls_back_copied_dirs() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    let source = tempfile::tempdir().unwrap();
    write_plugin(source.path(), "broken-plugin", "1.0.0", "broken_tool");

    let scope_paths =
        resolve_layer_paths(&cfg, PackageVisibility::Scope, Some(workspace.path())).unwrap();
    std::fs::create_dir_all(&scope_paths.layer_root).unwrap();
    std::fs::write(&scope_paths.packages_dir, "block registry writes here").unwrap();

    let prepared = manager
        .prepare_install(
            source.path(),
            PackageVisibility::Scope,
            Some(workspace.path()),
            false,
        )
        .unwrap();
    let error = manager.install(prepared).unwrap_err().to_string();

    assert!(
        !error.contains("dirty_state"),
        "unexpected dirty rollback: {error}"
    );
    assert!(
        !scope_paths.plugins_dir.join("broken-plugin").exists(),
        "copied plugin dir should be removed after rollback"
    );
    assert!(
        !scope_paths.package_registry_path.exists(),
        "package registry should be restored to non-existent state"
    );
}

#[test]
fn uninstall_uses_package_registry_for_precise_cleanup() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());
    let manager = PackageManager::new(&cfg);

    let source = tempfile::tempdir().unwrap();
    write_plugin(
        &source.path().join("plugins/cleanup-plugin"),
        "cleanup-plugin",
        "1.0.0",
        "cleanup_tool",
    );
    write_skill(
        &source.path().join("skills/cleanup-skill"),
        "cleanup-skill",
        "Cleanup skill",
    );
    write_package_manifest(
        source.path(),
        &["plugins/cleanup-plugin"],
        &["skills/cleanup-skill"],
    );

    let prepared = manager
        .prepare_install(
            source.path(),
            PackageVisibility::Scope,
            Some(workspace.path()),
            false,
        )
        .unwrap();
    manager.install(prepared).unwrap();

    let scope_paths =
        resolve_layer_paths(&cfg, PackageVisibility::Scope, Some(workspace.path())).unwrap();
    let uninstall = manager
        .uninstall(
            "combo-package",
            PackageVisibility::Scope,
            Some(workspace.path()),
        )
        .unwrap();
    assert_eq!(uninstall.record.name, "combo-package");
    assert!(!scope_paths.plugins_dir.join("cleanup-plugin").exists());
    assert!(!scope_paths.skills_dir.join("cleanup-skill").exists());
    assert!(load_package_registry(&scope_paths.package_registry_path)
        .packages
        .is_empty());
    assert!(load_plugin_registry(&scope_paths.plugin_registry_path)
        .plugins
        .is_empty());
}
