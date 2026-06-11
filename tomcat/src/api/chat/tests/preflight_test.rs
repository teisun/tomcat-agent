//! `preflight` 私有项单元测试（`tests/preflight_test.rs`，与同目录 `suite_test` / `cwd_lazy_prompt_test` 的 `*_test` 命名一致；由 `preflight.rs` 末尾 `#[path]` 挂载）。

use std::path::Path;
use std::sync::Arc;

use crate::infra::AppConfig;
use crate::infra::{wire, DefaultEventBus, EventBus, EventContext, ScopedEventEmitter};
use serde_json::json;

use super::{
    emit_git_preflight, emit_preflight, should_skip_git_preflight, should_skip_preflight,
    trim_for_event,
};

#[test]
fn should_skip_preflight_when_config_disables_auto_install() {
    let mut cfg = AppConfig::default();
    cfg.preflight.auto_install_search_tools = false;
    assert!(should_skip_preflight(&cfg));
}

#[test]
fn should_skip_git_preflight_when_config_disables_auto_install() {
    let mut cfg = AppConfig::default();
    cfg.preflight.auto_install_git = false;
    assert!(should_skip_git_preflight(&cfg));
}

#[test]
fn trim_for_event_limits_long_messages() {
    let input = "x".repeat(600);
    let out = trim_for_event(&input);
    assert!(out.ends_with("..."));
    assert!(out.len() < input.len());
}

#[cfg(unix)]
#[test]
fn nohup_shell_quotes_log_path_with_spaces() {
    use super::{build_nohup_shell_command, InstallPlan};

    let plan = InstallPlan {
        program: "brew",
        args: vec!["install", "--force-bottle", "ripgrep", "fd"],
    };
    let log = Path::new("/tmp/fake preflight.log");
    let cmd = build_nohup_shell_command(&plan, log);
    assert!(
        cmd.starts_with("HOMEBREW_NO_BUILD_FROM_SOURCE=1 nohup "),
        "brew install should forbid source builds: {cmd}"
    );
    assert!(cmd.contains("--force-bottle"));
    assert!(cmd.contains(">>"));
    assert!(cmd.ends_with(" 2>&1 &"));
    assert!(
        cmd.contains("'") || !log.display().to_string().contains(' '),
        "path with spaces should be quoted: {cmd}"
    );
}

#[cfg(unix)]
#[test]
fn nohup_shell_non_brew_has_no_homebrew_env_prefix() {
    use super::{build_nohup_shell_command, InstallPlan};

    let plan = InstallPlan {
        program: "apt-get",
        args: vec!["install", "-y", "ripgrep", "fd-find"],
    };
    let cmd = build_nohup_shell_command(&plan, Path::new("/tmp/p.log"));
    assert!(
        !cmd.contains("HOMEBREW_NO_BUILD_FROM_SOURCE"),
        "non-brew plans must not inject brew env: {cmd}"
    );
    assert!(cmd.starts_with("nohup apt-get "));
}

#[cfg(unix)]
#[test]
fn detached_marker_paths_are_distinct_per_preflight_kind() {
    use super::{detached_log_marker_path, DETACHED_LOG_MARKER_NAME, GIT_DETACHED_LOG_MARKER_NAME};

    let search_marker = detached_log_marker_path(DETACHED_LOG_MARKER_NAME).unwrap();
    let git_marker = detached_log_marker_path(GIT_DETACHED_LOG_MARKER_NAME).unwrap();
    assert_ne!(
        search_marker, git_marker,
        "search_tools 与 git 预检应使用不同 marker，避免互相误判为同一后台安装"
    );
}

#[test]
fn search_preflight_event_carries_session_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_SEARCH_TOOLS_PREFLIGHT,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(bus, "sid-preflight");

    emit_preflight(
        &emitter,
        "ready",
        "search tools ready",
        json!({ "missing": [] }),
    );

    let ctx = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应收到 search_tools_preflight");
    assert_eq!(ctx.session_id.as_deref(), Some("sid-preflight"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-preflight")
    );
}

#[test]
fn git_preflight_event_carries_session_id() {
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    bus.on(
        wire::WIRE_GIT_PREFLIGHT,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let emitter = ScopedEventEmitter::new(bus, "sid-git-preflight");

    emit_git_preflight(&emitter, "ready", "git ready", json!({}));

    let ctx = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应收到 git_preflight");
    assert_eq!(ctx.session_id.as_deref(), Some("sid-git-preflight"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-git-preflight")
    );
}
