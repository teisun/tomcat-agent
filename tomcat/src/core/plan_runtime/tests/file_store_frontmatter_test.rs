use super::super::file_store::{
    parse_plan_file, serialize_plan_file, PlanError, PlanFile, PlanFileState, TodoItem, TodoStatus,
};
use super::sample_frontmatter;

#[test]
fn plan_file_round_trip_frontmatter() {
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## Goal\n\nThis is the goal.\n".to_string(),
    };
    let text = serialize_plan_file(&plan).expect("serialize");
    assert!(text.starts_with("---\n"));
    assert!(text.contains("plan_id: demo_plan_1"));
    assert!(text.contains("schema_version: 1"));
    let parsed = parse_plan_file(&text).expect("parse");
    assert_eq!(parsed.frontmatter.plan_id, "demo_plan_1");
    assert_eq!(parsed.frontmatter.state, PlanFileState::Planning);
    assert_eq!(parsed.frontmatter.todos.len(), 2);
    assert_eq!(parsed.frontmatter.todos[1].status, TodoStatus::InProgress);
    assert_eq!(parsed.body.trim(), "## Goal\n\nThis is the goal.".trim());
}

#[test]
fn plan_file_round_trip_preserves_unknown_keys() {
    let mut frontmatter = sample_frontmatter();
    let mut extra = serde_yaml::Mapping::new();
    extra.insert(
        serde_yaml::Value::String("future_field".into()),
        serde_yaml::Value::String("forward-compat".into()),
    );
    frontmatter.unknown = extra;
    let plan = PlanFile {
        frontmatter,
        body: String::new(),
    };
    let text = serialize_plan_file(&plan).unwrap();
    assert!(
        text.contains("future_field: forward-compat"),
        "unknown 字段必须 round-trip：\n{text}"
    );
    let parsed = parse_plan_file(&text).unwrap();
    assert_eq!(
        parsed
            .frontmatter
            .unknown
            .get(serde_yaml::Value::String("future_field".into())),
        Some(&serde_yaml::Value::String("forward-compat".into()))
    );
}

#[test]
fn plan_file_missing_required_field_returns_error() {
    let yaml_missing_plan_id =
        "---\ngoal: g\nstate: planning\ncreated_at: t\nschema_version: 1\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_missing_plan_id).expect_err("缺 plan_id 应失败");
    matches!(
        err,
        PlanError::YamlParse(_) | PlanError::MissingField { .. }
    );

    let yaml_empty_plan_id =
        "---\nplan_id: \"\"\ngoal: g\nstate: planning\ncreated_at: t\nschema_version: 1\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_empty_plan_id).expect_err("空 plan_id 应失败");
    match &err {
        PlanError::MissingField { field } => assert_eq!(field, "plan_id"),
        other => panic!("expected MissingField(plan_id), got {other:?}"),
    }

    let yaml_empty_goal =
        "---\nplan_id: x\ngoal: \"\"\nstate: planning\ncreated_at: t\nschema_version: 1\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_empty_goal).expect_err("空 goal 应失败");
    match &err {
        PlanError::MissingField { field } => assert_eq!(field, "goal"),
        other => panic!("expected MissingField(goal), got {other:?}"),
    }
}

#[test]
fn plan_file_schema_version_v1_locked() {
    let yaml = "---\nplan_id: x\ngoal: g\nstate: planning\ncreated_at: t\nschema_version: 2\ntodos: []\n---\n";
    let err = parse_plan_file(yaml).expect_err("schema_version=2 应被拒");
    assert!(
        matches!(
            err,
            PlanError::SchemaVersion {
                actual: 2,
                expected: 1
            }
        ),
        "expected SchemaVersion(2,1), got {err:?}"
    );
}

#[test]
fn plan_file_rejects_multiple_in_progress_on_write() {
    let mut frontmatter = sample_frontmatter();
    frontmatter.todos.push(TodoItem {
        id: "t3".into(),
        content: "另一个 in_progress".into(),
        status: TodoStatus::InProgress,
    });
    let plan = PlanFile {
        frontmatter,
        body: String::new(),
    };
    let err = serialize_plan_file(&plan).expect_err("应拒多个 in_progress");
    assert!(
        matches!(err, PlanError::MultipleInProgress { count: 2 }),
        "got {err:?}"
    );
}

#[test]
fn plan_file_rejects_duplicate_todo_ids_on_write() {
    let mut frontmatter = sample_frontmatter();
    frontmatter.todos.push(TodoItem {
        id: "t1".into(),
        content: "dup".into(),
        status: TodoStatus::Pending,
    });
    let plan = PlanFile {
        frontmatter,
        body: String::new(),
    };
    let err = serialize_plan_file(&plan).expect_err("应拒重复 id");
    match &err {
        PlanError::DuplicateTodoId { id } => assert_eq!(id, "t1"),
        other => panic!("expected DuplicateTodoId, got {other:?}"),
    }
}

#[test]
fn plan_file_frontmatter_delim_missing_returns_error() {
    let err = parse_plan_file("not yaml at all\n").expect_err("无 --- 应失败");
    assert!(
        matches!(err, PlanError::FrontmatterDelimMissing),
        "got {err:?}"
    );

    let err = parse_plan_file("---\nplan_id: x\n").expect_err("缺结尾 --- 应失败");
    assert!(
        matches!(err, PlanError::FrontmatterDelimMissing),
        "got {err:?}"
    );
}
