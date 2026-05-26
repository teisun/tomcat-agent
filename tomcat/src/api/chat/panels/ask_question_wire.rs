use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::core::plan_runtime::panels::{AskQuestionPanel, AskQuestionResult, Question};
use crate::infra::{wire, EventBus, EventContext};

const CANCEL_POLL_MS: Duration = Duration::from_millis(10);

/// `ask_question` 宿主桥接请求。
///
/// `AskQuestionPanel` 与未来 IDE host 共享同一套 wire：
/// - request event 固定为 `plan.ask_question`
/// - response event 由 request payload 携带，约定仍以 `plan.ask_question` 为前缀
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionWireRequest {
    pub request_id: String,
    pub response_event: String,
    pub questions: Vec<Question>,
}

/// 宿主回包；未来 IDE host 与测试 mock host 都按此结构回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionWireResponse {
    pub request_id: String,
    pub result: AskQuestionResult,
}

pub fn ask_question_request_event_name() -> &'static str {
    wire::WIRE_PLAN_ASK_QUESTION
}

pub fn ask_question_response_event_name(request_id: &str) -> String {
    format!(
        "{}.response.{request_id}",
        ask_question_request_event_name()
    )
}

/// 基于 EventBus 的通用 AskQuestion bridge。
///
/// 该适配器本身不假设具体宿主是 IDE/测试 harness/别的前端；宿主只要监听
/// `plan.ask_question`，并向 request 指定的 `response_event` 回包即可。
pub struct EventBusAskQuestionPanel {
    event_bus: Arc<dyn EventBus>,
    next_request_seq: AtomicU64,
    request_id_prefix: String,
}

impl EventBusAskQuestionPanel {
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            next_request_seq: AtomicU64::new(0),
            request_id_prefix: "askq".to_string(),
        }
    }

    pub fn with_request_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.request_id_prefix = prefix.into();
        self
    }

    fn next_request_id(&self) -> String {
        let seq = self.next_request_seq.fetch_add(1, Ordering::Relaxed);
        format!("{}-{seq}", self.request_id_prefix)
    }
}

#[async_trait]
impl AskQuestionPanel for EventBusAskQuestionPanel {
    async fn ask(
        &self,
        questions: Vec<Question>,
        cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult {
        if cancel_signal.load(Ordering::Relaxed) {
            return cancelled_result();
        }

        let request_id = self.next_request_id();
        let response_event = ask_question_response_event_name(&request_id);
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let request_id_for_listener = request_id.clone();
        let tx_for_listener = tx.clone();
        let listener_id = self.event_bus.once(
            &response_event,
            Box::new(move |ctx| {
                let parsed = serde_json::from_value::<AskQuestionWireResponse>(ctx.payload).ok();
                let result = match parsed {
                    Some(resp) if resp.request_id == request_id_for_listener => resp.result,
                    _ => cancelled_result(),
                };
                if let Some(tx) = tx_for_listener.lock().take() {
                    let _ = tx.send(result);
                }
                Ok(())
            }),
        );

        let request = AskQuestionWireRequest {
            request_id,
            response_event: response_event.clone(),
            questions,
        };
        let payload = match serde_json::to_value(&request) {
            Ok(payload) => payload,
            Err(_) => {
                self.event_bus.off(listener_id);
                return cancelled_result();
            }
        };
        if self
            .event_bus
            .emit_sync(
                ask_question_request_event_name(),
                EventContext::new(ask_question_request_event_name(), payload),
            )
            .is_err()
        {
            self.event_bus.off(listener_id);
            return cancelled_result();
        }

        let cancel_wait = async move {
            while !cancel_signal.load(Ordering::Relaxed) {
                tokio::time::sleep(CANCEL_POLL_MS).await;
            }
        };
        tokio::pin!(cancel_wait);

        let result = tokio::select! {
            response = rx => match response {
                Ok(result) => result,
                Err(_) => cancelled_result(),
            },
            _ = &mut cancel_wait => cancelled_result(),
        };
        self.event_bus.off(listener_id);
        result
    }
}

fn cancelled_result() -> AskQuestionResult {
    AskQuestionResult {
        answers: vec![],
        cancelled: true,
    }
}
