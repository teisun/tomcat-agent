use super::super::{FunctionRegistry, ManifestFunction, RegisteredFunction};

fn manifest_function(point: &str, function: &str) -> ManifestFunction {
    ManifestFunction {
        point: point.to_string(),
        function: function.to_string(),
    }
}

#[test]
fn function_registry_groups_functions_by_point() {
    let registry = FunctionRegistry::new();
    let plugin_root = tempfile::tempdir().expect("tempdir");
    registry.register_plugin_functions(
        "plugin-a",
        plugin_root.path(),
        &[
            manifest_function("test.echo", "echo_host"),
            manifest_function("test.counter", "next_count"),
        ],
    );

    let echo = registry.functions_for_point("test.echo");
    let counter = registry.functions_for_point("test.counter");

    assert_eq!(echo.len(), 1);
    assert_eq!(echo[0].plugin_id, "plugin-a");
    assert_eq!(echo[0].function, "echo_host");
    assert_eq!(counter.len(), 1);
    assert_eq!(counter[0].function, "next_count");
}

#[test]
fn function_registry_supports_same_plugin_multi_point() {
    let registry = FunctionRegistry::new();
    let plugin_root = tempfile::tempdir().expect("tempdir");
    registry.register_plugin_functions(
        "plugin-a",
        plugin_root.path(),
        &[
            manifest_function("test.echo", "echo_host"),
            manifest_function("test.echo.secondary", "echo_secondary"),
        ],
    );

    assert_eq!(registry.functions_for_point("test.echo").len(), 1);
    assert_eq!(registry.functions_for_point("test.echo.secondary").len(), 1);
}

#[test]
fn function_registry_direct_registration_allows_multiple_candidates_per_point() {
    let registry = FunctionRegistry::new();
    let plugin_a_root = tempfile::tempdir().expect("tempdir a");
    let plugin_b_root = tempfile::tempdir().expect("tempdir b");
    registry.register_plugin_functions(
        "plugin-a",
        plugin_a_root.path(),
        &[manifest_function("test.echo", "echo_a")],
    );
    registry.register_plugin_functions(
        "plugin-b",
        plugin_b_root.path(),
        &[manifest_function("test.echo", "echo_b")],
    );

    let functions = registry.functions_for_point("test.echo");
    let ordered = functions
        .iter()
        .map(|entry| entry.plugin_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(functions.len(), 2);
    assert_eq!(ordered, vec!["plugin-a", "plugin-b"]);
}

#[test]
fn function_registry_replace_all_overwrites_previous_snapshot() {
    let registry = FunctionRegistry::new();
    let plugin_root = tempfile::tempdir().expect("tempdir");
    registry.register_plugin_functions(
        "plugin-a",
        plugin_root.path(),
        &[manifest_function("test.echo", "echo_a")],
    );

    registry.replace_all([RegisteredFunction {
        plugin_id: "winner-plugin".to_string(),
        plugin_root: plugin_root.path().to_path_buf(),
        point: "test.echo".to_string(),
        function: "winner_echo".to_string(),
    }]);

    let functions = registry.functions_for_point("test.echo");
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].plugin_id, "winner-plugin");
    assert_eq!(functions[0].function, "winner_echo");
}

#[test]
fn function_registry_remove_by_plugin_cleans_all_points() {
    let registry = FunctionRegistry::new();
    let plugin_root = tempfile::tempdir().expect("tempdir");
    registry.register_plugin_functions(
        "plugin-a",
        plugin_root.path(),
        &[
            manifest_function("test.echo", "echo_host"),
            manifest_function("test.counter", "next_count"),
        ],
    );

    let removed = registry.remove_by_plugin("plugin-a");
    assert_eq!(removed, 2);
    assert!(registry.functions_for_point("test.echo").is_empty());
    assert!(registry.functions_for_point("test.counter").is_empty());
}

#[test]
fn function_registry_unknown_point_returns_empty() {
    let registry = FunctionRegistry::new();
    assert!(registry.functions_for_point("missing.point").is_empty());
}
