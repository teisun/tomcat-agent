//! CLI 会话级 stderr 事件监听：`readline` 等待期间 Layer1 仍可能 emit，须在整段 `chat_loop` 内保持注册。

use std::io::{self, Write as IoWrite};

use crate::infra::event_bus::{EventContext, EventListenerId};
use crate::infra::{wire, EventBus};

pub(crate) struct ChatSessionStderrListenerIds {
    metrics: EventListenerId,
    l1_start: EventListenerId,
    l1_end: EventListenerId,
    l1_err: EventListenerId,
    l2: EventListenerId,
    l3_start: EventListenerId,
    l3_end: EventListenerId,
    l0: EventListenerId,
}

pub(crate) fn register_chat_session_stderr_listeners(bus: &dyn EventBus) -> ChatSessionStderrListenerIds {
    let metrics = bus.on(
        wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |evt: EventContext| {
            let tokens = evt
                .payload
                .get("inputTokensUsed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let ratio = evt
                .payload
                .get("contextUtilizationRatio")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let compactions = evt
                .payload
                .get("compactionCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let saved = evt
                .payload
                .get("compactionTokensFreed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let persisted = evt
                .payload
                .get("totalToolResultBytesPersisted")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let ratio_pct = (ratio * 100.0).min(99999.0);
            let persisted_display = if persisted >= 1024 {
                format!("{:.1} KB", persisted as f64 / 1024.0)
            } else {
                format!("{} B", persisted)
            };
            let preheat_in_progress = evt
                .payload
                .get("preheatInProgress")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let preheat_result_pending = evt
                .payload
                .get("preheatResultPending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (zh_suffix, en_suffix) = if preheat_in_progress {
                (" | 预热中…", " | Preheating…")
            } else if preheat_result_pending {
                (" | 摘要待应用", " | Summary pending apply")
            } else {
                ("", "")
            };
            eprint!(
                "\n\x1b[90m[ctx] {} 令牌 | {:.1}% 占用 | 压缩 x{} | 已节省 {} 令牌 | 已持久化 {}{}\x1b[0m\n",
                tokens, ratio_pct, compactions, saved, persisted_display, zh_suffix
            );
            eprint!(
                "\x1b[90m[ctx] {} tok | {:.1}% | compact x{} | saved {} tok | persisted {}{}\x1b[0m\n",
                tokens, ratio_pct, compactions, saved, persisted_display, en_suffix
            );
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l1_start = bus.on(
        wire::WIRE_AUTO_COMPACTION_START,
        Box::new(|_ctx: EventContext| {
            eprint!("\n\x1b[90m[ctx] 后台压缩已启动…\x1b[0m\n");
            eprint!("\x1b[90m[ctx] Background compaction started…\x1b[0m\n");
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l1_end = bus.on(
        wire::WIRE_AUTO_COMPACTION_END,
        Box::new(|evt: EventContext| {
            let before = evt
                .payload
                .get("estimatedCoveredTokensBefore")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let summ = evt
                .payload
                .get("estimatedSummaryTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let saved = evt
                .payload
                .get("estimatedTokensSaved")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            eprint!(
                "\n\x1b[90m[ctx] 压缩摘要就绪（待应用）| 覆盖区 ~{} 令牌 → 摘要 ~{} 令牌（估省 {} 令牌）\x1b[0m\n",
                before, summ, saved
            );
            eprint!(
                "\x1b[90m[ctx] Summary generated (pending apply) | covered ~{} tok → summary ~{} tok (saved ~{} tok)\x1b[0m\n",
                before, summ, saved
            );
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l1_err = bus.on(
        wire::WIRE_COMPACTION_ERROR,
        Box::new(|evt: EventContext| {
            let source = evt
                .payload
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let err_raw = evt
                .payload
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let err_display = if err_raw.chars().count() > 200 {
                let t: String = err_raw.chars().take(200).collect();
                format!("{}…", t)
            } else {
                err_raw.to_string()
            };
            if source == "apply" {
                eprint!(
                    "\n\x1b[33m[ctx] 摘要应用失败：{}\x1b[0m\n",
                    err_display
                );
                eprint!(
                    "\x1b[33m[ctx] Summary application failed: {}\x1b[0m\n",
                    err_display
                );
                let _ = io::stderr().flush();
                return Ok(());
            }
            let exhausted = evt
                .payload
                .get("exhaustedAfterRetries")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let attempts = evt
                .payload
                .get("attempts")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if exhausted && source == "preheat" {
                eprint!(
                    "\n\x1b[33m[ctx] 预热失败（已重试 {} 次）：{}\x1b[0m\n",
                    attempts, err_display
                );
                eprint!(
                    "\x1b[33m[ctx] Preheat failed after {} attempt(s): {}\x1b[0m\n",
                    attempts, err_display
                );
            } else if source == "preheat" {
                eprint!(
                    "\n\x1b[33m[ctx] 上下文压缩暂时失败，将在下次发送消息时自动重试：{}\x1b[0m\n",
                    err_display
                );
                eprint!(
                    "\x1b[33m[ctx] Context compaction temporarily failed; will retry on your next message: {}\x1b[0m\n",
                    err_display
                );
            }
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l2 = bus.on(
        wire::WIRE_BOUNDARY_SWITCHED,
        Box::new(|evt: EventContext| {
            let saved = evt
                .payload
                .get("estimatedTokensFreed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            eprint!(
                "\n\x1b[90m[ctx] 上下文已压缩重置，约节省 {} 令牌\x1b[0m\n",
                saved
            );
            eprint!(
                "\x1b[90m[ctx] Context compacted; saved ~{} tok\x1b[0m\n",
                saved
            );
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l3_start = bus.on(
        wire::WIRE_CONTEXT_OVERFLOW_TRIM_START,
        Box::new(|_ctx: EventContext| {
            eprint!("\n\x1b[33m[ctx] 上下文溢出，正在截断旧消息…\x1b[0m\n");
            eprint!("\x1b[33m[ctx] Context overflow; trimming older messages…\x1b[0m\n");
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l3_end = bus.on(
        wire::WIRE_CONTEXT_OVERFLOW_TRIM_END,
        Box::new(|evt: EventContext| {
            let saved = evt
                .payload
                .get("estimatedTokensFreed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let turns = evt
                .payload
                .get("turnsRemoved")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            eprint!(
                "\n\x1b[90m[ctx] 截断完成（删 {} 轮，估省 {} 令牌），正在重试\x1b[0m\n",
                turns, saved
            );
            eprint!(
                "\x1b[90m[ctx] Trim done ({} turns removed, ~{} tok saved); retrying\x1b[0m\n",
                turns, saved
            );
            let _ = io::stderr().flush();
            Ok(())
        }),
    );
    let l0 = bus.on(
        wire::WIRE_LAYER0_CONTEXT_RELEASE,
        Box::new(|evt: EventContext| {
            let p = evt
                .payload
                .get("persistTokensFreed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let ph = evt
                .payload
                .get("placeholderTokensFreed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            eprint!(
                "\n\x1b[90m[ctx] L0：大文件落盘释放 ~{} 令牌 | 历史工具结果释放 ~{} 令牌\x1b[0m\n",
                p, ph
            );
            eprint!(
                "\x1b[90m[ctx] L0: large file persist release ~{} tok | historical tool result release ~{} tok\x1b[0m\n",
                p, ph
            );
            let _ = io::stderr().flush();
            Ok(())
        }),
    );

    ChatSessionStderrListenerIds {
        metrics,
        l1_start,
        l1_end,
        l1_err,
        l2,
        l3_start,
        l3_end,
        l0,
    }
}

pub(crate) fn unregister_chat_session_stderr_listeners(
    bus: &dyn EventBus,
    ids: &ChatSessionStderrListenerIds,
) {
    bus.off(ids.metrics);
    bus.off(ids.l1_start);
    bus.off(ids.l1_end);
    bus.off(ids.l1_err);
    bus.off(ids.l2);
    bus.off(ids.l3_start);
    bus.off(ids.l3_end);
    bus.off(ids.l0);
}
