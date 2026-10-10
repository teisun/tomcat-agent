use super::super::run_shared_slash_command;
use super::test_support::{skill, Fixture};
use crate::infra::i18n::{tr_in, Locale};
use serial_test::serial;

fn put(workspace: &std::path::Path, relative: &str, body: &str) {
    let file = workspace.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cmd_reload_instruction_counts_paths_and_deny_source() {
    use crate::core::permission::{PathRule, PathRuleMode};
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    put(
        fixture.workspace.path(),
        ".cursor/commands/review.md",
        "Review",
    );
    put(
        fixture.workspace.path(),
        ".cursor/rules/on.md",
        "---\nalwaysApply: true\n---\nON",
    );
    put(
        fixture.workspace.path(),
        ".cursor/rules/off.md",
        "---\nalwaysApply: false\n---\nOFF",
    );
    put(
        fixture.workspace.path(),
        ".cursor/rules/secret.mdc",
        "---\nalwaysApply: true\n---\nSECRET",
    );
    let ctx = fixture.ctx();
    let root = ctx.scope_services.resource_root.canonicalize().unwrap();
    let denied = root
        .join(".cursor/rules/secret.mdc")
        .to_string_lossy()
        .into_owned();
    ctx.global_services.gate.grant_path_rule(PathRule {
        path: denied.clone(),
        mode: PathRuleMode::Deny,
    });
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(reply.ok, "{reply:?}");
    assert!(
        reply.text.contains(&tr_in(
            Locale::En,
            "runtime.commandsSummary",
            &[("count", "1"), ("skipped", "0")]
        )),
        "{}",
        reply.text
    );
    assert!(
        reply.text.contains(&tr_in(
            Locale::En,
            "runtime.rulesSummary",
            &[("count", "1"), ("skipped", "2")]
        )),
        "{}",
        reply.text
    );
    let reasons = reply
        .text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(".cursor/"))
        .collect::<Vec<_>>();
    assert_eq!(reasons.len(), 2, "{}", reply.text);
    assert!(reasons
        .iter()
        .any(|line| line.starts_with(".cursor/rules/off.md") && line.contains("alwaysApply")));
    assert!(reasons
        .iter()
        .any(|line| line.starts_with(".cursor/rules/secret.mdc")
            && line.contains(&format!("path_rule deny: {denied}"))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cmd_reload_instruction_diagnostics_truncate_exactly_twenty_one() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    for n in 0..21 {
        put(
            fixture.workspace.path(),
            &format!(".cursor/rules/off-{n:02}.md"),
            "---\nalwaysApply: false\n---\nOFF",
        );
    }
    let reply = run_shared_slash_command(&fixture.ctx(), "reload", &[]).await;
    assert!(reply.ok, "{reply:?}");
    assert!(
        reply.text.contains(&tr_in(
            Locale::En,
            "runtime.commandsSummary",
            &[("count", "0"), ("skipped", "0")]
        )),
        "{}",
        reply.text
    );
    assert!(
        reply.text.contains(&tr_in(
            Locale::En,
            "runtime.rulesSummary",
            &[("count", "0"), ("skipped", "21")]
        )),
        "{}",
        reply.text
    );
    assert_eq!(
        reply
            .text
            .lines()
            .filter(|line| line.trim().starts_with(".cursor/rules/"))
            .count(),
        20
    );
    assert!(
        reply.text.contains(&tr_in(
            Locale::En,
            "runtime.moreDiagnostics",
            &[("count", "1")]
        )),
        "{}",
        reply.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cmd_reload_noop_description_update_and_removal() {
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = Fixture::new();
    let root = fixture.workspace.path().join(".agents/skills/demo");
    skill(&root, "demo", "old");
    let ctx = fixture.ctx();
    ctx.spawn_skill_discovery_if_needed().await;
    ctx.await_skill_discovery().await;
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(reply.ok, "{reply:?}");
    assert!(
        reply
            .text
            .contains(&tr_in(Locale::En, "slash.sync.latest", &[]))
            && reply.text.contains("ms)")
    );
    skill(&root, "demo", "new-description");
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(
        reply.ok
            && reply.text.contains(&format!(
                "{} skill: demo",
                tr_in(Locale::En, "slash.sync.updated", &[])
            ))
    );
    std::fs::remove_dir_all(&root).unwrap();
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(
        reply.ok
            && reply.text.contains(&format!(
                "{} skill: demo",
                tr_in(Locale::En, "slash.sync.removed", &[])
            ))
    );
    let reply = run_shared_slash_command(&ctx, "reload", &["now".into()]).await;
    assert!(
        !reply.ok
            && reply.text.contains(&tr_in(
                Locale::En,
                "slash.usage",
                &[("name", "reload"), ("usage", "/reload"), ("suffix", "")]
            ))
    );
}
