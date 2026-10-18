//! `preflight` 私有项单元测试（`tests/preflight_test.rs`，与同目录 `suite_test` / `cwd_lazy_prompt_test` 的 `*_test` 命名一致；由 `preflight.rs` 末尾 `#[path]` 挂载）。

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

#[test]
fn install_plan_builds_the_same_command_for_every_manager_and_target() {
    use super::{install_plan, InstallTarget};
    use crate::infra::platform::PackageManager::{AptGet, Brew, Dnf, Pacman, TermuxPkg, Winget};

    const WINGET_FLAGS: &str = "--silent --accept-source-agreements --accept-package-agreements";
    let winget_search = format!(
        "winget install --id BurntSushi.ripgrep.MSVC {WINGET_FLAGS} && winget install --id sharkdp.fd {WINGET_FLAGS}"
    );
    let winget_git = format!("winget install --id Git.Git {WINGET_FLAGS}");
    let cases: [(_, _, &str, Vec<&str>); 12] = [
        (
            TermuxPkg,
            InstallTarget::SearchTools,
            "pkg",
            vec!["install", "-y", "ripgrep", "fd"],
        ),
        (
            TermuxPkg,
            InstallTarget::Git,
            "pkg",
            vec!["install", "-y", "git"],
        ),
        (
            Brew,
            InstallTarget::SearchTools,
            "brew",
            vec!["install", "--force-bottle", "ripgrep", "fd"],
        ),
        (
            Brew,
            InstallTarget::Git,
            "brew",
            vec!["install", "--force-bottle", "git"],
        ),
        (
            Winget,
            InstallTarget::SearchTools,
            "cmd",
            vec!["/C", winget_search.as_str()],
        ),
        (
            Winget,
            InstallTarget::Git,
            "cmd",
            vec!["/C", winget_git.as_str()],
        ),
        (
            AptGet,
            InstallTarget::SearchTools,
            "apt-get",
            vec!["install", "-y", "ripgrep", "fd-find"],
        ),
        (
            AptGet,
            InstallTarget::Git,
            "apt-get",
            vec!["install", "-y", "git"],
        ),
        (
            Dnf,
            InstallTarget::SearchTools,
            "dnf",
            vec!["install", "-y", "ripgrep", "fd-find"],
        ),
        (Dnf, InstallTarget::Git, "dnf", vec!["install", "-y", "git"]),
        (
            Pacman,
            InstallTarget::SearchTools,
            "pacman",
            vec!["-S", "--noconfirm", "ripgrep", "fd"],
        ),
        (
            Pacman,
            InstallTarget::Git,
            "pacman",
            vec!["-S", "--noconfirm", "git"],
        ),
    ];
    for (manager, target, program, args) in cases {
        let plan = install_plan(manager, target);
        assert_eq!(
            (plan.manager, plan.program, plan.args),
            (manager, program, args),
            "{manager:?} {target:?}"
        );
    }
}

#[test]
fn detached_message_mentions_reopening_the_terminal_only_when_path_needs_restart() {
    use super::{git_detached_message_key, search_detached_message_key};
    use crate::infra::platform::PackageManager::{AptGet, Brew, Winget};

    assert_eq!(
        search_detached_message_key(Winget, true),
        "preflight.searchDetachedRestart"
    );
    assert_eq!(
        search_detached_message_key(AptGet, false),
        "preflight.searchDetached"
    );
    assert_eq!(
        search_detached_message_key(Brew, false),
        "preflight.searchDetachedBrew"
    );
    assert_eq!(
        git_detached_message_key(true),
        "preflight.gitDetachedRestart"
    );
    assert_eq!(git_detached_message_key(false), "preflight.gitDetached");
}

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
