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
    let execute_bash = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "execute_bash")
        .expect("execute_bash catalog entry");
    assert!(matches!(
        execute_bash.scope,
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
