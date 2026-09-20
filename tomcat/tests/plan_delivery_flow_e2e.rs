//! End-to-end delivery path without runtime-owned close-out gates.
//!
//! This drives the same plan tools an LLM calls through a persisted PlanFile:
//! create → build/resume → work → acceptance hint → completed.

use serial_test::serial;
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFileState, TodoKind, TodoStatus,
};
use tomcat::core::plan_runtime::PlanRuntime;
use tomcat::core::session::{AgentMode, ResumeControlState};
use tomcat::core::tools::plan_tool::{create_plan, update_plan};

fn test_home() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("create isolated HOME");
    std::fs::create_dir_all(directory.path().join(".tomcat").join("plans"))
        .expect("create plans directory");
    directory
}

fn start_execution(runtime: &PlanRuntime, plan_id: &str) {
    let path = plan_path_for_id(plan_id).expect("derive plan path");
    let mut plan = read_plan(&path).expect("read created plan");
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("delivery-e2e".into());
    plan.frontmatter.session_id = Some("delivery-e2e-session".into());
    write_plan(&path, &plan, 2_000).expect("persist executing plan");
    runtime
        .attach_from_resume_state(ResumeControlState {
            mode: Some(AgentMode::Chat),
            plan_path: Some(path),
            plan_id: Some(plan_id.to_string()),
        })
        .expect("bind executing plan");
}

fn restore_home(previous_home: Option<std::ffi::OsString>) {
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn delivery_e2e_completes_via_llm_authored_acceptance_todo() {
    let previous_home = std::env::var_os("HOME");
    let home = test_home();
    std::env::set_var("HOME", home.path());

    let runtime = PlanRuntime::new("delivery-e2e");
    runtime.enter_plan().expect("enter plan mode");
    let created = create_plan::execute(
        &runtime,
        create_plan::CreatePlanArgs {
            goal: "exercise delivery flow".into(),
            draft: "Implement the delivery flow and accept it in plain language.".into(),
            todos: vec![
                create_plan::TodoArg {
                    id: "implement".into(),
                    content: "implement the requested change".into(),
                    status: TodoStatus::Pending,
                    kind: TodoKind::Work,
                },
                create_plan::TodoArg {
                    id: "acceptance".into(),
                    content: "验收：复核改动并验证受影响行为".into(),
                    status: TodoStatus::Pending,
                    kind: TodoKind::Acceptance,
                },
            ],
        },
    )
    .expect("create plan");
    let plan_id = created["plan_id"].as_str().expect("plan id").to_string();
    assert_eq!(created["items"].as_array().expect("items").len(), 2);

    start_execution(&runtime, &plan_id);
    update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "implement".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .expect("complete work todo");

    let acceptance_started = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "acceptance".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .expect("start acceptance todo");
    assert_eq!(acceptance_started["next_step"]["phase"], "run_verify");
    assert!(acceptance_started["next_step"]["hint"]
        .as_str()
        .expect("verify hint")
        .contains("load_skill(verify)"));

    let completed = update_plan::execute(
        &runtime,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "acceptance".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .expect("complete acceptance todo");
    assert_eq!(completed["plan_state_after"], "completed");
    assert_eq!(
        read_plan(&plan_path_for_id(&plan_id).expect("derive completed plan path"))
            .expect("read completed plan")
            .frontmatter
            .state,
        PlanFileState::Completed
    );

    restore_home(previous_home);
}
