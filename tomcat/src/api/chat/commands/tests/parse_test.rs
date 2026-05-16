//! Tests for `commands::parse` — slash-command recognition contract.

use super::super::{parse_chat_command, ChatCommand};

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
