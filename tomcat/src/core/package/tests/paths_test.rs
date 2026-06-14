use std::path::Path;

use crate::core::package::{resolve_layer_paths, resolve_runtime_layer_paths, PackageVisibility};
use crate::AppConfig;

fn test_config(work_dir: &Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg
}

#[test]
fn resolve_visibility_roots_global_agent_scope() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let nested = workspace.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let non_canonical_scope_root = nested.join("..");

    let cfg = test_config(work_dir.path());
    let global =
        resolve_layer_paths(&cfg, PackageVisibility::Global, Some(workspace.path())).unwrap();
    let canonical_work_dir = work_dir.path().canonicalize().unwrap();
    assert_eq!(global.plugins_dir, canonical_work_dir.join("plugins"));
    assert_eq!(global.packages_dir, canonical_work_dir.join("packages"));

    let agent =
        resolve_layer_paths(&cfg, PackageVisibility::Agent, Some(workspace.path())).unwrap();
    assert!(
        agent.plugins_dir.ends_with("plugins"),
        "agent plugins dir should end with plugins: {}",
        agent.plugins_dir.display()
    );
    assert!(
        agent.packages_dir.ends_with("packages"),
        "agent packages dir should end with packages: {}",
        agent.packages_dir.display()
    );

    let scope = resolve_layer_paths(
        &cfg,
        PackageVisibility::Scope,
        Some(&non_canonical_scope_root),
    )
    .unwrap();
    let canonical_scope = workspace.path().canonicalize().unwrap();
    assert_eq!(scope.scope_root.as_deref(), Some(canonical_scope.as_path()));
    assert_eq!(scope.layer_root, canonical_scope.join(".tomcat"));
    assert_eq!(scope.plugins_dir, canonical_scope.join(".tomcat/plugins"));
    assert_eq!(
        scope.package_registry_path,
        canonical_scope.join(".tomcat/packages/registry.json")
    );
}

#[test]
fn resolve_runtime_layer_paths_returns_scope_agent_global_order() {
    let work_dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cfg = test_config(work_dir.path());

    let layers = resolve_runtime_layer_paths(&cfg, Some(workspace.path())).unwrap();
    let visibilities = layers
        .iter()
        .map(|layer| layer.visibility)
        .collect::<Vec<_>>();
    assert_eq!(
        visibilities,
        vec![
            PackageVisibility::Scope,
            PackageVisibility::Agent,
            PackageVisibility::Global
        ]
    );
}
