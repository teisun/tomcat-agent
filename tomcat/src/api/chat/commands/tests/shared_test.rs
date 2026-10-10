use super::super::{parse_shared_slash, run_shared_slash_command, shared_slash_commands};
use super::test_support::Fixture;
use crate::infra::i18n::{tr_in, Locale};
use serial_test::serial;

#[test]
fn shared_test_summary_omits_zero_counts_and_does_not_claim_latest_after_read_warnings() {
    use super::super::shared::render_sync_summary;
    let mut report = crate::api::chat::InventoryReport::default();
    report.skills_removed.push("hello".into());
    report.plugins_removed.push("echo".into());
    let text = render_sync_summary("reload", &report);
    assert!(text.contains(&tr_in(
        Locale::En,
        "slash.sync.changed",
        &[
            ("name", "reload"),
            ("skills", "-1"),
            ("plugins", "-1"),
            ("elapsed", "0.0"),
        ]
    )));
    assert!(!text.contains("+0") && !text.contains("~0"));
    report.skills_removed.clear();
    report.plugins_removed.clear();
    report
        .warnings
        .push("plugin registry unreadable; previous inventory kept".into());
    let text = render_sync_summary("reload", &report);
    assert!(!text.contains(&tr_in(Locale::En, "slash.sync.latest", &[])));
    assert!(text.contains(&tr_in(
        Locale::En,
        "slash.sync.noChanges",
        &[
            ("name", "reload"),
            ("status", &tr_in(Locale::En, "slash.sync.warnings", &[])),
            ("elapsed", "0.0"),
        ]
    )));
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
        .contains(&tr_in(Locale::En, "slash.install.usage", &[])));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn shared_test_table_and_executor_agree_and_missing_arguments_never_prompt() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let ctx = fixture.ctx();
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let names = shared_slash_commands()
        .iter()
        .map(|command| command.name)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), shared_slash_commands().len());
    for command in shared_slash_commands() {
        let reply = run_shared_slash_command(&ctx, command.name, &[]).await;
        assert!(
            !reply.text.contains(&tr_in(
                Locale::En,
                "slash.unknown",
                &[("name", command.name), ("commands", "")]
            )),
            "{reply:?}"
        );
        if command.name != "reload" {
            assert!(!reply.ok);
            assert!(reply.text.contains(&command.usage));
        }
    }
    for name in ["install", "uninstall"] {
        let reply = run_shared_slash_command(&ctx, name, &["a-source-or-package".into()]).await;
        assert!(!reply.ok);
        assert!(reply
            .text
            .contains(&tr_in(Locale::En, &format!("slash.{name}.usage"), &[])));
    }
    let unknown = run_shared_slash_command(&ctx, "model", &[]).await;
    assert!(!unknown.ok);
    assert!(unknown.text.contains(&tr_in(
        Locale::En,
        "slash.unknown",
        &[("name", "model"), ("commands", "")]
    )));
    for name in names {
        assert!(unknown.text.contains(&format!("/{name}")));
    }
    assert!(!fixture
        .workspace
        .path()
        .join(".agents/packages/registry.json")
        .exists());
}
