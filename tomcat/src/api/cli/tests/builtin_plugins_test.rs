use super::{ensure_builtin_plugins, BuiltinPluginsStatus};
use crate::AppConfig;

fn test_config(work_dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());
    cfg
}

#[test]
fn ensure_builtin_plugins_creates_web_search_backends_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = test_config(temp.path());

    let status = ensure_builtin_plugins(&cfg).expect("builtin plugins created");
    assert_eq!(status, BuiltinPluginsStatus::Created);

    let plugin_dir = temp.path().join("plugins").join("web-search-backends");
    assert!(plugin_dir.join("plugin.json").exists());
    assert!(plugin_dir.join("main.js").exists());
    assert!(plugin_dir.join("README.md").exists());
}

#[test]
fn ensure_builtin_plugins_preserves_existing_files_and_fills_missing_ones() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = test_config(temp.path());
    let plugin_dir = temp.path().join("plugins").join("web-search-backends");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("main.js"), "// user edited").unwrap();

    let status = ensure_builtin_plugins(&cfg).expect("builtin plugins updated");
    assert_eq!(status, BuiltinPluginsStatus::UpdatedMissingFiles);
    assert_eq!(
        std::fs::read_to_string(plugin_dir.join("main.js")).unwrap(),
        "// user edited"
    );
    assert!(plugin_dir.join("plugin.json").exists());
    assert!(plugin_dir.join("README.md").exists());

    let status = ensure_builtin_plugins(&cfg).expect("builtin plugins stable");
    assert_eq!(status, BuiltinPluginsStatus::AlreadyPresent);
}
