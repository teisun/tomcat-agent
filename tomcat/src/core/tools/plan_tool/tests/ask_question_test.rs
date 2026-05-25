use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::common::*;
use crate::core::plan_runtime::panels::{Answer, AskQuestionResult, MockAskQuestionPanel};

fn rt_planning() -> std::sync::Arc<PlanRuntime> {
    let rt = PlanRuntime::new("s1");
    rt.enter_planning().unwrap();
    rt
}

fn good_args() -> serde_json::Value {
    serde_json::json!({
        "questions": [{
            "id": "q1",
            "prompt": "选一个",
            "options": [
                {"id": "a", "label": "A", "recommended": true},
                {"id": "b", "label": "B"}
            ]
        }]
    })
}

#[tokio::test]
async fn ask_question_visible_in_chat() {
    let rt = PlanRuntime::new("s1");
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        cancelled: false,
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }],
    }]);
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .expect("CHAT 模式应允许 ask_question");
    assert_eq!(out["cancelled"], false);
}

#[tokio::test]
async fn ask_question_emits_transcript_event_on_answer() {
    let rt = rt_planning();
    let captured = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        cancelled: false,
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }],
    }]);

    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["cancelled"], false);

    let guard = captured.lock();
    assert_eq!(guard.len(), 1, "成功回答应写一条 transcript 自定义事件");
    assert_eq!(guard[0]["event"], crate::infra::wire::WIRE_PLAN_ASK_QUESTION);
    assert_eq!(guard[0]["mode"], "planning");
    assert_eq!(guard[0]["questions"][0]["id"], "q1");
    assert_eq!(guard[0]["result"]["answers"][0]["question_id"], "q1");
}

#[tokio::test]
async fn ask_question_emits_transcript_event_on_cancelled() {
    let rt = rt_planning();
    let captured = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        cancelled: true,
        answers: vec![],
    }]);

    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["cancelled"], true);

    let guard = captured.lock();
    assert_eq!(guard.len(), 1, "取消也应写一条 transcript 自定义事件");
    assert_eq!(guard[0]["event"], crate::infra::wire::WIRE_PLAN_ASK_QUESTION);
    assert_eq!(guard[0]["result"]["cancelled"], true);
    assert!(guard[0]["result"]["answers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ask_question_invisible_in_exec_returns_tool_error() {
    let rt = PlanRuntime::new("s1");
    rt.set_executing_for_test("plan_x".into());
    let panel = MockAskQuestionPanel::new(vec![]);
    let err = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    match err {
        ToolError::InvisibleInMode { tool, mode } => {
            assert_eq!(tool, "ask_question");
            assert_eq!(mode, "executing");
        }
        other => panic!("expected InvisibleInMode, got {other:?}"),
    }
}

#[tokio::test]
async fn ask_question_schema_bounds_questions_count() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![]);
    let err = ask_question::execute(
        &rt,
        &panel,
        &serde_json::json!({"questions": []}),
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap_err();
    matches!(err, ToolError::BadArgs(_));

    let many: Vec<_> = (0..5)
        .map(|i| {
            serde_json::json!({
                "id": format!("q{i}"),
                "prompt": "x",
                "options": [
                    {"id":"a","label":"A","recommended":true},
                    {"id":"b","label":"B"}
                ]
            })
        })
        .collect();
    let err = ask_question::execute(
        &rt,
        &panel,
        &serde_json::json!({"questions": many}),
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap_err();
    matches!(err, ToolError::BadArgs(_));
}

#[tokio::test]
async fn ask_question_schema_bounds_options_count() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![]);
    let args = serde_json::json!({"questions": [{
        "id":"q1","prompt":"x","options":[{"id":"a","label":"A","recommended":true}]
    }]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));

    let args = serde_json::json!({"questions": [{
        "id":"q1","prompt":"x","options":[
            {"id":"a","label":"A","recommended":true},
            {"id":"b","label":"B"},
            {"id":"c","label":"C"},
            {"id":"d","label":"D"},
            {"id":"e","label":"E"}
        ]
    }]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));
}

#[tokio::test]
async fn ask_question_requires_exactly_one_recommended() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![]);
    let args = serde_json::json!({"questions": [{
        "id":"q1","prompt":"x","options":[
            {"id":"a","label":"A"},
            {"id":"b","label":"B"}
        ]
    }]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));

    let args = serde_json::json!({"questions": [{
        "id":"q1","prompt":"x","options":[
            {"id":"a","label":"A","recommended":true},
            {"id":"b","label":"B","recommended":true}
        ]
    }]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));
}

#[tokio::test]
async fn ask_question_rejects_reserved_custom_id() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![]);
    let args = serde_json::json!({"questions": [{
        "id":"q1","prompt":"x","options":[
            {"id":"__custom__","label":"X","recommended":true},
            {"id":"b","label":"B"}
        ]
    }]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));
}

#[tokio::test]
async fn ask_question_blocks_until_answered() {
    let _g = env_mutex().lock();
    std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }],
        cancelled: false,
    }])
    .with_delay(std::time::Duration::from_millis(80));
    let start = std::time::Instant::now();
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert!(start.elapsed() >= std::time::Duration::from_millis(70));
    assert_eq!(out["answers"][0]["picked_recommended"], true);
}

#[tokio::test]
async fn ask_question_handles_user_abort_returns_cancelled_not_err() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![],
        cancelled: true,
    }]);
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["cancelled"], true);
    assert!(out["answers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ask_question_first_ctrl_c_returns_cancelled_via_signal() {
    let _g = env_mutex().lock();
    std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![],
        cancelled: false,
    }])
    .with_delay(std::time::Duration::from_secs(2));
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cancel_clone.store(true, Ordering::Relaxed);
    });
    let out = ask_question::execute(&rt, &panel, &good_args(), cancel)
        .await
        .unwrap();
    assert_eq!(out["cancelled"], true);
}

#[tokio::test]
async fn ask_question_custom_text_required_when_custom_selected() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["__custom__".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: false,
        }],
        cancelled: false,
    }]);
    let err = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::Internal(_));
}

#[tokio::test]
async fn ask_question_custom_text_forbidden_otherwise() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: Some("不该出现".into()),
            skipped: false,
            picked_recommended: true,
        }],
        cancelled: false,
    }]);
    let err = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::Internal(_));
}

#[tokio::test]
async fn ask_question_result_carries_picked_recommended_flag() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["b".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: false,
        }],
        cancelled: false,
    }]);
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["answers"][0]["picked_recommended"], false);
    assert_eq!(out["answers"][0]["option_ids"][0], "b");
}

#[tokio::test]
async fn ask_question_ui_appends_custom_slot_via_panel_round_trip() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["__custom__".into()],
            custom_text: Some("free text answer".into()),
            skipped: false,
            picked_recommended: false,
        }],
        cancelled: false,
    }]);
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["answers"][0]["option_ids"][0], "__custom__");
    assert_eq!(out["answers"][0]["custom_text"], "free text answer");
}

#[tokio::test]
async fn ask_question_result_carries_skipped_flag_for_skipped_question() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec![],
            custom_text: None,
            skipped: true,
            picked_recommended: false,
        }],
        cancelled: false,
    }]);
    let out = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    assert_eq!(out["cancelled"], false);
    assert_eq!(out["answers"][0]["skipped"], true);
    assert_eq!(out["answers"][0]["option_ids"], serde_json::json!([]));
}

#[tokio::test]
async fn ask_question_skipped_answer_rejects_option_ids() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: None,
            skipped: true,
            picked_recommended: false,
        }],
        cancelled: false,
    }]);
    let err = ask_question::execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
        .await
        .expect_err("skipped answer should reject option_ids");
    assert!(
        err.to_string().contains("skipped=true"),
        "unexpected error: {err}"
    );
}

fn env_mutex() -> &'static parking_lot::Mutex<()> {
    static M: parking_lot::Mutex<()> = parking_lot::const_mutex(());
    &M
}

#[tokio::test]
async fn ask_question_timeout_returns_cancelled() {
    let _g = env_mutex().lock();
    std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![],
        cancelled: false,
    }])
    .with_delay(std::time::Duration::from_secs(2));
    let out = ask_question::execute_with_timeout(
        &rt,
        &panel,
        &good_args(),
        Arc::new(AtomicBool::new(false)),
        Some(40),
    )
    .await
    .unwrap();
    assert_eq!(out["cancelled"], true);
    assert!(out["answers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ask_question_env_overrides_config_timeout() {
    let _g = env_mutex().lock();
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        answers: vec![],
        cancelled: false,
    }])
    .with_delay(std::time::Duration::from_secs(2));
    std::env::set_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS", "40");
    let out = ask_question::execute_with_timeout(
        &rt,
        &panel,
        &good_args(),
        Arc::new(AtomicBool::new(false)),
        Some(60_000),
    )
    .await
    .unwrap();
    std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
    assert_eq!(out["cancelled"], true);
}

#[tokio::test]
async fn ask_question_zero_timeout_means_no_timeout() {
    let _g = env_mutex().lock();
    std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
        cancelled: false,
        answers: vec![Answer {
            question_id: "q1".into(),
            option_ids: vec!["a".into()],
            custom_text: None,
            skipped: false,
            picked_recommended: true,
        }],
    }])
    .with_delay(std::time::Duration::from_millis(80));
    let out = ask_question::execute_with_timeout(
        &rt,
        &panel,
        &good_args(),
        Arc::new(AtomicBool::new(false)),
        Some(0),
    )
    .await
    .unwrap();
    assert_eq!(out["cancelled"], false);
}

#[tokio::test]
async fn ask_question_duplicate_question_id_rejected() {
    let rt = rt_planning();
    let panel = MockAskQuestionPanel::new(vec![]);
    let args = serde_json::json!({"questions": [
        {"id":"q1","prompt":"x","options":[
            {"id":"a","label":"A","recommended":true},
            {"id":"b","label":"B"}
        ]},
        {"id":"q1","prompt":"y","options":[
            {"id":"a","label":"A","recommended":true},
            {"id":"b","label":"B"}
        ]}
    ]});
    let err = ask_question::execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    matches!(err, ToolError::BadArgs(_));
}
