use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    build_function_definitions, derive_default_category, render_tool_guidelines_with_policy,
    ToolCategory, BUILTIN_TOOL_CATALOG,
};
use crate::core::permission::PermissionScope;

fn desc_of(name: &str) -> &'static str {
    BUILTIN_TOOL_CATALOG
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} catalog entry"))
        .description
}

/// 成功率红线：精简 description 时，影响调用成功率的格式/枚举/互斥/唯一性/坑点
/// 必须留在原地。删了这些模型就会调错工具。
#[test]
fn success_rate_redline_keeps_critical_usage_in_descriptions() {
    // hashline_edit：行号#哈希锚点格式 + HashMismatch 坑点。
    let hashline = desc_of("hashline_edit");
    assert!(
        hashline.contains("<line>#<2char>"),
        "hashline_edit 必须保留 `<line>#<2char>` 锚点格式"
    );
    assert!(hashline.contains("HashMismatch"));

    // edit：唯一性/精确匹配约束（Ambiguous）+ ORIGINAL 快照语义 + Stale。
    let edit = desc_of("edit");
    assert!(edit.contains("Ambiguous"));
    assert!(edit.contains("exactly once"));
    assert!(edit.contains("ORIGINAL"));
    assert!(edit.contains("Stale"));

    // search_files：保留 target/usage 语义，不泄露底层实现细节。
    let search = desc_of("search_files");
    assert!(search.contains("target=content"));
    assert!(search.contains("target=files"));
    assert!(!search.contains("`rg`"));
    assert!(!search.contains("`fd`"));
    assert!(search.contains("regex"));

    // update_plan：ops 三种 kind 枚举。
    let update_plan = desc_of("update_plan");
    assert!(update_plan.contains("upsert"));
    assert!(update_plan.contains("set_status"));
    assert!(update_plan.contains("remove"));

    // config_set：append-only vs scalar 语义（删了会写错字段）。
    let config_set = desc_of("config_set");
    assert!(config_set.contains("append"));
    assert!(config_set.contains("scalar"));

    // bash：run_in_background 语义（后台任务全套用法在系统段）。
    assert!(desc_of("bash").contains("run_in_background"));
    let task_output = desc_of("task_output");
    assert!(task_output.contains("600000"));
    assert!(task_output.contains("5000-600000ms"));
}

/// 聚合去重：跨工具规则只说一遍，且包含被测试依赖的关键锚点。
#[test]
fn tool_guidelines_aggregate_dedup_and_contain_key_anchors() {
    let g = render_tool_guidelines_with_policy(true);

    // 关键跨工具锚点（原 tool_instructions 内联，现从 guidelines 注入）。
    assert!(g.contains("Default file-edit workflow: read -> edit"));
    assert!(g.contains("read(hashline=true) -> hashline_edit"));
    assert!(g.contains("never include display prefixes"));
    assert!(g.contains("prefer it over bash with grep/find/ls -R"));
    assert!(g.contains("Only claim you can access"));
    // 新增：path:line 引用 + 禁 codeblock 假编辑。
    assert!(g.contains("`path:line`"));
    assert!(g.contains("never print a code block pretending to edit"));
    // UI 内核（#8）已移出工具 guidelines，改由 core_identity/planner 承载。
    assert!(!g.contains("user-experience-first"));

    // 去重：write + edit 共享的 no-fake-edit 只出现一次。
    assert_eq!(
        g.matches("never print a code block pretending to edit")
            .count(),
        1,
        "no-fake-edit guideline 应去重为一条"
    );
    // 每条 guideline 以 `- ` 起头。
    assert!(g.lines().all(|l| l.starts_with("- ")));
}

#[test]
fn task_output_schema_matches_long_wait_slice_contract() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "task_output")
        .expect("task_output catalog entry");
    let schema = (entry.parameters)();
    let timeout = &schema["properties"]["wait_ms"];
    assert_eq!(timeout["minimum"].as_i64(), Some(0));
    assert_eq!(timeout["maximum"].as_i64(), Some(600000));
    assert!(
        schema["properties"].get("timeout_ms").is_none(),
        "legacy task_output timeout_ms must not be accepted"
    );
    let description = timeout["description"].as_str().unwrap_or("");
    assert!(description.contains("5000-600000ms"));
    assert!(description.contains("Ignored when block=false"));
}

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
        !entry.description.contains("`rg`") && !entry.description.contains("`fd`"),
        "description 不应泄露 search_files 的系统实现细节"
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

#[test]
fn bash_schema_rejects_legacy_timeout_name() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "bash")
        .unwrap();
    let schema = (entry.parameters)();
    assert!(schema["properties"].get("foreground_wait_ms").is_some());
    assert!(schema["properties"].get("timeout_ms").is_none());
}
