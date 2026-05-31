use super::common::*;

#[test]
fn build_plan_writes_plan_build_transcript_event() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let events = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    {
        let events = events.clone();
        rt.attach_transcript_appender(std::sync::Arc::new(move |extra| {
            events.lock().push(extra);
            Ok(())
        }));
    }
    write_disk_plan("eventful", PlanFileState::Planning);
    rt.build_plan("eventful", Some("sid-a".into())).unwrap();

    let entries = events.lock();
    let event = entries
        .iter()
        .find(|v| v["event"] == crate::infra::wire::WIRE_PLAN_BUILD)
        .expect("缺少 plan.build 事件");
    assert_eq!(event["plan_id"], "eventful");
    assert_eq!(event["state"], "executing");
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_active_executing_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("blockee", PlanFileState::Planning);

    rt.set_executing_for_test("other_plan".into());
    let err = rt.build_plan("blockee", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildBlocked(s) => assert!(s.contains("已在 EXEC"), "{s}"),
        other => panic!("expected BuildBlocked, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_warns_but_continues_with_active_session_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("blockee", PlanFileState::Planning);
    rt.replace_session_todos(vec![TodoItem {
        id: "live".into(),
        content: "x".into(),
        status: TodoStatus::Pending,
    }]);
    let outcome = rt
        .build_plan("blockee", Some("sid-a".into()))
        .expect("build should continue with warning");
    assert_eq!(outcome.plan_id, "blockee");
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    assert!(outcome.warnings[0].contains("scratchpad todos"));
    match rt.mode() {
        PlanState::Executing { plan_id } => assert_eq!(plan_id, "blockee"),
        other => panic!("expected Executing, got {other:?}"),
    }
    let plan = read_plan(&plan_path_for_id("blockee").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Executing));
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_completed_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done", PlanFileState::Completed);
    let err = rt.build_plan("done", None).unwrap_err();
    assert!(matches!(err, PlanRuntimeError::BuildBlocked(_)));
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_disk_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("racy", PlanFileState::Executing);
    let err = rt.build_plan("racy", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildBlocked(s) => assert!(s.contains("executing"), "{s}"),
        other => panic!("expected BuildBlocked, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_nonexistent_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let err = rt.build_plan("missing_plan", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildPlanNotFound { plan_id, hint } => {
            assert_eq!(plan_id, "missing_plan");
            assert!(hint.contains("create_plan"));
        }
        other => panic!("expected BuildPlanNotFound, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_nonexistent_explicit_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let missing = home.join("external").join("missing.plan.md");
    let err = rt.build_plan(&missing.to_string_lossy(), None).unwrap_err();
    match err {
        PlanRuntimeError::BuildPlanPathNotFound { path, hint } => {
            assert!(path.ends_with("missing.plan.md"), "{path}");
            assert!(hint.contains("<plan_id/path>"), "{hint}");
        }
        other => panic!("expected BuildPlanPathNotFound, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_unsafe_plan_id() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    let err = rt.build_plan("BAD", None).unwrap_err();
    assert!(matches!(err, PlanRuntimeError::UnsafePlanId(_)));
    cleanup_home(&home);
}

#[tokio::test]
async fn plan_build_accepts_explicit_path_and_followup_update_plan_uses_same_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    rt.set_max_code_review_rounds(0);
    let external_dir = home.join("external-plans");
    std::fs::create_dir_all(&external_dir).unwrap();
    let external_path = external_dir.join("external.plan.md");
    write_plan_file_at(&external_path, "external_plan", PlanFileState::Planning);
    let expected_path =
        crate::infra::platform::normalize_path(&external_path.to_string_lossy()).unwrap();

    let outcome = rt
        .build_plan(
            &external_path.to_string_lossy(),
            Some("sid-external".into()),
        )
        .expect("path build should succeed");
    assert_eq!(outcome.plan_id, "external_plan");
    assert_eq!(outcome.plan_path, expected_path.clone());
    assert_eq!(rt.active_plan_path(), Some(expected_path.clone()));

    let out = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: None,
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "step1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .expect("update_plan should reuse active path");
    let expected_path_str = crate::infra::platform::format_home_path(&expected_path);
    assert_eq!(out["path"].as_str(), Some(expected_path_str.as_str()));

    let plan = read_plan(&external_path).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Completed));
    assert!(matches!(
        plan.frontmatter.todos[0].status,
        TodoStatus::Completed
    ));
    cleanup_home(&home);
}

#[test]
fn plan_build_enters_exec_and_binds_active_plan_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("new-session-key");
    rt.set_max_code_review_rounds(0);
    write_disk_plan("five_things", PlanFileState::Planning);
    let outcome = rt
        .build_plan("five_things", Some("new-session-uuid".into()))
        .expect("build 成功");

    match rt.mode() {
        PlanState::Executing { plan_id } => assert_eq!(plan_id, "five_things"),
        other => panic!("expected Executing, got {other:?}"),
    }
    assert!(rt.active_planning_plan_id().is_none());

    let plan = read_plan(&plan_path_for_id("five_things").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.state, PlanFileState::Executing));
    assert_eq!(
        plan.frontmatter.session_key.as_deref(),
        Some("new-session-key")
    );
    assert_eq!(
        plan.frontmatter.session_id.as_deref(),
        Some("new-session-uuid")
    );
    assert_eq!(
        outcome.plan_path,
        plan_path_for_id("five_things").expect("plan path should resolve")
    );
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Planning));
    assert!(outcome.warnings.is_empty());
    cleanup_home(&home);
}

#[test]
fn pending_plan_resumable_via_build() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("orig-session-key");
    write_disk_plan("resumable", PlanFileState::Pending);
    let outcome = rt.build_plan("resumable", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Pending));
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    match rt.mode() {
        PlanState::Executing { plan_id } => assert_eq!(plan_id, "resumable"),
        other => panic!("expected Executing, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn pending_plan_session_override_warns() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("brand-new-session");
    write_disk_plan("crossover", PlanFileState::Pending);
    let outcome = rt.build_plan("crossover", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Pending));
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    assert!(outcome.warnings[0].contains("orig-session-key"));
    assert!(outcome.warnings[0].contains("brand-new-session"));
    cleanup_home(&home);
}

#[test]
fn pending_session_can_build_another_explicit_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    write_disk_plan("old_pending", PlanFileState::Planning);
    rt.build_plan("old_pending", Some("sid-old".into()))
        .expect("old plan build should succeed");
    let demoted = rt
        .demote_to_pending_on_cancel()
        .expect("demote should succeed");
    assert_eq!(demoted.as_deref(), Some("old_pending"));

    let old_path = plan_path_for_id("old_pending").unwrap();
    let old_plan = read_plan(&old_path).unwrap();
    assert!(matches!(old_plan.frontmatter.state, PlanFileState::Pending));

    write_disk_plan("new_plan", PlanFileState::Planning);
    let outcome = rt
        .build_plan("new_plan", Some("sid-new".into()))
        .expect("pending session should build another explicit plan");

    assert_eq!(outcome.plan_id, "new_plan");
    assert!(matches!(outcome.prev_disk_state, PlanFileState::Planning));
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    match rt.mode() {
        PlanState::Executing { plan_id } => assert_eq!(plan_id, "new_plan"),
        other => panic!("expected Executing, got {other:?}"),
    }
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id("new_plan").unwrap())
    );

    let new_plan = read_plan(&plan_path_for_id("new_plan").unwrap()).unwrap();
    assert!(matches!(
        new_plan.frontmatter.state,
        PlanFileState::Executing
    ));
    let old_plan = read_plan(&old_path).unwrap();
    assert!(matches!(old_plan.frontmatter.state, PlanFileState::Pending));
    cleanup_home(&home);
}

#[test]
fn completed_session_can_build_another_explicit_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");

    write_disk_plan("old_done", PlanFileState::Planning);
    rt.build_plan("old_done", Some("sid-old".into()))
        .expect("old plan build should succeed");
    let old_path = plan_path_for_id("old_done").unwrap();
    let mut old_plan = read_plan(&old_path).unwrap();
    old_plan.frontmatter.state = PlanFileState::Completed;
    old_plan.frontmatter.todos.iter_mut().for_each(|todo| {
        todo.status = TodoStatus::Completed;
    });
    write_plan(&old_path, &old_plan, 1000).unwrap();
    rt.set_mode_completed("old_done".into());

    write_disk_plan("new_plan", PlanFileState::Planning);
    let outcome = rt
        .build_plan("new_plan", Some("sid-new".into()))
        .expect("completed session should build another explicit plan");

    assert_eq!(outcome.plan_id, "new_plan");
    match rt.mode() {
        PlanState::Executing { plan_id } => assert_eq!(plan_id, "new_plan"),
        other => panic!("expected Executing, got {other:?}"),
    }
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id("new_plan").unwrap())
    );
    let new_plan = read_plan(&plan_path_for_id("new_plan").unwrap()).unwrap();
    assert!(matches!(
        new_plan.frontmatter.state,
        PlanFileState::Executing
    ));
    let old_plan = read_plan(&old_path).unwrap();
    assert!(matches!(
        old_plan.frontmatter.state,
        PlanFileState::Completed
    ));
    cleanup_home(&home);
}

#[test]
fn default_build_target_prefers_planning_pending_then_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();

    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();
    let first = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "first".into(),
            draft: "draft-1".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "step".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let second = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "second".into(),
            draft: "draft-2".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "step".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    assert_eq!(
        rt.default_build_target().unwrap(),
        second["plan_id"].as_str().unwrap()
    );
    assert_ne!(first["plan_id"], second["plan_id"]);

    let rt = PlanRuntime::new("session-a");
    write_disk_plan("pending-default", PlanFileState::Pending);
    rt.set_mode_pending("pending-default".into());
    assert_eq!(rt.default_build_target().unwrap(), "pending-default");

    let rt = PlanRuntime::new("session-a");
    let external_dir = home.join("external-default");
    std::fs::create_dir_all(&external_dir).unwrap();
    let external_path = external_dir.join("external.plan.md");
    write_plan_file_at(&external_path, "external_default", PlanFileState::Planning);
    let normalized =
        crate::infra::platform::normalize_path(&external_path.to_string_lossy()).unwrap();
    rt.build_plan(&external_path.to_string_lossy(), Some("sid-path".into()))
        .unwrap();
    rt.set_mode_completed("external_default".into());
    assert_eq!(
        rt.default_build_target().unwrap(),
        crate::infra::platform::format_home_path(&normalized)
    );
    let _ = rt.finalize_completed_to_chat();
    assert_eq!(
        rt.default_build_target().unwrap(),
        crate::infra::platform::format_home_path(&normalized)
    );

    cleanup_home(&home);
}
