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

use crate::core::llm::{ChatMessage, ContinuityMetadata, ReasoningContinuation};
use crate::core::session::manager::INTERRUPTED_TOOL_RESULT_TEXT;
use crate::infra::events::{AgentEvent, ContentBlock, ExtensionEvent, Message, ToolOutput};

use super::steering_injection::inject_steering_messages;
use super::tool_exec;
use super::types::{AgentLoop, DispatchOutcome, LoopError, ToolCallInfo};

/// 逐个派工具执行、发事件、塞结果回 `messages`；返回 `(tool_results, steered)`。
///
/// ## 参数语义（**严禁混淆**，混淆即 T-017 类 token 水位漂移的孪生 bug）
///
/// - `assistant_content`: 本轮 delta 累积（`outcome.content_buf`），用于
///   `on_message_appended(assistant_chars)` 的**当次**计费。
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
/// `inject_steering_messages(...)` 统一走「记账 + append/persist + push」通道，
/// 然后 `steered = true; break;`。**当次** tool 的 result 已入 messages；余下
/// tool_calls **不执行**。调用方应 `continue` reasoning loop 让下一次 LLM 请求
/// 携带 steering 消息。
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
    // ── 1. 计费：assistant 消息（含 tool_calls wire payload 估算） ──
    if let Some(ref mut ctx_state) = agent.context_state {
        let assistant_chars = assistant_content.len()
            + tool_calls
                .iter()
                .map(|tc| tc.name.len() + tc.arguments.len() + tc.id.len() + 40)
                .sum::<usize>();
        ctx_state.on_message_appended(assistant_chars);
    }

    // ── 2. push assistant_with_tool_calls ──
    {
        let tc_json: Vec<serde_json::Value> = tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments
                    }
                })
            })
            .collect();
        agent
            .push_message(
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
                .with_reasoning_state(
                    thinking_text,
                    reasoning_continuation,
                    continuity,
                ),
            )
            .map_err(LoopError::Fatal)?;
    }

    let mut tool_results: Vec<Message> = Vec::new();
    let mut steered = false;

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
            tool_results,
            steered,
        });
    }

    // ── 4. 顺序调度 ──
    let cancel: CancellationToken = agent.cancel_token.clone();
    for tc in tool_calls {
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
        let outcome = {
            let expose_skills_to_reviewer = agent
                .config
                .plan_runtime
                .as_ref()
                .is_some_and(|rt| rt.expose_skills_to_reviewer())
                && agent
                    .config
                    .skill_set
                    .as_ref()
                    .is_some_and(|skill_set| !skill_set.read().visible_skills().is_empty());
            let exec = tool_exec::execute_tool_full_with_policy(
                &agent.primitive,
                &agent.config_backend,
                &agent.bash_task_registry,
                Some(&agent.config.read_file_state),
                agent.config.openai_files_runtime.as_ref(),
                agent.web_fetch_runtime.as_ref(),
                agent.web_search_runtime.as_ref(),
                agent.config.plan_runtime.as_ref(),
                agent.config.skill_set.as_ref(),
                agent.config.subagent_type,
                agent.config.review_kind,
                expose_skills_to_reviewer,
                &cancel,
                tc,
                Some(&agent.event_bus),
                agent.completion_routes.as_ref(),
            );
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // cancel 先触发：先发 ToolExecutionEnd 让 UI 配平，再构造 Aborted。
                    agent.emit_event(AgentEvent::ToolExecutionEnd {
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        result: ToolOutput(serde_json::json!(INTERRUPTED_TOOL_RESULT_TEXT)),
                        display: None,
                        is_error: true,
                    });
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
            input: args,
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

        if let Some(ref mut ctx_state) = agent.context_state {
            ctx_state.on_message_appended(model_text.len());
        }

        agent
            .push_message(messages, ChatMessage::tool(&tc.id, &model_text))
            .map_err(LoopError::Fatal)?;
        tool_results.push(Message(
            serde_json::json!({ "content": model_text.clone() }),
        ));

        // PR-RJ T3-c：read 命中 image / pdf → tool 消息已经写了占位句，
        // 这里紧接着 push 一条 user 消息把真正的 InputImage / InputFile 注入对话。
        // 注意时序：必须**在** tool 消息之后、steering break 之前——
        // 1) tool→user 顺序固定，OpenAI Responses 才能把 part 关联到上一条 tool；
        // 2) 若 follow-up 之后被 steering break 跳过剩余 tool，下一轮 LLM
        //    仍能看到完整的「占位句 + 实物」对，不丢图。
        if !follow_up_parts.is_empty() {
            let parts_chars: usize = follow_up_parts
                .iter()
                .map(|p| match p {
                    crate::core::llm::ChatMessageContentPart::InputText { text } => text.len(),
                    crate::core::llm::ChatMessageContentPart::InputImage { .. } => 3600,
                    crate::core::llm::ChatMessageContentPart::InputFile { .. } => 8000,
                })
                .sum();
            if let Some(ref mut ctx_state) = agent.context_state {
                ctx_state.on_message_appended(parts_chars);
            }
            agent
                .push_message(messages, ChatMessage::user_with_parts(follow_up_parts))
                .map_err(LoopError::Fatal)?;
        }

        // Steering break：每个 tool 执行后检查 queue；非空则注入 + 跳过剩余。
        if inject_steering_messages(agent, messages).map_err(LoopError::Fatal)? {
            steered = true;
            break;
        }
    }

    Ok(DispatchOutcome {
        tool_results,
        steered,
    })
}
