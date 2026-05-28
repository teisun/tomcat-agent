use fs2::FileExt;

use super::common::*;

#[test]
fn cancel_token_demotes_executing_to_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("cancellable", PlanFileState::Planning);
    rt.build_plan("cancellable", None).unwrap();
    assert!(matches!(rt.mode(), PlanState::Executing { .. }));

    let demoted = rt.demote_to_pending_on_cancel().unwrap();
    assert_eq!(demoted.as_deref(), Some("cancellable"));
    match rt.mode() {
        PlanState::Pending { plan_id } => assert_eq!(plan_id, "cancellable"),
        other => panic!("expected Pending, got {other:?}"),
    }

    let plan = read_plan(&plan_path_for_id("cancellable").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Pending));
    cleanup_home(&home);
}

#[test]
fn cancel_outside_exec_is_noop() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    assert!(rt.demote_to_pending_on_cancel().unwrap().is_none());

    rt.enter_planning().unwrap();
    assert!(rt.demote_to_pending_on_cancel().unwrap().is_none());
    assert!(matches!(rt.mode(), PlanState::Planning));
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
    rt.demote_to_pending_on_cancel().unwrap();

    let rt2 = PlanRuntime::with_lock_timeout("session-b", 200);
    let outcome = rt2
        .build_plan("lockable", None)
        .expect("demote 后 lock 应已释放，再 build 应成功");
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Pending));
    cleanup_home(&home);
}

#[test]
fn finalize_completed_to_chat_keeps_retain_fields() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done_path", PlanFileState::Planning);
    rt.build_plan("done_path", None).unwrap();
    rt.set_mode_completed("done_path".into());
    let pid = rt.finalize_completed_to_chat().expect("Some(plan_id)");
    assert_eq!(pid, "done_path");
    assert!(matches!(rt.mode(), PlanState::Chat));
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id("done_path").unwrap())
    );
    assert!(rt.finalize_completed_to_chat().is_none());
    cleanup_home(&home);
}

#[test]
fn plan_mode_raw_edit_blocked_for_plan_files_in_planning_and_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("guarded", PlanFileState::Planning);
    let plan_path = plan_path_for_id("guarded").unwrap();

    assert!(matches!(rt.mode(), PlanState::Chat));
    assert!(rt.allow_raw_edit_to_path(&plan_path));

    rt.enter_planning().unwrap();
    assert!(!rt.allow_raw_edit_to_path(&plan_path));

    rt.exit_to_chat().unwrap();
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
    assert!(matches!(rt.mode(), PlanState::Chat));
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
    assert!(matches!(rt.mode(), PlanState::Executing { .. }));
    cleanup_home(&home);
}

#[test]
fn attach_from_event_missing_path_falls_back_to_chat() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new_with_session_id("session-a", "run-new");
    rt.attach_from_event(Some(PlanEventRef {
        kind: PlanEventKind::Build,
        plan_id: "orphan-plan".into(),
        path: plan_path_for_id("orphan-plan").unwrap(),
    }))
    .unwrap();

    assert!(matches!(rt.mode(), PlanState::Chat));
    cleanup_home(&home);
}

#[test]
fn attach_from_event_create_restores_active_planning_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let path = plan_path_for_id("draft-plan").unwrap();
    rt.attach_from_event(Some(PlanEventRef {
        kind: PlanEventKind::Create,
        plan_id: "draft-plan".into(),
        path,
    }))
    .unwrap();

    assert!(matches!(rt.mode(), PlanState::Chat));
    assert_eq!(rt.active_planning_plan_id().as_deref(), Some("draft-plan"));
    assert!(rt.active_plan_path().is_none());
    cleanup_home(&home);
}

#[test]
fn attach_from_event_restores_executing_from_latest_plan_event() {
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
    rt.attach_from_event(Some(PlanEventRef {
        kind: PlanEventKind::Build,
        plan_id: plan_id.into(),
        path: path.clone(),
    }))
    .unwrap();

    match rt.mode() {
        PlanState::Executing { plan_id: ref pid } => assert_eq!(pid, plan_id),
        other => panic!("expected Executing, got {other:?}"),
    }
    assert_eq!(rt.active_plan_path(), Some(path));
    cleanup_home(&home);
}

#[test]
fn attach_from_event_completed_disk_state_rehydrates_chat_with_retain() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let plan_id = "completed-plan";
    write_disk_plan(plan_id, PlanFileState::Completed);
    let path = plan_path_for_id(plan_id).unwrap();

    let rt = PlanRuntime::new("session-a");
    rt.attach_from_event(Some(PlanEventRef {
        kind: PlanEventKind::Update,
        plan_id: plan_id.into(),
        path: path.clone(),
    }))
    .unwrap();

    assert!(matches!(rt.mode(), PlanState::Chat));
    assert_eq!(rt.active_plan_path(), Some(path));
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
    assert!(matches!(rt.mode(), PlanState::Chat));

    let restored = rt.reload_active_plan_from_disk().unwrap();
    assert_eq!(restored.as_deref(), Some(plan_id));
    assert!(matches!(rt.mode(), PlanState::Executing { .. }));
    cleanup_home(&home);
}
