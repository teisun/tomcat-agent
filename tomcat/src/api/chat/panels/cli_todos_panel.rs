use crate::core::plan_runtime::file_store::TodoStatus;
use crate::core::plan_runtime::panels::{TodosPanel, TodosPanelSnapshot};

/// CLI 默认 panel：把 snapshot 渲染成简短 board 写 stderr。
pub struct CliTodosPanel;

pub(crate) fn render_cli_panel_header(snapshot: &TodosPanelSnapshot) -> String {
    format!(
        "[panel#{}] {} {} in_progress={}",
        snapshot.panel_snapshot_id,
        snapshot.scope,
        snapshot.progress_summary(),
        snapshot.active_in_progress_id()
    )
}

impl TodosPanel for CliTodosPanel {
    fn refresh(&self, s: &TodosPanelSnapshot) {
        eprintln!("{}", render_cli_panel_header(s));
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
