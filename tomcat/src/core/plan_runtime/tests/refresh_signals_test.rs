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
        }],
    );

    notifier.notify(&snapshot);

    let captured = panel.snapshots.lock();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].scope, "plan:plan-1");
    assert_eq!(captured[0].active_in_progress_id(), "t1");
}
