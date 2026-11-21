use super::{materialize_builtin_plugins, BuiltinPluginsStatus, DIRECTORY, FILES};
use crate::ext::bundle_plugin_from_path;
use crate::AppConfig;

#[test]
fn creates_refreshes_and_keeps_unchanged_builtin_files() {
    let home = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(home.path().to_string_lossy().into_owned());
    let plugin = home.path().join("plugins").join(DIRECTORY);
    assert_eq!(
        materialize_builtin_plugins(&cfg).unwrap(),
        BuiltinPluginsStatus::Created
    );
    for (name, bytes) in FILES {
        assert_eq!(std::fs::read(plugin.join(name)).unwrap(), bytes);
    }

    std::fs::write(plugin.join("plugin.json"), b"not the managed manifest").unwrap();
    std::fs::write(plugin.join("main.js"), b"// edit").unwrap();
    std::fs::remove_file(plugin.join("README.md")).unwrap();
    assert_eq!(
        materialize_builtin_plugins(&cfg).unwrap(),
        BuiltinPluginsStatus::UpdatedExistingPlugin
    );
    let timestamps: Vec<_> = FILES
        .iter()
        .map(|(name, bytes)| {
            let path = plugin.join(name);
            assert_eq!(std::fs::read(&path).unwrap(), *bytes);
            std::fs::metadata(path).unwrap().modified().unwrap()
        })
        .collect();

    assert_eq!(
        materialize_builtin_plugins(&cfg).unwrap(),
        BuiltinPluginsStatus::AlreadyPresent
    );
    for ((name, _), modified) in FILES.iter().zip(timestamps) {
        assert_eq!(
            std::fs::metadata(plugin.join(name))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
    }
}

#[test]
fn bundled_plugin_is_discovered_without_a_registry_entry_unless_disabled() {
    let home = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(home.path().to_string_lossy().into_owned());
    materialize_builtin_plugins(&cfg).unwrap();
    let registry = home.path().join("plugins/registry.json");
    let discover = || crate::ext::plugin::PluginCatalog::discover_host_root(&cfg).unwrap();
    std::fs::write(
        &registry,
        r#"{"plugins":[{"id":"another-plugin","enabled":true}]}"#,
    )
    .unwrap();
    assert!(discover().get(super::ID).is_some());

    std::fs::write(
        &registry,
        serde_json::json!({"plugins":[{"id": super::ID, "enabled": false}]}).to_string(),
    )
    .unwrap();
    assert!(discover().get(super::ID).is_none());
}

#[test]
fn declarations_match_network_grants_and_catalog() {
    let manifest = super::super::parse_manifest(include_str!(
        "../../../../assets/plugins/web-search-backends/plugin.json"
    ))
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let models = crate::core::llm::ModelCatalog::load_from_path(
        &AppConfig::default(),
        temp.path().join("models.toml"),
    )
    .unwrap();
    let metadata = manifest.functions[0].metadata.as_ref().unwrap();
    let declarations =
        crate::core::tools::web_search::declarations::parse_declarations(Some(metadata));
    assert_eq!(
        declarations.len(),
        metadata["backends"].as_array().unwrap().len()
    );
    for row in &declarations {
        if let Some(model) = &row.model {
            assert!(models.lookup(model).is_some());
        } else {
            let secret = row.secret.as_ref().expect("REST backends declare a secret");
            assert!(manifest.required_secrets.contains(secret));
            assert!(manifest.allowed_hosts.contains(row.host.as_ref().unwrap()));
        }
        assert!(
            row.key_status(&models, true).is_some(),
            "builtin models must not share a key with {} at another address",
            row.id
        );
    }
}

#[test]
fn committed_main_matches_source_bundle() {
    let plugin =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/plugins/web-search-backends");
    let bundled = bundle_plugin_from_path(&plugin).expect("bundle builtin src");
    let committed = std::fs::read_to_string(plugin.join("main.js")).expect("read committed bundle");
    assert_eq!(
        committed, bundled.output,
        "rebuild the plugin bundle after editing src/"
    );
}
