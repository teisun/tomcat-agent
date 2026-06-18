//! 单元测试：`chat_cmd` 中的 Ctrl+C 双击检测纯函数。

use super::{build_runtime_and_context, check_double_tap, DoubleTap, DOUBLE_TAP_WINDOW};
use std::time::{Duration, Instant};

use crate::{AppConfig, SessionMode};

#[test]
fn soft_when_first_press() {
    let now = Instant::now();
    assert_eq!(
        check_double_tap(None, now, DOUBLE_TAP_WINDOW),
        DoubleTap::Soft
    );
}

#[test]
fn hard_when_second_press_within_window() {
    let first = Instant::now();
    let second = first + Duration::from_millis(500);
    assert_eq!(
        check_double_tap(Some(first), second, DOUBLE_TAP_WINDOW),
        DoubleTap::Hard
    );
}

#[test]
fn soft_when_second_press_outside_window() {
    let first = Instant::now();
    let second = first + Duration::from_secs(3);
    assert_eq!(
        check_double_tap(Some(first), second, DOUBLE_TAP_WINDOW),
        DoubleTap::Soft
    );
}

#[test]
fn hard_at_exact_window_boundary() {
    // 2s 边界值：应当仍判为 Hard（`<= window`）。
    let first = Instant::now();
    let second = first + DOUBLE_TAP_WINDOW;
    assert_eq!(
        check_double_tap(Some(first), second, DOUBLE_TAP_WINDOW),
        DoubleTap::Hard
    );
}

#[test]
fn build_runtime_and_context_constructs_chat_context_with_tokio_handle() {
    let work_dir = tempfile::tempdir().expect("work dir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some("CHAT_CMD_BUILD_RUNTIME_TEST_KEY".to_string());

    unsafe {
        std::env::set_var("CHAT_CMD_BUILD_RUNTIME_TEST_KEY", "stub");
    }

    let (_rt, ctx) =
        build_runtime_and_context(&cfg, SessionMode::Claw).expect("build runtime and context");
    assert!(
        ctx.scope_services
            .scope_container
            .dispatcher
            .has_tokio_handle(),
        "shared builder should construct ChatContext within rt.enter() so async hostcall can spawn"
    );

    unsafe {
        std::env::remove_var("CHAT_CMD_BUILD_RUNTIME_TEST_KEY");
    }
}
