//! Explicit plan execution entry; the dispatcher authorizes and pins the target first.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::plan_runtime::{
    file_store::read_plan,
    panels::{
        AskQuestionIdentity, AskQuestionOutcome, AskQuestionTermination, Question, QuestionOption,
    },
    PlanRuntime,
};
use crate::core::session::AgentMode;
use crate::infra::i18n::tr;

use super::ToolError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartPlanArgs {
    pub plan_id: Option<String>,
}

pub fn resolve_target_plan_path(
    runtime: &PlanRuntime,
    args: &StartPlanArgs,
) -> Result<PathBuf, ToolError> {
    match args.plan_id.as_deref() {
        Some(id) => runtime.resolved_plan_path(id).map_err(ToolError::BadArgs),
        None => runtime
            .default_build_target()
            .map(PathBuf::from)
            .map_err(|error| ToolError::BadArgs(error.to_string())),
    }
}

/// No plan lock is held while the user decides. All state gates and the commit belong to build_plan.
pub async fn execute_at_path(
    runtime: &PlanRuntime,
    path: PathBuf,
    termination: AskQuestionTermination,
    tool_call_id: Option<&str>,
) -> Result<Value, ToolError> {
    if runtime.mode() == AgentMode::Plan {
        let panel = runtime.ask_question_panel().ok_or_else(|| {
            ToolError::Internal(
                "start_plan needs a question panel in PLAN mode; use Build or /plan build instead"
                    .into(),
            )
        })?;
        // Read identity only, not state gates; the authorized path remains pinned across the wait.
        let plan_id = read_plan(&path)?.frontmatter.plan_id;
        let result = panel
            .ask_with_identity(
                AskQuestionIdentity {
                    session_id: runtime.current_session_id(),
                    tool_call_id: tool_call_id.map(str::to_owned),
                },
                vec![Question {
                    id: "plan-start".into(),
                    allow_custom: false,
                    prompt: tr("plan.start.prompt", &[("id", &plan_id)]),
                    options: vec![
                        QuestionOption {
                            id: "start".into(),
                            label: tr("plan.start.accept", &[]),
                            recommended: true,
                        },
                        QuestionOption {
                            id: "stay".into(),
                            label: tr("plan.start.stay", &[]),
                            recommended: false,
                        },
                    ],
                }],
                termination.clone(),
            )
            .await;
        if termination.reason().is_some()
            || matches!(
                result.outcome,
                AskQuestionOutcome::Interrupted
                    | AskQuestionOutcome::HostDisconnected
                    | AskQuestionOutcome::CancelledUnknown
            )
        {
            return Ok(json!({"started": false, "reason": "interrupted"}));
        }
        if !result.is_single_choice("plan-start", "start") {
            return Ok(json!({"started": false, "reason": "user_declined"}));
        }
    }
    if termination.reason().is_some() {
        return Ok(json!({"started": false, "reason": "interrupted"}));
    }
    // Synchronous after the last await: no nested turn and no partially awaited commit.
    let outcome = runtime
        .build_plan(&path.to_string_lossy(), runtime.current_session_id())
        .map_err(|error| ToolError::Internal(error.to_string()))?;
    Ok(json!({
        "started": true,
        "plan_id": outcome.plan_id,
        "path": crate::infra::platform::format_home_path(&outcome.plan_path),
        "state": "executing",
        "warnings": outcome.warnings,
    }))
}
