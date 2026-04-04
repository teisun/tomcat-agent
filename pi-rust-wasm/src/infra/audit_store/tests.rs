use super::wire;
use super::*;

#[test]
fn audit_entry_row_roundtrip() {
    let row = AuditEntryRow {
        timestamp: "2025-03-13T12:00:00Z".to_string(),
        payload: AuditKindPayload::Primitive {
            operation: "Read".to_string(),
            path_or_cmd: "/tmp/foo".to_string(),
            plugin_id: "p1".to_string(),
            user_approved: true,
            success: true,
            detail: None,
        },
    };
    let j = serde_json::to_string(&row).unwrap();
    let back: AuditEntryRow = serde_json::from_str(&j).unwrap();
    assert_eq!(back.timestamp, row.timestamp);
}

#[test]
fn audit_store_append_and_query() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");
    let store = AuditStore {
        path: path.clone(),
        retention_days: 90,
        append_guard: Mutex::new(()),
    };
    let row1 = AuditEntryRow {
        timestamp: "2025-03-13T10:00:00Z".to_string(),
        payload: AuditKindPayload::Primitive {
            operation: "Read".to_string(),
            path_or_cmd: "/tmp/a".to_string(),
            plugin_id: "p1".to_string(),
            user_approved: true,
            success: true,
            detail: None,
        },
    };
    let row2 = AuditEntryRow {
        timestamp: "2025-03-13T11:00:00Z".to_string(),
        payload: AuditKindPayload::ToolCall {
            tool_name: "run".to_string(),
            plugin_id: "p1".to_string(),
            caller_plugin_id: "p1".to_string(),
            success: true,
            detail: None,
        },
    };
    store.append(&row1).unwrap();
    store.append(&row2).unwrap();
    let entries = store.query(&AuditFilter::default()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, 2);
    assert_eq!(entries[0].kind_label(), wire::WIRE_TOOL_CALL);
    assert_eq!(entries[1].id, 1);
    assert_eq!(entries[1].kind_label(), wire::WIRE_AUDIT_PRIMITIVE);
}
