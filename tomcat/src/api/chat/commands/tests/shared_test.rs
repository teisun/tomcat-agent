use super::super::{parse_shared_slash, run_shared_slash_command, SHARED_SLASH_COMMANDS};
use super::test_support::Fixture;
use serial_test::serial;

#[test]
fn shared_test_summary_omits_zero_counts_and_does_not_claim_latest_after_read_warnings() {
    use super::super::shared::render_sync_summary;
    let mut report = crate::api::chat::InventoryReport::default();
    report.skills_removed.push("hello".into());
    report.plugins_removed.push("echo".into());
    let text = render_sync_summary("reload", &report);
    assert!(text.contains("Skill -1") && text.contains("插件 -1"));
    assert!(!text.contains("+0") && !text.contains("~0"));
    report.skills_removed.clear();
    report.plugins_removed.clear();
    report
        .warnings
        .push("plugin registry unreadable; previous inventory kept".into());
    let text = render_sync_summary("reload", &report);
    assert!(!text.contains("已是最新"));
    assert!(text.contains("保持不变") && text.contains("警告"));
}

#[test]
fn shared_test_parse_exact_tokens_and_shell_words() {
    assert_eq!(
        parse_shared_slash(" /reload \n").unwrap().unwrap(),
        ("reload".into(), vec![])
    );
    assert_eq!(
        parse_shared_slash("/install './folder with space' agent")
            .unwrap()
            .unwrap(),
        (
            "install".into(),
            vec!["./folder with space".into(), "agent".into()]
        )
    );
    for text in [
        "/reloadx",
        "/model",
        "/Users/foo",
        "hello /reload",
        "/RELOAD",
        "",
        "/reload/foo",
    ] {
        assert!(parse_shared_slash(text).is_none(), "{text}");
    }
    assert!(parse_shared_slash("/install 'unterminated")
        .unwrap()
        .unwrap_err()
        .contains("用法"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn shared_test_table_and_executor_agree_and_missing_arguments_never_prompt() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let names = SHARED_SLASH_COMMANDS
        .iter()
        .map(|command| command.name)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), SHARED_SLASH_COMMANDS.len());
    for command in SHARED_SLASH_COMMANDS {
        let reply = run_shared_slash_command(&ctx, command.name, &[]).await;
        assert!(!reply.text.contains("未知命令"), "{reply:?}");
        if command.name != "reload" {
            assert!(!reply.ok);
            assert!(reply.text.contains(command.usage));
        }
    }
    for name in ["install", "uninstall"] {
        let reply = run_shared_slash_command(&ctx, name, &["a-source-or-package".into()]).await;
        assert!(!reply.ok);
        assert!(reply.text.contains("用法"));
    }
    let unknown = run_shared_slash_command(&ctx, "model", &[]).await;
    assert!(!unknown.ok);
    assert!(unknown.text.contains("未知命令"));
    for name in names {
        assert!(unknown.text.contains(&format!("/{name}")));
    }
    assert!(!fixture
        .workspace
        .path()
        .join(".agents/packages/registry.json")
        .exists());
}
