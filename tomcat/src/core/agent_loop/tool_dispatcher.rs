//! # Agent Loop 工具调度子模块
//!
//! 职责单一：接收本轮 LLM 产出的 `tool_calls` 列表，逐个派工具执行、发事件、
//! 把结果塞回 `messages`；期间处理 `block_tool_calls` 短路、steering queue
//! 打断、cancel token 抢占三种特殊时序。
//!
//! 历史：原嵌在 `run.rs:683-813` 的 130 行调度代码整块搬入本文件。聚合到单一
//! 领域文件后，T2-P0-003 的 `ToolLoopGuard`（近态同名工具计数 / 输出相似度）可
//! 就地新增判定分支；Phase 4 也得以为"blocked/steered/cancelled/completed"四
//! 态做穷举单测。
//!
//! ## 与 `tool_exec::execute_tool` 的职责分工
//!
//! - `tool_exec::execute_tool`：只**执行**单次 tool call，不发事件、不改 messages。
//! - 本模块 `run_tool_calls`：**调度**上层——发 `ToolExecutionStart/End`、
//!   `ExtensionEvent::ToolCall/ToolResult`、cancel select、push `ChatMessage::tool`、
//!   `on_message_appended` 计费、steering break。

use tokio_util::sync::CancellationToken;

use crate::core::llm::{
    ChatMessage, ChatMessageContentPart, ContinuityMetadata, ReasoningContinuation, TokenUsage,
};
use crate::core::session::manager::INTERRUPTED_TOOL_RESULT_TEXT;
use crate::infra::error::AppError;
use crate::infra::events::{AgentEvent, ContentBlock, ExtensionEvent, Message, ToolOutput};

use super::steering_injection::inject_steering_messages;
use super::tool_exec;
use super::tool_summary_update;
use super::turn_summary;
use super::types::{AgentLoop, DispatchOutcome, LoopError, ToolCallInfo};

/// `InputImage` / `InputFile` 不按原始 base64 长度计入上下文。这里的固定估算与
/// 既有 read 工具的 follow-up 记账契约保持一致。
fn follow_up_parts_chars(parts: &[ChatMessageContentPart]) -> usize {
    parts
        .iter()
        .map(|part| match part {
            ChatMessageContentPart::InputText { text } => text.chars().count(),
            ChatMessageContentPart::InputReference { reference } => {
                reference.to_prompt_text().chars().count()
            }
            ChatMessageContentPart::InputImage { .. }
            | ChatMessageContentPart::InputImageRef { .. } => 3600,
            ChatMessageContentPart::InputFile { .. } => 8000,
        })
        .sum()
}

const STEERED_TOOL_RESULT_TEXT: &str =
    "[Tool call skipped because a steering message superseded the remaining tool batch.]";

/// A provider may emit more than one tool call in an assistant message. If a steering message
/// arrives after one has completed, the remaining calls must still receive terminal results before
/// any user message is appended; otherwise the OpenAI tool-call chain is invalid.
fn append_steered_tool_result(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    tc: &ToolCallInfo,
    tool_results: &mut Vec<Message>,
) -> Result<(), LoopError> {
    let args = serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
    agent.emit_event(AgentEvent::ToolExecutionStart {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        args: args.clone(),
    });
    agent.emit_extension_event(ExtensionEvent::ToolCall {
        tool_name: tc.name.clone(),
        tool_call_id: tc.id.clone(),
        input: args.clone(),
    });
    agent.emit_extension_event(ExtensionEvent::ToolResult {
        tool_name: tc.name.clone(),
        tool_call_id: tc.id.clone(),
        input: args,
        content: vec![ContentBlock(
            serde_json::json!({ "text": STEERED_TOOL_RESULT_TEXT }),
        )],
        details: None,
        is_error: true,
    });
    agent.emit_event(AgentEvent::ToolExecutionEnd {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        result: ToolOutput(serde_json::json!(STEERED_TOOL_RESULT_TEXT)),
        display: None,
        is_error: true,
    });
    if let Some(ref mut ctx_state) = agent.context_state {
        ctx_state.on_message_appended(STEERED_TOOL_RESULT_TEXT.len());
    }
    agent
        .push_message(
            messages,
            ChatMessage::tool(&tc.id, STEERED_TOOL_RESULT_TEXT),
        )
        .map_err(LoopError::Fatal)?;
    tool_results.push(Message(
        serde_json::json!({ "content": STEERED_TOOL_RESULT_TEXT }),
    ));
    Ok(())
}

fn tool_parallelism_event(
    tool_calls_per_turn: usize,
    tool_results_completed: usize,
    steered: bool,
) -> serde_json::Value {
    serde_json::json!({
        "event": "agent.tool_parallelism",
        "tool_calls_per_turn": tool_calls_per_turn,
        "tool_results_completed": tool_results_completed,
        "steered": steered,
    })
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod parallelism_metric_tests {
    use super::tool_parallelism_event;

    #[test]
    fn parallelism_metric_test() {
        assert_eq!(
            tool_parallelism_event(4, 4, false),
            serde_json::json!({
                "event": "agent.tool_parallelism",
                "tool_calls_per_turn": 4,
                "tool_results_completed": 4,
                "steered": false,
            })
        );
    }
}

#[cfg(test)]
mod tool_result_media_tests {
    use super::super::tool_exec::media::extract_mcp_tool_result_media;
    use crate::core::llm::ChatMessageContentPart;

    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9p8qAAAAAASUVORK5CYII=";

    #[tokio::test]
    async fn mcp_image_block_becomes_input_image() {
        let result = serde_json::json!({
            "content": {
                "content": [
                    { "type": "text", "text": "captured" },
                    { "type": "image", "mimeType": "image/png", "data": TINY_PNG_B64 }
                ]
            }
        });
        let outcome = extract_mcp_tool_result_media(&result, None).await;

        assert!(outcome.model_text.contains("captured"));
        assert_eq!(outcome.follow_up_parts.len(), 1);
        assert!(matches!(
            outcome.follow_up_parts.first(),
            Some(ChatMessageContentPart::InputImage { .. })
        ));
    }

    #[tokio::test]
    async fn text_only_plugin_result_preserves_prior_empty_follow_up_parts_behavior() {
        let result = serde_json::json!({ "content": "plugin text" });
        let outcome = extract_mcp_tool_result_media(&result, None).await;

        assert_eq!(outcome.model_text, "plugin text");
        assert!(outcome.follow_up_parts.is_empty());
        assert!(!outcome.is_error);
    }

    #[tokio::test]
    async fn text_block_becomes_model_text() {
        let result = serde_json::json!({
            "content": { "content": [{ "type": "text", "text": "MCP text result" }] }
        });

        let outcome = extract_mcp_tool_result_media(&result, None).await;

        assert_eq!(outcome.model_text, "MCP text result");
        assert!(outcome.follow_up_parts.is_empty());
    }

    #[tokio::test]
    async fn unknown_block_becomes_a_text_summary() {
        let result = serde_json::json!({
            "content": { "content": [{ "type": "resource", "uri": "file:///report.txt" }] }
        });

        let outcome = extract_mcp_tool_result_media(&result, None).await;

        assert!(outcome
            .model_text
            .contains("Unsupported MCP content block 'resource'"));
        assert!(outcome.model_text.contains("file:///report.txt"));
        assert!(outcome.follow_up_parts.is_empty());
    }
}

fn emit_interrupted_tool_events(agent: &mut AgentLoop, tc: &ToolCallInfo, args: serde_json::Value) {
    agent.emit_extension_event(ExtensionEvent::ToolResult {
        tool_name: tc.name.clone(),
        tool_call_id: tc.id.clone(),
        input: args,
        content: vec![ContentBlock(
            serde_json::json!({ "text": INTERRUPTED_TOOL_RESULT_TEXT }),
        )],
        details: None,
        is_error: true,
    });
    agent.emit_event(AgentEvent::ToolExecutionEnd {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        result: ToolOutput(serde_json::json!(INTERRUPTED_TOOL_RESULT_TEXT)),
        display: None,
        is_error: true,
    });
}

/// 逐个派工具执行、发事件、塞结果回 `messages`；返回 `(tool_results, steered)`。
///
/// ## 参数语义（**严禁混淆**，混淆即 T-017 类 token 水位漂移的孪生 bug）
///
/// - `assistant_content`: 本轮 delta 累积（`outcome.content_buf`），用于
///   `on_assistant_message_appended(assistant_chars)` 的 fallback 上下文估算。
///   **不得**传跨轮累积的 `final_text`，否则历史轮 token 会被重复计入。
/// - `partial_text_for_abort`: cancel 分支构造 `make_aborted(messages, partial)`
///   时使用；此处传 `&final_text`（已累积）即可，因为 partial_text 的语义就是
///   "本轮至中断点的全部文本"（包含中断前所有 delta）。
///
/// ## 事件时序保证
///
/// 对每个 `tc` 严格按以下顺序：
///
/// 1. `ToolExecutionStart { tool_call_id, tool_name, args }`
/// 2. `ExtensionEvent::ToolCall { tool_name, tool_call_id, input: args }`
/// 3. `tool_exec::execute_tool(...)` await（被 `tokio::select!` + `biased;` 保护）
/// 4. `ExtensionEvent::ToolResult { ... }`
/// 5. `ToolExecutionEnd { ... }`
///
/// Cancel 抢占位点：
/// - 进入循环体即 `cancel.is_cancelled()` 预检 → 立即 `make_aborted`（尚未发 Start）
/// - `execute_tool` await 期间被 cancel → 发 `ToolExecutionEnd(result="[interrupted]",
///   is_error=true)` 让 UI 完成配对，再 `make_aborted`
///
/// ## `block_tool_calls` 短路
///
/// 当 `agent.block_tool_calls == true` 时（当前生产代码无处置为 true，仅为 L2
/// 压缩期预留）：所有 `tc` 都以 `"[Tool call blocked: ...]"` 文本注入 `messages`；
/// **不**发 `ToolExecutionStart/End`、**不**调用 primitive；然后清零 flag。
///
/// ## Steering break
///
/// 每个 tool 执行完毕后检查 `steering_queue`；非空则通过
/// `steered = true` 标记当前 batch；余下 tool_calls **不执行**，但先各自写入一个
/// “skipped by steering” terminal tool result。等整个 tool batch 闭合后，才统一写入
/// media follow-up 与 steering 消息。调用方随后 `continue` reasoning loop，让下一次
/// LLM 请求携带 steering 消息。
///
/// 这是不带 usage 的测试便利包装；生产路径统一调用
/// [`run_tool_calls_with_usage`]，以便将 provider usage 透传到落盘事件。
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_tool_calls(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    tool_calls: &[ToolCallInfo],
    assistant_content: &str,
    partial_text_for_abort: &str,
    finish_reason: Option<String>,
    error_message: Option<String>,
    error_code: Option<String>,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: Option<ContinuityMetadata>,
) -> Result<DispatchOutcome, LoopError> {
    run_tool_calls_with_usage(
        agent,
        messages,
        tool_calls,
        assistant_content,
        partial_text_for_abort,
        finish_reason,
        error_message,
        error_code,
        thinking_text,
        reasoning_continuation,
        continuity,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_tool_calls_with_usage(
    agent: &mut AgentLoop,
    messages: &mut Vec<ChatMessage>,
    tool_calls: &[ToolCallInfo],
    assistant_content: &str,
    partial_text_for_abort: &str,
    finish_reason: Option<String>,
    error_message: Option<String>,
    error_code: Option<String>,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: Option<ContinuityMetadata>,
    usage: Option<TokenUsage>,
) -> Result<DispatchOutcome, LoopError> {
    let persisted_arguments: Vec<String> = tool_calls
        .iter()
        .map(tool_exec::persisted_tool_call_arguments)
        .collect();

    // ── 1. 记录 assistant 消息（含 tool_calls wire payload 估算） ──
    if let Some(ref mut ctx_state) = agent.context_state {
        let assistant_chars = assistant_content.len()
            + tool_calls
                .iter()
                .zip(persisted_arguments.iter())
                .map(|(tc, persisted_args)| tc.name.len() + persisted_args.len() + tc.id.len() + 40)
                .sum::<usize>();
        // Provider completion_tokens already cover assistant text and tool-call
        // arguments. Keep these bytes in the no-usage fallback estimate only,
        // rather than adding a second post-usage delta. The small 40-char
        // per-call wire overhead follows the same path: avoiding a dedicated
        // counter is preferable to reintroducing a large duplicate estimate.
        ctx_state.on_assistant_message_appended(assistant_chars);
    }

    // ── 2. push assistant_with_tool_calls ──
    let summary_title = turn_summary::resolve_turn_summary_title(tool_calls);
    let assistant_message_id = {
        let tc_json: Vec<serde_json::Value> = tool_calls
            .iter()
            .zip(persisted_arguments.iter())
            .map(|(tc, persisted_arguments)| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": persisted_arguments
                    }
                })
            })
            .collect();
        let forced_id = agent.take_or_mint_pending_assistant_entry_id();
        Some(
            agent
                .push_message_with_forced_id(
                    messages,
                    ChatMessage::assistant_with_tool_calls(
                        if assistant_content.is_empty() {
                            None
                        } else {
                            Some(assistant_content)
                        },
                        tc_json,
                    )
                    .with_completion_metadata(finish_reason, error_message, error_code)
                    .with_summary_title(summary_title.clone())
                    .with_reasoning_state(thinking_text, reasoning_continuation, continuity)
                    .with_usage(usage),
                    &forced_id,
                )
                .map_err(LoopError::Fatal)?,
        )
    };

    let mut tool_results: Vec<Message> = Vec::new();
    let mut steered = false;
    // A user message is forbidden between tool results from one assistant tool-call batch.
    // Keep media in provider order and append one user-with-parts message only after every
    // requested call has a terminal tool result.
    let mut deferred_follow_up_parts = Vec::new();

    // ── 3. block_tool_calls 短路 ──
    if agent.block_tool_calls {
        for tc in tool_calls {
            let blocked_msg = format!(
                "[Tool call blocked: context usage too high. Tool '{}' was not executed.]",
                tc.name
            );
            if let Some(ref mut ctx_state) = agent.context_state {
                ctx_state.on_message_appended(blocked_msg.len());
            }
            agent
                .push_message(messages, ChatMessage::tool(&tc.id, &blocked_msg))
                .map_err(LoopError::Fatal)?;
            tool_results.push(Message(serde_json::json!({ "content": blocked_msg })));
        }
        agent.block_tool_calls = false;
        return Ok(DispatchOutcome {
            assistant_message_id,
            tool_results,
            steered,
        });
    }

    // ── 4. 顺序调度 ──
    let cancel: CancellationToken = agent.cancel_token.clone();
    for (tool_index, tc) in tool_calls.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(agent.make_aborted(messages, partial_text_for_abort.to_string()));
        }

        let args: serde_json::Value =
            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);

        agent.emit_event(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: args.clone(),
        });

        agent.emit_extension_event(ExtensionEvent::ToolCall {
            tool_name: tc.name.clone(),
            tool_call_id: tc.id.clone(),
            input: args.clone(),
        });

        // 工具执行本身是 await 点，用 select! 包住；`kill_on_drop(true)` 由
        // PrimitiveExecutor::execute_bash 内部兜底，保证子进程 / HTTP 连接被及时释放。
        // PR-RJ T3-c：返回值新增 `follow_up_parts`——image / pdf 等需要在
        // **下一条 user 消息** 注入 `Parts` 的场景由本调度器在 push tool 之后立刻
        // push 一条 `ChatMessage::user_with_parts(parts)` 实现。
        let outcome = if let Some(registry) = agent.tool_registry.clone() {
            match registry.get_tool(tc.name.as_str()).await {
                Ok(_) => {
                    let exec = registry.call_tool(
                        tc.name.as_str(),
                        args.clone(),
                        tool_exec::AGENT_PLUGIN_ID,
                        Some(agent.config.session_id.as_str()),
                    );
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            emit_interrupted_tool_events(agent, tc, args.clone());
                            return Err(agent.make_aborted(messages, partial_text_for_abort.to_string()));
                        }
                        result = exec => match result {
                            Ok(result) => tool_exec::media::extract_mcp_tool_result_media(
                                &result,
                                agent.config.openai_files_runtime.as_ref(),
                            ).await,
                            Err(err) => tool_exec::ToolExecOutcome::err(err.to_string()),
                        },
                    }
                }
                Err(AppError::Tool(_)) => {
                    let expose_skills_to_reviewer = agent
                        .config
                        .skill_set
                        .as_ref()
                        .is_some_and(|skill_set| !skill_set.read().visible_skills().is_empty());
                    let exec = tool_exec::execute_tool_full_with_policy_and_connectors(
                        &agent.primitive,
                        agent.config.session_id.as_str(),
                        &agent.config_backend,
                        &agent.package_install_backend,
                        &agent.bash_task_registry,
                        Some(&agent.config.read_file_state),
                        agent.config.openai_files_runtime.as_ref(),
                        agent.web_fetch_runtime.as_ref(),
                        agent.web_search_runtime.as_ref(),
                        agent.todos_runtime.as_ref(),
                        agent.config.plan_runtime.as_ref(),
                        agent.config.skill_set.as_ref(),
                        agent.connector_registry.as_ref(),
                        agent.plugin_engine_config.as_ref(),
                        agent.config.subagent_type,
                        expose_skills_to_reviewer,
                        &cancel,
                        tc,
                        Some(&agent.emitter),
                        agent.completion_routes.as_ref(),
                    );
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            emit_interrupted_tool_events(agent, tc, args.clone());
                            return Err(agent.make_aborted(messages, partial_text_for_abort.to_string()));
                        }
                        out = exec => out,
                    }
                }
                Err(err) => tool_exec::ToolExecOutcome::err(err.to_string()),
            }
        } else {
            let expose_skills_to_reviewer = agent
                .config
                .skill_set
                .as_ref()
                .is_some_and(|skill_set| !skill_set.read().visible_skills().is_empty());
            let exec = tool_exec::execute_tool_full_with_policy_and_connectors(
                &agent.primitive,
                agent.config.session_id.as_str(),
                &agent.config_backend,
                &agent.package_install_backend,
                &agent.bash_task_registry,
                Some(&agent.config.read_file_state),
                agent.config.openai_files_runtime.as_ref(),
                agent.web_fetch_runtime.as_ref(),
                agent.web_search_runtime.as_ref(),
                agent.todos_runtime.as_ref(),
                agent.config.plan_runtime.as_ref(),
                agent.config.skill_set.as_ref(),
                agent.connector_registry.as_ref(),
                agent.plugin_engine_config.as_ref(),
                agent.config.subagent_type,
                expose_skills_to_reviewer,
                &cancel,
                tc,
                Some(&agent.emitter),
                agent.completion_routes.as_ref(),
            );
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    emit_interrupted_tool_events(agent, tc, args.clone());
                    return Err(agent.make_aborted(messages, partial_text_for_abort.to_string()));
                }
                out = exec => out,
            }
        };
        let model_text = outcome.model_text;
        let is_error = outcome.is_error;
        let display = outcome.display;
        let follow_up_parts = outcome.follow_up_parts;

        agent.emit_extension_event(ExtensionEvent::ToolResult {
            tool_name: tc.name.clone(),
            tool_call_id: tc.id.clone(),
            input: args.clone(),
            content: vec![ContentBlock(
                serde_json::json!({ "text": model_text.clone() }),
            )],
            details: None,
            is_error,
        });

        agent.emit_event(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            result: ToolOutput(serde_json::json!(model_text.clone())),
            display: display.clone(),
            is_error,
        });

        // bash 卡片标题异步升级：命令执行完后由 utility 模型生成"目的短句"，
        // 经 `tool.summary_updated` 按 toolCallId 热更新前端；不阻塞后续调度。
        tool_summary_update::maybe_spawn_tool_summary_update(
            agent,
            &tc.id,
            &tc.name,
            &args,
            &model_text,
        );

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.on_message_appended(model_text.len());
        }

        agent
            .push_message(
                messages,
                ChatMessage::tool(&tc.id, &model_text).with_tool_display(display.clone()),
            )
            .map_err(LoopError::Fatal)?;
        tool_results.push(Message(
            serde_json::json!({ "content": model_text.clone() }),
        ));

        deferred_follow_up_parts.extend(follow_up_parts);

        // Steering may preempt remaining work, never the tool-result protocol. Close the
        // remaining calls with explicit skipped results, then inject steering after this loop.
        if !agent.steering_queue.lock().is_empty() {
            steered = true;
            for skipped_tc in &tool_calls[tool_index + 1..] {
                append_steered_tool_result(agent, messages, skipped_tc, &mut tool_results)?;
            }
            break;
        }
    }

    if !deferred_follow_up_parts.is_empty() {
        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.on_message_appended(follow_up_parts_chars(&deferred_follow_up_parts));
        }
        agent
            .push_message(
                messages,
                ChatMessage::user_with_parts(deferred_follow_up_parts),
            )
            .map_err(LoopError::Fatal)?;
    }

    if steered {
        let injected = inject_steering_messages(agent, messages).map_err(LoopError::Fatal)?;
        debug_assert!(
            injected,
            "steering queue was non-empty when the batch was preempted"
        );
    }

    if let Some(plan_runtime) = agent.config.plan_runtime.as_ref() {
        plan_runtime.write_transcript_custom(tool_parallelism_event(
            tool_calls.len(),
            tool_results.len(),
            steered,
        ));
    }

    Ok(DispatchOutcome {
        assistant_message_id,
        tool_results,
        steered,
    })
}
