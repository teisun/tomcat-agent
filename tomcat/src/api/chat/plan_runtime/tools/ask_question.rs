//! `ask_question` 工具实现（plan-runtime.md §AQ-A/B/C/E, [ask-question.md]）。
//!
//! 语义：
//! - 仅 `Planning` 模式可见；EXEC/CHAT/Pending/Completed 调用 → `InvisibleInMode`。
//! - 入参校验：
//!   - `questions.len() ∈ [1, 4]`
//!   - 每题 `options.len() ∈ [2, 4]`、`option.id` 唯一、保留 `__custom__` 拒
//!   - 每题恰好一个 `recommended: true`
//! - 调 [`super::super::ask_question_panel::AskQuestionPanel::ask`] 阻塞 await；
//!   监听 `cancel_signal` → `cancelled: true`。
//! - 返回 `{ answers: [{ question_id, option_ids, custom_text?, picked_recommended }], cancelled }`。
//! - **选中 `__custom__`** → 必带 `custom_text`（非空、≤ 500）；
//!   未选中 `__custom__` → 不得携带 `custom_text`（防止 LLM 误用）。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::api::chat::plan_runtime::{
    ask_question_panel::{AskQuestionPanel, AskQuestionResult, Question, CUSTOM_OPTION_ID},
    mode::PlanMode,
    PlanRuntime,
};

use super::ToolError;

/// `ask_question` 执行入口。`panel` 通常来自 `ChatContext.ask_question_panel`；
/// `cancel_signal` 来自 chat_loop 当前回合的 `CancellationToken` adapter。
///
/// N13：等待用户回答的墙钟超时取自（优先级从高到低）：
/// 1. 环境变量 `TOMCAT_ASK_QUESTION_TIMEOUT_MS`（解析失败/0 视为不超时）；
/// 2. `[ask_question].timeout_ms`（由 caller 通过 [`execute_with_timeout`] 传入）；
/// 3. 默认 300_000 ms（5 分钟）。
///
/// 超时后返回 `cancelled: true` 而非 `Err`——与 Ctrl-C 路径同口径。
pub async fn execute(
    runtime: &PlanRuntime,
    panel: &dyn AskQuestionPanel,
    raw_args: &serde_json::Value,
    cancel_signal: Arc<AtomicBool>,
) -> Result<serde_json::Value, ToolError> {
    execute_with_timeout(runtime, panel, raw_args, cancel_signal, None).await
}

/// 与 [`execute`] 同语义，但显式接受 caller 提供的超时（毫秒）。`config_timeout_ms == Some(0)`
/// 或环境变量 `TOMCAT_ASK_QUESTION_TIMEOUT_MS=0` 表示无超时。
pub async fn execute_with_timeout(
    runtime: &PlanRuntime,
    panel: &dyn AskQuestionPanel,
    raw_args: &serde_json::Value,
    cancel_signal: Arc<AtomicBool>,
    config_timeout_ms: Option<u64>,
) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    // B11：CHAT / Planning / Pending / Completed 都可见；EXEC 隐藏（防止 agent loop 阻塞）。
    if matches!(mode, PlanMode::Executing { .. }) {
        return Err(ToolError::InvisibleInMode {
            tool: "ask_question",
            mode: mode.as_str().to_string(),
        });
    }
    let questions = parse_and_validate_questions(raw_args)?;
    let timeout_ms = resolve_timeout_ms(config_timeout_ms);
    let ask_fut = panel.ask(questions.clone(), cancel_signal);
    let result = if let Some(ms) = timeout_ms {
        match tokio::time::timeout(std::time::Duration::from_millis(ms), ask_fut).await {
            Ok(r) => r,
            Err(_) => AskQuestionResult {
                cancelled: true,
                answers: vec![],
            },
        }
    } else {
        ask_fut.await
    };
    if result.cancelled {
        return Ok(serde_json::json!({
            "cancelled": true,
            "answers": [],
        }));
    }
    validate_answers(&questions, &result)?;
    Ok(answer_to_json(&result))
}

/// 解析超时（毫秒）：env > config > 默认 300_000。`Some(0)` / env `0` → `None`（不超时）。
fn resolve_timeout_ms(config_timeout_ms: Option<u64>) -> Option<u64> {
    if let Ok(s) = std::env::var("TOMCAT_ASK_QUESTION_TIMEOUT_MS") {
        if let Ok(n) = s.trim().parse::<u64>() {
            return if n == 0 { None } else { Some(n) };
        }
    }
    let cfg = config_timeout_ms.unwrap_or(300_000);
    if cfg == 0 {
        None
    } else {
        Some(cfg)
    }
}

fn parse_and_validate_questions(raw: &serde_json::Value) -> Result<Vec<Question>, ToolError> {
    let questions: Vec<Question> = match raw.get("questions") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| ToolError::BadArgs(format!("questions 反序列化失败: {e}")))?,
        None => {
            return Err(ToolError::BadArgs(
                "ask_question 缺少 questions 字段".into(),
            ))
        }
    };
    if questions.is_empty() {
        return Err(ToolError::BadArgs("questions 至少 1 题".into()));
    }
    if questions.len() > 4 {
        return Err(ToolError::BadArgs(format!(
            "questions 最多 4 题，当前 {}",
            questions.len()
        )));
    }
    // 题目 id 单次调用内唯一
    let mut seen_qid = std::collections::HashSet::new();
    for q in &questions {
        if !seen_qid.insert(&q.id) {
            return Err(ToolError::BadArgs(format!("question.id 重复: {}", q.id)));
        }
        validate_single_question(q)?;
    }
    Ok(questions)
}

fn validate_single_question(q: &Question) -> Result<(), ToolError> {
    if q.prompt.trim().is_empty() {
        return Err(ToolError::BadArgs(format!(
            "question {}: prompt 不可为空",
            q.id
        )));
    }
    if q.options.len() < 2 || q.options.len() > 4 {
        return Err(ToolError::BadArgs(format!(
            "question {}: options 必须 2-4 个，当前 {}",
            q.id,
            q.options.len()
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut recommended_count = 0;
    for opt in &q.options {
        if opt.id == CUSTOM_OPTION_ID {
            return Err(ToolError::BadArgs(format!(
                "question {}: option.id 不得使用保留值 \"{}\"",
                q.id, CUSTOM_OPTION_ID
            )));
        }
        if !seen.insert(&opt.id) {
            return Err(ToolError::BadArgs(format!(
                "question {}: option.id 重复 \"{}\"",
                q.id, opt.id
            )));
        }
        if opt.label.trim().is_empty() {
            return Err(ToolError::BadArgs(format!(
                "question {}: option {} label 不可为空",
                q.id, opt.id
            )));
        }
        if opt.recommended {
            recommended_count += 1;
        }
    }
    if recommended_count != 1 {
        return Err(ToolError::BadArgs(format!(
            "question {}: 必须**恰好**一个 recommended=true 选项（当前 {}）",
            q.id, recommended_count
        )));
    }
    Ok(())
}

fn validate_answers(questions: &[Question], result: &AskQuestionResult) -> Result<(), ToolError> {
    if result.answers.len() != questions.len() {
        return Err(ToolError::Internal(format!(
            "panel 返回答案数 {} 与问题数 {} 不一致",
            result.answers.len(),
            questions.len()
        )));
    }
    for (q, ans) in questions.iter().zip(result.answers.iter()) {
        if ans.question_id != q.id {
            return Err(ToolError::Internal(format!(
                "panel 返回 question_id={} 与问题 {} 不匹配",
                ans.question_id, q.id
            )));
        }
        if ans.option_ids.is_empty() {
            return Err(ToolError::Internal(format!(
                "question {}: 至少选择 1 个选项",
                q.id
            )));
        }
        if !q.allow_multiple && ans.option_ids.len() != 1 {
            return Err(ToolError::Internal(format!(
                "question {}: 单选题应只选 1 个，实际 {}",
                q.id,
                ans.option_ids.len()
            )));
        }
        let has_custom = ans.option_ids.iter().any(|id| id == CUSTOM_OPTION_ID);
        if has_custom {
            let text = ans.custom_text.as_deref().unwrap_or("");
            if text.is_empty() || text.len() > 500 {
                return Err(ToolError::Internal(format!(
                    "question {}: 选中 __custom__ 时 custom_text 必须 1-500 字符（当前 {}）",
                    q.id,
                    text.len()
                )));
            }
        } else if ans.custom_text.is_some() {
            return Err(ToolError::Internal(format!(
                "question {}: 未选 __custom__ 时不应携带 custom_text",
                q.id
            )));
        }
        // 校验每个 option_id 都合法（在 q.options 或 == __custom__）
        for oid in &ans.option_ids {
            if oid == CUSTOM_OPTION_ID {
                continue;
            }
            if !q.options.iter().any(|o| &o.id == oid) {
                return Err(ToolError::Internal(format!(
                    "question {}: 答案中含未知 option_id={}",
                    q.id, oid
                )));
            }
        }
    }
    Ok(())
}

fn answer_to_json(result: &AskQuestionResult) -> serde_json::Value {
    serde_json::json!({
        "cancelled": result.cancelled,
        "answers": result
            .answers
            .iter()
            .map(|a| {
                let mut obj = serde_json::json!({
                    "question_id": a.question_id,
                    "option_ids": a.option_ids,
                    "picked_recommended": a.picked_recommended,
                });
                if let Some(t) = &a.custom_text {
                    obj["custom_text"] = serde_json::Value::String(t.clone());
                }
                obj
            })
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::api::chat::plan_runtime::ask_question_panel::{
        Answer, AskQuestionResult, MockAskQuestionPanel,
    };
    use std::sync::atomic::AtomicBool;

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
                "allow_multiple": false,
                "options": [
                    {"id": "a", "label": "A", "recommended": true},
                    {"id": "b", "label": "B"}
                ]
            }]
        })
    }

    #[tokio::test]
    async fn ask_question_visible_in_chat() {
        // B11（2026-05）：ask_question 在 CHAT 模式也可用。
        let rt = PlanRuntime::new("s1");
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            cancelled: false,
            answers: vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["a".into()],
                custom_text: None,
                picked_recommended: true,
            }],
        }]);
        let out = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
            .await
            .expect("CHAT 模式应允许 ask_question");
        assert_eq!(out["cancelled"], false);
    }

    #[tokio::test]
    async fn ask_question_invisible_in_exec_returns_tool_error() {
        let rt = PlanRuntime::new("s1");
        rt.set_executing_for_test("plan_x".into());
        let panel = MockAskQuestionPanel::new(vec![]);
        let err = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
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
        // 0 题
        let err = execute(
            &rt,
            &panel,
            &serde_json::json!({"questions": []}),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();
        matches!(err, ToolError::BadArgs(_));
        // 5 题
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
        let err = execute(
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
        // 1 选项
        let args = serde_json::json!({"questions": [{
            "id":"q1","prompt":"x","options":[{"id":"a","label":"A","recommended":true}]
        }]});
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err();
        matches!(err, ToolError::BadArgs(_));
        // 5 选项
        let args = serde_json::json!({"questions": [{
            "id":"q1","prompt":"x","options":[
                {"id":"a","label":"A","recommended":true},
                {"id":"b","label":"B"},
                {"id":"c","label":"C"},
                {"id":"d","label":"D"},
                {"id":"e","label":"E"}
            ]
        }]});
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err();
        matches!(err, ToolError::BadArgs(_));
    }

    #[tokio::test]
    async fn ask_question_requires_exactly_one_recommended() {
        let rt = rt_planning();
        let panel = MockAskQuestionPanel::new(vec![]);
        // 0 个 recommended
        let args = serde_json::json!({"questions": [{
            "id":"q1","prompt":"x","options":[
                {"id":"a","label":"A"},
                {"id":"b","label":"B"}
            ]
        }]});
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err();
        matches!(err, ToolError::BadArgs(_));
        // 2 个 recommended
        let args = serde_json::json!({"questions": [{
            "id":"q1","prompt":"x","options":[
                {"id":"a","label":"A","recommended":true},
                {"id":"b","label":"B","recommended":true}
            ]
        }]});
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
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
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
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
                picked_recommended: true,
            }],
            cancelled: false,
        }])
        .with_delay(std::time::Duration::from_millis(80));
        let start = std::time::Instant::now();
        let out = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
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
        let out = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
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
        let out = execute(&rt, &panel, &good_args(), cancel).await.unwrap();
        assert_eq!(out["cancelled"], true);
    }

    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn ask_question_custom_text_required_when_custom_selected() {
        let rt = rt_planning();
        // panel 错误返回：选 __custom__ 但缺 custom_text → 出参校验拒
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["__custom__".into()],
                custom_text: None,
                picked_recommended: false,
            }],
            cancelled: false,
        }]);
        let err = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
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
                picked_recommended: true,
            }],
            cancelled: false,
        }]);
        let err = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
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
                picked_recommended: false,
            }],
            cancelled: false,
        }]);
        let out = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert_eq!(out["answers"][0]["picked_recommended"], false);
        assert_eq!(out["answers"][0]["option_ids"][0], "b");
    }

    #[tokio::test]
    async fn ask_question_ui_appends_custom_slot_via_panel_round_trip() {
        // panel 在 UI 里追加 __custom__ 是渲染语义；这里直接证明 panel 端返回 __custom__
        // 的答案能正确写入出参（含 custom_text）。
        let rt = rt_planning();
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["__custom__".into()],
                custom_text: Some("free text answer".into()),
                picked_recommended: false,
            }],
            cancelled: false,
        }]);
        let out = execute(&rt, &panel, &good_args(), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert_eq!(out["answers"][0]["option_ids"][0], "__custom__");
        assert_eq!(out["answers"][0]["custom_text"], "free text answer");
    }

    /// 序列化所有"修改 `TOMCAT_ASK_QUESTION_TIMEOUT_MS` env"的测试，
    /// 防止与并行的 ask_question_blocks_until_answered 等读 env 的测试相互干扰。
    fn env_mutex() -> &'static parking_lot::Mutex<()> {
        static M: parking_lot::Mutex<()> = parking_lot::const_mutex(());
        &M
    }

    #[tokio::test]
    async fn ask_question_timeout_returns_cancelled() {
        let _g = env_mutex().lock();
        // 隔离 env 干扰
        std::env::remove_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS");
        // N13：config_timeout_ms 触发 → cancelled=true，不报 Err。
        let rt = rt_planning();
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: false,
        }])
        .with_delay(std::time::Duration::from_secs(2));
        let out = execute_with_timeout(
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
        // env 优先级 > config_timeout_ms。
        let rt = rt_planning();
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: false,
        }])
        .with_delay(std::time::Duration::from_secs(2));
        std::env::set_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS", "40");
        let out = execute_with_timeout(
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
        // config_timeout_ms=0 → 无超时；env 同。
        let rt = rt_planning();
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            cancelled: false,
            answers: vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["a".into()],
                custom_text: None,
                picked_recommended: true,
            }],
        }])
        .with_delay(std::time::Duration::from_millis(80));
        let out = execute_with_timeout(
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
        let err = execute(&rt, &panel, &args, Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err();
        matches!(err, ToolError::BadArgs(_));
    }
}
