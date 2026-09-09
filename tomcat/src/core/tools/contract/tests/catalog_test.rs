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

#[test]
fn dispatch_agent_description_enforces_bounded_explorer_usage() {
    let description = desc_of("dispatch_agent");

    for forbidden in [
        "a simple task",
        "project rules or AGENTS/README files",
        "a known file or symbol",
        "confirmation of one implementation",
        "one or two batches of direct-tool calls",
        "self-review or final audit of your own Plan",
        "work already covered by the automatic Plan/Code Reviewer",
    ] {
        assert!(
            description.contains(forbidden),
            "dispatch_agent description should forbid: {forbidden}"
        );
    }

    for required in [
        "Direct `search_files` / `read` calls are the default",
        "(1) the task crosses multiple unknown subsystems",
        "(2) the questions can be investigated independently in parallel",
        "(3) returning the necessary raw reads would materially bloat the parent context",
        "every currently known independent question into one `tasks` array",
        "After reports return, prefer direct tools to fill evidence gaps",
        "Dispatch again only if the reports reveal a new blocker that direct tools cannot answer",
    ] {
        assert!(
            description.contains(required),
            "dispatch_agent description should require: {required}"
        );
    }
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
    assert!(
        hashline.contains("HashMismatch"),
        "hashline_edit description lost its HashMismatch guidance: {hashline}"
    );

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

#[test]
fn edit_schema_exposes_only_canonical_path_and_edits_shape() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "edit")
        .expect("edit catalog entry");
    let schema = (entry.parameters)();
    let properties = &schema["properties"];
    assert!(properties.get("path").is_some());
    assert!(properties.get("edits").is_some());
    assert!(properties.get("old_content").is_none());
    assert!(properties.get("new_content").is_none());
    assert!(properties.get("files").is_none());
    assert_eq!(schema["required"], serde_json::json!(["path", "edits"]));
}

#[test]
fn edit_guidelines_mention_batch_multi_file() {
    let guidelines = render_tool_guidelines_with_policy(true);
    assert!(guidelines.contains("multiple independent files"));
    assert!(guidelines.contains("SAME tool round"));
}

#[test]
fn green_build_evidence_schema_requires_recorded_command_and_task() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "update_plan")
        .expect("update_plan catalog entry");
    let schema = (entry.parameters)();
    let evidence = &schema["properties"]["green_build_evidence"]["items"];
    assert_eq!(
        evidence["required"],
        serde_json::json!(["command", "task_id"])
    );
}

#[test]
fn todo_set_status_schema_accepts_evidence() {
    let update_plan = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "update_plan")
        .expect("update_plan catalog entry");
    let update_plan_schema = (update_plan.parameters)();
    let set_status = &update_plan_schema["properties"]["ops"]["items"]["oneOf"][1];
    assert_eq!(set_status["properties"]["evidence"]["type"], "array");
    assert_eq!(
        set_status["properties"]["evidence"]["items"]["type"],
        "string"
    );

    let todos = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "todos")
        .expect("todos catalog entry");
    let todos_schema = (todos.parameters)();
    let todos_set_status = &todos_schema["properties"]["ops"]["items"]["oneOf"][1];
    assert!(
        todos_set_status["properties"].get("evidence").is_none(),
        "plan completion evidence must not leak into the session-local todos contract"
    );
}

/// 聚合去重：跨工具规则只说一遍，且包含被测试依赖的关键锚点。
#[test]
fn tool_guidelines_aggregate_dedup_and_contain_key_anchors() {
    let g = render_tool_guidelines_with_policy(true);

    // 关键跨工具锚点（原 tool_instructions 内联，现从 guidelines 注入）。
    assert!(g.contains("Default file-edit workflow: read -> edit"));
    assert!(g.contains("read(hashline=true) -> hashline_edit"));
    assert!(g.contains("prefer edit for prose and Markdown"));
    assert!(g.contains("never mix anchors from separate reads"));
    assert!(g.contains("line-count changes do not shift other batch anchors"));
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
fn replay_safety_is_independent_from_read_only() {
    let ask_question = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "ask_question")
        .expect("ask_question catalog entry");
    let read = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "read")
        .expect("read catalog entry");
    let edit = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "edit")
        .expect("edit catalog entry");

    assert!(ask_question.requires_user_interaction);
    assert!(
        super::super::catalog::is_replay_safe_tool(ask_question.name),
        "ask_question only re-shows a durable user interaction and is safe to continue"
    );
    assert!(
        read.read_only && !super::super::catalog::is_replay_safe_tool(read.name),
        "read-only does not prove an interrupted invocation did not already run"
    );
    assert!(
        !super::super::catalog::is_replay_safe_tool(edit.name),
        "mutation tools must never be silently replayed"
    );
    assert!(
        !super::super::catalog::is_replay_safe_tool("third_party_tool"),
        "unknown tools are fail-closed"
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
fn bash_schema_exposes_only_a_single_command_string() {
    let entry = BUILTIN_TOOL_CATALOG
        .iter()
        .find(|entry| entry.name == "bash")
        .unwrap();
    let schema = (entry.parameters)();
    assert!(schema["properties"].get("foreground_wait_ms").is_some());
    assert!(schema["properties"].get("timeout_ms").is_none());
    assert!(schema["properties"].get("args").is_none());
    assert!(schema["properties"]["command"]["description"]
        .as_str()
        .is_some_and(|description| {
            description.contains("Complete shell command line")
                && description.contains("pipes")
                && description.contains("sh -c")
        }));
}
