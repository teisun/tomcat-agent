use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::plan_runtime::panels::{
    Answer, AskQuestionIdentity, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult,
    AskQuestionTermination, Question, QuestionOption,
};
use crate::infra::event_bus::EventListenerId;
use crate::infra::{wire, EventBus, ScopedEventEmitter};

const CANCEL_POLL_MS: Duration = Duration::from_millis(10);

struct ResponseListenerGuard {
    event_bus: Arc<dyn EventBus>,
    listener_id: EventListenerId,
}

impl Drop for ResponseListenerGuard {
    fn drop(&mut self) {
        self.event_bus.off(self.listener_id);
    }
}

/// `ask_question` 宿主桥接请求。
///
/// `AskQuestionPanel` 与未来 IDE host 共享同一套 wire：
/// - request event 固定为 `plan.ask_question`
/// - response event 由 request payload 携带，约定仍以 `plan.ask_question` 为前缀
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionWireRequest {
    #[serde(alias = "request_id")]
    pub request_id: String,
    #[serde(alias = "response_event")]
    pub response_event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(with = "wire_questions")]
    pub questions: Vec<Question>,
}

/// 宿主回包；未来 IDE host 与测试 mock host 都按此结构回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionWireResponse {
    #[serde(alias = "request_id")]
    pub request_id: String,
    #[serde(with = "wire_result")]
    pub result: AskQuestionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireQuestion {
    #[serde(
        default = "crate::core::plan_runtime::panels::allow_custom_default",
        skip_serializing_if = "crate::core::plan_runtime::panels::is_true"
    )]
    allow_custom: bool,
    id: String,
    prompt: String,
    options: Vec<WireQuestionOption>,
}

impl From<Question> for WireQuestion {
    fn from(value: Question) -> Self {
        Self {
            id: value.id,
            prompt: value.prompt,
            options: value.options.into_iter().map(Into::into).collect(),
            allow_custom: value.allow_custom,
        }
    }
}

impl From<WireQuestion> for Question {
    fn from(value: WireQuestion) -> Self {
        Self {
            id: value.id,
            prompt: value.prompt,
            options: value.options.into_iter().map(Into::into).collect(),
            allow_custom: value.allow_custom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireQuestionOption {
    id: String,
    label: String,
    #[serde(default)]
    recommended: bool,
}

impl From<QuestionOption> for WireQuestionOption {
    fn from(value: QuestionOption) -> Self {
        Self {
            id: value.id,
            label: value.label,
            recommended: value.recommended,
        }
    }
}

impl From<WireQuestionOption> for QuestionOption {
    fn from(value: WireQuestionOption) -> Self {
        Self {
            id: value.id,
            label: value.label,
            recommended: value.recommended,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAnswer {
    #[serde(alias = "question_id")]
    question_id: String,
    #[serde(alias = "option_ids")]
    option_ids: Vec<String>,
    #[serde(
        default,
        alias = "custom_text",
        skip_serializing_if = "Option::is_none"
    )]
    custom_text: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    skipped: bool,
    #[serde(alias = "picked_recommended")]
    picked_recommended: bool,
}

impl From<Answer> for WireAnswer {
    fn from(value: Answer) -> Self {
        Self {
            question_id: value.question_id,
            option_ids: value.option_ids,
            custom_text: value.custom_text,
            skipped: value.skipped,
            picked_recommended: value.picked_recommended,
        }
    }
}

impl From<WireAnswer> for Answer {
    fn from(value: WireAnswer) -> Self {
        Self {
            question_id: value.question_id,
            option_ids: value.option_ids,
            custom_text: value.custom_text,
            skipped: value.skipped,
            picked_recommended: value.picked_recommended,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAskQuestionResult {
    answers: Vec<WireAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<AskQuestionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelled: Option<bool>,
}

impl From<AskQuestionResult> for WireAskQuestionResult {
    fn from(value: AskQuestionResult) -> Self {
        let cancelled = value.legacy_cancelled();
        Self {
            answers: value.answers.into_iter().map(Into::into).collect(),
            outcome: Some(value.outcome),
            cancelled: Some(cancelled),
        }
    }
}

impl From<WireAskQuestionResult> for AskQuestionResult {
    fn from(value: WireAskQuestionResult) -> Self {
        let outcome = value.outcome.unwrap_or_else(|| {
            if value.cancelled.unwrap_or(false) {
                AskQuestionOutcome::CancelledUnknown
            } else {
                AskQuestionOutcome::Answered
            }
        });
        Self {
            answers: value.answers.into_iter().map(Into::into).collect(),
            outcome,
        }
    }
}

mod wire_questions {
    use super::*;

    pub fn serialize<S>(questions: &[Question], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = questions
            .iter()
            .cloned()
            .map(WireQuestion::from)
            .collect::<Vec<_>>();
        wire.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Question>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = Vec::<WireQuestion>::deserialize(deserializer)?;
        Ok(wire.into_iter().map(Question::from).collect())
    }
}

mod wire_result {
    use super::*;

    pub fn serialize<S>(result: &AskQuestionResult, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        WireAskQuestionResult::from(result.clone()).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<AskQuestionResult, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireAskQuestionResult::deserialize(deserializer)?;
        Ok(wire.into())
    }
}

fn is_false(value: &bool) -> bool {
    !*value
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
    request_id_instance: String,
    request_emitter: ScopedEventEmitter,
    next_request_seq: AtomicU64,
    request_id_prefix: String,
}

impl EventBusAskQuestionPanel {
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            request_emitter: ScopedEventEmitter::new_optional(event_bus.clone(), None),
            event_bus,
            request_id_instance: Uuid::new_v4().simple().to_string(),
            next_request_seq: AtomicU64::new(0),
            request_id_prefix: "askq".to_string(),
        }
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.request_emitter = ScopedEventEmitter::new(self.event_bus.clone(), session_id);
        self
    }

    pub fn with_request_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.request_id_prefix = prefix.into();
        self
    }

    fn next_request_id(&self) -> String {
        let seq = self.next_request_seq.fetch_add(1, Ordering::Relaxed);
        format!(
            "{}-{}-{seq}",
            self.request_id_prefix, self.request_id_instance
        )
    }
}

#[async_trait]
impl AskQuestionPanel for EventBusAskQuestionPanel {
    async fn ask(
        &self,
        questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult {
        self.ask_with_identity(AskQuestionIdentity::default(), questions, termination)
            .await
    }

    async fn ask_with_identity(
        &self,
        identity: AskQuestionIdentity,
        questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult {
        if let Some(result) = termination.result() {
            return result;
        }

        let request_id = self.next_request_id();
        let response_event = ask_question_response_event_name(&request_id);
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let request_id_for_listener = request_id.clone();
        let tx_for_listener = tx.clone();
        let listener_id = self.event_bus.on(
            &response_event,
            Box::new(move |ctx| {
                let Ok(resp) = serde_json::from_value::<AskQuestionWireResponse>(ctx.payload)
                else {
                    return Ok(());
                };
                if resp.request_id != request_id_for_listener {
                    return Ok(());
                }
                if let Some(tx) = tx_for_listener.lock().take() {
                    let _ = tx.send(resp.result);
                }
                Ok(())
            }),
        );

        let _listener_guard = ResponseListenerGuard {
            event_bus: Arc::clone(&self.event_bus),
            listener_id,
        };

        let request = AskQuestionWireRequest {
            request_id,
            response_event: response_event.clone(),
            session_id: identity.session_id,
            tool_call_id: identity.tool_call_id,
            questions,
        };
        let payload = match serde_json::to_value(&request) {
            Ok(payload) => payload,
            Err(_) => {
                // This is a local serialization defect, not evidence that the host dropped.
                return AskQuestionResult::terminal(AskQuestionOutcome::CancelledUnknown);
            }
        };
        if self
            .request_emitter
            .emit_payload(ask_question_request_event_name(), payload)
            .is_err()
        {
            return confirmed_host_disconnect(&termination);
        }

        let termination_for_wait = termination.clone();
        let cancel_wait = async move {
            while termination_for_wait.reason().is_none() {
                tokio::time::sleep(CANCEL_POLL_MS).await;
            }
            termination_for_wait
                .result()
                .expect("termination reason was observed")
        };
        tokio::pin!(cancel_wait);

        let result = tokio::select! {
            response = rx => match response {
                Ok(result) => result,
                Err(_) => confirmed_host_disconnect(&termination),
            },
            result = &mut cancel_wait => result,
        };
        result
    }
}

/// Only transport failures that prove the live host path disappeared receive the
/// `host_disconnected` terminal label. Local failures must not masquerade as a host action.
pub(super) fn confirmed_host_disconnect(termination: &AskQuestionTermination) -> AskQuestionResult {
    termination.host_disconnected();
    termination
        .result()
        .expect("host_disconnected was just recorded")
}
