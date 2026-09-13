use super::common::*;
use std::time::Duration;

use crate::core::plan_runtime::file_store::GreenBuildEvidence;

// These tests predate visible close-out gates. Keep their domain assertions, but
// drive the production protocol explicitly: work update → review gate → (when
// evidence is supplied) acceptance gate. Production never performs this implicit
// driving; it remains the agent's responsibility.
mod explicit_gates_update_plan {
    use crate::core::plan_runtime::file_store::{
        TodoKind, TodoStatus, GATE_ACCEPTANCE_TODO_ID, GATE_CODE_REVIEW_TODO_ID,
    };
    use crate::core::plan_runtime::PlanRuntime;
    use crate::core::tools::plan_tool::{update_plan as raw, ToolError};

    pub use raw::{DisputeFindingArg, GreenBuildEvidenceArg, UpdateOp, UpdatePlanArgs};

    pub async fn execute(
        runtime: &PlanRuntime,
        args: UpdatePlanArgs,
    ) -> Result<serde_json::Value, ToolError> {
        // Legacy unit tests often supplied a mock reviewer without an isolated
        // workspace. Give those tests an actual code workspace so they exercise
        // review semantics; docs-only tests attach their own workspace explicitly.
        if runtime.workspace_root().is_none() {
            runtime.attach_workspace_root(std::env::current_dir().expect("test workspace root"));
        }
        let UpdatePlanArgs {
            plan_id,
            path,
            replace,
            ops,
            dispute_findings,
            green_build_pass,
            green_build_evidence,
            acceptance_commands,
        } = args;
        let mut out = raw::execute(
            runtime,
            UpdatePlanArgs {
                plan_id: plan_id.clone(),
                path: path.clone(),
                replace,
                ops,
                dispute_findings,
                green_build_pass: None,
                green_build_evidence: Vec::new(),
                acceptance_commands,
            },
        )
        .await?;

        if out["next_step"]["phase"].as_str() == Some("start_review") {
            out = raw::execute(
                runtime,
                gate_args(plan_id.clone(), path.clone(), GATE_CODE_REVIEW_TODO_ID),
            )
            .await?;
        }

        if green_build_pass.is_some() && out["code_review_pass"].as_bool() == Some(true) {
            let review_result = out["code_review"].clone();
            let plan_id_for_acceptance = plan_id
                .as_deref()
                .or_else(|| out["plan_id"].as_str())
                .expect("acceptance requires a plan id");
            if !gate_is_in_progress(&out, TodoKind::GateAcceptance)
                && !runtime.acceptance_is_in_flight(plan_id_for_acceptance)
            {
                raw::execute(
                    runtime,
                    gate_args(plan_id.clone(), path.clone(), GATE_ACCEPTANCE_TODO_ID),
                )
                .await?;
            }
            let mut completed = raw::execute(
                runtime,
                UpdatePlanArgs {
                    plan_id,
                    path,
                    replace: false,
                    ops: Vec::new(),
                    dispute_findings: Vec::new(),
                    green_build_pass,
                    green_build_evidence,
                    acceptance_commands: None,
                },
            )
            .await?;
            if !review_result.is_null() {
                completed["code_review"] = review_result;
            }
            out = completed;
        }
        Ok(out)
    }

    fn gate_args(plan_id: Option<String>, path: Option<String>, id: &str) -> UpdatePlanArgs {
        UpdatePlanArgs {
            plan_id,
            path,
            replace: false,
            ops: vec![UpdateOp::SetStatus {
                id: id.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
        }
    }

    fn gate_is_in_progress(out: &serde_json::Value, kind: TodoKind) -> bool {
        out["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["kind"].as_str() == Some(kind.as_str())
                    && item["status"].as_str() == Some("in_progress")
            })
        })
    }
}

use explicit_gates_update_plan as update_plan;

fn git_workspace_with_code(prefix: &str) -> tempfile::TempDir {
    let workspace = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let init_status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(workspace.path())
        .status()
        .unwrap();
    assert!(
        init_status.success(),
        "temporary workspace must be a Git repository"
    );
    workspace
}

fn passing_code_review() -> CodeReviewSummary {
    CodeReviewSummary {
        aborted: false,
        verdict: Some("pass".into()),
        summary: "review passed".into(),
        ..Default::default()
    }
}

fn complete_all_args(
    plan_id: &str,
    green_build_pass: Option<bool>,
    green_build_evidence: Vec<update_plan::GreenBuildEvidenceArg>,
) -> update_plan::UpdatePlanArgs {
    update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.into()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass,
        green_build_evidence,
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    }
}

fn submit_green_evidence_args(
    plan_id: &str,
    command: &str,
    task_id: &str,
) -> update_plan::UpdatePlanArgs {
    update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.into()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: Some(true),
        green_build_evidence: vec![update_plan::GreenBuildEvidenceArg {
            command: command.into(),
            task_id: task_id.into(),
        }],
        acceptance_commands: None,
        ops: Vec::new(),
    }
}

fn assert_bad_args(error: ToolError, expected: impl AsRef<str>) {
    match error {
        ToolError::BadArgs(message) => assert_eq!(message, expected.as_ref()),
        other => panic!("expected ToolError::BadArgs, got {other:?}"),
    }
}

fn seed_previous_full_gate(plan: &mut PlanFile, completion_gate_cycles: u32) {
    plan.frontmatter.code_review_pass = true;
    plan.frontmatter.code_review_pass_at_ms = Some(0);
    plan.frontmatter.green_build_pass = true;
    plan.frontmatter.green_build_evidence = vec![GreenBuildEvidence {
        command: "old verification".into(),
        task_id: "old-task".into(),
        started_at_ms: 0,
        exit_code: 0,
    }];
    plan.frontmatter.completion_gate_cycles = completion_gate_cycles;
}

#[tokio::test]
async fn code_review_pass_completes_without_verifier() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_state_restart_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "code review passed".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: vec![crate::core::plan_runtime::review::Finding::new(
                "suggestion".into(),
                "tests".into(),
                "nice to have".into(),
            )],
            ..Default::default()
        },
    ])));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["code_review"]["verdict"], "pass");
    assert_eq!(out["code_review"]["findings"][0]["area"], "tests");
    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(out["next_step"]["phase"], "run_acceptance");
    assert!(out.get("verify").is_none());
    assert_eq!(review_rounds(&plan_id), 1);
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Executing);
    assert_eq!(rt.mode(), AgentMode::Plan);
    let events = captured.lock();
    let code_review_event = events
        .iter()
        .find(|v| v["event"] == "plan.code_review")
        .expect("缺少 plan.code_review");
    assert_eq!(code_review_event["verdict"], "pass");
    assert_eq!(code_review_event["findings"][0]["area"], "tests");
    assert_eq!(code_review_event["rounds"], 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn p0_blocks_its_review_round_and_hands_off_when_budget_is_exhausted() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_p0_handoff_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "incorrectly marked pass".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            findings: vec![crate::core::plan_runtime::review::Finding::new(
                "P0".into(),
                "authorization".into(),
                "missing ownership check".into(),
            )],
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(open_finding_references(&plan_id), vec!["F01"]);
    assert!(out["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("verdict=pass"))));
    assert_eq!(out["code_review_pass"], false);
    assert_eq!(out["next_step"]["phase"], "handoff");
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.code_review_handoff);
    assert_eq!(
        persisted.frontmatter.code_review_residual_findings,
        vec!["F01 [P0] authorization: missing ownership check"]
    );

    assert_eq!(
        rt.refresh_code_review_budget_after_user_message()
            .expect("user acknowledgement refreshes the budget"),
        Some(plan_id.clone())
    );
    let refreshed = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(refreshed.frontmatter.code_review_rounds, 0);
    assert!(!refreshed.frontmatter.code_review_handoff);
    assert!(refreshed.frontmatter.code_review_handoff_acknowledged);
    assert_eq!(refreshed.frontmatter.code_review_open_findings.len(), 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn exhausted_review_budget_still_rejects_fabricated_green_build_evidence() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_fail_open_invalid_evidence");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry);
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "review found a defect".into(),
            findings: vec![Finding::new(
                "P1".into(),
                "logic".into(),
                "missing regression coverage".into(),
            )],
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let first = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .expect("first review should return its finding");
    assert_eq!(first["code_review_pass"], true);
    assert_eq!(first["next_step"]["phase"], "run_acceptance");

    let error = update_plan::execute(
        &rt,
        submit_green_evidence_args(&plan_id, "true", "fabricated-task-id"),
    )
    .await
    .expect_err("fail-open must not accept a fabricated green-build task");
    assert!(
        error
            .to_string()
            .contains("找不到后台 bash 任务 `fabricated-task-id`"),
        "unexpected error: {error}"
    );

    cleanup_home(&home);
}

#[tokio::test]
async fn p2_only_finding_does_not_block_completion_even_when_reviewer_says_fail() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_p2_only_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![CodeReviewSummary {
        aborted: false,
        verdict: Some("fail".into()),
        summary: "non-blocking cleanup suggestion".into(),
        findings: vec![Finding::new(
            "P2".into(),
            "style".into(),
            "rename the internal helper".into(),
        )],
        ..Default::default()
    }]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["code_review"]["verdict"], "fail");
    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(out["next_step"]["phase"], "run_acceptance");
    assert_eq!(review_rounds(&plan_id), 1);
    assert!(open_findings(&plan_id).is_empty());
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Executing);
    assert!(persisted.frontmatter.code_review_pass);
    cleanup_home(&home);
}

#[tokio::test]
async fn p1_dispute_with_reason_unblocks_and_excludes_open_finding() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_p1_dispute_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "first pass".into(),
            findings: vec![Finding::new(
                "P1".into(),
                "compatibility".into(),
                "old clients remain unsupported".into(),
            )],
            ..Default::default()
        },
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "accepted trade-off was not re-reported".into(),
            ..Default::default()
        },
    ]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(2);

    let finish = |dispute_findings| update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.clone()),
        path: None,
        replace: false,
        dispute_findings,
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    };

    let first = update_plan::execute(&rt, finish(Vec::new())).await.unwrap();
    assert_eq!(first["plan_state_after"], "executing");
    assert_eq!(open_finding_references(&plan_id), vec!["F01"]);

    let mut dispute = finish(vec![update_plan::DisputeFindingArg {
        reference: "F01".into(),
        area: "compatibility".into(),
        resolution: "wontfix".into(),
        reason: "support for this retired client is an explicit product decision".into(),
    }]);
    dispute.ops.clear();
    let out = update_plan::execute(&rt, dispute).await.unwrap();

    assert_eq!(out["plan_state_after"], "executing");
    assert_eq!(out["next_step"]["phase"], "run_acceptance");
    assert!(open_findings(&plan_id).is_empty());
    assert_eq!(disputed_findings(&plan_id).len(), 1);
    let open_findings = reviewer.open_findings_per_round();
    assert_eq!(open_findings.len(), 2);
    assert!(open_findings[1].is_empty(), "disputed P1 must not be open");
    cleanup_home(&home);
}

#[tokio::test]
async fn p0_cannot_be_disputed() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    set_open_findings(
        &plan_id,
        vec![Finding::new(
            "P0".into(),
            "authorization".into(),
            "any user can read another tenant".into(),
        )
        .with_reference("F01")],
    );

    let error = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F01".into(),
                area: "authorization".into(),
                resolution: "wontfix".into(),
                reason: "not actually an issue".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect_err("P0 may not be disputed");
    assert!(error.to_string().contains("P0"));
    cleanup_home(&home);
}

#[tokio::test]
async fn p0_can_be_disputed_after_the_user_acknowledges_a_handoff() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.code_review_handoff = true;
    plan.frontmatter.code_review_handoff_acknowledged = true;
    plan.frontmatter.code_review_open_findings = vec![Finding::new(
        "P0".into(),
        "authorization".into(),
        "legacy endpoint intentionally remains unauthenticated".into(),
    )
    .with_reference("F01")];
    write_plan(&path, &plan, 1_000).unwrap();
    rt.bind_plan_file_for_test(path.clone());

    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F01".into(),
                area: "authorization".into(),
                resolution: "wontfix".into(),
                reason: "the user explicitly accepted this legacy compatibility trade-off".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect("an acknowledged P0 handoff may record a user-approved wontfix");

    let persisted = read_plan(&path).unwrap();
    assert!(persisted.frontmatter.code_review_open_findings.is_empty());
    assert_eq!(persisted.frontmatter.code_review_disputed_findings.len(), 1);
    assert_eq!(
        persisted.frontmatter.code_review_disputed_findings[0].severity,
        "P0"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn dispute_rejects_non_wontfix_resolution() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    set_open_findings(
        &plan_id,
        vec![
            Finding::new("P1".into(), "tests".into(), "missing regression".into())
                .with_reference("F01"),
        ],
    );

    let error = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F01".into(),
                area: "tests".into(),
                resolution: "fixed".into(),
                reason: "unrelated".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect_err("only wontfix is a dispute");
    assert!(error.to_string().contains("wontfix"));
    cleanup_home(&home);
}

#[tokio::test]
async fn green_build_gate_blocks_completion_until_pass() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = std::env::temp_dir().join(format!(
        "tomcat_green_build_gate_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/main.rs"), "fn main() {}\n").unwrap();
    let init_status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .status()
        .unwrap();
    assert!(
        init_status.success(),
        "temporary workspace must be a Git repository"
    );

    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.clone());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "review passed".into(),
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let close = |green_build_pass, green_build_evidence| update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.clone()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass,
        green_build_evidence,
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    };

    let review_passed = update_plan::execute(&rt, close(None, Vec::new()))
        .await
        .expect("review pass should navigate to the visible acceptance gate");
    assert_eq!(review_passed["next_step"]["phase"], "run_acceptance");
    assert!(review_passed["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("load_skill(verify)")));
    assert!(review_passed["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("declared `acceptance_commands`")));
    assert!(!review_passed["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("complete check set")));
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.code_review_pass);
    assert!(!persisted.frontmatter.green_build_pass);

    let ticket = registry
        .spawn("true".into(), Some(workspace.clone()))
        .await
        .unwrap();
    registry
        .wait_for_finish(ticket.task_id.as_str())
        .await
        .unwrap();
    let out = update_plan::execute(
        &rt,
        close(
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: "true".into(),
                task_id: ticket.task_id.to_string(),
            }],
        ),
    )
    .await
    .unwrap();
    assert_eq!(out["plan_state_after"], "completed");
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.green_build_pass);
    assert_eq!(
        persisted.frontmatter.green_build_evidence[0].command,
        "true"
    );

    let _ = std::fs::remove_dir_all(workspace);
    cleanup_home(&home);
}

#[tokio::test]
async fn gates_skipped_when_diff_has_no_code() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = std::env::temp_dir().join(format!(
        "tomcat_non_code_diff_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("README.md"), "# documentation only\n").unwrap();
    let init_status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .status()
        .unwrap();
    assert!(
        init_status.success(),
        "temporary workspace must be a Git repository"
    );

    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(Vec::new()));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.clone());
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["plan_state_after"], "completed");
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    let _ = std::fs::remove_dir_all(workspace);
    cleanup_home(&home);
}

/// D1-b 后门 1：reviewer 中止拿不到通过结论，绝不能当作通过收工。
#[tokio::test]
async fn aborted_code_review_keeps_plan_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_abort_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![aborted_code_review(
        "reviewer spawn failed",
    )]));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_code_reviewer(reviewer.clone());
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["code_review"]["verdict"], "aborted");
    assert_eq!(out["plan_state_after"], "executing");
    assert!(out.get("verify").is_none());
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Executing);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        review_rounds(&plan_id),
        0,
        "技术故障不得消耗正常 review 预算"
    );
    let warnings = out["warnings"].as_array().expect("warnings array");
    assert!(
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|text| text.contains("aborted"))
        }),
        "aborted code review 应明确说明未拿到通过结论"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn aborted_second_review_preserves_the_first_completed_round_state() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_aborted_second_review_");
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            verdict: Some("fail".into()),
            findings: vec![Finding::new(
                "P1".into(),
                "logic".into(),
                "missing guard".into(),
            )],
            ..Default::default()
        },
        aborted_code_review("second review lost its worker"),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(4);

    update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .expect("first review completes with its finding");
    let before = review_frontmatter(&plan_id);

    tokio::time::sleep(Duration::from_millis(5)).await;
    std::fs::write(
        workspace.path().join("src/main.rs"),
        "fn main() { /* guard added */ }\n",
    )
    .unwrap();
    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: GATE_CODE_REVIEW_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .expect("the second review may start but aborts");

    let after = review_frontmatter(&plan_id);
    assert_eq!(after.code_review_rounds, before.code_review_rounds);
    assert_eq!(
        after.code_review_baseline_ms,
        before.code_review_baseline_ms
    );
    assert_eq!(
        after.code_review_open_findings,
        before.code_review_open_findings
    );
    assert_eq!(
        after.code_review_disputed_findings,
        before.code_review_disputed_findings
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn aborted_code_review_refunds_round_and_stops_after_bounded_retries() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_infra_retry_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        aborted_code_review("first infrastructure failure"),
        aborted_code_review("second infrastructure failure"),
        aborted_code_review("third infrastructure failure"),
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "must not be dispatched after retry handoff".into(),
            ..Default::default()
        },
    ]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let complete_all = || update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.clone()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    };
    let reopen_t1 = || update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.clone()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        acceptance_commands: None,
        ops: vec![update_plan::UpdateOp::SetStatus {
            id: "t1".into(),
            content: None,
            status: TodoStatus::InProgress,
        }],
    };

    for retry in 1..=3 {
        if retry > 1 {
            update_plan::execute(&rt, reopen_t1()).await.unwrap();
        }
        let out = update_plan::execute(&rt, complete_all()).await.unwrap();

        assert_eq!(out["code_review"]["verdict"], "aborted");
        assert_eq!(out["plan_state_after"], "executing");
        assert_eq!(review_rounds(&plan_id), 0);
        assert_eq!(rt.review_infra_retries(&plan_id), retry);
        let warnings = out["warnings"].as_array().expect("warnings array");
        let expected_warning = if retry <= 2 {
            format!("第 {retry}/2 次基础设施重试")
        } else {
            "连续技术故障已超过 2 次".into()
        };
        assert!(
            warnings.iter().any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains(&expected_warning))),
            "retry {retry} should report bounded retry state"
        );
    }

    let handoff = update_plan::execute(&rt, reopen_t1()).await.unwrap();
    assert!(handoff["code_review"].is_null());
    assert_eq!(handoff["plan_state_after"], "executing");
    assert_eq!(handoff["next_step"]["phase"], "handoff");
    assert_eq!(review_rounds(&plan_id), 0);
    assert_eq!(rt.review_infra_retries(&plan_id), 3);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        3,
        "the fourth completion attempt must hand off without redispatching"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn final_review_round_immediately_advances_to_acceptance_with_p1_residual() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_p1_review_exhaustion_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![CodeReviewSummary {
        aborted: false,
        verdict: Some("fail".into()),
        summary: "code review found a concrete issue".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        findings: vec![crate::core::plan_runtime::review::Finding::new(
            "P1".into(),
            "logic".into(),
            "missing guard".into(),
        )],
        ..Default::default()
    }]));
    let verifier = std::sync::Arc::new(MockVerifierDispatcher::new(vec![ok_verify_pass()]));
    rt.attach_code_reviewer(reviewer.clone());
    rt.attach_verifier(verifier.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let first = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(first["code_review"]["verdict"], "fail");
    assert_eq!(first["plan_state_after"], "executing");
    assert!(first.get("verify").is_none());
    assert_eq!(first["code_review"]["findings"][0]["note"], "missing guard");
    assert_eq!(first["items"].as_array().unwrap().len(), 4);
    assert!(first["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| !item["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("cr_fix_")));
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let after_first_review = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    let current_code = crate::core::plan_runtime::code_reviewer::collect_code_diff_context(
        &rt.workspace_root().expect("wrapper attaches workspace"),
    )
    .await;
    assert!(
        current_code.newest_edit_mtime_ms <= after_first_review.frontmatter.code_review_baseline_ms,
        "zero-code-change review must retain its baseline: current={current_code:?}, baseline={:?}",
        after_first_review.frontmatter.code_review_baseline_ms
    );
    assert_eq!(first["next_step"]["phase"], "run_acceptance");
    assert!(first["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("configured round budget")));
    assert!(first["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("F01 [P1] logic: missing guard")));
    assert!(first["items"].as_array().is_some_and(|items| items
        .iter()
        .any(|item| item["id"] == GATE_CODE_REVIEW_TODO_ID && item["status"] == "completed")));
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Executing);
    assert!(persisted.frontmatter.code_review_pass);
    assert_eq!(
        persisted.frontmatter.code_review_residual_findings,
        vec!["F01 [P1] logic: missing guard"]
    );
    assert_eq!(review_rounds(&plan_id), 1);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let warnings = first["warnings"].as_array().expect("warnings array");
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("仅剩 P1"))));

    let events = captured.lock();
    let exhausted = events
        .iter()
        .find(|v| v["event"] == "plan.code_review.exhausted")
        .expect("缺少 plan.code_review.exhausted");
    assert_eq!(exhausted["rounds"], 1);
    assert_eq!(
        exhausted["outcome"],
        "passed_to_acceptance_with_residual_findings"
    );
    assert_eq!(exhausted["code_review_pass"], true);
    assert_eq!(
        exhausted["residual_findings"]
            .as_array()
            .expect("residual_findings array")
            .len(),
        1
    );
    assert!(
        !events.iter().any(|v| v["event"] == "plan.complete"),
        "轮数用尽不得写 plan.complete"
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn final_review_round_immediately_hands_off_with_p0_residual() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_p0_review_exhaustion_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "code review found a critical defect".into(),
            findings: vec![Finding::new(
                "P0".into(),
                "security".into(),
                "authorization can be bypassed".into(),
            )],
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let first = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .unwrap();
    assert_eq!(first["code_review"]["findings"][0]["severity"], "P0");
    assert_eq!(first["code_review_pass"], false);
    assert_eq!(first["plan_state_after"], "executing");
    assert_eq!(first["next_step"]["phase"], "handoff");
    assert!(first["next_step"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("F01 [P0] security: authorization can be bypassed")));
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.code_review_handoff);
    assert_eq!(
        persisted
            .frontmatter
            .todos
            .iter()
            .find(|todo| todo.id == GATE_CODE_REVIEW_TODO_ID)
            .expect("review gate")
            .status,
        TodoStatus::Pending
    );
    assert_eq!(
        persisted.frontmatter.code_review_residual_findings,
        vec!["F01 [P0] security: authorization can be bypassed"]
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn third_p0_review_can_reach_the_fourth_and_fourth_hands_off() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_four_round_p0_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(
        (0..4)
            .map(|_| CodeReviewSummary {
                verdict: Some("fail".into()),
                findings: vec![Finding::new(
                    "P0".into(),
                    "authorization".into(),
                    "missing ownership check".into(),
                )],
                ..Default::default()
            })
            .collect(),
    ));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(4);

    let first = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .unwrap();
    assert_eq!(first["next_step"]["phase"], "fix_findings");

    for round in 2..=4 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        std::fs::write(
            workspace.path().join("src/main.rs"),
            format!("fn main() {{ /* ownership attempt {round} */ }}\n"),
        )
        .unwrap();
        let out = update_plan::execute(
            &rt,
            update_plan::UpdatePlanArgs {
                plan_id: Some(plan_id.clone()),
                path: None,
                replace: false,
                dispute_findings: Vec::new(),
                green_build_pass: None,
                green_build_evidence: Vec::new(),
                acceptance_commands: None,
                ops: vec![update_plan::UpdateOp::SetStatus {
                    id: GATE_CODE_REVIEW_TODO_ID.into(),
                    content: None,
                    status: TodoStatus::InProgress,
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            out["next_step"]["phase"],
            if round < 4 { "fix_findings" } else { "handoff" },
            "round {round}"
        );
    }
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        4
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.code_review_rounds, 4);
    assert!(persisted.frontmatter.code_review_handoff);
    cleanup_home(&home);
}

#[tokio::test]
async fn resolved_findings_converge_to_completion_within_review_budget() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_resolved_review_finding_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let first_finding = Finding::new(
        "P1".into(),
        "logic".into(),
        "missing an authorization guard".into(),
    )
    .with_reference("F01");
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "add the authorization guard".into(),
            findings: vec![first_finding.clone()],
            ..Default::default()
        },
        CodeReviewSummary {
            aborted: false,
            verdict: Some("pass".into()),
            summary: "the authorization guard is now present".into(),
            ..Default::default()
        },
    ]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(2);

    let complete_all = || update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id.clone()),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    };

    let first = update_plan::execute(&rt, complete_all()).await.unwrap();
    assert_eq!(first["plan_state_after"], "executing");
    assert_eq!(open_findings(&plan_id), vec![first_finding.clone()]);
    assert_eq!(review_rounds(&plan_id), 1);

    tokio::time::sleep(Duration::from_millis(5)).await;
    std::fs::write(
        workspace.path().join("src/main.rs"),
        "fn main() { /* authorization guard added */ }\n",
    )
    .unwrap();
    let second = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .unwrap();

    assert_eq!(second["code_review"]["verdict"], "pass");
    assert_eq!(second["plan_state_after"], "executing");
    assert_eq!(second["next_step"]["phase"], "run_acceptance");
    assert_eq!(review_rounds(&plan_id), 2);
    assert!(open_findings(&plan_id).is_empty());
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        reviewer.open_findings_per_round(),
        vec![Vec::new(), vec![first_finding]],
        "the second round must recheck the first round's unresolved finding"
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Executing);
    assert!(persisted.frontmatter.code_review_pass);
    cleanup_home(&home);
}

#[tokio::test]
async fn code_review_transcript_matches_tool_result_after_normalization() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_transcript_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: None,
            summary: "review finished without verdict".into(),
            changes_summary: "none".into(),
            applied_changes: false,
            ..Default::default()
        },
    ])));
    rt.attach_verifier(std::sync::Arc::new(MockVerifierDispatcher::new(vec![
        ok_verify_pass(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["code_review"]["verdict"], "partial");
    let events = captured.lock();
    let code_review_event = events
        .iter()
        .find(|v| v["event"] == "plan.code_review")
        .expect("缺少 plan.code_review");
    assert_eq!(code_review_event["verdict"], out["code_review"]["verdict"]);
    assert_eq!(code_review_event["summary"], out["code_review"]["summary"]);
    assert_eq!(code_review_event["rounds"], 1);
    cleanup_home(&home);
}

/// D1-d：第 2 轮 review 必须拿到第 1 轮的未清 finding，并且已修项要被核销。
#[tokio::test]
async fn second_review_round_receives_previous_open_findings_and_clears_fixed_ones() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_state_restart_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let round1 =
        Finding::new("P1".into(), "logic".into(), "missing guard".into()).with_reference("F01");
    let round2 = Finding::new("P1".into(), "tests".into(), "no regression test".into())
        .with_reference("F01");
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "round 1".into(),
            findings: vec![round1.clone()],
            ..Default::default()
        },
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "round 2".into(),
            findings: vec![round2.clone()],
            ..Default::default()
        },
    ]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(8);

    let complete_all = |plan_id: String| update_plan::UpdatePlanArgs {
        plan_id: Some(plan_id),
        path: None,
        replace: false,
        dispute_findings: Vec::new(),
        green_build_pass: None,
        green_build_evidence: Vec::new(),
        acceptance_commands: None,
        ops: vec![
            update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            },
            update_plan::UpdateOp::SetStatus {
                id: "t2".into(),
                content: None,
                status: TodoStatus::Completed,
            },
        ],
    };

    update_plan::execute(&rt, complete_all(plan_id.clone()))
        .await
        .unwrap();
    assert_eq!(open_finding_references(&plan_id), vec!["F01".to_string()]);

    // Simulate a backend restart: only the PlanFile may carry the review
    // baseline and the open finding into the second dispatch.
    let recreated = PlanRuntime::new("session-a");
    recreated.attach_code_reviewer(reviewer.clone());
    recreated.set_max_code_review_rounds(8);
    recreated.bind_plan_file_for_test(plan_path_for_id(&plan_id).unwrap());
    recreated.attach_workspace_root(workspace.path().to_path_buf());
    std::thread::sleep(Duration::from_millis(5));
    std::fs::write(
        workspace.path().join("src/main.rs"),
        "fn main() { /* fixed */ }\n",
    )
    .unwrap();

    // 重开一个 todo 再收口，触发第 2 轮。
    update_plan::execute(
        &recreated,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();
    update_plan::execute(&recreated, complete_all(plan_id.clone()))
        .await
        .unwrap();

    let seen = reviewer.open_findings_per_round();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].is_empty(), "第 1 轮没有历史 finding");
    assert_eq!(
        seen[1],
        vec![round1.clone()],
        "第 2 轮应收到第 1 轮的未清项"
    );
    let dispatches = reviewer.dispatches();
    assert_eq!(dispatches.len(), 2);
    assert_eq!(dispatches[0].round, 1);
    assert!(!dispatches[0].is_incremental);
    assert_eq!(dispatches[1].round, 2);
    assert!(
        dispatches[1].is_incremental,
        "重启后第 2 轮必须带前一轮的 durable baseline"
    );
    // 第 2 轮没再报 round1 → 视为已修，只剩 round2。
    assert_eq!(open_finding_references(&plan_id), vec!["F01".to_string()]);
    cleanup_home(&home);
}

/// F0X 是当前 review 结果的局部引用，跨轮不承诺稳定。
#[test]
fn findings_from_mock_reviews_do_not_require_content_hash_ids() {
    let finding =
        Finding::new("P1".into(), "logic".into(), "missing guard".into()).with_reference("F01");
    assert_eq!(finding.reference, "F01");
    assert!(finding.blocks());
}

#[tokio::test]
async fn green_build_evidence_rejects_nonzero_and_running_background_tasks() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_green_evidence_rejections_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let reviewer =
        std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![passing_code_review()]));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let failed = registry
        .spawn("false".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&failed.task_id).await.unwrap();
    let nonzero_error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: "false".into(),
                task_id: failed.task_id.clone(),
            }],
        ),
    )
    .await
    .expect_err("a nonzero background task must not satisfy the green-build gate");
    assert!(nonzero_error.to_string().contains("exit_code="));

    let running = registry
        .spawn("sleep 1".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    let running_error = update_plan::execute(
        &rt,
        submit_green_evidence_args(&plan_id, "sleep 1", &running.task_id),
    )
    .await
    .expect_err("a still-running background task must not satisfy the green-build gate");
    assert!(running_error.to_string().contains("尚未成功结束"));
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "evidence retries must reuse the fresh review"
    );
    registry.wait_for_finish(&running.task_id).await.unwrap();
    cleanup_home(&home);
}

#[tokio::test]
async fn green_build_evidence_rejects_task_started_before_latest_code_edit() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_green_evidence_stale_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        passing_code_review(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let old_task = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&old_task.task_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    std::fs::write(
        workspace.path().join("src/main.rs"),
        "fn main() { println!(\"changed after verification\"); }\n",
    )
    .unwrap();

    let error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: "true".into(),
                task_id: old_task.task_id,
            }],
        ),
    )
    .await
    .expect_err("a verification task started before the latest edit is stale");
    assert!(error.to_string().contains("证据已过期"));
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.code_review_pass);
    assert!(!persisted.frontmatter.green_build_pass);
    cleanup_home(&home);
}

#[tokio::test]
async fn shot_command_satisfies_green_build_gate() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_shot_green_gate_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let reviewer =
        std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![passing_code_review()]));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(reviewer);
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let scripts_dir = workspace
        .path()
        .join(".agents")
        .join("skills")
        .join("verify")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    let shot_script = scripts_dir.join("shot.mjs");
    std::fs::write(&shot_script, "process.exit(0);\n").unwrap();
    let output_dir = workspace.path().join(".agents").join("shots");
    let command = format!(
        "node {} http://127.0.0.1:3000 --out {}",
        shot_script.display(),
        output_dir.display()
    );
    let task = registry
        .spawn(command.clone(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&task.task_id).await.unwrap();

    let out = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command,
                task_id: task.task_id,
            }],
        ),
    )
    .await
    .expect("a fresh successful shot command is valid green-build evidence");
    assert_eq!(out["plan_state_after"], "completed");
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.green_build_pass);
    cleanup_home(&home);
}

#[tokio::test]
async fn persisted_fresh_review_skips_rerun_after_runtime_recreation() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_persisted_review_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let initial_reviewer =
        std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![passing_code_review()]));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(initial_reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let review_passed = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .expect("review pass should navigate to acceptance");
    assert_eq!(review_passed["next_step"]["phase"], "run_acceptance");
    let path = plan_path_for_id(&plan_id).unwrap();
    let after_review = read_plan(&path).unwrap();
    assert!(after_review.frontmatter.code_review_pass);
    assert!(!after_review.frontmatter.green_build_pass);

    let ticket = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&ticket.task_id).await.unwrap();

    let recreated_reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(Vec::new()));
    let recreated = PlanRuntime::new("session-a");
    recreated.attach_workspace_root(workspace.path().to_path_buf());
    recreated.attach_bash_task_registry(registry);
    recreated.attach_code_reviewer(recreated_reviewer.clone());
    recreated.set_max_code_review_rounds(1);
    recreated
        .attach_from_resume_state(ResumeControlState {
            mode: Some(AgentMode::Chat),
            plan_path: Some(path),
            plan_id: Some(plan_id.clone()),
        })
        .unwrap();

    let out = update_plan::execute(
        &recreated,
        submit_green_evidence_args(&plan_id, "true", &ticket.task_id),
    )
    .await
    .unwrap();
    assert_eq!(out["plan_state_after"], "completed");
    assert_eq!(
        initial_reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        recreated_reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the persisted fresh review should survive runtime recreation"
    );
    cleanup_home(&home);
}

/// Once review reaches its configured budget with P1-only residuals, acceptance owns
/// verification of later edits without reopening a review that would only fail open.
#[tokio::test]
async fn edit_during_acceptance_after_exhausted_review_keeps_gates_and_accepts_fresh_evidence() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_exhausted_acceptance_edit_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    let captured: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let sink = std::sync::Arc::clone(&captured);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        CodeReviewSummary {
            aborted: false,
            verdict: Some("fail".into()),
            summary: "review found a defect".into(),
            findings: vec![Finding::new(
                "P1".into(),
                "logic".into(),
                "missing regression coverage".into(),
            )
            .with_reference("F01")],
            ..Default::default()
        },
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let exhausted = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .expect("first review returns the finding");
    assert_eq!(exhausted["code_review_pass"], true);

    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: GATE_ACCEPTANCE_TODO_ID.into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .expect("start acceptance after exhausted review");

    tokio::time::sleep(Duration::from_millis(25)).await;
    std::fs::write(
        workspace.path().join("src/main.rs"),
        "fn main() { println!(\"fixed during acceptance\"); }\n",
    )
    .unwrap();
    let proof = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&proof.task_id).await.unwrap();
    rt.record_plan_cache_usage(1_000, Some(410), false);

    let completed = update_plan::execute(
        &rt,
        submit_green_evidence_args(&plan_id, "true", proof.task_id.as_str()),
    )
    .await
    .expect("fresh acceptance evidence must be accepted without reopening review");
    assert_eq!(completed["plan_state_after"], "completed");
    assert_eq!(completed["code_review_pass"], true);
    assert_eq!(completed["green_build_pass"], true);
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Completed);
    assert_eq!(
        persisted.frontmatter.code_review_residual_findings,
        vec!["F01 [P1] logic: missing regression coverage"]
    );

    let events = captured.lock();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "plan.code_review.exhausted")
            .count(),
        1
    );
    let unreviewed_edit = events
        .iter()
        .find(|event| event["event"] == "plan.code_review.unreviewed_edit")
        .expect("must retain an auditable unreviewed edit event");
    assert_eq!(unreviewed_edit["rounds"], 1);
    assert_eq!(
        unreviewed_edit["changed_code_files"],
        serde_json::json!(["src/main.rs"])
    );
    let complete = events
        .iter()
        .find(|event| event["event"] == "plan.complete")
        .expect("completion event should include the build cache observation");
    assert_eq!(complete["cacheObservation"]["cacheHitRatio"], 0.41);
    cleanup_home(&home);
}

#[tokio::test]
async fn stale_green_evidence_after_exhausted_review_reopens_only_acceptance() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_exhausted_stale_green_");
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.code_review_rounds = 1;
    plan.frontmatter.code_review_baseline_ms = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    );
    for todo in &mut plan.frontmatter.todos {
        if matches!(
            todo.kind,
            TodoKind::GateCodeReview | TodoKind::GateAcceptance
        ) {
            todo.status = TodoStatus::Completed;
        }
    }
    seed_previous_full_gate(&mut plan, 0);
    plan.frontmatter.code_review_residual_findings =
        vec!["F01 [P1] tests: pending acceptance".into()];
    write_plan(&path, &plan, 2000).unwrap();
    rt.refresh_active_plan_after_write(path, &plan);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect("stale green evidence must reopen only acceptance");

    assert_eq!(out["code_review_pass"], true);
    assert_eq!(out["green_build_pass"], false);
    assert_eq!(out["next_step"]["phase"], "run_acceptance");
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.completion_gate_cycles, 1);
    assert_eq!(
        persisted.frontmatter.code_review_residual_findings,
        vec!["F01 [P1] tests: pending acceptance"]
    );
    assert!(persisted
        .frontmatter
        .todos
        .iter()
        .any(|todo| todo.kind == TodoKind::GateCodeReview && todo.status == TodoStatus::Completed));
    assert!(persisted
        .frontmatter
        .todos
        .iter()
        .any(|todo| todo.kind == TodoKind::GateAcceptance && todo.status == TodoStatus::Pending));
    cleanup_home(&home);
}

#[tokio::test]
async fn code_edit_invalidates_full_gate_and_requires_review_and_fresh_evidence() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_gate_edit_invalidation_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let reviewer =
        std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![passing_code_review()]));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.todos[1].status = TodoStatus::Completed;
    seed_previous_full_gate(&mut plan, 0);
    write_plan(&path, &plan, 2000).unwrap();
    rt.refresh_active_plan_after_write(path, &plan);

    let fresh_task = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .unwrap();
    registry.wait_for_finish(&fresh_task.task_id).await.unwrap();
    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: Some(true),
            green_build_evidence: vec![update_plan::GreenBuildEvidenceArg {
                command: "true".into(),
                task_id: fresh_task.task_id.clone(),
            }],
            acceptance_commands: None,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["code_review"]["verdict"], "pass");
    assert_eq!(out["plan_state_after"], "completed");
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the edit must invalidate the prior review"
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(persisted.frontmatter.code_review_pass);
    assert!(persisted.frontmatter.green_build_pass);
    assert_eq!(
        persisted.frontmatter.green_build_evidence[0].task_id,
        fresh_task.task_id
    );
    assert_eq!(persisted.frontmatter.completion_gate_cycles, 1);
    cleanup_home(&home);
}

#[tokio::test]
async fn completion_gate_cycle_cap_closes_without_another_review_rerun() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_gate_cycle_cap_");
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(Vec::new()));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);
    rt.set_max_completion_gate_cycles(1);

    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.todos[1].status = TodoStatus::Completed;
    seed_previous_full_gate(&mut plan, 1);
    write_plan(&path, &plan, 2000).unwrap();
    rt.refresh_active_plan_after_write(path, &plan);

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();

    assert_eq!(out["plan_state_after"], "completed");
    assert!(out["warnings"].as_array().unwrap().iter().any(|warning| {
        warning
            .as_str()
            .is_some_and(|text| text.contains("验收重跑已达到上限 1"))
    }));
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the configured cap must prevent another reviewer dispatch"
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert_eq!(persisted.frontmatter.state, PlanFileState::Completed);
    assert_eq!(persisted.frontmatter.completion_gate_cycles, 1);
    cleanup_home(&home);
}

/// D1-e：同一进程里二次 build 同一个计划，轮数预算重新发放。
#[tokio::test]
async fn rebuild_resets_code_review_rounds() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let workspace = git_workspace_with_code("tomcat_review_rebuild_");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    let reviewer = std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![CodeReviewSummary {
        aborted: false,
        verdict: Some("fail".into()),
        summary: "round 1".into(),
        findings: vec![Finding::new(
            "P1".into(),
            "logic".into(),
            "missing guard".into(),
        )],
        ..Default::default()
    }]));
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: Vec::new(),
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(review_rounds(&plan_id), 1);

    // 计划回到 pending 后重新 build —— 预算按次发放，计数与未清项一起清零。
    let path = plan_path_for_id(&plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Pending;
    write_plan(&path, &plan, 2000).unwrap();
    rt.refresh_active_plan_after_write(path.clone(), &plan);
    rt.build_plan(&plan_id, Some("sid-session-a".into()))
        .expect("二次 build 失败");

    assert_eq!(review_rounds(&plan_id), 0);
    assert!(open_finding_references(&plan_id).is_empty());
    cleanup_home(&home);
}

#[tokio::test]
async fn green_build_evidence_rejects_command_mismatch_for_completed_registered_task() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_green_evidence_command_mismatch_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        passing_code_review(),
    ])));
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let task = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .expect("spawn a registered verification task");
    registry
        .wait_for_finish(&task.task_id)
        .await
        .expect("wait for the verification task");
    assert!(matches!(
        registry
            .get_info(&task.task_id)
            .expect("registered task info")
            .status,
        crate::core::tools::primitive::BashTaskStatus::Finished { exit_code: 0 }
    ));

    let error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: "cargo test".into(),
                task_id: task.task_id.clone(),
            }],
        ),
    )
    .await
    .expect_err("a mismatched command must not reuse a successful background task");
    assert_bad_args(
        error,
        format!(
            "green_build_evidence.command 与后台任务 `{}` 的实际命令不一致；请原样提交任务启动命令",
            task.task_id
        ),
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn green_build_evidence_rejects_empty_duplicate_unknown_and_blank_inputs() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_green_evidence_bad_inputs_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let reviewer =
        std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![passing_code_review()]));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(reviewer.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let task = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .expect("spawn a registered verification task");
    registry
        .wait_for_finish(&task.task_id)
        .await
        .expect("wait for the verification task");

    let empty_error =
        update_plan::execute(&rt, complete_all_args(&plan_id, Some(true), Vec::new()))
            .await
            .expect_err("green_build_pass=true requires evidence");
    assert_bad_args(
        empty_error,
        "green_build_pass=true 必须同时传入至少一个 green_build_evidence.task_id",
    );

    let duplicate_error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![
                update_plan::GreenBuildEvidenceArg {
                    command: "true".into(),
                    task_id: task.task_id.clone(),
                },
                update_plan::GreenBuildEvidenceArg {
                    command: "true".into(),
                    task_id: task.task_id.clone(),
                },
            ],
        ),
    )
    .await
    .expect_err("the same verification task may only be submitted once");
    assert_bad_args(
        duplicate_error,
        format!("green_build_evidence.task_id 重复：{}", task.task_id),
    );

    let unknown_error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: "true".into(),
                task_id: "unknown-task".into(),
            }],
        ),
    )
    .await
    .expect_err("unregistered task ids must be rejected");
    assert_bad_args(
        unknown_error,
        "找不到后台 bash 任务 `unknown-task`；只能引用本会话 verify skill 实际启动的 task_id",
    );

    let blank_command_error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![update_plan::GreenBuildEvidenceArg {
                command: " \t ".into(),
                task_id: task.task_id,
            }],
        ),
    )
    .await
    .expect_err("blank commands must be rejected before task lookup");
    assert_bad_args(
        blank_command_error,
        "green_build_evidence.command 不能为空；请填写对应后台 bash 的实际命令",
    );
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "input retries must reuse the fresh review"
    );
    cleanup_home(&home);
}

fn declare_acceptance_commands(plan_id: &str, commands: &[&str]) {
    let path = plan_path_for_id(plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.acceptance_commands = commands.iter().map(|c| c.to_string()).collect();
    write_plan(&path, &plan, 2000).unwrap();
}

async fn finished_task(
    registry: &crate::core::tools::primitive::BashTaskRegistry,
    workspace: &std::path::Path,
    command: &str,
) -> update_plan::GreenBuildEvidenceArg {
    let task = registry
        .spawn(command.into(), Some(workspace.to_path_buf()))
        .await
        .expect("spawn a registered verification task");
    registry
        .wait_for_finish(&task.task_id)
        .await
        .expect("wait for the verification task");
    update_plan::GreenBuildEvidenceArg {
        command: command.into(),
        task_id: task.task_id,
    }
}

/// The declared list is the floor: extra commands are fine, but every declared entry needs its
/// own finished task. Whitespace-only differences still match; anything else does not.
#[tokio::test]
async fn acceptance_reconciles_evidence_against_declared_commands() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_acceptance_declared_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let events: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        passing_code_review(),
    ])));
    {
        let sink = std::sync::Arc::clone(&events);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    let plan_id = fresh_planning_plan(&rt);
    declare_acceptance_commands(&plan_id, &["true", "echo declared"]);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let ran_true = finished_task(&registry, workspace.path(), "true").await;
    let ran_extra = finished_task(&registry, workspace.path(), "echo extra").await;
    // A narrower variant of a declared command is a different command.
    let ran_narrow = finished_task(&registry, workspace.path(), "echo declared --only one").await;

    let error = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![ran_true.clone(), ran_extra.clone(), ran_narrow],
        ),
    )
    .await
    .expect_err("evidence that skips a declared command must not pass acceptance");
    assert_bad_args(
        error,
        "计划声明的 acceptance_commands 尚有 1 条没有对应的绿构建证据：`echo declared`。请原样运行每一条声明命令（可以额外运行更多，但不能用更窄的命令替代），再连同全部 task_id 一并提交",
    );
    let persisted = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(!persisted.frontmatter.green_build_pass);
    assert!(persisted.frontmatter.green_build_evidence.is_empty());
    assert!(
        !events
            .lock()
            .iter()
            .any(|event| event["event"] == "plan.green_build"),
        "a rejected reconciliation must not record a green build"
    );

    // Whitespace-only differences in the launched command still satisfy the declaration.
    let ran_declared = finished_task(&registry, workspace.path(), "echo   declared").await;
    let out = update_plan::execute(
        &rt,
        complete_all_args(
            &plan_id,
            Some(true),
            vec![ran_true, ran_extra, ran_declared],
        ),
    )
    .await
    .expect("every declared command plus extras passes acceptance");
    assert_eq!(out["plan_state_after"], "completed");
    let green = events
        .lock()
        .iter()
        .find(|event| event["event"] == "plan.green_build")
        .cloned()
        .expect("plan.green_build event");
    assert_eq!(
        green["declared_acceptance_commands"],
        serde_json::json!(["true", "echo declared"])
    );
    assert_eq!(green["evidence"].as_array().map(Vec::len), Some(3));
    assert!(
        !events
            .lock()
            .iter()
            .any(|event| event["event"] == "plan.acceptance.commands_undeclared"),
        "a declared plan must not be flagged as undeclared"
    );
    cleanup_home(&home);
}

/// Plans written before the declared list existed (or that chose not to declare one) still
/// complete on evidence alone; the runtime records that no floor was available to reconcile.
#[tokio::test]
async fn acceptance_without_declared_commands_passes_and_records_undeclared_event() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_acceptance_undeclared_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let events: std::sync::Arc<parking_lot::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    rt.attach_code_reviewer(std::sync::Arc::new(MockCodeReviewerDispatcher::new(vec![
        passing_code_review(),
    ])));
    {
        let sink = std::sync::Arc::clone(&events);
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            sink.lock().push(extra);
            Ok(())
        }));
    }
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);
    assert!(read_plan(&plan_path_for_id(&plan_id).unwrap())
        .unwrap()
        .frontmatter
        .acceptance_commands
        .is_empty());

    let ran_true = finished_task(&registry, workspace.path(), "true").await;
    let out = update_plan::execute(&rt, complete_all_args(&plan_id, Some(true), vec![ran_true]))
        .await
        .expect("undeclared plans keep completing on verified evidence");
    assert_eq!(out["plan_state_after"], "completed");

    let undeclared = events
        .lock()
        .iter()
        .find(|event| event["event"] == "plan.acceptance.commands_undeclared")
        .cloned()
        .expect("plan.acceptance.commands_undeclared event");
    assert_eq!(undeclared["plan_id"], plan_id);
    assert_eq!(undeclared["evidence_count"], 1);
    assert!(events
        .lock()
        .iter()
        .any(|event| event["event"] == "plan.green_build"));
    cleanup_home(&home);
}

#[tokio::test]
async fn disputes_reject_non_unresolved_p2_and_reasonless_p1_findings() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");

    let missing_reference_error = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F01".into(),
                area: "tests".into(),
                resolution: "wontfix".into(),
                reason: "the finding is not present".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect_err("only unresolved findings may be disputed");
    assert_bad_args(
        missing_reference_error,
        "F01（area=\"tests\"）不在未决清单；它可能已修、已申辩，或只是不会阻塞的 P2",
    );

    set_open_findings(
        &plan_id,
        vec![Finding::new(
            "P2".into(),
            "style".into(),
            "rename the internal helper".into(),
        )
        .with_reference("F02")],
    );
    let p2_error = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F02".into(),
                area: "style".into(),
                resolution: "wontfix".into(),
                reason: "not needed".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect_err("P2 findings do not need a completion dispute");
    assert_bad_args(p2_error, "F02 是 P2，不会阻塞收口，无需申辩");

    set_open_findings(
        &plan_id,
        vec![Finding::new(
            "P1".into(),
            "compatibility".into(),
            "old clients remain unsupported".into(),
        )
        .with_reference("F03")],
    );
    let missing_reason_error = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id),
            path: None,
            replace: false,
            dispute_findings: vec![update_plan::DisputeFindingArg {
                reference: "F03".into(),
                area: "compatibility".into(),
                resolution: "wontfix".into(),
                reason: " \n ".into(),
            }],
            green_build_pass: None,
            green_build_evidence: Vec::new(),
            acceptance_commands: None,
            ops: Vec::new(),
        },
    )
    .await
    .expect_err("P1 disputes must document their reason");
    assert_bad_args(missing_reason_error, "申辩 F03 必须提供 reason");
    cleanup_home(&home);
}

#[tokio::test]
async fn missing_reviewer_skips_review_but_still_requires_real_green_build_evidence() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let workspace = git_workspace_with_code("tomcat_missing_reviewer_green_build_");
    let registry = std::sync::Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
        workspace.path().join(".task-logs"),
    ));
    let rt = PlanRuntime::new("session-a");
    rt.attach_workspace_root(workspace.path().to_path_buf());
    rt.attach_bash_task_registry(registry.clone());
    let plan_id = fresh_planning_plan(&rt);
    mark_plan_executing(&rt, &plan_id, "session-a");
    rt.set_max_code_review_rounds(1);

    let review_skipped = update_plan::execute(&rt, complete_all_args(&plan_id, None, Vec::new()))
        .await
        .expect("skipped review should navigate to the green-build acceptance gate");
    assert_eq!(review_skipped["next_step"]["phase"], "run_acceptance");
    let after_skipped_review = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(after_skipped_review.frontmatter.code_review_pass);
    assert!(!after_skipped_review.frontmatter.green_build_pass);
    assert_eq!(
        after_skipped_review.frontmatter.state,
        PlanFileState::Executing
    );

    let task = registry
        .spawn("true".into(), Some(workspace.path().to_path_buf()))
        .await
        .expect("spawn a registered verification task");
    registry
        .wait_for_finish(&task.task_id)
        .await
        .expect("wait for the verification task");
    let out = update_plan::execute(
        &rt,
        submit_green_evidence_args(&plan_id, "true", &task.task_id),
    )
    .await
    .expect("real successful evidence should complete the plan");
    assert_eq!(out["plan_state_after"], "completed");
    let completed = read_plan(&plan_path_for_id(&plan_id).unwrap()).unwrap();
    assert!(completed.frontmatter.code_review_pass);
    assert!(completed.frontmatter.green_build_pass);
    cleanup_home(&home);
}
