use super::super::parse_tools;
use super::{fake_server_args, manager_with_fake_server};

#[test]
fn parse_tools_keeps_distinct_raw_names_after_sanitization() {
    let tools = parse_tools(
        "source",
        &serde_json::json!({"tools": [
            {"name":"a.b"}, {"name":"a_b"}
        ]}),
    )
    .unwrap();
    assert_eq!(tools.len(), 2, "one raw tool must not overwrite another");
    assert_eq!(
        tools
            .values()
            .map(|tool| tool.raw_name.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["a.b", "a_b"].into_iter().collect()
    );
}

#[test]
fn parse_tools_rejects_duplicate_final_names() {
    let result = parse_tools(
        "source",
        &serde_json::json!({"tools": [
            {"name":"same", "description":"first"}, {"name":"same", "description":"second"}
        ]}),
    );
    assert!(
        result.is_err(),
        "duplicate catalog entries must not silently overwrite: {result:?}"
    );
}

#[tokio::test]
async fn model_lookup_refuses_an_ambiguous_final_name() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 10_000);
    manager.connect_server("fake").await.unwrap();
    {
        let mut entries = manager.entries.write();
        // Inject a second catalog with the same final model name. The lookup
        // boundary must stay safe even if truncation/hash aliases collide.
        let mut duplicate = entries.values().next().unwrap().clone();
        duplicate.server.name = "second".into();
        duplicate.server.config_key = "global:second".into();
        entries.insert("global:second".into(), duplicate);
    }
    let name = "mcp__fake__capture".to_string();
    let result = manager.call_model_tool(&name, serde_json::json!({})).await;
    let described = manager.describe_many(std::slice::from_ref(&name));
    // Retire the real fixture before assertions so failure cannot leave it running.
    let retirements = {
        let mut entries = manager.entries.write();
        entries
            .values_mut()
            .for_each(|entry| entry.retire_connection());
        entries
            .values()
            .filter_map(|entry| entry.retiring.clone())
            .collect::<Vec<_>>()
    };
    for tracker in retirements {
        tokio::time::timeout(std::time::Duration::from_secs(5), tracker.wait())
            .await
            .unwrap();
    }
    assert!(
        result.is_err(),
        "ambiguous model name must not pick the first server: {result:?}"
    );
    assert!(described.tools.is_empty());
    assert_eq!(described.errors.len(), 1);
}
