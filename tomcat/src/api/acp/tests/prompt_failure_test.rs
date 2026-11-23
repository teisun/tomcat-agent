use super::*;
use crate::infra::error::{LlmErrorStage, LlmFailureKind};
use crate::infra::events::{AgentEvent, AgentIdleOutcome, TurnFailure, WireEvent};
use serde_json::json;

async fn failed_prompt(failure: Option<TurnFailure>) -> Value {
    let (input, _commands) = mpsc::unbounded_channel();
    let replies = Replies::new(input);
    let (completion, result) = oneshot::channel();
    replies.prompts.lock().insert(
        "s1".into(),
        PendingPrompt {
            request_id: "prompt-1".into(),
            completion: Some(completion),
            error: None,
            cancellation: tokio_util::sync::CancellationToken::new(),
        },
    );
    // Deliberately localized: classification must never depend on display text.
    replies.event(&json!({"type":"agent_end", "sessionId":"s1", "error":"请求失败，请稍后重试"}));
    let idle = serde_json::to_value(WireEvent {
        session_id: Some("s1".into()),
        event: AgentEvent::AgentIdle {
            outcome: AgentIdleOutcome::Failed,
            failure,
        },
    })
    .unwrap();
    replies.event(&idle);
    assert!(
        replies.prompts.lock()["s1"].completion.is_some(),
        "idle is not terminal for an ACP prompt"
    );
    let mut settled = idle;
    settled["type"] = json!("session_settled");
    replies.event(&settled);
    let error = serde_json::to_value(result.await.unwrap().unwrap_err()).unwrap();
    assert_eq!(error["code"], -32603);
    assert_eq!(error["message"], "请求失败，请稍后重试");
    error
}

#[tokio::test]
async fn stream_interruption_reports_stable_network_diagnostics_without_changing_summary() {
    for (phase, diagnostic) in [
        (
            Some(LlmErrorStage::Connect),
            "network error: unable to connect to the model provider",
        ),
        (
            Some(LlmErrorStage::Send),
            "network error: error sending request to the model provider",
        ),
        (
            Some(LlmErrorStage::ReadTimeout),
            "network error: i/o timeout waiting for the model provider",
        ),
        (
            Some(LlmErrorStage::BodyRead),
            "network error: model provider stream disconnected",
        ),
        (
            Some(LlmErrorStage::IdleTimeout),
            "network error: model provider stream disconnected",
        ),
        (None, "network error: model provider stream disconnected"),
    ] {
        let error = failed_prompt(Some(TurnFailure {
            kind: LlmFailureKind::StreamInterrupted,
            phase,
        }))
        .await;
        assert_eq!(error["data"], diagnostic);
    }
}

#[tokio::test]
async fn http_account_and_unclassified_failures_are_never_disguised_as_network_errors() {
    for kind in [
        LlmFailureKind::RateLimit,
        LlmFailureKind::UpstreamTransient,
        LlmFailureKind::Authentication,
        LlmFailureKind::Billing,
        LlmFailureKind::Unknown,
    ] {
        let error = failed_prompt(Some(TurnFailure {
            kind,
            phase: Some(LlmErrorStage::Send),
        }))
        .await;
        assert!(error.get("data").is_none(), "{error}");
    }
    assert!(failed_prompt(None).await.get("data").is_none());
}
