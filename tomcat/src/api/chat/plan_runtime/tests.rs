//! `PlanRuntime` 单元测试（plan §9.3 A）。

use super::*;

#[test]
fn enter_planning_from_chat_transitions_to_planning() {
    let rt = PlanRuntime::new("sess-1");
    assert!(matches!(rt.mode(), PlanMode::Chat));
    rt.enter_planning().unwrap();
    assert!(matches!(rt.mode(), PlanMode::Planning));
}

#[test]
fn enter_planning_from_completed_transitions_to_planning() {
    let rt = PlanRuntime::new("sess-1");
    rt.set_mode_completed("done-1".into());
    rt.enter_planning().unwrap();
    assert!(matches!(rt.mode(), PlanMode::Planning));
}

#[test]
fn enter_planning_twice_is_rejected() {
    let rt = PlanRuntime::new("sess-1");
    rt.enter_planning().unwrap();
    let err = rt.enter_planning().unwrap_err();
    assert!(matches!(err, PlanRuntimeError::AlreadyInMode(_)));
    assert!(matches!(rt.mode(), PlanMode::Planning));
}

#[test]
fn exit_to_chat_from_planning_resets() {
    let rt = PlanRuntime::new("sess-1");
    rt.enter_planning().unwrap();
    rt.exit_to_chat().unwrap();
    assert!(matches!(rt.mode(), PlanMode::Chat));
}

#[test]
fn exit_to_chat_when_already_chat_errors() {
    let rt = PlanRuntime::new("sess-1");
    let err = rt.exit_to_chat().unwrap_err();
    assert!(matches!(err, PlanRuntimeError::AlreadyInMode(_)));
}

#[test]
fn recover_in_initial_chat_is_noop() {
    let rt = PlanRuntime::new("sess-1");
    rt.recover().unwrap();
    assert!(matches!(rt.mode(), PlanMode::Chat));
}

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
        "A",         // uppercase forbidden
        "ship!",     // non-allowed punct
        "ship\nbad", // control char
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
fn prompts_render_executor_reminder_substitutes_plan_id() {
    let s = prompts::render_executor_reminder("ship-001");
    assert!(s.contains("ship-001"));
    assert!(!s.contains("{plan_id}"));
}

#[test]
fn session_prefix_for_chat_is_empty() {
    assert!(session_prefix::user_prefix_for_mode(&PlanMode::Chat, None).is_empty());
}

#[test]
fn session_prefix_for_planning_carries_plan_path_when_present() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Planning,
        Some(std::path::Path::new("/tmp/active.plan.md")),
    );
    assert!(p.starts_with("[mode: PLAN "));
    assert!(p.contains("plan_path=/tmp/active.plan.md"));
}

#[test]
fn session_prefix_for_executing_carries_plan_id() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Executing {
            plan_id: "ship-001".into(),
        },
        Some(std::path::Path::new("/tmp/exec.plan.md")),
    );
    assert!(p.contains("[mode: EXEC plan_id=ship-001"));
    assert!(p.contains("plan_path=/tmp/exec.plan.md"));
}

#[test]
fn session_prefix_for_pending_is_empty() {
    let p = session_prefix::user_prefix_for_mode(
        &PlanMode::Pending {
            plan_id: "ship-001".into(),
        },
        None,
    );
    assert!(p.is_empty(), "pending must NOT prefix LLM input");
}

#[test]
fn strip_user_prefix_removes_plan_label_when_present() {
    let s = session_prefix::strip_user_prefix("[mode: PLAN]\nhello world");
    assert_eq!(s, "hello world");
}

#[test]
fn strip_user_prefix_removes_exec_label_with_plan_id() {
    let s = session_prefix::strip_user_prefix("[mode: EXEC plan_id=ship-001]\nstart please");
    assert_eq!(s, "start please");
}

#[test]
fn strip_user_prefix_passthrough_when_no_label() {
    let s = session_prefix::strip_user_prefix("normal user text\nwith linebreak");
    assert_eq!(s, "normal user text\nwith linebreak");
}

#[test]
fn strip_user_prefix_passthrough_when_lookalike_does_not_close() {
    let s = session_prefix::strip_user_prefix("[mode: malformed\nrest of text");
    assert_eq!(s, "[mode: malformed\nrest of text");
}

#[test]
fn plan_enter_injects_planner_reminder_into_system() {
    // chat_loop 的 system 注入逻辑（见 chat/mod.rs）等价于 `format!("{}{}", system_text, PLANNER_REMINDER)`。
    // 本测试锁住 PLANNER_REMINDER 的关键结构（system_reminder 标签 + 关键关键词），并验证拼接副作用。
    let reminder: &str = &prompts::PLANNER_REMINDER;
    assert!(
        reminder.contains("<system_reminder") && reminder.contains("</system_reminder>"),
        "PLANNER_REMINDER 必须使用 <system_reminder ...> ... </system_reminder> 包裹，实际：\n{reminder}"
    );
    assert!(
        reminder.to_lowercase().contains("plan"),
        "PLANNER_REMINDER 必须显式提示当前在 PLAN/规划 模式，实际：\n{reminder}"
    );

    let composed = format!("BASE_SYSTEM_PROMPT\n{reminder}");
    assert!(composed.starts_with("BASE_SYSTEM_PROMPT"));
    assert!(composed.contains("<system_reminder"));
}

#[test]
fn executor_reminder_format_uses_system_reminder_tags() {
    let plan_id = "demo-plan-1";
    let s = prompts::render_executor_reminder(plan_id);
    assert!(
        s.contains("<system_reminder") && s.contains("</system_reminder>"),
        "EXECUTOR reminder 必须使用 <system_reminder ...> ... </system_reminder> 包裹，实际：\n{s}"
    );
    assert!(s.contains(plan_id), "EXECUTOR reminder 必须包含 plan_id");
}

#[test]
fn mode_active_plan_id_returns_some_only_for_attached_states() {
    assert_eq!(PlanMode::Chat.active_plan_id(), None);
    assert_eq!(PlanMode::Planning.active_plan_id(), None);
    assert_eq!(
        PlanMode::Executing {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
    assert_eq!(
        PlanMode::Pending {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
    assert_eq!(
        PlanMode::Completed {
            plan_id: "x".into()
        }
        .active_plan_id(),
        Some("x")
    );
}
