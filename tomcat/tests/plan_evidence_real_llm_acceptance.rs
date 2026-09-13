//! Manual real-model acceptance for `update_plan.set_status.evidence`.
//!
//! The model supplies the tool arguments; persistence and Board rendering are then
//! checked through the production `update_plan` implementation.

mod common;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serial_test::serial;
use tomcat::core::plan_runtime::file_store::{
    PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoKind, TodoStatus,
    GATE_ACCEPTANCE_TODO_CONTENT, GATE_ACCEPTANCE_TODO_ID, GATE_CODE_REVIEW_TODO_CONTENT,
    GATE_CODE_REVIEW_TODO_ID,
};
use tomcat::core::plan_runtime::PlanRuntime;
use tomcat::core::session::{AgentMode, ResumeControlState};
use tomcat::core::tools::contract::catalog::builtin_tool_by_name;
use tomcat::core::tools::plan_tool::update_plan::{self, UpdatePlanArgs};
use tomcat::{AppConfig, ChatMessage, ChatRequest};

const MODEL_ENV: &str = "TOMCAT_E2E_GUARD_REAL_MODEL";
const DEFAULT_MODEL: &str = "idatatlas/gpt-5.6-terra";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

fn selected_model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn tool_definition() -> serde_json::Value {
    let entry = builtin_tool_by_name("update_plan").expect("update_plan must be registered");
    serde_json::json!({
        "type": "function",
        "function": {
            "name": entry.name,
            "description": entry.description,
            "parameters": (entry.parameters)(),
        }
    })
}

fn parse_update_plan_args(
    message: &ChatMessage,
) -> Result<(UpdatePlanArgs, serde_json::Value), String> {
    let tool_call = message
        .tool_calls
        .as_ref()
        .and_then(|calls| {
            calls
                .iter()
                .find(|call| call["function"]["name"].as_str() == Some("update_plan"))
        })
        .ok_or_else(|| format!("missing update_plan tool call: {:?}", message.tool_calls))?;
    let raw_args = tool_call["function"]["arguments"]
        .as_str()
        .ok_or_else(|| format!("missing tool-call arguments: {tool_call:?}"))?;
    let value: serde_json::Value =
        serde_json::from_str(raw_args).map_err(|error| format!("invalid tool JSON: {error}"))?;
    let args = UpdatePlanArgs::from_json(&value)
        .map_err(|error| format!("invalid update_plan args: {error}"))?;
    Ok((args, value))
}

fn evidence_for_completion<'a>(
    value: &'a serde_json::Value,
    todo_id: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    value
        .get("ops")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|op| {
            op["kind"].as_str() == Some("set_status")
                && op["id"].as_str() == Some(todo_id)
                && op["status"].as_str() == Some("completed")
        })?
        .get("evidence")
        .and_then(serde_json::Value::as_array)
}

#[tokio::test]
#[ignore = "manual: real gpt-5.6-terra update_plan evidence acceptance"]
#[serial]
async fn real_terra_completion_evidence_persists_without_board_rendering() {
    common::setup_logging();
    let model_id = selected_model();
    assert!(
        matches!(
            model_id.as_str(),
            "idatatlas/gpt-5.6-terra" | "fcodex/gpt-5.6-terra"
        ),
        "{MODEL_ENV} must select idatatlas/gpt-5.6-terra or fcodex/gpt-5.6-terra"
    );
    let models_toml = common::real_models_toml_path();
    let runtime_env = common::real_runtime_env_path();
    let _home = common::TempHomeGuard::new();
    let work_dir = common::dot_tomcat_e2e_workdir("plan_evidence_real_llm_acceptance");

    let mut config = AppConfig::default();
    config.storage.work_dir = Some(work_dir.display().to_string());
    let wire_model = match common::apply_models_toml_entry_app_config(
        &mut config,
        &models_toml,
        &runtime_env,
        &model_id,
    ) {
        Ok(wire_model) => wire_model,
        Err(error) => {
            eprintln!("skipping real evidence acceptance: {error}");
            return;
        }
    };
    let binding = common::resolve_main_call(&config);

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let plan_id = format!("real-evidence-{}-{suffix}", std::process::id());
    let plan_path = tomcat::core::plan_runtime::file_store::plan_path_for_id(&plan_id)
        .expect("resolve plan path");
    let todo_id = "record-real-evidence";
    let todo_content = "Run the evidence acceptance verification".to_string();
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.clone(),
            goal: "Verify real-model evidence routing".to_string(),
            state: PlanFileState::Executing,
            session_key: Some("real-evidence-session".to_string()),
            session_id: Some("real-evidence-session-id".to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: 1,
            todos: vec![
                TodoItem {
                    id: todo_id.to_string(),
                    content: todo_content.clone(),
                    status: TodoStatus::InProgress,
                    evidence: Vec::new(),
                    kind: TodoKind::Work,
                },
                TodoItem {
                    id: GATE_CODE_REVIEW_TODO_ID.to_string(),
                    content: GATE_CODE_REVIEW_TODO_CONTENT.to_string(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: TodoKind::GateCodeReview,
                },
                TodoItem {
                    id: GATE_ACCEPTANCE_TODO_ID.to_string(),
                    content: GATE_ACCEPTANCE_TODO_CONTENT.to_string(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: TodoKind::GateAcceptance,
                },
            ],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_rounds: 0,
            code_review_baseline_ms: None,
            code_review_open_findings: Vec::new(),
            code_review_disputed_findings: Vec::new(),
            code_review_handoff: false,
            code_review_handoff_acknowledged: false,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            unknown: Default::default(),
        },
        body: "\
## Todos Board

<!-- todos-board:auto:begin -->
<!-- todos-board:auto:end -->
"
        .to_string(),
    };
    tomcat::core::plan_runtime::file_store::write_plan(&plan_path, &plan, 1_000)
        .expect("write executing plan");
    let runtime = Arc::new(PlanRuntime::new_with_session_id(
        "real-evidence-session",
        "real-evidence-session-id",
    ));
    runtime
        .attach_from_resume_state(ResumeControlState {
            mode: Some(AgentMode::Chat),
            plan_path: Some(plan_path.clone()),
            plan_id: Some(plan_id.clone()),
        })
        .expect("attach executing plan");

    let request = ChatRequest {
        messages: vec![
            ChatMessage::system(
                "You operate an executing plan. Use only the update_plan tool. Do not answer in prose.",
            ),
            ChatMessage::user(format!(
                "Complete the in-progress todo `{todo_id}: {todo_content}`. Call update_plan \
                 exactly once with one set_status operation. Set status to completed and provide \
                 a non-empty evidence string array containing the verification command and \
                 produced artifact. Do not include content, upsert, remove, replace, or any \
                 other operation."
            )),
        ],
        model: wire_model,
        temperature: None,
        max_tokens: Some(512),
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        cache_key: None,
        tools: Some(vec![tool_definition()]),
    };
    let response =
        tokio::time::timeout(REQUEST_TIMEOUT, binding.provider_impl.chat_collect(request))
            .await
            .expect("real evidence request timed out")
            .expect("real evidence request failed");
    let message = response
        .choices
        .first()
        .expect("real model returned no choices")
        .message
        .clone();
    let (args, raw_args) = parse_update_plan_args(&message).unwrap_or_else(|error| {
        panic!("real model did not produce valid completion evidence: {error}")
    });
    let evidence = evidence_for_completion(&raw_args, todo_id)
        .filter(|entries| !entries.is_empty())
        .expect("real model must put non-empty evidence on its completed set_status operation");
    assert!(
        evidence.iter().all(serde_json::Value::is_string),
        "evidence must be a string array: {evidence:?}"
    );

    let result = update_plan::execute(&runtime, args)
        .await
        .expect("execute real-model update_plan arguments");
    assert_eq!(
        result["items"][0]["evidence"].as_array().map(Vec::len),
        Some(evidence.len()),
        "update_plan output must retain real-model evidence"
    );
    let persisted =
        tomcat::core::plan_runtime::file_store::read_plan(&plan_path).expect("read persisted plan");
    let todo = persisted
        .frontmatter
        .todos
        .iter()
        .find(|todo| todo.id == todo_id)
        .expect("persisted work todo");
    assert!(
        !todo.evidence.is_empty(),
        "evidence must persist in frontmatter"
    );
    assert!(
        !persisted.body.contains("\n  - evidence:"),
        "Todos Board must not duplicate frontmatter evidence; body:\n{}",
        persisted.body
    );
    assert!(
        persisted
            .body
            .contains(&format!("- [x] {todo_id}: {todo_content}")),
        "Board must still render the completed todo itself"
    );
}
