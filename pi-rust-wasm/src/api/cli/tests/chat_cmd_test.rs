//! 单元测试：`chat_cmd` 中的 Ctrl+C 双击检测纯函数。

use super::{check_double_tap, DoubleTap, DOUBLE_TAP_WINDOW};
use std::time::{Duration, Instant};

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
