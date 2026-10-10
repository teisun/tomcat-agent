use super::super::{run_shared_slash_command, shared::run_terminal_shared};
use super::test_support::{plugin, skill, Fixture};
use crate::api::chat::panels::{Answer, AskQuestionPanel, AskQuestionResult, MockAskQuestionPanel};
use crate::core::plan_runtime::AskQuestionOutcome;
use crate::infra::i18n::{tr_in, Locale};
use serial_test::serial;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(env_lock)]
async fn cmd_uninstall_package_removes_live_vm_skill_tools_but_keeps_unrelated_resources() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let source = tempfile::tempdir().unwrap();
    plugin(&source.path().join("plugins/echo"), "echo", "echo_tool");
    skill(&source.path().join("skills/hello"), "hello", "greeting");
    std::fs::write(source.path().join("package.json"), r#"{"name":"bundle","version":"1.0.0","tomcat":{"plugins":["plugins/echo"],"skills":["skills/hello"]}}"#).unwrap();
    let unrelated_source = tempfile::tempdir().unwrap();
    plugin(unrelated_source.path(), "unrelated", "keep_tool");
    skill(
        &fixture.workspace.path().join(".agents/skills/keep"),
        "keep",
        "unchanged",
    );
    let ctx = fixture.ctx();
    let installed = run_shared_slash_command(
        &ctx,
        "install",
        &[
            unrelated_source.path().to_string_lossy().into_owned(),
            "current-project".into(),
        ],
    )
    .await;
    assert!(installed.ok, "{installed:?}");
    let reply = run_shared_slash_command(
        &ctx,
        "install",
        &[
            source.path().to_string_lossy().into_owned(),
            "current-project".into(),
        ],
    )
    .await;
    assert!(reply.ok, "{reply:?}");
    let sid = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .unwrap();
    let pm = ctx.global_services.plugin_manager.as_ref().unwrap();
    let old = pm.start_session_vm(&sid, "echo").await.unwrap();
    let unrelated = pm.start_session_vm(&sid, "unrelated").await.unwrap();
    let reply = run_shared_slash_command(
        &ctx,
        "uninstall",
        &["bundle".into(), "current-project".into()],
    )
    .await;
    assert!(
        reply.ok
            && reply.text.contains(&tr_in(
                Locale::En,
                "slash.uninstall.done",
                &[("name", "bundle"), ("target", "current-project")]
            )),
        "{reply:?}"
    );
    assert!(reply.text.contains(&format!(
        "{} skill: hello",
        tr_in(Locale::En, "slash.sync.removed", &[])
    )));
    assert!(reply.text.contains(&format!(
        "{} plugin: echo",
        tr_in(Locale::En, "slash.sync.removed", &[])
    )));
    assert!(!ctx.skill_set_snapshot().by_name.contains_key("hello"));
    assert!(ctx.skill_set_snapshot().by_name.contains_key("keep"));
    assert!(ctx
        .global_services
        .tool_registry
        .get_tool("echo_tool")
        .await
        .is_err());
    assert!(ctx
        .global_services
        .tool_registry
        .get_tool("keep_tool")
        .await
        .is_ok());
    assert!(!pm.has_session_vm(&sid, "echo"));
    assert!(Arc::ptr_eq(
        &unrelated.state,
        &pm.start_session_vm(&sid, "unrelated").await.unwrap().state
    ));
    pm.end_session(&sid).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while old.current_state() != crate::ext::VmActorState::Stopped
            || unrelated.current_state() != crate::ext::VmActorState::Stopped
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cmd_uninstall_missing_and_manual_plugin_fail_without_changing_resources() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let manual = fixture
        .workspace
        .path()
        .join(".agents/plugins/directory-not-id");
    plugin(&manual, "manual-id", "manual_tool");
    let original = std::fs::read(manual.join("plugin.json")).unwrap();
    let ctx = fixture.ctx();
    let before = ctx
        .global_services
        .tool_registry
        .list_tools(None)
        .await
        .unwrap();
    for name in ["not-installed", "manual-id"] {
        let reply =
            run_shared_slash_command(&ctx, "uninstall", &[name.into(), "current-project".into()])
                .await;
        assert!(!reply.ok);
        let paths = crate::core::package::resolve_layer_paths(
            &ctx.config,
            crate::core::package::PackageVisibility::Scope,
            Some(&ctx.scope_services.resource_root),
        )
        .unwrap();
        assert!(reply.text.contains(&tr_in(
            Locale::En,
            "package.notInstalled",
            &[
                ("layer", &paths.visibility.to_string()),
                ("name", name),
                (
                    "plugins",
                    &crate::infra::platform::format_home_path(&paths.plugins_dir)
                ),
                (
                    "skills",
                    &crate::infra::platform::format_home_path(&paths.skills_dir)
                ),
            ]
        )));
        assert!(reply.text.contains(
            &fixture
                .workspace
                .path()
                .join(".agents/plugins")
                .display()
                .to_string()
        ));
        assert!(reply.text.contains(
            &fixture
                .workspace
                .path()
                .join(".agents/skills")
                .display()
                .to_string()
        ));
        assert_eq!(std::fs::read(manual.join("plugin.json")).unwrap(), original);
        assert_eq!(
            ctx.global_services
                .tool_registry
                .list_tools(None)
                .await
                .unwrap()
                .iter()
                .map(|tool| &tool.name)
                .collect::<Vec<_>>(),
            before.iter().map(|tool| &tool.name).collect::<Vec<_>>()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cmd_uninstall_terminal_chooser_selects_agent_and_cancel_preserves_package() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let source = tempfile::tempdir().unwrap();
    skill(source.path(), "agent-hello", "greeting");
    let panel: Arc<dyn AskQuestionPanel> = Arc::new(MockAskQuestionPanel::new(vec![
        AskQuestionResult {
            answers: vec![],
            outcome: AskQuestionOutcome::Skipped,
        },
        AskQuestionResult {
            answers: vec![Answer {
                question_id: "uninstall-target".into(),
                option_ids: vec!["agent".into()],
                custom_text: None,
                skipped: false,
                picked_recommended: false,
            }],
            outcome: AskQuestionOutcome::Answered,
        },
    ]));
    let ctx = fixture.ctx_with_panel(panel);
    let reply = run_shared_slash_command(
        &ctx,
        "install",
        &[source.path().to_string_lossy().into_owned(), "agent".into()],
    )
    .await;
    assert!(reply.ok, "{reply:?}");
    run_terminal_shared(&ctx, "uninstall", vec!["agent-hello".into()]).await;
    assert!(ctx.skill_set_snapshot().by_name.contains_key("agent-hello"));
    run_terminal_shared(&ctx, "uninstall", vec!["agent-hello".into()]).await;
    assert!(!ctx.skill_set_snapshot().by_name.contains_key("agent-hello"));
    assert!(!fixture
        .work
        .path()
        .join("agents/main/skills/agent-hello")
        .exists());
}
