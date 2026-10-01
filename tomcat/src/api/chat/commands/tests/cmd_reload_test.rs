use super::super::run_shared_slash_command;
use super::test_support::{skill, Fixture};
use serial_test::serial;

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
    assert!(reply.text.contains("没有变化") && reply.text.contains("用时"));
    skill(&root, "demo", "new-description");
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(reply.ok && reply.text.contains("更新 skill: demo"));
    std::fs::remove_dir_all(&root).unwrap();
    let reply = run_shared_slash_command(&ctx, "reload", &[]).await;
    assert!(reply.ok && reply.text.contains("移除 skill: demo"));
    let reply = run_shared_slash_command(&ctx, "reload", &["now".into()]).await;
    assert!(!reply.ok && reply.text.contains("用法：/reload"));
}
