//! `TodosPanel` + `RefreshNotifier`（plan-runtime.md §E / G2 panel_snapshot_id）。
//!
//! 角色：每次 `update_plan` / `todos` 成功后，runtime 把"最新 items snapshot + 派生
//! `panel_snapshot_id`"推给 panel；UI 层（CLI / IDE）订阅 [`RefreshNotifier`] 即时刷新
//! 视图。本模块只提供 trait + 默认 noop / CLI 实现；具体 IDE 适配在调用方完成。
//!
//! 设计口径：
//! - **不**强耦合具体 UI：trait + Arc<dyn> 调用，CLI 默认实现把 board 写 stderr 行；
//! - **不**阻塞主流程：通知失败仅 log，与 `D 防御：磁盘/UI 异常不阻 in-memory`。
//! - **panel_snapshot_id**：单调递增（ms 时间戳）；UI 侧用来去重 / 防回退。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::api::chat::plan_runtime::file_store::TodoItem;

/// 一次 todos / update_plan mutation 完成后推给 panel 的 snapshot。
#[derive(Debug, Clone)]
pub struct TodosPanelSnapshot {
    /// 单调递增的 snapshot id；当 UI 收到比已展示更小的 id 时直接丢弃（防回退）。
    pub panel_snapshot_id: u64,
    /// scope：`session` / `plan:<plan_id>`。CLI 据此选择渲染模板。
    pub scope: String,
    /// 当前 items 列表（顺序与 `update_plan` 返回保持一致）。
    pub items: Vec<TodoItem>,
    /// `update_plan` 累积的 warnings（一致性校验等）。
    pub warnings: Vec<String>,
}

impl TodosPanelSnapshot {
    pub fn new_session(items: Vec<TodoItem>) -> Self {
        Self {
            panel_snapshot_id: next_panel_snapshot_id(),
            scope: "session".to_string(),
            items,
            warnings: Vec::new(),
        }
    }

    pub fn new_plan(plan_id: &str, items: Vec<TodoItem>) -> Self {
        Self {
            panel_snapshot_id: next_panel_snapshot_id(),
            scope: format!("plan:{plan_id}"),
            items,
            warnings: Vec::new(),
        }
    }
}

/// 单调递增的 panel_snapshot_id；与 `update_plan` 返回值同源。
pub fn next_panel_snapshot_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    // 拼接 ms 时间戳 + 进程内自增序列，确保单调（即使同 ms 多次调用）。
    now_ms.saturating_mul(1_000_000) + seq
}

/// Panel 通知器 trait。
pub trait TodosPanel: Send + Sync {
    fn refresh(&self, snapshot: &TodosPanelSnapshot);
}

/// 默认 noop panel——`PlanRuntime` 未注入 panel 时使用，避免 None 分支泄漏到调用点。
pub struct NoopTodosPanel;

impl TodosPanel for NoopTodosPanel {
    fn refresh(&self, _snapshot: &TodosPanelSnapshot) {}
}

/// CLI 默认 panel：把 snapshot 渲染成简短 board 写 stderr。
///
/// 行格式：
/// ```text
/// [panel#<id>] <scope> items=<n> in_progress=<id|->
///   [~] t1 ▸ step a
///   [ ] t2 ▸ step b
/// ```
pub struct CliTodosPanel;

impl TodosPanel for CliTodosPanel {
    fn refresh(&self, s: &TodosPanelSnapshot) {
        use crate::api::chat::plan_runtime::file_store::TodoStatus;
        let in_progress = s
            .items
            .iter()
            .find(|t| matches!(t.status, TodoStatus::InProgress))
            .map(|t| t.id.as_str())
            .unwrap_or("-");
        eprintln!(
            "[panel#{}] {} items={} in_progress={}",
            s.panel_snapshot_id,
            s.scope,
            s.items.len(),
            in_progress
        );
        for t in &s.items {
            let mark = match t.status {
                TodoStatus::Completed => "x",
                TodoStatus::InProgress => "~",
                TodoStatus::Cancelled => "-",
                TodoStatus::Pending => " ",
            };
            eprintln!("  [{mark}] {} ▸ {}", t.id, t.content);
        }
        for w in &s.warnings {
            eprintln!("  ⚠ {w}");
        }
    }
}

/// `RefreshNotifier`：把 snapshot fanout 给所有注册的 [`TodosPanel`]。
///
/// 注册者由 `ChatContext::from_config` / IDE 适配层挂载；同时也可注册零个，等价于 noop。
pub struct RefreshNotifier {
    panels: parking_lot::Mutex<Vec<Arc<dyn TodosPanel>>>,
}

impl RefreshNotifier {
    pub fn new() -> Self {
        Self {
            panels: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn register(&self, panel: Arc<dyn TodosPanel>) {
        self.panels.lock().push(panel);
    }

    pub fn notify(&self, snapshot: &TodosPanelSnapshot) {
        let panels = self.panels.lock().clone();
        for p in panels {
            // panic / 慢 panel 不阻塞下游——CLI panel 已经走 eprintln 同步，无须 spawn。
            p.refresh(snapshot);
        }
    }
}

impl Default for RefreshNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::chat::plan_runtime::file_store::TodoStatus;

    #[derive(Default)]
    struct CapturePanel {
        log: parking_lot::Mutex<Vec<u64>>,
    }

    impl TodosPanel for CapturePanel {
        fn refresh(&self, s: &TodosPanelSnapshot) {
            self.log.lock().push(s.panel_snapshot_id);
        }
    }

    #[test]
    fn panel_snapshot_id_is_monotonic() {
        let a = next_panel_snapshot_id();
        let b = next_panel_snapshot_id();
        let c = next_panel_snapshot_id();
        assert!(
            a < b && b < c,
            "ids should be strictly increasing: {a} {b} {c}"
        );
    }

    #[test]
    fn notifier_fans_out_to_all_registered_panels() {
        let n = RefreshNotifier::new();
        let p1 = Arc::new(CapturePanel::default());
        let p2 = Arc::new(CapturePanel::default());
        n.register(p1.clone());
        n.register(p2.clone());
        let snap = TodosPanelSnapshot::new_session(vec![TodoItem {
            id: "t".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
        }]);
        n.notify(&snap);
        assert_eq!(p1.log.lock().len(), 1);
        assert_eq!(p2.log.lock().len(), 1);
    }

    #[test]
    fn noop_panel_compiles_and_runs() {
        let p = NoopTodosPanel;
        p.refresh(&TodosPanelSnapshot::new_session(vec![]));
    }
}
