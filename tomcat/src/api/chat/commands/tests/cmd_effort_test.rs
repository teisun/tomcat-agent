//! `/effort` 命令解析与持久化的焦小测。

use super::super::cmd_effort::{apply_level, parse_effort_level};
use super::super::{help_text, parse_chat_command, ChatCommand};
use crate::core::llm::ThinkingLevel;

#[test]
fn effort_levels_parse_correctly() {
    assert_eq!(parse_effort_level("low"), Some(ThinkingLevel::Low));
    assert_eq!(parse_effort_level("medium"), Some(ThinkingLevel::Medium));
    assert_eq!(parse_effort_level("high"), Some(ThinkingLevel::High));
    assert_eq!(parse_effort_level("xhigh"), Some(ThinkingLevel::Xhigh));
}

#[test]
fn effort_command_parses_all_supported_levels() {
    assert_eq!(
        parse_chat_command("/effort low"),
        ChatCommand::Effort {
            level: ThinkingLevel::Low,
        }
    );
    assert_eq!(
        parse_chat_command("/effort medium"),
        ChatCommand::Effort {
            level: ThinkingLevel::Medium,
        }
    );
    assert_eq!(
        parse_chat_command("/effort high"),
        ChatCommand::Effort {
            level: ThinkingLevel::High,
        }
    );
    assert_eq!(
        parse_chat_command("/effort xhigh"),
        ChatCommand::Effort {
            level: ThinkingLevel::Xhigh,
        }
    );
}

#[test]
fn effort_without_arg_is_usage_error() {
    assert!(matches!(
        parse_chat_command("/effort"),
        ChatCommand::UsageError { .. }
    ));
}

#[test]
fn effort_unknown_arg_is_usage_error() {
    let cmd = parse_chat_command("/effort turbo");
    let msg = match cmd {
        ChatCommand::UsageError { message } => message,
        other => panic!("应为 UsageError，实际：{:?}", other),
    };
    assert!(
        msg.contains("low|medium|high|xhigh"),
        "错误文案应列出合法值：{msg}"
    );
    assert!(msg.contains("turbo"), "错误文案应回显非法输入：{msg}");
}

#[test]
#[serial_test::serial(env_lock)]
fn apply_level_persists_model_override() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = crate::AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    let ctx = crate::api::chat::ChatContext::from_config(cfg).unwrap();
    let store = &ctx.global_services.model_prefs;

    apply_level(&ctx, "gpt-5.4", ThinkingLevel::Xhigh).unwrap();
    assert_eq!(store.reasoning_for("gpt-5.4"), ThinkingLevel::Xhigh);

    let path = crate::resolve_model_thinking_path(&ctx.config).unwrap();
    let reloaded = crate::ModelPrefsStore::load(&path, ThinkingLevel::Medium).unwrap();
    assert_eq!(reloaded.reasoning_for("gpt-5.4"), ThinkingLevel::Xhigh);
}

#[test]
#[serial_test::serial(env_lock)]
fn effort_manual_default_clears_only_this_sessions_matching_override_and_reports_partial_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = crate::AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().into());
    cfg.llm.default_model = "gpt-5.4".into();
    let ctx = crate::api::chat::ChatContext::from_config(cfg).unwrap();
    let session = &ctx.session_runtime.session;
    let key = session.current_session_key();
    session.create_session(key, None).unwrap();
    session
        .update_session(key, |entry| entry.thinking_level = Some("low".into()))
        .unwrap();
    let other = crate::SessionManager::new_scoped(
        crate::resolve_sessions_dir(&ctx.config).unwrap(),
        "agent:main:other",
    );
    other
        .create_session(other.current_session_key(), None)
        .unwrap();
    other
        .update_session(other.current_session_key(), |entry| {
            entry.thinking_level = Some("high".into())
        })
        .unwrap();
    apply_level(&ctx, "other-model", ThinkingLevel::High).unwrap();
    assert_eq!(
        session
            .current_session_entry()
            .unwrap()
            .unwrap()
            .thinking_level
            .as_deref(),
        Some("low")
    );
    apply_level(&ctx, "gpt-5.4", ThinkingLevel::High).unwrap();
    assert_eq!(
        session
            .current_session_entry()
            .unwrap()
            .unwrap()
            .thinking_level,
        None
    );
    assert_eq!(
        other
            .current_session_entry()
            .unwrap()
            .unwrap()
            .thinking_level
            .as_deref(),
        Some("high")
    );

    session
        .update_session(key, |entry| entry.thinking_level = Some("low".into()))
        .unwrap();
    let prefs = crate::resolve_model_thinking_path(&ctx.config).unwrap();
    let backup = dir.path().join("prefs-backup");
    std::fs::rename(&prefs, &backup).unwrap();
    std::fs::create_dir(&prefs).unwrap();
    assert!(apply_level(&ctx, "gpt-5.4", ThinkingLevel::Medium).is_err());
    assert_eq!(
        ctx.global_services.model_prefs.reasoning_for("gpt-5.4"),
        ThinkingLevel::High
    );
    assert_eq!(
        session
            .current_session_entry()
            .unwrap()
            .unwrap()
            .thinking_level
            .as_deref(),
        Some("low")
    );
    std::fs::remove_dir(&prefs).unwrap();
    std::fs::rename(&backup, &prefs).unwrap();

    // Fault only this disposable store's write lock; reads still succeed.
    let lock = crate::resolve_sessions_dir(&ctx.config)
        .unwrap()
        .join("sessions.json.lock");
    let lock_backup = lock.with_extension("backup");
    std::fs::rename(&lock, &lock_backup).unwrap();
    std::fs::create_dir(&lock).unwrap();
    let error = apply_level(&ctx, "gpt-5.4", ThinkingLevel::Xhigh).unwrap_err();
    let partial = crate::infra::i18n::tr(
        "slash.effort.defaultSavedOverrideNotCleared",
        &[("detail", "")],
    );
    assert!(error.to_string().contains(partial.trim_end()), "{error}");
    assert_eq!(
        crate::ModelPrefsStore::load(&prefs, ThinkingLevel::Medium)
            .unwrap()
            .reasoning_for("gpt-5.4"),
        ThinkingLevel::Xhigh
    );
    assert_eq!(
        session
            .current_session_entry()
            .unwrap()
            .unwrap()
            .thinking_level
            .as_deref(),
        Some("low")
    );
    std::fs::remove_dir(&lock).unwrap();
    std::fs::rename(&lock_backup, &lock).unwrap();
}

#[test]
fn help_text_mentions_effort_command() {
    let h = help_text();
    assert!(h.contains("/effort"), "/help 应列出 /effort：{h}");
    assert!(
        h.contains("low|medium|high|xhigh"),
        "帮助文案应展示 /effort 支持档位：{h}"
    );
}
