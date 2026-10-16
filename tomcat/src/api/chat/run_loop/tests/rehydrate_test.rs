use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::super::rehydrate::execute_resumed_ask_question;
use crate::core::plan_runtime::{file_store::PlanFileState, panels::*, PlanRuntime};

#[tokio::test]
async fn resumed_ask_question_observes_current_turn_stop_without_a_host_cancel() {
    for already_cancelled in [false, true] {
        let rt = PlanRuntime::new_with_session_id("resumed", "session-id");
        rt.seed_active_plan_for_test("pending-plan".into(), PlanFileState::Pending);
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = events.clone();
        rt.attach_transcript_appender(Arc::new(move |value| {
            sink.lock().push(value);
            Ok(())
        }));
        rt.attach_ask_question_panel(Arc::new(
            MockAskQuestionPanel::new(vec![AskQuestionResult::answered(vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["a".into()],
                custom_text: None,
                skipped: false,
                picked_recommended: true,
            }])])
            .with_delay(Duration::from_secs(60)),
        ));
        let token = CancellationToken::new();
        let args = json!({"questions":[{"id":"q1", "prompt":"Recovered question", "options":[
            {"id":"a","label":"A","recommended":true}, {"id":"b","label":"B"}
        ]}]});
        if already_cancelled {
            token.cancel();
        }
        let pending = execute_resumed_ask_question(&rt, "durable-call-id", &args, &token);
        tokio::pin!(pending);
        if !already_cancelled {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut pending)
                    .await
                    .is_err()
            );
            token.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(1), &mut pending)
            .await
            .expect("Stop must terminate the recovered question without a control_cancel")
            .unwrap();
        assert_eq!(result["outcome"], "interrupted");
        assert_eq!(result["cancelled"], true);
        assert!(
            rt.executing_plan_id().is_none(),
            "recovery must never start a pending plan"
        );
        let captured = events.lock();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0]["tool_call_id"], "durable-call-id");
        assert_eq!(captured[0]["result"]["outcome"], "interrupted");
    }
}
