//! Background reports use the existing follow-up queue. A batch is wake-eligible
//! only once its last member has finished; a busy turn may drain earlier reports.
use super::explorer::ExplorerReport;
use crate::core::llm::{ChatMessage, MessageKind};
use parking_lot::Mutex;
use std::{collections::HashSet, sync::Arc};
use tokio_util::sync::CancellationToken;

type Queue = Arc<Mutex<Vec<ChatMessage>>>;
const SOURCE: &str = "subagent_report";
#[derive(Default)]
pub struct ChildReports {
    queue: Mutex<Option<Queue>>,
    active: Mutex<HashSet<String>>,
    /// Edge trigger only: always re-check the queue and batch completion.
    pub changed: tokio::sync::Notify,
}
impl ChildReports {
    pub(crate) fn bind(&self, queue: Queue) {
        *self.queue.lock() = Some(queue);
    }
    pub(crate) fn batch(self: &Arc<Self>) -> Result<Arc<ReportBatch>, String> {
        let queue = self
            .queue
            .lock()
            .clone()
            .ok_or("background report delivery unavailable")?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.active.lock().insert(id.clone());
        Ok(Arc::new(ReportBatch {
            id,
            queue,
            owner: self.clone(),
        }))
    }
    pub(crate) fn has_open_batch(&self) -> bool {
        !self.active.lock().is_empty()
    }
    pub fn ready(&self) -> bool {
        let active = self.active.lock();
        self.queue.lock().as_ref().is_some_and(|queue| {
            queue
                .lock()
                .iter()
                .any(|m| batch_id(m).is_some_and(|id| !active.contains(id)))
        })
    }
    pub fn discard(&self) {
        // A supervisor may already have released its child registration but not
        // yet published. Invalidate its batch as well as retiring queued reports.
        let mut active = self.active.lock();
        active.clear();
        if let Some(queue) = self.queue.lock().as_ref() {
            queue.lock().retain(|m| batch_id(m).is_none());
        }
    }
}
pub(crate) fn receipt(messages: &[ChatMessage]) -> Option<crate::infra::events::AgentEvent> {
    let count = messages.iter().filter(|m| batch_id(m).is_some()).count();
    (count > 0).then_some(crate::infra::events::AgentEvent::SubagentReportsReceived { count })
}

pub(crate) fn batch_id(message: &ChatMessage) -> Option<&str> {
    if message.kind != MessageKind::Signal {
        return None;
    }
    message.name.as_deref()?.strip_prefix("subagent_report_")
}
pub struct ReportBatch {
    id: String,
    queue: Queue,
    owner: Arc<ChildReports>,
}
impl ReportBatch {
    pub(crate) fn publish(&self, report: &ExplorerReport, token: &CancellationToken) {
        let active = self.owner.active.lock();
        let mut queue = self.queue.lock();
        if !active.contains(&self.id) || token.is_cancelled() {
            return;
        }
        let mut msg = ChatMessage::user(serde_json::json!({"source":SOURCE,"childSessionId":report.child_session_id,"type":"explorer","status":report.stop_reason,"report":report.report}).to_string());
        msg.kind = MessageKind::Signal;
        msg.name = Some(format!("{SOURCE}_{}", self.id));
        queue.push(msg);
    }
}
impl Drop for ReportBatch {
    fn drop(&mut self) {
        self.owner.active.lock().remove(&self.id);
        self.owner.changed.notify_one();
    }
}
