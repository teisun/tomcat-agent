//! `ask_question` 工具实现（plan-runtime.md §AQ-A/B/C/E, [ask-question.md]）。
//!
//! 语义：
//! - 计划执行时不可用；其它会话阶段均可调用。
//! - 入参校验：
//!   - `questions.len() ∈ [1, 4]`
//!   - 每题 `options.len() ∈ [2, 4]`、`option.id` 唯一、保留 `__custom__` 拒
//!   - 每题恰好一个 `recommended: true`
//! - 调 [`super::super::panels::AskQuestionPanel::ask`] 阻塞 await；
//!   监听 `cancel_signal` → `cancelled: true`。
//! - 返回 `{ answers: [{ question_id, option_ids, custom_text?, skipped?, picked_recommended }], cancelled }`。
//! - **选中 `__custom__`** → 必带 `custom_text`（非空、≤ 500）；
//!   未选中 `__custom__` → 不得携带 `custom_text`（防止 LLM 误用）。

use crate::core::plan_runtime::{
    panels::{
        AskQuestionIdentity, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult,
        AskQuestionTermination, Question, CUSTOM_OPTION_ID,
    },
    PlanRuntime,
};

use super::ToolError;

/// `ask_question` execution entry. It has deliberately no deadline: only a user response,
/// explicit turn interruption, or an unrecoverable host-channel closure may settle the wait.
pub async fn execute(
    runtime: &PlanRuntime,
    panel: &dyn AskQuestionPanel,
    raw_args: &serde_json::Value,
    termination: AskQuestionTermination,
) -> Result<serde_json::Value, ToolError> {
    execute_for_tool(runtime, panel, raw_args, termination, None).await
}

pub async fn execute_for_tool(
    runtime: &PlanRuntime,
    panel: &dyn AskQuestionPanel,
    raw_args: &serde_json::Value,
    termination: AskQuestionTermination,
    tool_call_id: Option<&str>,
) -> Result<serde_json::Value, ToolError> {
    let mode = runtime.mode();
    // Executing plans reject questions to avoid blocking the agent loop.
    if runtime.executing_plan_id().is_some() {
        return Err(ToolError::RejectedInMode {
            tool: "ask_question",
            mode: mode.as_str().to_string(),
            guidance: "The plan is executing; record any clarification needed as an open question in the plan".into(),
        });
    }
    let questions = parse_and_validate_questions(raw_args)?;
    let result = panel
        .ask_with_identity(
            AskQuestionIdentity {
                session_id: runtime.current_session_id(),
                tool_call_id: tool_call_id.map(str::to_owned),
            },
            questions.clone(),
            termination,
        )
        .await;
    if matches!(result.outcome, AskQuestionOutcome::Answered) {
        validate_answers(&questions, &result)?;
    } else if !result.answers.is_empty() {
        return Err(ToolError::Internal(format!(
            "terminal ask_question outcome {:?} must not carry answers",
            result.outcome
        )));
    }
    let payload = answer_to_json(&result);
    write_ask_question_transcript(runtime, &questions, &payload, tool_call_id);
    Ok(payload)
}

fn parse_and_validate_questions(raw: &serde_json::Value) -> Result<Vec<Question>, ToolError> {
    let questions: Vec<Question> = match raw.get("questions") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| ToolError::BadArgs(format!("Could not deserialize questions: {e}")))?,
        None => {
            return Err(ToolError::BadArgs(
                "ask_question requires a questions field".to_string(),
            ))
        }
    };
    if questions.is_empty() {
        return Err(ToolError::BadArgs(
            "questions must contain at least one question".to_string(),
        ));
    }
    if questions.len() > 4 {
        return Err(ToolError::BadArgs(format!(
            "questions allows at most 4 questions; received {}",
            questions.len()
        )));
    }
    // 题目 id 单次调用内唯一
    let mut seen_qid = std::collections::HashSet::new();
    for q in &questions {
        if !seen_qid.insert(&q.id) {
            return Err(ToolError::BadArgs(format!(
                "Duplicate question.id: {}",
                q.id
            )));
        }
        validate_single_question(q)?;
    }
    Ok(questions)
}

fn validate_single_question(q: &Question) -> Result<(), ToolError> {
    if q.prompt.trim().is_empty() {
        return Err(ToolError::BadArgs(format!(
            "question {}: prompt cannot be empty",
            q.id
        )));
    }
    if q.options.len() < 2 || q.options.len() > 4 {
        return Err(ToolError::BadArgs(format!(
            "question {}: options must contain 2–4 entries; received {}",
            q.id,
            q.options.len()
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut recommended_count = 0;
    for opt in &q.options {
        if opt.id == CUSTOM_OPTION_ID {
            return Err(ToolError::BadArgs(format!(
                "question {}: option.id cannot use reserved value \"{CUSTOM_OPTION_ID}\"",
                q.id
            )));
        }
        if !seen.insert(&opt.id) {
            return Err(ToolError::BadArgs(format!(
                "question {}: duplicate option.id \"{}\"",
                q.id, opt.id
            )));
        }
        if opt.label.trim().is_empty() {
            return Err(ToolError::BadArgs(format!(
                "question {}: option {} label cannot be empty",
                q.id, opt.id
            )));
        }
        if opt.recommended {
            recommended_count += 1;
        }
    }
    if recommended_count != 1 {
        return Err(ToolError::BadArgs(format!("question {}: exactly one option must have recommended=true (received {recommended_count})", q.id)));
    }
    Ok(())
}

fn validate_answers(questions: &[Question], result: &AskQuestionResult) -> Result<(), ToolError> {
    if result.answers.len() != questions.len() {
        return Err(ToolError::Internal(format!(
            "Panel returned {} answers for {} questions",
            result.answers.len(),
            questions.len()
        )));
    }
    for (q, ans) in questions.iter().zip(result.answers.iter()) {
        if ans.question_id != q.id {
            return Err(ToolError::Internal(format!(
                "Panel returned question_id={}, which does not match question {}",
                ans.question_id, q.id
            )));
        }
        if ans.skipped {
            if !ans.option_ids.is_empty() {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true requires empty option_ids",
                    q.id
                )));
            }
            if ans.custom_text.is_some() {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true must not include custom_text",
                    q.id
                )));
            }
            if ans.picked_recommended {
                return Err(ToolError::Internal(format!(
                    "question {}: skipped=true requires picked_recommended=false",
                    q.id
                )));
            }
            continue;
        }
        if ans.option_ids.len() != 1 {
            return Err(ToolError::Internal(format!(
                "question {}: exactly 1 choice is required; received {}",
                q.id,
                ans.option_ids.len()
            )));
        }
        let has_custom = ans.option_ids.iter().any(|id| id == CUSTOM_OPTION_ID);
        if has_custom {
            let text = ans.custom_text.as_deref().unwrap_or("");
            let char_count = text.chars().count();
            if char_count == 0 || char_count > 500 {
                return Err(ToolError::Internal(format!("question {}: selecting __custom__ requires custom_text of 1–500 characters (received {char_count})", q.id)));
            }
        } else if ans.custom_text.is_some() {
            return Err(ToolError::Internal(format!(
                "question {}: custom_text must not be present unless __custom__ is selected",
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
                    "question {}: answer contains unknown option_id={oid}",
                    q.id
                )));
            }
        }
    }
    Ok(())
}

fn answer_to_json(result: &AskQuestionResult) -> serde_json::Value {
    serde_json::json!({
        "outcome": result.outcome,
        "cancelled": result.legacy_cancelled(),
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
    tool_call_id: Option<&str>,
) {
    let mut extra = serde_json::json!({
        "event": crate::infra::wire::WIRE_PLAN_ASK_QUESTION,
        "questions": questions,
        "result": payload,
        "mode": runtime.mode().as_str(),
    });
    let plan_id = runtime.active_plan().map(|plan| plan.id);
    if let (Some(obj), Some(plan_id)) = (extra.as_object_mut(), plan_id) {
        obj.insert("plan_id".into(), serde_json::Value::String(plan_id));
    }
    if let (Some(obj), Some(tool_call_id)) = (extra.as_object_mut(), tool_call_id) {
        obj.insert(
            "tool_call_id".into(),
            serde_json::Value::String(tool_call_id.to_string()),
        );
    }
    runtime.write_transcript_custom(extra);
}
