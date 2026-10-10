use super::parse_tasks;

fn args(value: serde_json::Value) -> serde_json::Value {
    value
}

#[test]
fn parses_tasks_and_defaults_missing_ids() {
    let tasks = parse_tasks(&args(serde_json::json!({
        "tasks": [
            { "prompt": "  where is paste handled?  " },
            { "id": "rust", "prompt": "where does the backend store attachments?" }
        ]
    })))
    .expect("should parse");

    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "task-1");
    assert_eq!(tasks[0].prompt, "where is paste handled?");
    assert_eq!(tasks[1].id, "rust");
}

#[test]
fn rejects_empty_missing_and_oversized_task_lists() {
    for bad in [
        serde_json::json!({}),
        serde_json::json!({ "tasks": [] }),
        serde_json::json!({ "tasks": [{ "prompt": "   " }] }),
        serde_json::json!({ "tasks": [{ "id": "a" }] }),
    ] {
        assert!(parse_tasks(&bad).is_err(), "should reject {bad}");
    }

    let too_many = serde_json::json!({
        "tasks": (0..7).map(|i| serde_json::json!({ "prompt": format!("q{i}") })).collect::<Vec<_>>()
    });
    let err = parse_tasks(&too_many).expect_err("should reject oversized batch");
    assert!(
        err.contains("dispatch_agent allows at most 6 tasks per call"),
        "err={err}"
    );
}

#[test]
fn rejects_duplicate_ids_because_reports_are_matched_by_id() {
    let err = parse_tasks(&args(serde_json::json!({
        "tasks": [
            { "id": "dup", "prompt": "one" },
            { "id": "dup", "prompt": "two" }
        ]
    })))
    .expect_err("duplicate ids must be rejected");
    assert!(err.contains("Duplicate id"), "err={err}");
}
