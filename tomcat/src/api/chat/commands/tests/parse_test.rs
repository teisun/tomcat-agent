//! Tests for `commands::parse` — slash-command recognition contract.

use super::super::{parse_chat_command, ChatCommand, PlanCommand, SkillCommand};

fn assert_not_command(input: &str) {
    assert!(matches!(
        parse_chat_command(input),
        ChatCommand::NotACommand(_)
    ));
}

#[test]
fn unknown_slash_commands_are_chat() {
    assert_not_command("/foo /a");
    assert_not_command("/abs/path");
}

#[test]
fn normal_text_with_path_is_chat() {
    assert_not_command("帮我看下 /a");
}

#[test]
fn ckpt_commands_parse() {
    assert_eq!(
        parse_chat_command("/ckpt list"),
        ChatCommand::CkptList { limit: None }
    );
    assert_eq!(
        parse_chat_command("/ckpt list --limit 5"),
        ChatCommand::CkptList { limit: Some(5) }
    );
    assert_eq!(
        parse_chat_command("/ckpt show ck_1"),
        ChatCommand::CkptShow {
            checkpoint_id: "ck_1".to_string()
        }
    );
    assert_eq!(
        parse_chat_command("/ckpt diff ck_2"),
        ChatCommand::CkptDiff {
            checkpoint_id: "ck_2".to_string()
        }
    );
}

#[test]
fn restore_command_parses_paths_and_dry_run() {
    assert_eq!(
        parse_chat_command("/restore ck_1 --path src/a.rs --path Cargo.toml --dry-run"),
        ChatCommand::Restore {
            checkpoint_id: "ck_1".to_string(),
            paths: vec!["src/a.rs".into(), "Cargo.toml".into()],
            dry_run: true,
        }
    );
}

// ─── T2-P1-002 PR-PLA：`/plan` 三件套解析回归 ───────────────────────────────

#[test]
fn plan_command_without_args_enters_planning() {
    assert_eq!(
        parse_chat_command("/plan"),
        ChatCommand::Plan(PlanCommand::Enter)
    );
}

#[test]
fn plan_command_exit_parses() {
    assert_eq!(
        parse_chat_command("/plan exit"),
        ChatCommand::Plan(PlanCommand::Exit)
    );
}

#[test]
fn plan_command_build_with_id_parses() {
    assert_eq!(
        parse_chat_command("/plan build ship-001"),
        ChatCommand::Plan(PlanCommand::Build {
            plan_target: Some("ship-001".to_string()),
        })
    );
}

#[test]
fn plan_command_build_with_path_parses() {
    assert_eq!(
        parse_chat_command("/plan build /tmp/ship-001.plan.md"),
        ChatCommand::Plan(PlanCommand::Build {
            plan_target: Some("/tmp/ship-001.plan.md".to_string()),
        })
    );
}

#[test]
fn plan_command_build_without_target_parses() {
    assert_eq!(
        parse_chat_command("/plan build"),
        ChatCommand::Plan(PlanCommand::Build { plan_target: None })
    );
}

#[test]
fn plan_command_with_extra_arg_returns_usage_error() {
    assert!(matches!(
        parse_chat_command("/plan ship-plan-mode"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn plan_command_in_chat_text_does_not_match() {
    // `/plan` 必须是行首 token；混在普通文本里不解析为命令
    assert!(matches!(
        parse_chat_command("帮我 /plan exit"),
        ChatCommand::NotACommand(_)
    ));
}

#[test]
fn skill_use_list_reload_parsed() {
    assert_eq!(
        parse_chat_command("/skill list"),
        ChatCommand::Skill(SkillCommand::List)
    );
    assert_eq!(
        parse_chat_command("/skill reload"),
        ChatCommand::Skill(SkillCommand::Reload)
    );
    assert_eq!(
        parse_chat_command("/skill use commit \"ship release\""),
        ChatCommand::Skill(SkillCommand::Use {
            name: "commit".to_string(),
            intent: "ship release".to_string(),
        })
    );
}
