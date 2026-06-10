//! 交互式对话模式共享入口：`claw` / `code` 复用同一套 Ctrl+C 与 runtime 桥接逻辑。
//!
//! 本模块负责 L0（进程级）中断信号到 L1（chat 会话）取消令牌的桥接，
//! 具体分层与时序参见 `docs/architecture/interrupt-and-cancellation.md`。

use std::time::{Duration, Instant};

use crate::{AppConfig, AppError, SessionMode};

/// 软 vs 硬中断判定结果。`Hard` 意味着 2 秒内第二次 Ctrl+C，
/// 调用方应走 `std::process::exit(130)`；`Soft` 则仅 cancel 当前回合 token。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleTap {
    Soft,
    Hard,
}

/// 纯函数：根据上次 Ctrl+C 时刻与当前时刻判断本次是否构成"硬中断"。
///
/// - `last`：上一次 Ctrl+C 触发的时刻；`None` 表示首击。
/// - `now`：当前时刻（通常是 `Instant::now()`）。
/// - `window`：双击判定窗口，推荐 `Duration::from_secs(2)`，与常见终端 POSIX 约定一致。
///
/// 返回 `Hard` 当且仅当 `last` 非空且 `now - last <= window`；其余情况返回 `Soft`。
/// 抽成纯函数后可脱离 `ctrlc::set_handler` 做单元测试，避免全局副作用污染。
pub fn check_double_tap(last: Option<Instant>, now: Instant, window: Duration) -> DoubleTap {
    match last {
        Some(prev) if now.saturating_duration_since(prev) <= window => DoubleTap::Hard,
        _ => DoubleTap::Soft,
    }
}

/// 双击判定默认窗口。
const DOUBLE_TAP_WINDOW: Duration = Duration::from_secs(2);

pub(super) fn run_chat_mode(
    resume: bool,
    cfg: &AppConfig,
    mode: SessionMode,
) -> Result<(), AppError> {
    let ctx = super::super::chat::ChatContext::from_config_with_mode(cfg.clone(), mode)?;

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Config(format!("创建 tokio 运行时失败: {}", e)))?;

    // 桥接 L0 → L1：SIGINT → ChatContext.cancel_token.cancel() + 双击检测。
    let cancel_token = ctx.session_runtime.cancel_token.clone();
    let last_interrupt_at = ctx.session_runtime.last_interrupt_at.clone();
    let append_in_flight = ctx.session_runtime.session.append_in_flight_counter();
    ctrlc::set_handler(move || {
        let now = Instant::now();
        let prev = {
            let mut guard = last_interrupt_at.lock();
            let prev = *guard;
            *guard = Some(now);
            prev
        };
        match check_double_tap(prev, now, DOUBLE_TAP_WINDOW) {
            DoubleTap::Hard => {
                let deadline = Instant::now() + Duration::from_millis(500);
                while append_in_flight.load(std::sync::atomic::Ordering::SeqCst) > 0
                    && Instant::now() < deadline
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                // 进程级硬中断：POSIX 约定 128 + SIGINT(2) = 130。
                // 依赖 `SessionManager` 的 append-only JSONL 在首击 partial 落盘时已
                // flush，此处即便进程立即结束，transcript 也完整。
                std::process::exit(130);
            }
            DoubleTap::Soft => {
                // 软中断：通知当前回合取消。token 一旦 cancel 不可逆，chat_loop
                // 会在下一次 readline 读到非空输入后重建 token。
                cancel_token.lock().cancel();
            }
        }
    })
    .ok();

    rt.block_on(super::super::chat::chat_loop(&ctx, resume))
}

#[cfg(test)]
#[path = "tests/chat_cmd_test.rs"]
mod tests;
