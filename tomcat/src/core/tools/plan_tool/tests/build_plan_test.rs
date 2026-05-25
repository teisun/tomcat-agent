use super::common::*;

#[test]
fn plan_build_requires_no_active_plan_or_todos() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("blockee", PlanFileMode::Planning);

    rt.set_executing_for_test("other_plan".into());
    let err = rt.build_plan("blockee", None).unwrap_err();
    matches!(err, PlanRuntimeError::BuildBlocked(_));

    let rt = PlanRuntime::new("session-a");
    rt.replace_session_todos(vec![TodoItem {
        id: "live".into(),
        content: "x".into(),
        status: TodoStatus::Pending,
    }]);
    let err = rt.build_plan("blockee", None).unwrap_err();
    match err {
        PlanRuntimeError::BuildBlocked(s) => assert!(s.contains("未完成 todos"), "{s}"),
        other => panic!("expected BuildBlocked, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_completed_plan() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("done", PlanFileMode::Completed);
    let err = rt.build_plan("done", None).unwrap_err();
    matches!(err, PlanRuntimeError::BuildBlocked(_));
    cleanup_home(&home);
}

#[test]
fn plan_build_rejects_disk_executing() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("session-a");
    write_disk_plan("racy", PlanFileMode::Executing);
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
    write_plan_file_at(&external_path, "external_plan", PlanFileMode::Planning);
    let expected_path =
        crate::infra::platform::normalize_path(&external_path.to_string_lossy()).unwrap();

    let outcome = rt
        .build_plan(&external_path.to_string_lossy(), Some("sid-external".into()))
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
    assert!(matches!(plan.frontmatter.mode, PlanFileMode::Completed));
    assert!(matches!(plan.frontmatter.todos[0].status, TodoStatus::Completed));
    cleanup_home(&home);
}

#[test]
fn plan_build_enters_exec_and_binds_active_plan_path() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("new-session-key");
    rt.set_max_code_review_rounds(0);
    write_disk_plan("five_things", PlanFileMode::Planning);
    let outcome = rt
        .build_plan("five_things", Some("new-session-uuid".into()))
        .expect("build 成功");

    match rt.mode() {
        PlanMode::Executing { plan_id } => assert_eq!(plan_id, "five_things"),
        other => panic!("expected Executing, got {other:?}"),
    }
    assert!(rt.active_planning_plan_id().is_none());

    let plan = read_plan(&plan_path_for_id("five_things").unwrap()).unwrap();
    assert!(matches!(plan.frontmatter.mode, PlanFileMode::Executing));
    assert_eq!(plan.frontmatter.session_key.as_deref(), Some("new-session-key"));
    assert_eq!(
        plan.frontmatter.session_id.as_deref(),
        Some("new-session-uuid")
    );
    assert_eq!(
        outcome.plan_path,
        plan_path_for_id("five_things").expect("plan path should resolve")
    );
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Planning));
    assert!(outcome.warnings.is_empty());
    cleanup_home(&home);
}

#[test]
fn pending_plan_resumable_via_build() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("orig-session-key");
    write_disk_plan("resumable", PlanFileMode::Pending);
    let outcome = rt.build_plan("resumable", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Pending));
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    match rt.mode() {
        PlanMode::Executing { plan_id } => assert_eq!(plan_id, "resumable"),
        other => panic!("expected Executing, got {other:?}"),
    }
    cleanup_home(&home);
}

#[test]
fn pending_plan_session_override_warns() {
    let _g = home_lock().lock().unwrap();
    let home = setup_isolated_home();
    let rt = PlanRuntime::new("brand-new-session");
    write_disk_plan("crossover", PlanFileMode::Pending);
    let outcome = rt.build_plan("crossover", None).expect("续跑 build 成功");
    assert!(matches!(outcome.prev_disk_mode, PlanFileMode::Pending));
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    assert!(outcome.warnings[0].contains("orig-session-key"));
    assert!(outcome.warnings[0].contains("brand-new-session"));
    cleanup_home(&home);
}

