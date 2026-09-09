use fs2::FileExt;

use super::common::*;
use crate::core::plan_runtime::ProgressSource;

#[test]
fn cancel_token_demotes_executing_to_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("cancellable", PlanFileState::Planning);
    rt.build_plan("cancellable", None).unwrap();
    assert_eq!(rt.mode(), AgentMode::Chat);

    let demoted = rt.park_executing_plan().unwrap();
    assert_eq!(demoted.as_deref(), Some("cancellable"));
    assert_eq!(rt.mode(), AgentMode::Chat);
    assert_eq!(rt.executing_plan_id(), None);
    assert_eq!(rt.active_plan().unwrap().id, "cancellable");

    let plan = read_plan(&plan_path_for_id("cancellable").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Pending));
    cleanup_home(&home);
}

#[test]
fn cancel_outside_exec_is_noop() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    assert!(rt.park_executing_plan().unwrap().is_none());

    rt.enter_plan().unwrap();
    assert!(rt.park_executing_plan().unwrap().is_none());
    assert_eq!(rt.mode(), AgentMode::Plan);
    cleanup_home(&home);
}

#[test]
fn attach_cancel_hook_rebinds_replaces_old_token() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let first = tokio_util::sync::CancellationToken::new();
    rt.attach_cancel_hook(first.clone());
    let cur = rt.current_cancel_token().expect("有 token");
    assert!(!cur.is_cancelled());

    let second = tokio_util::sync::CancellationToken::new();
    rt.attach_cancel_hook(second.clone());
    let cur2 = rt.current_cancel_token().expect("有 token");
    first.cancel();
    assert!(!cur2.is_cancelled(), "上一轮 cancel 不应影响新 token");
    second.cancel();
    let cur3 = rt.current_cancel_token().expect("有 token");
    assert!(cur3.is_cancelled());
    cleanup_home(&home);
}

#[test]
fn concurrent_write_plan_serialized_by_lock() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let path = plan_path_for_id("hot_plan").unwrap();
    let base = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "hot_plan".into(),
            goal: "concurrent".into(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: "2026-05-19T00:00:00Z".into(),
            schema_version: 1,
            todos: vec![],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            unknown: Default::default(),
        },
        body: "## seed\n".into(),
    };
    write_plan(&path, &base, 2000).unwrap();

    let p1 = path.clone();
    let p2 = path.clone();
    let h1 = std::thread::spawn(move || {
        for i in 0..5 {
            let mut plan = read_plan(&p1).unwrap();
            plan.frontmatter.todos = vec![TodoItem {
                id: format!("t{i}-a"),
                content: format!("a-{i}"),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            }];
            write_plan(&p1, &plan, 2000).unwrap();
        }
    });
    let h2 = std::thread::spawn(move || {
        for i in 0..5 {
            let mut plan = read_plan(&p2).unwrap();
            plan.frontmatter.todos = vec![TodoItem {
                id: format!("t{i}-b"),
                content: format!("b-{i}"),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            }];
            write_plan(&p2, &plan, 2000).unwrap();
        }
    });
    h1.join().unwrap();
    h2.join().unwrap();
    let final_plan = read_plan(&path).expect("最终态可解析");
    validate_frontmatter_invariants(&final_plan.frontmatter).expect("最终态合法");
    cleanup_home(&home);
}

#[test]
fn cancel_token_releases_plan_lock() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::with_lock_timeout("session-a", 200);
    write_disk_plan("lockable", PlanFileState::Planning);
    rt.build_plan("lockable", None).unwrap();
    rt.park_executing_plan().unwrap();

    let rt2 = PlanRuntime::with_lock_timeout("session-b", 200);
    let outcome = rt2
        .build_plan("lockable", None)
        .expect("demote 后 lock 应已释放，再 build 应成功");
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Pending));
    cleanup_home(&home);
}

#[test]
fn completed_plan_keeps_chat_session_mode_and_active_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done_path", PlanFileState::Planning);
    rt.build_plan("done_path", None).unwrap();
    let path = plan_path_for_id("done_path").unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Completed;
    write_plan(&path, &plan, 2000).unwrap();
    rt.refresh_active_plan_after_write(path, &plan);
    assert_eq!(rt.mode(), AgentMode::Chat);
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id("done_path").unwrap())
    );
    cleanup_home(&home);
}

#[test]
fn plan_mode_raw_edit_blocked_for_plan_files_in_planning_and_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("guarded", PlanFileState::Planning);
    let plan_path = plan_path_for_id("guarded").unwrap();

    assert_eq!(rt.mode(), AgentMode::Chat);
    assert!(rt.allow_raw_edit_to_path(&plan_path));

    rt.enter_plan().unwrap();
    assert!(!rt.allow_raw_edit_to_path(&plan_path));

    rt.exit_plan().unwrap();
    rt.build_plan("guarded", None).unwrap();
    assert!(!rt.allow_raw_edit_to_path(&plan_path));

    let other = home.join(".tomcat").join("notes.md");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "ok").unwrap();
    assert!(rt.allow_raw_edit_to_path(&other));
    cleanup_home(&home);
}

#[test]
fn plan_build_atomic_rollback_on_write_failure() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let _rt = PlanRuntime::new("session-a");
    write_disk_plan("rollback", PlanFileState::Planning);

    let plan_path = plan_path_for_id("rollback").unwrap();
    let lock_path = plan_path.with_file_name(format!(
        "{}.lock",
        plan_path.file_name().unwrap().to_string_lossy()
    ));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    f.try_lock_exclusive().unwrap();

    let rt = PlanRuntime::with_lock_timeout("session-a", 50);
    let err = rt.build_plan("rollback", None).unwrap_err();
    assert_eq!(rt.mode(), AgentMode::Chat);
    match err {
        PlanRuntimeError::Io(s) => {
            assert!(s.contains("锁") || s.contains("lock") || s.contains("LockBusy"));
        }
        other => panic!("expected Io (LockBusy), got {other:?}"),
    }

    FileExt::unlock(&f).unwrap();
    drop(f);
    let rt = PlanRuntime::with_lock_timeout("session-a", 1000);
    let _ok = rt
        .build_plan("rollback", None)
        .expect("放锁后 build 应成功");
    assert_eq!(rt.mode(), AgentMode::Chat);
    cleanup_home(&home);
}

#[test]
fn attach_exec_with_missing_plan_file_falls_back_to_chat() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new_with_session_id("session-a", "run-new");
    rt.attach_from_resume_state(ResumeControlState {
        mode: Some(AgentMode::Chat),
        plan_path: Some(plan_path_for_id("orphan-plan").unwrap()),
        plan_id: Some("orphan-plan".into()),
    })
    .unwrap();

    assert_eq!(rt.mode(), AgentMode::Chat);
    cleanup_home(&home);
}

#[test]
fn attach_plan_mode_survives_without_plan_file() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    // 进 PLAN 时计划文件往往还不存在，这正是旧实现会静默掉回 CHAT 的场景。
    rt.attach_from_resume_state(ResumeControlState {
        mode: Some(AgentMode::Plan),
        plan_path: None,
        plan_id: None,
    })
    .unwrap();

    assert_eq!(rt.mode(), AgentMode::Plan);
    cleanup_home(&home);
}

#[test]
fn attach_plan_mode_without_path_does_not_invent_an_active_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_from_resume_state(ResumeControlState {
        mode: Some(AgentMode::Plan),
        plan_path: None,
        plan_id: Some("draft-plan".into()),
    })
    .unwrap();

    assert_eq!(rt.mode(), AgentMode::Plan);
    assert!(rt.active_plan().is_none());
    cleanup_home(&home);
}

#[test]
fn restart_after_plan_build_restores_chat_mode_and_executing_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let plan_id = "owned-plan";
    write_disk_plan(plan_id, PlanFileState::Executing);
    let path = plan_path_for_id(plan_id).unwrap();
    let mut p = read_plan(&path).unwrap();
    p.frontmatter.session_key = Some("session-a".into());
    p.frontmatter.session_id = Some("run-a".into());
    write_plan(&path, &p, 2000).unwrap();

    let rt = PlanRuntime::new_with_session_id("session-a", "run-a");
    rt.attach_from_resume_state(ResumeControlState {
        mode: Some(AgentMode::Chat),
        plan_path: Some(path.clone()),
        plan_id: Some(plan_id.into()),
    })
    .unwrap();

    assert_eq!(rt.mode(), AgentMode::Chat);
    assert_eq!(rt.executing_plan_id().as_deref(), Some(plan_id));
    assert_eq!(rt.active_plan_path(), Some(path));
    cleanup_home(&home);
}

#[test]
fn restart_after_plan_completion_restores_chat_mode() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let plan_id = "completed-plan";
    write_disk_plan(plan_id, PlanFileState::Completed);
    let path = plan_path_for_id(plan_id).unwrap();

    // 旧 sidecar 缺少 agent_mode 时按新契约降级为 Chat；计划文件仍恢复为 active plan。
    let rt = PlanRuntime::new("session-a");
    rt.attach_from_resume_state(ResumeControlState {
        mode: None,
        plan_path: Some(path.clone()),
        plan_id: Some(plan_id.into()),
    })
    .unwrap();

    assert_eq!(rt.mode(), AgentMode::Chat);
    assert_eq!(rt.active_plan().unwrap().id, plan_id);
    assert_eq!(rt.active_plan_path(), Some(path));
    cleanup_home(&home);
}

#[test]
fn attach_without_any_information_falls_back_to_chat() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.attach_from_resume_state(ResumeControlState::default())
        .unwrap();

    assert_eq!(rt.mode(), AgentMode::Chat);
    assert!(rt.active_plan_path().is_none());
    cleanup_home(&home);
}

#[test]
fn control_snapshot_reports_three_valued_mode_and_file_state() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let plan_id = "snapshot-plan";
    write_disk_plan(plan_id, PlanFileState::Executing);
    let path = plan_path_for_id(plan_id).unwrap();

    let rt = PlanRuntime::new("session-a");
    rt.attach_from_resume_state(ResumeControlState {
        mode: Some(AgentMode::Chat),
        plan_path: Some(path.clone()),
        plan_id: Some(plan_id.into()),
    })
    .unwrap();

    let snap = rt.control_snapshot(Some("gpt-5.6-sol"));
    assert_eq!(snap.mode, AgentMode::Chat);
    assert_eq!(snap.plan_file_state.as_deref(), Some("executing"));
    assert_eq!(snap.plan_path, Some(path));
    assert_eq!(snap.plan_id.as_deref(), Some(plan_id));
    assert_eq!(snap.model.as_deref(), Some("gpt-5.6-sol"));
    assert!(matches!(
        snap.progress,
        Some(ProgressSource::PlanFile { ref todos })
            if todos.len() == 3
                && todos[0].id == "step1"
                && todos[1].id == GATE_CODE_REVIEW_TODO_ID
                && todos[2].id == GATE_ACCEPTANCE_TODO_ID
    ));
    cleanup_home(&home);
}

#[test]
fn control_snapshot_chooses_plan_todos_then_scratchpad_then_none() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();

    let with_plan = PlanRuntime::new("session-a");
    write_disk_plan("progress-plan", PlanFileState::Planning);
    let plan_path = plan_path_for_id("progress-plan").unwrap();
    let mut empty_plan = read_plan(&plan_path).unwrap();
    empty_plan.frontmatter.todos.clear();
    write_plan(&plan_path, &empty_plan, 2000).unwrap();
    with_plan.bind_plan_file_for_test(plan_path);
    with_plan.replace_session_todos(vec![TodoItem {
        id: "scratch".into(),
        content: "must not leak over a readable empty plan".into(),
        status: TodoStatus::Pending,
        evidence: Vec::new(),
        kind: Default::default(),
    }]);
    assert!(matches!(
        with_plan.control_snapshot(None).progress,
        Some(ProgressSource::PlanFile { ref todos }) if todos.is_empty()
    ));

    let scratchpad_only = PlanRuntime::new("session-b");
    scratchpad_only.seed_active_plan_for_test("missing-plan".into(), PlanFileState::Planning);
    scratchpad_only.replace_session_todos(vec![TodoItem {
        id: "scratch".into(),
        content: "keep investigating".into(),
        status: TodoStatus::InProgress,
        evidence: Vec::new(),
        kind: Default::default(),
    }]);
    assert!(matches!(
        scratchpad_only.control_snapshot(None).progress,
        Some(ProgressSource::SessionScratchpad { ref todos })
            if todos.len() == 1 && todos[0].id == "scratch"
    ));

    let none = PlanRuntime::new("session-c");
    assert!(none.control_snapshot(None).progress.is_none());
    cleanup_home(&home);
}

#[test]
fn e7_reload_active_plan_from_disk_picks_up_session_owned_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let plan_id = "reload-plan";
    write_disk_plan(plan_id, PlanFileState::Executing);
    let path = plan_path_for_id(plan_id).unwrap();
    let mut p = read_plan(&path).unwrap();
    p.frontmatter.session_key = Some("session-a".into());
    p.frontmatter.session_id = Some("run-a".into());
    write_plan(&path, &p, 2000).unwrap();

    let rt = PlanRuntime::new_with_session_id("session-a", "run-a");
    assert_eq!(rt.mode(), AgentMode::Chat);

    let restored = rt.sync_active_plan_from_disk().unwrap();
    assert_eq!(restored.as_deref(), Some(plan_id));
    assert_eq!(rt.mode(), AgentMode::Chat);
    assert_eq!(rt.executing_plan_id().as_deref(), Some(plan_id));
    cleanup_home(&home);
}
