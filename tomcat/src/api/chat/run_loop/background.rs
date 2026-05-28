use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::warn;

use crate::api::chat::ChatContext;
use crate::core::tools::primitive::{BackgroundTaskLifecycleEvent, BashTaskStatus};

pub(super) fn spawn_completion_subscriber(ctx: &ChatContext) -> JoinHandle<()> {
    let registry = ctx.bash_task_registry.clone();
    let routes = ctx.completion_routes.clone();
    let queue = ctx.follow_up_queue.clone();
    let signal = ctx.follow_up_signal.clone();
    let delivered = ctx.delivered_completion.clone();

    let mut rx = registry.subscribe_lifecycle();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(BackgroundTaskLifecycleEvent {
                    task_id,
                    final_status,
                    log_path,
                    command,
                }) => {
                    {
                        let mut delivered_guard = delivered.lock();
                        if delivered_guard.contains(&task_id) {
                            continue;
                        }
                        delivered_guard.insert(task_id.clone());
                    }
                    let should_push = {
                        let mut routes_guard = routes.lock();
                        match routes_guard.get(&task_id).copied() {
                            Some(crate::core::agent_loop::CompletionRoute::ToolWillDeliver)
                            | Some(crate::core::agent_loop::CompletionRoute::Delivered) => false,
                            _ => {
                                routes_guard.insert(
                                    task_id.clone(),
                                    crate::core::agent_loop::CompletionRoute::Delivered,
                                );
                                true
                            }
                        }
                    };
                    if !should_push {
                        continue;
                    }
                    let exit_code = match final_status {
                        BashTaskStatus::Finished { exit_code } => exit_code,
                        BashTaskStatus::Stopped => -1,
                        BashTaskStatus::Running => continue,
                    };
                    let tail = registry.tail_log(&task_id, 4096).await;
                    let text = format!(
                        "<background-task-finished task_id=\"{task_id}\" exit_code=\"{exit_code}\" log_path=\"{log_path}\" command=\"{cmd}\">\n{tail}\n</background-task-finished>",
                        task_id = task_id,
                        exit_code = exit_code,
                        log_path = log_path,
                        cmd = command.replace('"', "\\\""),
                    );
                    queue.lock().push(crate::core::llm::ChatMessage::user(text));
                    signal.notify_one();
                    eprintln!(
                        "\n[bg] task {} finished (exit={}); queued for next turn.",
                        task_id, exit_code
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        target: "tomcat_chat_diag",
                        phase = "completion_subscriber_lagged",
                        skipped = skipped,
                        "lifecycle broadcast subscriber lagged; some events skipped"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

pub(super) fn spawn_readline_waker(signal: Arc<tokio::sync::Notify>) -> JoinHandle<()> {
    tokio::spawn(async move {
        signal.notified().await;
        wake_blocking_readline();
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn wake_blocking_readline() {
    // `rustyline` 会把 SIGWINCH 转成 `ReadlineError::Signal(Signal::Resize)`；借它把阻塞中的
    // `readline()` 温和唤醒，让 chat loop 立刻进入 auto-drain，而不是等用户再按一次回车。
    unsafe {
        libc::raise(libc::SIGWINCH);
    }
}

#[cfg(any(not(unix), target_os = "macos"))]
// macOS 下优先保证 IME 输入稳定，宁可退回“等用户下一次交互再 drain”。
pub(super) fn wake_blocking_readline() {}
