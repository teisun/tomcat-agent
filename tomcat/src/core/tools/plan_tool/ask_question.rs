//! `ask_question` 工具实现（plan-runtime.md §AQ-A/B/C/E, [ask-question.md]）。
//!
//! 语义：
//! - 仅 `Planning` 模式可见；EXEC/CHAT/Pending/Completed 调用 → `InvisibleInMode`。
//! - 入参校验：
//!   - `questions.len() ∈ [1, 4]`
//!   - 每题 `options.len() ∈ [2, 4]`、`option.id` 唯一、保留 `__custom__` 拒
//!   - 每题恰好一个 `recommended: true`
//! - 调 [`super::super::panels::AskQuestionPanel::ask`] 阻塞 await；
//!   监听 `cancel_signal` → `cancelled: true`。
//! - 返回 `{ answers: [{ question_id, option_ids, custom_text?, skipped?, picked_recommended }], cancelled }`。
//! - **选中 `__custom__`** → 必带 `custom_text`（非空、≤ 500）；
//!   未选中 `__custom__` → 不得携带 `custom_text`（防止 LLM 误用）。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::plan_runtime::{
    panels::{AskQuestionPanel, AskQuestionResult, Question, CUSTOM_OPTION_ID},
    state::PlanState,
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
    if matches!(mode, PlanState::Executing { .. }) {
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
        let payload = serde_json::json!({
            "cancelled": true,
            "answers": [],
        });
        write_ask_question_transcript(runtime, &questions, &payload);
        return Ok(payload);
    }
    validate_answers(&questions, &result)?;
    let payload = answer_to_json(&result);
    write_ask_question_transcript(runtime, &questions, &payload);
    Ok(payload)
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
        if ans.skipped {
            if !ans.option_ids.is_empty() {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true 时 option_ids 必须为空",
                    q.id
                )));
            }
            if ans.custom_text.is_some() {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true 时不应携带 custom_text",
                    q.id
                )));
            }
            if ans.picked_recommended {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true 时 picked_recommended 必须为 false",
                    q.id
                )));
            }
            continue;
        }
        if ans.option_ids.len() != 1 {
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
                if a.skipped {
                    obj["skipped"] = serde_json::Value::Bool(true);
                }
                if let Some(t) = &a.custom_text {
                    obj["custom_text"] = serde_json::Value::String(t.clone());
                }
                obj
            })
            .collect::<Vec<_>>(),
    })
}

fn write_ask_question_transcript(
    runtime: &PlanRuntime,
    questions: &[Question],
    payload: &serde_json::Value,
) {
    let mut extra = serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        "questions": questions,
        "result": payload,
        "mode": runtime.mode().as_str(),
    });
    let plan_id = runtime
        .mode()
        .active_plan_id()
        .map(ToOwned::to_owned)
        .or_else(|| runtime.active_planning_plan_id());
    if let (Some(obj), Some(plan_id)) = (extra.as_object_mut(), plan_id) {
        obj.insert("plan_id".into(), serde_json::Value::String(plan_id));
    }
    runtime.write_transcript_custom(extra);
}
