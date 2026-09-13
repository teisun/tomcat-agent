use super::super::file_store::{
    normalize_acceptance_command, normalize_acceptance_commands, parse_plan_file,
    serialize_plan_file, PlanError, PlanFile, PlanFileState, TodoItem, TodoStatus,
};
use super::sample_frontmatter;

#[test]
fn acceptance_commands_round_trip_and_default_to_empty_for_legacy_files() {
    let mut frontmatter = sample_frontmatter();
    frontmatter.acceptance_commands = vec![
        "cd tomcat && cargo test --lib plan_tool".into(),
        "cd tomcat-vscode-ext && npm test".into(),
    ];
    let plan = PlanFile {
        frontmatter,
        body: String::new(),
    };
    let text = serialize_plan_file(&plan).expect("serialize");
    assert!(
        text.contains("acceptance_commands:\n- cd tomcat && cargo test --lib plan_tool\n- cd tomcat-vscode-ext && npm test\n"),
        "declared commands must be persisted verbatim as a YAML list:\n{text}"
    );
    let parsed = parse_plan_file(&text).expect("parse");
    assert_eq!(
        parsed.frontmatter.acceptance_commands,
        plan.frontmatter.acceptance_commands
    );

    let legacy_text = serialize_plan_file(&PlanFile {
        frontmatter: sample_frontmatter(),
        body: String::new(),
    })
    .expect("serialize legacy shape");
    assert!(
        !legacy_text.contains("acceptance_commands"),
        "an undeclared list must not be written as an empty key:\n{legacy_text}"
    );
    let legacy = parse_plan_file(&legacy_text).expect("legacy plan remains readable");
    assert!(legacy.frontmatter.acceptance_commands.is_empty());
}

#[test]
fn code_review_state_round_trips_and_defaults_for_legacy_files() {
    let mut frontmatter = sample_frontmatter();
    frontmatter.code_review_rounds = 2;
    frontmatter.code_review_baseline_ms = Some(123_456);
    frontmatter.code_review_open_findings = vec![crate::core::plan_runtime::Finding::new(
        "P1".into(),
        "runtime".into(),
        "missing regression coverage".into(),
    )
    .with_reference("F01")];
    frontmatter.code_review_disputed_findings = vec![crate::core::plan_runtime::DisputedFinding {
        reference: "F02".into(),
        severity: "P1".into(),
        area: "compatibility".into(),
        note: "legacy protocol stays enabled".into(),
        resolution: "wontfix".into(),
        reason: "explicit product trade-off".into(),
    }];
    let plan = PlanFile {
        frontmatter: frontmatter.clone(),
        body: String::new(),
    };

    let parsed = parse_plan_file(&serialize_plan_file(&plan).expect("serialize")).expect("parse");
    assert_eq!(parsed.frontmatter.code_review_rounds, 2);
    assert_eq!(parsed.frontmatter.code_review_baseline_ms, Some(123_456));
    assert_eq!(
        parsed.frontmatter.code_review_open_findings,
        frontmatter.code_review_open_findings
    );
    assert_eq!(
        parsed.frontmatter.code_review_disputed_findings,
        frontmatter.code_review_disputed_findings
    );

    let legacy = parse_plan_file(
        "---\nplan_id: legacy\ngoal: g\nstate: planning\ncreated_at: t\nschema_version: 1\ntodos: []\n---\n",
    )
    .expect("legacy plan remains readable");
    assert_eq!(legacy.frontmatter.code_review_rounds, 0);
    assert_eq!(legacy.frontmatter.code_review_baseline_ms, None);
    assert!(legacy.frontmatter.code_review_open_findings.is_empty());
    assert!(legacy.frontmatter.code_review_disputed_findings.is_empty());
}

#[test]
fn acceptance_command_normalization_collapses_whitespace_and_dedupes_only() {
    assert_eq!(
        normalize_acceptance_command("  cd tomcat  &&\tcargo   test  "),
        "cd tomcat && cargo test"
    );
    // Whitespace-only differences collapse; anything else is a different command. A narrower
    // filter never matches the declared broad command it was carved out of.
    assert_eq!(
        normalize_acceptance_commands(&[
            "cargo test",
            "  cargo   test ",
            "",
            "   ",
            "cargo test --lib foo",
            "cargo test",
        ]),
        vec!["cargo test".to_string(), "cargo test --lib foo".to_string()]
    );
}

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
fn read_plan_normalizes_legacy_runtime_gates_from_in_progress_to_pending() {
    let legacy = "---\nplan_id: legacy\ngoal: g\nstate: executing\ncreated_at: t\nschema_version: 1\ntodos:\n  - id: gate-review\n    content: \"[gate] review\"\n    status: in_progress\n    kind: gate_code_review\n  - id: gate-acceptance\n    content: \"[gate] Acceptance\"\n    status: in_progress\n    kind: gate_acceptance\n---\n";

    let parsed = parse_plan_file(legacy).expect("legacy plan remains readable");

    assert_eq!(parsed.frontmatter.todos[0].status, TodoStatus::Pending);
    assert_eq!(parsed.frontmatter.todos[1].status, TodoStatus::Pending);
}

#[test]
fn plan_file_rejects_more_than_three_in_progress_on_write() {
    let mut frontmatter = sample_frontmatter();
    for id in ["t3", "t4", "t5"] {
        frontmatter.todos.push(TodoItem {
            id: id.into(),
            content: format!("另一个 in_progress: {id}"),
            status: TodoStatus::InProgress,
            evidence: Vec::new(),
            kind: Default::default(),
        });
    }
    let plan = PlanFile {
        frontmatter,
        body: String::new(),
    };
    let err = serialize_plan_file(&plan).expect_err("应拒超过三个 in_progress");
    assert!(
        matches!(err, PlanError::MultipleInProgress { count: 4 }),
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
        evidence: Vec::new(),
        kind: Default::default(),
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
