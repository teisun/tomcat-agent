use super::{
    add_global_server, connector_config_key, global_mcp_path, load_server_definitions,
    load_servers, project_mcp_path, set_global_tool_enabled, McpConfigSource, McpServerConfig,
    ToolFilterMatcher, IGNORED_SERVER_KEYS,
};
use crate::infra::config::get_work_dir;
use crate::AppConfig;

#[derive(Clone)]
struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("lock test log")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn minimal_cursor_style_server_uses_optional_field_defaults() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
    std::fs::write(
        global,
        r#"{"mcpServers":{"browser":{"command":"npx","args":["-y","browser-mcp@1.2.3"]}}}"#,
    )
    .expect("write config");

    let servers = load_servers(&cfg, Some(&workspace)).expect("parse minimal MCP config");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "browser");
    assert_eq!(servers[0].config.command, "npx");
    assert_eq!(servers[0].config.args, ["-y", "browser-mcp@1.2.3"]);
    assert_eq!(servers[0].source, McpConfigSource::Global);
    assert!(servers[0].config.env.is_empty());
    assert!(servers[0].config.cwd.is_none());
    assert!(servers[0].config.tool_filter.include.is_empty());
    assert!(servers[0].config.tool_filter.exclude.is_empty());
}
#[test]
fn legacy_server_keys_are_tolerated_without_rewriting_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = global_mcp_path(&cfg).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let contents = serde_json::json!({
        "mcpServers": {
            "legacy": {
                "command": "node",
                "startupTimeoutMs": 30_000,
                "callTimeoutMs": 90_000,
                "maxConcurrentCalls": 16,
                "trusted": true,
                "integrity": "obsolete"
            }
        }
    })
    .to_string();
    std::fs::write(&path, &contents).unwrap();

    let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer({
            let logs = std::sync::Arc::clone(&logs);
            move || SharedLogWriter(std::sync::Arc::clone(&logs))
        })
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let servers = load_servers(&cfg, None).expect("legacy keys must not block loading");
    assert_eq!(servers.len(), 1);
    let serialized = serde_json::to_string(&servers[0].config).unwrap();
    let rendered = String::from_utf8(logs.lock().expect("lock test log").clone())
        .expect("warnings must be UTF-8");
    for key in [
        "startupTimeoutMs",
        "callTimeoutMs",
        "maxConcurrentCalls",
        "trusted",
        "integrity",
    ] {
        assert!(
            !serialized.contains(key),
            "ignored legacy key leaked into output: {key}"
        );
        assert!(
            IGNORED_SERVER_KEYS
                .iter()
                .any(|(ignored, _)| ignored == &key),
            "legacy key must retain its ignored-with-WARN classification: {key}"
        );
        assert!(
            rendered.contains(&format!("ignored_key={key}")),
            "missing ignored-key warning for {key}: {rendered}"
        );
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
}

#[test]
fn truly_unknown_fields_are_silently_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = global_mcp_path(&cfg).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"mcpServers":{"future":{"command":"node","futureField":{"version":2}}}}"#,
    )
    .unwrap();

    let servers = load_servers(&cfg, None).expect("unknown server fields must be ignored");
    assert_eq!(servers[0].name, "future");
    assert_eq!(servers[0].config.command, "node");
}

#[test]
fn single_tool_mutation_uses_exact_patterns_and_preserves_batch_rules() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let server: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "node", "args": []
    }))
    .expect("server configuration");
    add_global_server(&cfg, "fake".to_string(), server).expect("add server");

    assert!(set_global_tool_enabled(&cfg, "fake", "capture", false).expect("disable capture"));
    let disabled = load_servers(&cfg, None).expect("load disabled server");
    assert_eq!(disabled[0].config.tool_filter.exclude, ["capture"]);
    assert!(!ToolFilterMatcher::compile(&disabled[0].config.tool_filter)
        .expect("compile disabled filter")
        .allows("capture"));

    assert!(set_global_tool_enabled(&cfg, "fake", "capture", true).expect("re-enable capture"));
    let enabled = load_servers(&cfg, None).expect("load enabled server");
    assert!(enabled[0].config.tool_filter.exclude.is_empty());
    assert!(ToolFilterMatcher::compile(&enabled[0].config.tool_filter)
        .expect("compile enabled filter")
        .allows("capture"));

    let mixed_include = r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"include":["status*"]}}}}"#;
    std::fs::write(global_mcp_path(&cfg).expect("global config"), mixed_include)
        .expect("write mixed include rule");
    assert!(set_global_tool_enabled(&cfg, "fake", "capture", true)
        .expect("add exact capture include beside a glob"));
    let mixed_enabled = load_servers(&cfg, None).expect("load mixed include rule");
    assert_eq!(
        mixed_enabled[0].config.tool_filter.include,
        ["status*", "capture"]
    );
    assert!(!set_global_tool_enabled(&cfg, "fake", "capture", true)
        .expect("same desired state is a no-op"));

    let special_name = "capture[1]";
    std::fs::write(
        global_mcp_path(&cfg).expect("global config"),
        r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"include":["status*"]}}}}"#,
    )
    .expect("reset mixed include rule");
    assert!(set_global_tool_enabled(&cfg, "fake", special_name, true)
        .expect("escape special-character tool name"));
    let special_enabled = load_servers(&cfg, None).expect("load special tool rule");
    assert!(
        ToolFilterMatcher::compile(&special_enabled[0].config.tool_filter)
            .expect("compile special tool rule")
            .allows(special_name)
    );
    let path = global_mcp_path(&cfg).expect("global config");
    let batch_rule = r#"{"mcpServers":{"fake":{"command":"node","args":[],"toolFilter":{"exclude":["capture*"]}}}}"#;
    std::fs::write(&path, batch_rule).expect("write batch rule");
    let error = set_global_tool_enabled(&cfg, "fake", "capture", true)
        .expect_err("a single tool must not weaken a batch rule");
    assert!(error.to_string().contains(&crate::infra::i18n::tr_in(
        crate::infra::i18n::Locale::En,
        "mcp.config.batchRule",
        &[]
    )));
    assert_eq!(
        std::fs::read_to_string(path).expect("read unchanged batch rule"),
        batch_rule
    );
}

#[test]
fn no_project_root_loads_global_mcp_without_touching_a_project_path() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(global.parent().expect("global parent")).unwrap();
    std::fs::write(
        global,
        r#"{"mcpServers":{"global-only":{"command":"node","args":[]}}}"#,
    )
    .unwrap();

    let servers = load_servers(&cfg, None).expect("global MCP must work without a project");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "global-only");
    assert_eq!(servers[0].source, McpConfigSource::Global);
}

#[test]
fn project_mcp_path_uses_configured_resource_directory_and_rejects_invalid_values() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let mut cfg = AppConfig::default();

    assert_eq!(
        project_mcp_path(&cfg, &workspace).expect("default project config path"),
        workspace.join(".agents").join("mcp.json")
    );

    cfg.workspace.project_resource_dir = ".workspace-data".to_string();
    assert_eq!(
        project_mcp_path(&cfg, &workspace).expect("custom project config path"),
        workspace.join(".workspace-data").join("mcp.json")
    );

    cfg.workspace.project_resource_dir = "../outside".to_string();
    assert!(project_mcp_path(&cfg, &workspace).is_err());
}

#[test]
fn project_server_overrides_global_server_with_same_name() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let workspace = temp.path().join("workspace");
    let mut cfg = AppConfig::default();
    cfg.workspace.project_resource_dir = ".workspace-data".to_string();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let project = project_mcp_path(&cfg, &workspace).expect("project config path");
    std::fs::create_dir_all(project.parent().expect("project config directory"))
        .expect("project config directory");
    let global = get_work_dir(&cfg).expect("work dir").join("mcp.json");
    std::fs::create_dir_all(global.parent().expect("global parent")).expect("global parent");
    std::fs::write(
        global,
        r#"{"mcpServers":{"same":{"command":"global","args":[]}}}"#,
    )
    .expect("write global config");
    std::fs::write(
        project_mcp_path(&cfg, &workspace).expect("project config path"),
        r#"{"mcpServers":{"same":{"command":"project","args":[]}}}"#,
    )
    .expect("write project config");

    let servers = load_servers(&cfg, Some(&workspace)).expect("load merged config");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].config.command, "project");
    assert_eq!(servers[0].source, McpConfigSource::Project);
    assert!(
        !workspace.join(".agents").join("mcp.json").exists(),
        "loading the custom path must not fall back to the default project path"
    );
}

#[test]
fn http_server_requires_exactly_one_transport_selector() {
    let both: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "node",
        "args": [],
        "url": "https://example.test/mcp"
    }))
    .expect("config");
    assert!(both.validate("both").is_err());

    let http: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://example.test/mcp"
    }))
    .expect("HTTP config");
    assert!(http.validate("http").is_ok());

    let invalid: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "file:///tmp/mcp"
    }))
    .expect("invalid URL config parses before validation");
    assert!(invalid.validate("invalid").is_err());
}

#[test]
fn config_key_is_stable_for_path_aliases_and_separates_sources() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let config_dir = temp.path().join("configs");
    std::fs::create_dir_all(&config_dir).expect("config directory");
    let direct = config_dir.join("mcp.json");
    let alias = config_dir.join("subdir").join("..").join("mcp.json");
    let same = connector_config_key(&direct, "server").expect("direct key");
    assert_eq!(
        same,
        connector_config_key(&alias, "server").expect("alias key")
    );
    assert_ne!(
        same,
        connector_config_key(&direct, "other").expect("name key")
    );
    assert_ne!(
        same,
        connector_config_key(&temp.path().join("other.json"), "server").expect("path key")
    );
}

#[test]
fn unknown_fields_are_ignored_without_rewriting_the_file() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = global_mcp_path(&cfg).expect("global config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
    let raw = r#"{"$schema":"https://example.invalid/mcp.schema.json","comment":"kept until a save","mcpServers":{"browser":{"command":"node","args":[],"removedField":true}}}"#;
    std::fs::write(&path, raw).expect("write config with unknown fields");

    let servers = load_servers(&cfg, None).expect("unknown fields must be ignored");
    assert_eq!(servers[0].config.command, "node");
    assert_eq!(std::fs::read_to_string(path).expect("read source"), raw);
}

#[test]
fn unknown_fields_inside_tool_filter_remain_errors() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = global_mcp_path(&cfg).expect("global config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
    let raw = r#"{"mcpServers":{"browser":{"command":"node","toolFilter":{"unknownRule":true}}}}"#;
    std::fs::write(&path, raw).expect("write invalid nested configuration");

    let error = load_servers(&cfg, None)
        .expect_err("toolFilter must keep rejecting unknown fields")
        .to_string();
    assert!(error.contains("unknownRule"), "{error}");
    assert_eq!(std::fs::read_to_string(path).expect("read source"), raw);
}

#[test]
fn saving_drops_unrecognized_keys_without_touching_known_ones() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let path = global_mcp_path(&cfg).expect("global config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
    std::fs::write(
        &path,
        r#"{"$schema":"https://example.invalid/mcp.schema.json","mcpServers":{"browser":{"command":"node","args":["bridge"],"futureField":true}}}"#,
    )
    .expect("write config with unknown fields");

    assert!(set_global_tool_enabled(&cfg, "browser", "capture", false)
        .expect("save a known configuration mutation"));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read rewritten configuration"))
            .expect("parse rewritten configuration");
    assert!(saved.get("$schema").is_none());
    assert!(saved["mcpServers"]["browser"].get("futureField").is_none());
    assert_eq!(saved["mcpServers"]["browser"]["command"], "node");
    assert_eq!(
        saved["mcpServers"]["browser"]["args"],
        serde_json::json!(["bridge"])
    );
    assert_eq!(
        saved["mcpServers"]["browser"]["toolFilter"]["exclude"],
        serde_json::json!(["capture"])
    );
}

#[test]
fn add_rejects_duplicate_without_overwriting_existing_server() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let initial: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "node", "args": ["first"]
    }))
    .expect("initial config");
    add_global_server(&cfg, "same".to_string(), initial).expect("first add");
    let replacement: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "node", "args": ["replacement"]
    }))
    .expect("replacement config");

    let error = add_global_server(&cfg, "same".to_string(), replacement)
        .expect_err("duplicate add must fail");
    assert!(error.to_string().contains(&crate::infra::i18n::tr_in(
        crate::infra::i18n::Locale::En,
        "mcp.config.exists",
        &[("name", "same"), ("path", "")]
    )));
    let servers = load_servers(&cfg, None).expect("read existing server");
    assert_eq!(servers[0].config.args, ["first"]);
}

#[test]
fn concurrent_adds_keep_both_current_format_servers() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        for name in ["left", "right"] {
            let cfg = cfg.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                let server: McpServerConfig = serde_json::from_value(serde_json::json!({
                    "command": "node", "args": [name]
                }))
                .expect("server config");
                add_global_server(&cfg, name.to_string(), server).expect("concurrent add");
            });
        }
    });
    let names = load_server_definitions(&cfg, None)
        .expect("read both adds")
        .into_iter()
        .map(|server| server.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["left", "right"]);
}

// Source removal regression cases.
#[test]
fn removing_a_disabled_global_source_only_erases_its_own_credentials() {
    use crate::core::connector::mcp::oauth::OAuthTokenStore;
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    cfg.connector.enabled = false;
    cfg.connector.disabled = vec!["same".into()];
    let workspace = temp.path().join("project");
    let server: McpServerConfig =
        serde_json::from_value(serde_json::json!({"command":"node"})).unwrap();
    add_global_server(&cfg, "same".into(), server.clone()).unwrap();
    super::add_project_server(&cfg, &workspace, "same".into(), server).unwrap();
    let global = global_mcp_path(&cfg).unwrap();
    let project = project_mcp_path(&cfg, &workspace).unwrap();
    let global_key = connector_config_key(&global, "same").unwrap();
    let project_key = connector_config_key(&project, "same").unwrap();
    let tokens = OAuthTokenStore::open(&cfg).unwrap();
    tokens
        .save_static_bearer(&global_key, "global-fixture".into(), None)
        .unwrap();
    tokens
        .save_static_bearer(&project_key, "project-fixture".into(), None)
        .unwrap();
    assert!(super::remove_global_server(&cfg, "same").unwrap());
    assert!(tokens.load(&global_key).unwrap().is_none());
    assert!(tokens.load(&project_key).unwrap().is_some());
    assert!(super::read_mcp_file(&global)
        .unwrap()
        .mcp_servers
        .is_empty());
    assert!(super::read_mcp_file(&project)
        .unwrap()
        .mcp_servers
        .contains_key("same"));
    assert!(super::remove_project_server(&cfg, &workspace, "same").unwrap());
    assert!(tokens.load(&project_key).unwrap().is_none());
}

#[test]
fn removal_reports_credential_failure_and_can_retry_after_config_was_removed() {
    use crate::core::connector::mcp::oauth::OAuthTokenStore;
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let server = serde_json::from_value(serde_json::json!({"command":"node"})).unwrap();
    add_global_server(&cfg, "retry".into(), server).unwrap();
    let config = global_mcp_path(&cfg).unwrap();
    let key = connector_config_key(&config, "retry").unwrap();
    let tokens = OAuthTokenStore::open(&cfg).unwrap();
    tokens
        .save_static_bearer(&key, "retry-fixture".into(), None)
        .unwrap();
    let credential_path = temp.path().join("connector-oauth.json");
    let original = std::fs::read(&credential_path).unwrap();
    std::fs::write(&credential_path, "invalid-json").unwrap();
    assert!(super::remove_global_server(&cfg, "retry").is_err());
    assert!(super::read_mcp_file(&config)
        .unwrap()
        .mcp_servers
        .is_empty());
    std::fs::write(&credential_path, original).unwrap();
    assert!(!super::remove_global_server(&cfg, "retry").unwrap());
    assert!(tokens.load(&key).unwrap().is_none());
}
