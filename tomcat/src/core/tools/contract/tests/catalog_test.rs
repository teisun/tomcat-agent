use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    build_function_definitions, derive_default_category, ToolCategory, BUILTIN_TOOL_CATALOG,
};
use crate::core::permission::PermissionScope;

#[test]
fn catalog_entries_are_well_formed() {
    for entry in BUILTIN_TOOL_CATALOG {
        assert!(!entry.name.trim().is_empty(), "tool name is required");
        assert!(
            entry.description.ends_with('\n'),
            "{} description must end with one newline",
            entry.name
        );
        assert!(
            !entry.description.trim().is_empty(),
            "{} description is required",
            entry.name
        );
        assert_schema_has_parameter_descriptions(entry.name, &(entry.parameters)());
    }
}

#[test]
fn catalog_and_function_definitions_have_same_names() {
    let catalog_names: BTreeSet<_> = BUILTIN_TOOL_CATALOG
        .iter()
        .map(|entry| entry.name)
        .collect();
    let definitions = build_function_definitions();
    let definition_names: BTreeSet<_> = definitions
        .iter()
        .map(|definition| {
            definition["function"]["name"]
                .as_str()
                .expect("function.name")
                .to_string()
        })
        .collect();

    let catalog_names: BTreeSet<_> = catalog_names.into_iter().map(str::to_string).collect();
    assert_eq!(catalog_names, definition_names);
    assert_eq!(BUILTIN_TOOL_CATALOG.len(), definitions.len());
}

#[test]
fn catalog_scope_and_category_contracts_hold() {
    let bash = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "bash")
        .expect("bash catalog entry");
    assert!(matches!(
        bash.scope,
        PermissionScope::Bash | PermissionScope::BashApproval
    ));

    for entry in BUILTIN_TOOL_CATALOG {
        if entry.category.is_none() {
            assert_eq!(
                entry.effective_category(),
                derive_default_category(entry.scope),
                "{} category should derive from scope",
                entry.name
            );
        }
    }

    for name in ["config_get", "config_set"] {
        let entry = BUILTIN_TOOL_CATALOG
            .iter()
            .find(|entry| entry.name == name)
            .expect("config catalog entry");
        assert_eq!(entry.category, Some(ToolCategory::Config));
    }
}

#[test]
fn search_files_catalog_contract_matches_plan() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "search_files")
        .expect("search_files catalog entry");

    assert_eq!(entry.scope, PermissionScope::Read);
    assert!(entry.read_only);
    assert!(!entry.destructive);

    let schema = (entry.parameters)();
    let properties = schema["properties"].as_object().expect("properties");
    assert_eq!(
        properties["target"]["enum"]
            .as_array()
            .expect("target enum")
            .len(),
        2
    );
    assert_eq!(
        properties["output_mode"]["enum"]
            .as_array()
            .expect("output mode enum")
            .len(),
        3
    );
    assert!(
        properties.get("multiline").is_none(),
        "首版 schema 不应暴露 multiline"
    );
    assert!(
        entry.description.contains("rg") && entry.description.contains("fd"),
        "description 应说明系统二进制依赖"
    );
}

#[test]
fn web_search_registered() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "web_search")
        .expect("web_search catalog entry");

    assert_eq!(entry.scope, PermissionScope::Read);
    assert_eq!(entry.category, Some(ToolCategory::Exec));
    assert!(entry.read_only);
    assert!(!entry.destructive);
    assert!(entry.description.contains("web_fetch"));
    assert!(entry.description.contains("automatic fallback"));
    assert!(entry.description.contains("source attribution"));

    let schema = (entry.parameters)();
    let properties = schema["properties"].as_object().expect("properties");
    assert_eq!(properties.len(), 6, "web_search should expose 6 fields");
    assert_eq!(
        schema["required"].as_array().expect("required"),
        &vec![Value::String("query".to_string())]
    );
    assert_eq!(
        properties["count"]["maximum"].as_i64(),
        Some(20),
        "count should be capped at 20"
    );
    assert_eq!(
        properties["freshness"]["enum"]
            .as_array()
            .expect("freshness enum")
            .len(),
        5
    );
}

#[test]
fn web_fetch_registered() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "web_fetch")
        .expect("web_fetch catalog entry");

    assert_eq!(entry.scope, PermissionScope::Read);
    assert_eq!(entry.category, Some(ToolCategory::Exec));
    assert!(entry.read_only);
    assert!(!entry.destructive);
    assert!(entry.description.contains("web_search"));
    assert!(entry.description.contains("redirect"));
    assert!(entry.description.contains("persisted_output_path"));

    let schema = (entry.parameters)();
    let properties = schema["properties"].as_object().expect("properties");
    assert_eq!(properties.len(), 3, "web_fetch should expose 3 fields");
    assert_eq!(
        schema["required"].as_array().expect("required"),
        &vec![Value::String("url".to_string())]
    );
    assert_eq!(
        properties["format"]["enum"]
            .as_array()
            .expect("format enum")
            .len(),
        2
    );
}

#[test]
fn load_skill_registered() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "load_skill")
        .expect("load_skill catalog entry");

    assert_eq!(entry.scope, PermissionScope::Read);
    assert!(entry.read_only);
    assert!(!entry.destructive);
    assert!(entry.description.contains("skill body"));
    assert!(entry.description.contains("permission gate"));

    let schema = (entry.parameters)();
    let properties = schema["properties"].as_object().expect("properties");
    assert_eq!(
        schema["required"].as_array().expect("required"),
        &vec![Value::String("name".to_string())]
    );
    assert!(properties.contains_key("file"));
}

fn assert_schema_has_parameter_descriptions(tool_name: &str, schema: &Value) {
    assert_eq!(
        schema["type"].as_str(),
        Some("object"),
        "{} schema must be an object",
        tool_name
    );
    let properties = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("{} schema.properties must be object", tool_name));
    for (name, prop) in properties {
        let description = prop["description"].as_str().unwrap_or("");
        assert!(
            !description.trim().is_empty(),
            "{}.{} needs a parameter description",
            tool_name,
            name
        );
    }
}
