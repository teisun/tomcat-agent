use super::parse_tasks;
use serde_json::json;
fn task() -> serde_json::Value {
    json!({"type":"explorer","description":"  Trace cache  ","prompt":" inspect cache\n and callers "})
}
#[test]
fn accepts_parallel_foreground_and_explicit_background() {
    let (tasks, bg) = parse_tasks(&json!({"tasks":vec![task();7]})).unwrap();
    assert!(!bg);
    assert_eq!(tasks.len(), 7);
    assert_eq!(tasks[0].description, "Trace cache");
    assert_eq!(tasks[0].prompt, "inspect cache\n and callers");
    assert!(
        parse_tasks(&json!({"tasks":[task()],"run_in_background":true}))
            .unwrap()
            .1
    );
}
#[test]
fn rejects_entire_invalid_batch_before_dispatch() {
    for (k, v) in [
        ("id", json!("old")),
        ("type", json!("worker")),
        ("type", json!(null)),
        ("description", json!("a\nb")),
        ("description", json!("  ")),
        ("prompt", json!("")),
    ] {
        let mut bad = task();
        bad[k] = v;
        assert!(parse_tasks(&json!({"tasks":[task(),bad]})).is_err());
    }
    assert!(parse_tasks(&json!({"tasks":[]})).is_err());
    assert!(parse_tasks(&json!({"tasks":[task()],"run_in_background":"false"})).is_err());
}
