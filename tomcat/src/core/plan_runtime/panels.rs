//! plan runtime 的 UI 抽象层。
//!
//! `core` 只持有稳定的数据结构、trait 与默认/测试实现；CLI 等具体表现层实现位于
//! `api/chat/panels/`。

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::file_store::{TodoItem, TodoStatus};

/// 保留 option id；LLM 不得显式声明此 id；UI 端 panel 自动追加同 id 的兜底槽。
pub const CUSTOM_OPTION_ID: &str = "__custom__";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Runtime-owned presentation option. Model tool arguments cannot disable free text.
    #[serde(
        default = "allow_custom_default",
        skip_deserializing,
        skip_serializing_if = "is_true"
    )]
    pub allow_custom: bool,
    pub id: String,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub question_id: String,
    pub option_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped: bool,
    pub picked_recommended: bool,
}

pub(crate) fn allow_custom_default() -> bool {
    true
}
pub(crate) fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Canonical terminal state for one `ask_question` request.
///
/// `Answered` includes mixed batches where individual answers are marked `skipped`; `Skipped`
/// means the user skipped the whole request. `CancelledUnknown` is read-only compatibility for
/// history/wire payloads produced before terminal reasons were explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskQuestionOutcome {
    Answered,
    Skipped,
    Interrupted,
    HostDisconnected,
    CancelledUnknown,
}

impl AskQuestionOutcome {
    pub fn legacy_cancelled(self) -> bool {
        !matches!(self, Self::Answered)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionResult {
    pub answers: Vec<Answer>,
    pub outcome: AskQuestionOutcome,
}

impl AskQuestionResult {
    pub fn answered(answers: Vec<Answer>) -> Self {
        Self {
            answers,
            outcome: AskQuestionOutcome::Answered,
        }
    }

    pub fn terminal(outcome: AskQuestionOutcome) -> Self {
        debug_assert!(!matches!(outcome, AskQuestionOutcome::Answered));
        Self {
            answers: Vec::new(),
            outcome,
        }
    }

    pub fn legacy_cancelled(&self) -> bool {
        self.outcome.legacy_cancelled()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskQuestionTerminationReason {
    Interrupted,
    HostDisconnected,
}

#[derive(Debug, Clone, Default)]
pub struct AskQuestionTermination {
    state: Arc<AtomicU8>,
}

impl AskQuestionTermination {
    pub fn interrupt(&self) {
        let _ = self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }

    pub fn host_disconnected(&self) {
        let _ = self
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }

    pub fn reason(&self) -> Option<AskQuestionTerminationReason> {
        match self.state.load(Ordering::Acquire) {
            1 => Some(AskQuestionTerminationReason::Interrupted),
            2 => Some(AskQuestionTerminationReason::HostDisconnected),
            _ => None,
        }
    }

    pub fn result(&self) -> Option<AskQuestionResult> {
        self.reason().map(|reason| match reason {
            AskQuestionTerminationReason::Interrupted => {
                AskQuestionResult::terminal(AskQuestionOutcome::Interrupted)
            }
            AskQuestionTerminationReason::HostDisconnected => {
                AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected)
            }
        })
    }
}

#[async_trait]
pub trait AskQuestionPanel: Send + Sync {
    async fn ask(
        &self,
        questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult;

    async fn ask_with_identity(
        &self,
        identity: AskQuestionIdentity,
        questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult {
        let _ = identity;
        self.ask(questions, termination).await
    }
}

/// 测试专用：构造时给一组预编排的 `AskQuestionResult`；按调用顺序返回。
pub struct MockAskQuestionPanel {
    queue: parking_lot::Mutex<Vec<AskQuestionResult>>,
    delay: Option<std::time::Duration>,
    honor_cancel: bool,
}

impl MockAskQuestionPanel {
    pub fn new(results: Vec<AskQuestionResult>) -> Self {
        Self {
            queue: parking_lot::Mutex::new(results),
            delay: None,
            honor_cancel: true,
        }
    }

    pub fn with_delay(mut self, d: std::time::Duration) -> Self {
        self.delay = Some(d);
        self
    }

    pub fn ignore_cancel(mut self) -> Self {
        self.honor_cancel = false;
        self
    }

    pub fn remaining(&self) -> usize {
        self.queue.lock().len()
    }
}

#[async_trait]
impl AskQuestionPanel for MockAskQuestionPanel {
    async fn ask(
        &self,
        _questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult {
        if self.honor_cancel {
            if let Some(result) = termination.result() {
                return result;
            }
        }
        if let Some(d) = self.delay {
            let start = std::time::Instant::now();
            while start.elapsed() < d {
                if self.honor_cancel {
                    if let Some(result) = termination.result() {
                        return result;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
        let mut q = self.queue.lock();
        if q.is_empty() {
            AskQuestionResult::terminal(AskQuestionOutcome::CancelledUnknown)
        } else {
            q.remove(0)
        }
    }
}

/// 一次 todos / update_plan mutation 完成后推给 panel 的 snapshot。
#[derive(Debug, Clone)]
pub struct TodosPanelSnapshot {
    pub panel_snapshot_id: u64,
    pub scope: String,
    pub items: Vec<TodoItem>,
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

    pub fn completed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|t| matches!(t.status, TodoStatus::Completed))
            .count()
    }

    pub fn total_count(&self) -> usize {
        self.items.len()
    }

    pub fn progress_summary(&self) -> String {
        format!("{} of {} Done", self.completed_count(), self.total_count())
    }

    pub fn active_in_progress_id(&self) -> &str {
        self.items
            .iter()
            .find(|t| matches!(t.status, TodoStatus::InProgress))
            .map(|t| t.id.as_str())
            .unwrap_or("-")
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
    now_ms.saturating_mul(1_000_000) + seq
}

pub trait TodosPanel: Send + Sync {
    fn refresh(&self, snapshot: &TodosPanelSnapshot);
}

pub struct NoopTodosPanel;

impl TodosPanel for NoopTodosPanel {
    fn refresh(&self, _snapshot: &TodosPanelSnapshot) {}
}

/// 把 snapshot fanout 给所有注册的 [`TodosPanel`]。
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
            p.refresh(snapshot);
        }
    }
}

impl Default for RefreshNotifier {
    fn default() -> Self {
        Self::new()
    }
}
