use std::sync::Arc;

use parking_lot::Mutex;

use super::super::file_store::{TodoItem, TodoStatus};
use super::super::panels::{
    next_panel_snapshot_id, RefreshNotifier, TodosPanel, TodosPanelSnapshot,
};

#[derive(Default)]
struct CapturePanel {
    snapshots: Mutex<Vec<TodosPanelSnapshot>>,
}

impl TodosPanel for CapturePanel {
    fn refresh(&self, snapshot: &TodosPanelSnapshot) {
        self.snapshots.lock().push(snapshot.clone());
    }
}

#[tokio::test]
async fn repeated_real_plan_updates_reach_registered_panel_as_full_snapshots() {
    use crate::core::plan_runtime::file_store::{write_plan, PlanFile};
    use crate::core::plan_runtime::PlanRuntime;
    use crate::core::tools::plan_tool::update_plan;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("panel.plan.md");
    let mut frontmatter = super::sample_frontmatter();
    for item in &mut frontmatter.todos {
        item.status = TodoStatus::Pending;
    }
    let plan_id = frontmatter.plan_id.clone();
    write_plan(
        &path,
        &PlanFile {
            frontmatter,
            body: "panel fixture".into(),
        },
        2000,
    )
    .unwrap();
    let runtime = PlanRuntime::new("panel-session");
    runtime.build_plan(&path.to_string_lossy(), None).unwrap();
    let panel = Arc::new(CapturePanel::default());
    runtime.register_todos_panel(panel.clone());
    let mut results = Vec::new();
    for ops in [
        serde_json::json!([{"kind":"set_status","id":"t1","status":"in_progress"}]),
        serde_json::json!([
            {"kind":"set_status","id":"t1","status":"completed"},
            {"kind":"set_status","id":"t2","status":"in_progress"}
        ]),
    ] {
        let args =
            update_plan::UpdatePlanArgs::from_json(&serde_json::json!({"path":path,"ops":ops}))
                .unwrap();
        results.push(update_plan::execute(&runtime, args).await.unwrap());
    }
    let snapshots = panel.snapshots.lock();
    assert_eq!(snapshots.len(), 2);
    for (snapshot, result) in snapshots.iter().zip(&results) {
        assert_eq!(snapshot.scope, format!("plan:{plan_id}"));
        assert_eq!(
            snapshot.items.len(),
            2,
            "panels receive the full list, not only changed items"
        );
        assert_eq!(
            snapshot.panel_snapshot_id,
            result["panel_snapshot_id"].as_u64().unwrap()
        );
        assert_eq!(
            snapshot.active_in_progress_id(),
            result["active_in_progress"].as_str().unwrap()
        );
    }
    assert_eq!(snapshots[0].active_in_progress_id(), "t1");
    assert_eq!(snapshots[0].items[1].status, TodoStatus::Pending);
    assert_eq!(snapshots[1].active_in_progress_id(), "t2");
    assert_eq!(snapshots[1].items[0].status, TodoStatus::Completed);
    assert_eq!(snapshots[1].completed_count(), 1);
}

#[test]
fn next_panel_snapshot_id_is_monotonic() {
    let first = next_panel_snapshot_id();
    let second = next_panel_snapshot_id();
    assert!(second > first);
}

#[test]
fn refresh_notifier_fanouts_registered_panels() {
    let notifier = RefreshNotifier::new();
    let panel = Arc::new(CapturePanel::default());
    notifier.register(panel.clone());
    let snapshot = TodosPanelSnapshot::new_plan(
        "plan-1",
        vec![TodoItem {
            id: "t1".into(),
            content: "ship".into(),
            status: TodoStatus::InProgress,
            evidence: Vec::new(),
            kind: Default::default(),
        }],
    );

    notifier.notify(&snapshot);

    let captured = panel.snapshots.lock();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].scope, "plan:plan-1");
    assert_eq!(captured[0].active_in_progress_id(), "t1");
}
