//! `/compact`：用户主动触发一次持久化上下文压缩。

use crate::api::chat::ChatContext;
use crate::core::compaction::compact_tool_results;
use crate::core::compaction::preheat::{generate_summary_with_output_limit, SummaryRequestOptions};
use crate::core::llm::{LlmScene, PromptCacheKeyFamily};
use crate::core::session::manager::{init_context_state_with_limits, ContextState};
use crate::core::session::user_message_sidecar::ensure_user_message_sidecar_current;

use crate::AppError;

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CompactReport {
    pub before_ratio: f64,
    pub after_ratio: f64,
    pub covered_count: usize,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Compact,
        _ => ChatCommand::UsageError {
            message: "用法错误：/compact 不接受参数。".to_string(),
        },
    }
}

/// 把当前 transcript 的已折叠上下文压成一个新的 boundary summary。
///
/// 这是一个本地命令：完成后不会把 `/compact` 原文发送给主模型。摘要使用配置中的
/// Compaction scene，因此不会意外消耗用户正在使用的主对话模型配额。
pub(crate) async fn run(
    ctx: &ChatContext,
    context_state: &mut crate::core::session::manager::ContextState,
    system_text: &str,
) -> ChatCommandOutcome {
    match compact_session(ctx).await {
        Ok(report) if report.covered_count == 0 => {
            println!("当前会话没有可压缩的上下文。");
        }
        Ok(report) => {
            match load_context_state(ctx, system_text) {
                Ok(rehydrated) => *context_state = rehydrated,
                Err(error) => {
                    println!("压缩结果已保存，但内存上下文重载失败：{error}");
                    return ChatCommandOutcome::Handled;
                }
            }
            println!(
                "上下文已压缩：{:.1}% → {:.1}%（覆盖 {} 条消息）。",
                report.before_ratio * 100.0,
                report.after_ratio * 100.0,
                report.covered_count
            );
        }
        Err(error) => println!("上下文压缩失败：{error}"),
    }
    ChatCommandOutcome::Handled
}

/// 执行 `/compact` 的共享核心，供 CLI 和 serve 入口调用。
pub(crate) async fn compact_session(ctx: &ChatContext) -> Result<CompactReport, AppError> {
    let mut state = load_context_state(ctx, "")?;
    let mut messages = std::mem::take(&mut state.messages);
    if messages.is_empty() {
        return Ok(CompactReport {
            before_ratio: state.usage_ratio(),
            after_ratio: state.usage_ratio(),
            covered_count: 0,
        });
    }

    let before_ratio = state.usage_ratio();
    // 先复用自动压缩的第一档，避免把不再需要的超大工具结果再次送给摘要模型。
    // 摘要成功后会用 boundary 丢弃整个前缀，因此这里不需要把临时占位符写回 transcript。
    let history_end = messages.len();
    compact_tool_results(&mut state, &mut messages, &ctx.config.context, history_end);
    let covered_count = messages.len();
    let covered_start_id = messages.first().and_then(|message| message.msg_id.clone());
    let covered_end_id = messages.last().and_then(|message| message.msg_id.clone());
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())?;
    let compaction_call = ctx.resolve_call(LlmScene::Compaction, entry.as_ref())?;
    let control = ctx
        .session_runtime
        .plan_runtime
        .control_snapshot(Some(compaction_call.wire_model()));
    let summary = generate_summary_with_output_limit(
        &messages,
        None,
        compaction_call.provider_impl.as_ref(),
        &compaction_call.model,
        Some(&control),
        SummaryRequestOptions {
            cache_key: PromptCacheKeyFamily::Compaction
                .key_for(ctx.session_runtime.session.current_session_key())
                .as_deref(),
            resolved_output_limit: compaction_call.output_limit_for_request(None).0,
            transcript_path: Some(&state.transcript_path),
        },
    )
    .await?;

    ctx.session_runtime.session.append_compaction_boundary(
        &summary,
        covered_start_id,
        covered_end_id,
        covered_count,
    )?;
    let _ = ensure_user_message_sidecar_current(&state.transcript_path).await;

    let after_ratio = load_context_state(ctx, "")?.usage_ratio();
    Ok(CompactReport {
        before_ratio,
        after_ratio,
        covered_count,
    })
}

fn load_context_state(ctx: &ChatContext, system_text: &str) -> Result<ContextState, AppError> {
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())?;
    let main_call = ctx.resolve_call(LlmScene::Main, entry.as_ref())?;
    init_context_state_with_limits(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
        &main_call.limits,
    )
}
