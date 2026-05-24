use super::super::cmd_plan::{parse_args, PlanCommand};
use super::super::parse::ChatCommand;

#[test]
fn parse_plan_without_args_enters_planning() {
    let cmd = parse_args(vec!["/plan".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Enter)));
}

#[test]
fn parse_plan_exit() {
    let cmd = parse_args(vec!["/plan".into(), "exit".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Exit)));
}

#[test]
fn parse_plan_build_with_id() {
    let cmd = parse_args(vec!["/plan".into(), "build".into(), "ship-001".into()]);
    assert!(matches!(
        cmd,
        ChatCommand::Plan(PlanCommand::Build { ref plan_target }) if plan_target == "ship-001"
    ));
}

#[test]
fn parse_plan_build_with_path() {
    let cmd = parse_args(vec![
        "/plan".into(),
        "build".into(),
        "/tmp/ship-001.plan.md".into(),
    ]);
    assert!(matches!(
        cmd,
        ChatCommand::Plan(PlanCommand::Build { ref plan_target })
            if plan_target == "/tmp/ship-001.plan.md"
    ));
}

#[test]
fn parse_plan_with_extra_arg_returns_usage_error() {
    let cmd = parse_args(vec!["/plan".into(), "ship".into()]);
    assert!(matches!(cmd, ChatCommand::UsageError { .. }));
}

#[test]
fn parse_plan_list() {
    let cmd = parse_args(vec!["/plan".into(), "list".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::List)));
}

#[test]
fn parse_plan_build_without_id_returns_usage_error() {
    let cmd = parse_args(vec!["/plan".into(), "build".into()]);
    assert!(matches!(cmd, ChatCommand::UsageError { .. }));
}
