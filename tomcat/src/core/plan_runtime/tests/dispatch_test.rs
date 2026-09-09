use super::super::file_store::{
    write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
};
use super::super::{safety, PlanRuntime};
use crate::core::session::AgentMode;

#[test]
fn safety_assert_plan_id_safe_accepts_normal_id() {
    safety::assert_plan_id_safe("ship-plan-mode_001").unwrap();
}

#[test]
fn safety_assert_plan_id_safe_rejects_traversal_paths() {
    let bad = [
        "",
        "..",
        "../etc",
        "a/b",
        "a\\b",
        "a b",
        "A",
        "ship!",
        "ship\nbad",
    ];
    for id in bad {
        let r = safety::assert_plan_id_safe(id);
        assert!(
            r.is_err(),
            "should reject unsafe plan_id {id:?}, got: {r:?}"
        );
    }
}

#[test]
fn resolved_plan_path_prefers_active_external_path() {
    let workspace = tempfile::tempdir().unwrap();
    let external_dir = workspace.path().join("external");
    std::fs::create_dir_all(&external_dir).unwrap();
    let external_path = external_dir.join("custom.plan.md");
    write_plan(
        &external_path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: "external_path_plan".into(),
                goal: "goal".into(),
                state: PlanFileState::Planning,
                session_key: Some("sess".into()),
                session_id: Some("uuid".into()),
                created_at: "2026-05-24T00:00:00Z".into(),
                schema_version: 1,
                todos: vec![TodoItem {
                    id: "t1".into(),
                    content: "ship".into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: Default::default(),
                }],
                green_build_pass: false,
                green_build_evidence: Vec::new(),
                code_review_pass: false,
                code_review_pass_at_ms: None,
                code_review_residual_findings: Vec::new(),
                completion_gate_cycles: 0,
                unknown: Default::default(),
            },
            body: "## Goal\nexternal\n".into(),
        },
        1000,
    )
    .unwrap();

    let runtime = PlanRuntime::new("sess");
    runtime
        .build_plan(&external_path.to_string_lossy(), Some("uuid-path".into()))
        .unwrap();

    assert_eq!(
        runtime.resolved_plan_path("external_path_plan").unwrap(),
        crate::normalize_path(&external_path.to_string_lossy()).unwrap()
    );
}

#[test]
fn concurrent_exit_and_build_leave_one_chat_mode_transition() {
    let workspace = tempfile::tempdir().unwrap();
    let plan_path = workspace.path().join("concurrent.plan.md");
    write_plan(
        &plan_path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: "concurrent-plan".into(),
                goal: "prove concurrent mode transitions are serialized".into(),
                state: PlanFileState::Planning,
                session_key: None,
                session_id: None,
                created_at: "2026-05-24T00:00:00Z".into(),
                schema_version: 1,
                todos: vec![TodoItem {
                    id: "t1".into(),
                    content: "ship".into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: Default::default(),
                }],
                green_build_pass: false,
                green_build_evidence: Vec::new(),
                code_review_pass: false,
                code_review_pass_at_ms: None,
                code_review_residual_findings: Vec::new(),
                completion_gate_cycles: 0,
                unknown: Default::default(),
            },
            body: "## Goal\nconcurrent transitions\n".into(),
        },
        1000,
    )
    .unwrap();

    let runtime = PlanRuntime::new("session");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        runtime.attach_transcript_appender(std::sync::Arc::new(move |event| {
            events.lock().push(event);
            Ok(())
        }));
    }
    runtime.enter_plan().unwrap();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let exit = {
        let barrier = barrier.clone();
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            barrier.wait();
            runtime.exit_plan()
        })
    };
    let build = {
        let barrier = barrier.clone();
        let runtime = runtime.clone();
        let plan_path = plan_path.to_string_lossy().to_string();
        std::thread::spawn(move || {
            barrier.wait();
            runtime.build_plan(&plan_path, Some("session-id".into()))
        })
    };

    assert!(
        matches!(
            exit.join().unwrap(),
            Ok(()) | Err(super::super::PlanRuntimeError::AlreadyInMode(_))
        ),
        "exit either performs the only Chat transition or observes build already did"
    );
    build.join().unwrap().expect("build must promote the plan");

    assert_eq!(runtime.mode(), AgentMode::Chat);
    assert_eq!(
        runtime.executing_plan_id().as_deref(),
        Some("concurrent-plan")
    );
    let mode_events = events
        .lock()
        .iter()
        .filter(|event| {
            event["event"]
                .as_str()
                .is_some_and(|name| name == crate::infra::wire::WIRE_SESSION_AGENT_MODE_CHANGED)
        })
        .count();
    assert_eq!(
        mode_events, 2,
        "enter produces Plan and exactly one of exit/build produces Chat"
    );
}
