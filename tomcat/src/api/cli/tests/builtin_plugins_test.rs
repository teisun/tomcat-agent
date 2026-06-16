use super::{ensure_builtin_plugins, merge_manifest_fields, BuiltinPluginsStatus};
use crate::ext::bundle_plugin_from_path;
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

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugin_dir.join("plugin.json")).expect("read plugin.json"),
    )
    .expect("parse plugin.json");
    assert_eq!(
        manifest["requiredPermissions"],
        serde_json::json!(["net:fetch"])
    );
    assert!(manifest["requiredSecrets"]
        .as_array()
        .expect("requiredSecrets array")
        .iter()
        .any(|item| item == "TAVILY_API_KEY"));
    assert!(manifest["allowedHosts"]
        .as_array()
        .expect("allowedHosts array")
        .iter()
        .any(|item| item == "api.tavily.com"));

    let committed_main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("plugins")
            .join("web-search-backends")
            .join("main.js"),
    )
    .expect("read committed builtin main.js");
    assert_eq!(
        std::fs::read_to_string(plugin_dir.join("main.js")).expect("read installed main.js"),
        committed_main
    );
}

#[test]
fn ensure_builtin_plugins_refreshes_runtime_artifacts_and_fills_missing_ones() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = test_config(temp.path());
    let plugin_dir = temp.path().join("plugins").join("web-search-backends");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("main.js"), "// user edited").unwrap();
    std::fs::write(plugin_dir.join("README.md"), "stale readme").unwrap();

    let status = ensure_builtin_plugins(&cfg).expect("builtin plugins updated");
    assert_eq!(status, BuiltinPluginsStatus::UpdatedExistingPlugin);
    let committed_main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("plugins")
            .join("web-search-backends")
            .join("main.js"),
    )
    .expect("read committed builtin main.js");
    assert_eq!(
        std::fs::read_to_string(plugin_dir.join("main.js")).unwrap(),
        committed_main
    );
    assert!(plugin_dir.join("plugin.json").exists());
    assert!(plugin_dir.join("README.md").exists());
    assert!(std::fs::read_to_string(plugin_dir.join("README.md"))
        .unwrap()
        .contains("Official host-function plugin"));

    let status = ensure_builtin_plugins(&cfg).expect("builtin plugins stable");
    assert_eq!(status, BuiltinPluginsStatus::AlreadyPresent);
}

#[test]
fn merge_manifest_fields_backfills_required_arrays_without_overwriting_existing_entries() {
    let mut existing = serde_json::json!({
        "id": "tomcat.web-search-backends",
        "requiredPermissions": ["read"],
        "requiredSecrets": ["BRAVE_API_KEY"]
    });
    let bundled = serde_json::json!({
        "requiredPermissions": ["net:fetch"],
        "requiredSecrets": ["TAVILY_API_KEY", "BRAVE_API_KEY"],
        "allowedHosts": ["api.tavily.com", "api.search.brave.com"]
    });

    let changed = merge_manifest_fields(&mut existing, &bundled).expect("merge manifest fields");
    assert!(changed);
    assert_eq!(
        existing["requiredPermissions"],
        serde_json::json!(["read", "net:fetch"])
    );
    assert_eq!(
        existing["requiredSecrets"],
        serde_json::json!(["BRAVE_API_KEY", "TAVILY_API_KEY"])
    );
    assert_eq!(
        existing["allowedHosts"],
        serde_json::json!(["api.tavily.com", "api.search.brave.com"])
    );
}

#[test]
fn committed_main_js_matches_freshly_bundled_src() {
    let plugin_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("web-search-backends");
    let bundled = bundle_plugin_from_path(&plugin_dir).expect("bundle builtin src");
    let committed = std::fs::read_to_string(plugin_dir.join("main.js")).expect("read committed");
    assert_eq!(
        committed, bundled.output,
        "builtin web-search-backends/main.js 与 src/ 构建产物不一致，请重新运行构建"
    );
}
