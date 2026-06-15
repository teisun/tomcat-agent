use super::super::{PluginActivation, PluginCatalog, PluginSource};
use crate::AppConfig;
use std::fs;
use std::path::Path;

#[test]
fn parse_manifest_supports_static_tools_and_activation() {
    let json = r#"{
        "id": "demo",
        "name": "Demo",
        "version": "0.1.0",
        "description": "demo plugin",
        "author": "tester",
        "main": "index.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [
            {
                "name": "echo",
                "description": "Echo params",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    }
                }
            }
        ],
        "events": ["session_start"],
        "activation": "session"
    }"#;

    let manifest = super::super::parse_manifest(json).expect("manifest should parse");
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.tools[0].name, "echo");
    assert_eq!(manifest.events, vec!["session_start"]);
    assert_eq!(manifest.activation, PluginActivation::Session);
}

#[test]
fn discover_three_tier_first_wins() {
    let work_dir = tempfile::tempdir().expect("create work dir");
    let project_dir = tempfile::tempdir().expect("create project dir");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().into_owned());
    cfg.agent.id = "agent-a".to_string();

    write_plugin(&work_dir.path().join("plugins").join("demo"), "demo", "managed");
    write_plugin(
        &work_dir
            .path()
            .join("agents")
            .join("agent-a")
            .join("plugins")
            .join("demo"),
        "demo",
        "agent",
    );
    write_plugin(
        &project_dir
            .path()
            .join(".tomcat")
            .join("plugins")
            .join("demo"),
        "demo",
        "project",
    );
    write_plugin(
        &work_dir.path().join("plugins").join("managed-only"),
        "managed-only",
        "managed-only",
    );

    let catalog = PluginCatalog::discover(&cfg, project_dir.path()).expect("discover catalog");
    assert_eq!(catalog.len(), 2);

    let demo = catalog.get("demo").expect("demo entry");
    assert_eq!(demo.source, PluginSource::Project);
    assert_eq!(demo.manifest.description, "project");
    assert!(
        catalog
            .warnings
            .iter()
            .any(|warning| warning.contains("plugin_shadowed:demo")),
        "duplicate ids should emit a warning"
    );

    let managed_only = catalog.get("managed-only").expect("managed-only entry");
    assert_eq!(managed_only.source, PluginSource::Managed);
}

fn write_plugin(root: &Path, plugin_id: &str, description: &str) {
    fs::create_dir_all(root).expect("create plugin root");
    fs::write(
        root.join("plugin.json"),
        format!(
            r#"{{
                "id": "{plugin_id}",
                "name": "{plugin_id}",
                "version": "0.1.0",
                "description": "{description}",
                "author": "tester",
                "main": "index.js",
                "requiredPermissions": [],
                "requiredApiVersion": "1.0",
                "tags": []
            }}"#
        ),
    )
    .expect("write manifest");
}
