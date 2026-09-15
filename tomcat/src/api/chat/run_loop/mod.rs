use std::io::{self, Write as IoWrite};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::{check_before_request, BoundaryEnv};
use crate::core::llm::resolver::validate_capabilities;
use crate::core::llm::{
    degrade_unsupported_multimodal, ChatMessage, LlmScene, PromptCacheKeyFamily,
    SystemPromptSnapshot, ToolSurface,
};
use crate::core::session::manager::{
    estimate_msg_chars, init_context_state, init_context_state_with_limits, turn_count,
};
use crate::infra::error::AppError;
use crate::infra::events::AgentEvent;
use crate::infra::ScopedEventEmitter;
use crate::{AgentLoop, AgentLoopConfig, CheckpointKind};

use crate::core::plan_runtime;

use super::super::render::MarkdownRenderer;
use super::commands::{dispatch_chat_command, parse_chat_command, ChatCommandOutcome};
use super::context::ChatContext;
use super::prompt::{agent_prompt_for_mode, user_prompt_for_mode_with_model};
use super::{cli_turn_renderer, events, preflight};

mod background;
mod cleanup;
mod input;
mod persist;
mod rehydrate;
mod session_title;
mod thinking_persist;
mod workspace_state;

pub(crate) use self::background::spawn_completion_subscriber;
use self::cleanup::ensure_session;
pub(crate) use self::persist::drain_checkpoint_record_tasks;
use self::persist::push_turn_message;
pub(crate) use self::rehydrate::has_resumable_tail_ask_question;
use self::rehydrate::{make_fallback_context_state, nonfatal_error_hint};
pub(crate) use self::rehydrate::{recover_context_state_after_failed_turn, render_error_message};
use self::session_title::{maybe_emit_rule_session_title, maybe_spawn_semantic_session_title};
pub(crate) use self::workspace_state::runtime_tail_provider;

const CHECKPOINT_SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

/// A resumed process may send a zero-input request only after it has repaired a
/// persisted `ask_question` tail into a completed tool result. An assistant-ended
/// transcript has no new causal input, so replaying it would violate the request
/// protocol and needlessly wake the model.
pub(crate) fn should_auto_drain_resume(resume_requested: bool, has_pending_question: bool) -> bool {
    resume_requested && has_pending_question
}

#[cfg(test)]
pub(crate) use self::cleanup::cleanup_plugin_sessions_on_session_end;
#[cfg(test)]
pub(crate) use self::workspace_state::render_plan_runtime_reminder;

#[cfg(test)]
pub(crate) use self::cleanup::cleanup_openai_files_on_session_end;
#[cfg(test)]
pub(crate) use self::persist::{
    build_turn_checkpoint_request, checkpoint_warn_line, persist_turn_result,
    schedule_checkpoint_prune,
};
#[cfg(test)]
pub(crate) use self::rehydrate::{
    is_append_message_chain_invariant, is_fatal_error,
    try_rehydrate_context_state_after_append_invariant,
};
#[cfg(test)]
pub(crate) use self::thinking_persist::{
    register_thinking_persist_listeners, unregister_thinking_persist_listeners,
};

pub(crate) async fn build_tool_definitions(ctx: &ChatContext) -> Vec<serde_json::Value> {
    observe_tool_surface(ctx)
        .await
        .function_definitions()
        .to_vec()
}

async fn observe_tool_surface(ctx: &ChatContext) -> ToolSurface {
    ctx.spawn_connector_startup_if_needed().await;
    let skill_set = ctx.skill_set_snapshot();
    let allow_load_skill = ctx.config.skills.enabled && !skill_set.visible_skills().is_empty();
    let plugin_tools = match ctx.global_services.tool_registry.list_tools(None).await {
        Ok(plugin_tools) => plugin_tools,
        Err(err) => {
            warn!(error = %err, "list plugin tools for prompt surface failed; continuing with builtins only");
            Vec::new()
        }
    };
    let plugin_signature = plugin_tools
        .iter()
        .map(|tool| format!("{}:{}", tool.plugin_id, tool.name))
        .collect::<Vec<_>>()
        .join(",");
    let skill_signature = skill_set
        .visible_skills()
        .into_iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    info!(
        target: "tomcat_chat_diag",
        phase = "prompt_runtime_snapshot",
        plugin_tools = %plugin_signature,
        visible_skills = %skill_signature,
    );
    let allow_connector_tools = ctx
        .global_services
        .connector_registry
        .as_ref()
        .is_some_and(|connectors| connectors.has_configured_mcp_servers());
    let allow_package_install =
        ctx.session_runtime.plan_runtime.mode() != crate::core::session::manager::AgentMode::Plan;
    ToolSurface::from_plugin_tools_with_runtime_policies(
        allow_load_skill,
        allow_connector_tools,
        allow_package_install,
        &plugin_tools,
    )
}

fn workspace_context(ctx: &ChatContext) -> crate::core::llm::system_prompt::WorkspaceContext {
    crate::core::llm::system_prompt::WorkspaceContext {
        agent_workspace_dir: ctx
            .scope_services
            .agent_workspace_dir
            .to_string_lossy()
            .to_string(),
        agent_definition_dir: ctx
            .scope_services
            .agent_definition_dir
            .to_string_lossy()
            .to_string(),
        agent_plans_dir: plan_runtime::file_store::plans_dir()
            .map(|path| crate::infra::platform::format_home_path(path.as_path()))
            .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
        agent_trail_dir: ctx
            .scope_services
            .agent_trail_dir
            .to_string_lossy()
            .to_string(),
        tool_lines: None,
    }
}

pub(crate) async fn build_prompt_snapshot(
    ctx: &ChatContext,
    context_budget_chars: usize,
) -> SystemPromptSnapshot {
    let skill_set = ctx.skill_set_snapshot();
    let tool_surface = observe_tool_surface(ctx).await;
    SystemPromptSnapshot::new(
        &workspace_context(ctx),
        &tool_surface,
        Some(&skill_set),
        Some(&ctx.config.skills),
        context_budget_chars,
    )
}

pub(crate) async fn refresh_prompt_snapshot(
    ctx: &ChatContext,
    context_budget_chars: usize,
    snapshot: &mut SystemPromptSnapshot,
) -> bool {
    let skill_set = ctx.skill_set_snapshot();
    let tool_surface = observe_tool_surface(ctx).await;
    snapshot.refresh(
        &workspace_context(ctx),
        &tool_surface,
        Some(&skill_set),
        Some(&ctx.config.skills),
        context_budget_chars,
    )
}

const AUTO_TURN_BUDGET: u32 = 8;

fn current_user_prompt(ctx: &ChatContext) -> String {
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
        .ok()
        .flatten();
    let active_plan = ctx.session_runtime.plan_runtime.active_plan();
    user_prompt_for_mode_with_model(
        ctx.session_runtime.plan_runtime.mode(),
        active_plan.as_ref(),
        &ctx.effective_model(entry.as_ref()),
    )
}

fn drain_follow_up_messages(ctx: &ChatContext) -> Vec<ChatMessage> {
    {
        let mut queue = ctx.session_runtime.follow_up_queue.lock();
        if queue.is_empty() {
            Vec::new()
        } else {
            queue.drain(..).collect::<Vec<_>>()
        }
    }
}

fn compose_planned_turn_messages_from_message(
    input_message: Option<ChatMessage>,
    drained_follow_ups: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    // Synthetic background completions are runtime signals, not a fresher user ask.
    // Keep any real typed prompt last so the next request preserves user intent.
    let mut planned = drained_follow_ups;
    if let Some(message) = input_message {
        planned.push(message);
    }
    planned
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn compose_planned_turn_messages(
    input: &str,
    drained_follow_ups: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    compose_planned_turn_messages_from_message(
        if input.is_empty() {
            None
        } else {
            Some(ChatMessage::user(input))
        },
        drained_follow_ups,
    )
}

fn drain_planned_turn_messages(
    ctx: &ChatContext,
    input_message: Option<ChatMessage>,
) -> Vec<ChatMessage> {
    compose_planned_turn_messages_from_message(input_message, drain_follow_up_messages(ctx))
}

fn append_planned_messages_with_rehydrate_retry(
    ctx: &ChatContext,
    system_text: &str,
    context_config: &crate::infra::ContextConfig,
    planned_messages: &[ChatMessage],
    context_state: &mut crate::core::ContextState,
    messages: &mut Vec<ChatMessage>,
) -> Result<Option<String>, AppError> {
    let mut next_pending_idx = 0usize;
    let mut retried_after_rehydrate = false;
    let mut first_appended_id = None;
    loop {
        let messages_before_append = messages.len();
        let mut append_error = None;
        for message in planned_messages.iter().skip(next_pending_idx) {
            if let Err(err) = push_turn_message(
                messages,
                &ctx.session_runtime.message_append_sink,
                message.clone(),
            ) {
                append_error = Some(err);
                break;
            }
            context_state.on_message_appended(estimate_msg_chars(message));
            if first_appended_id.is_none() {
                first_appended_id = messages.last().and_then(|message| message.msg_id.clone());
            }
        }

        if let Some(err) = append_error {
            if !retried_after_rehydrate
                && rehydrate::try_rehydrate_context_state_after_append_invariant(
                    ctx,
                    context_config,
                    system_text,
                    &err,
                    context_state,
                )
            {
                next_pending_idx += messages.len().saturating_sub(messages_before_append);
                *messages = std::mem::take(&mut context_state.messages);
                retried_after_rehydrate = true;
                continue;
            }
            return Err(err);
        }

        return Ok(first_appended_id);
    }
}

pub async fn chat_loop(ctx: &ChatContext, resume: bool) -> Result<(), AppError> {
    ensure_session(ctx)?;
    if ctx.config.skills.enabled {
        ctx.spawn_skill_discovery_if_needed().await;
    }

    // 启动像素风吉祥物 Splash（仅 TTY 时绘制；文本 banner 仍由下方 println 负责）。
    crate::api::cli::splash::render_mascot(&ctx.config.splash);

    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());

    if resume {
        println!(
            "恢复会话: {}",
            ctx.session_runtime.session.current_session_key()
        );
    }
    println!("tomcat 对话模式 (模型: {})", model);
    println!("输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。");
    println!("输入 /help 查看命令列表。\n");

    let mut rl = input::make_readline_editor()?;

    #[cfg(target_os = "macos")]
    // macOS 中文输入法在 `ExternalPrinter` 激活的输入路径下更容易出现回显异常。
    let search_tools_printer: Option<
        Arc<std::sync::Mutex<Box<dyn rustyline::ExternalPrinter + Send>>>,
    > = None;
    #[cfg(not(target_os = "macos"))]
    let search_tools_printer = rl.create_external_printer().ok().map(|printer| {
        Arc::new(std::sync::Mutex::new(
            Box::new(printer) as Box<dyn rustyline::ExternalPrinter + Send>
        ))
    });

    let context_config = &ctx.config.context;
    if ctx.config.skills.enabled {
        let _ = ctx.await_skill_discovery().await;
    }
    let initial_main_call = ctx.resolve_call(LlmScene::Main, entry.as_ref())?;
    let context_budget_chars = crate::infra::config::compute_context_budget_chars_from_tokens(
        initial_main_call.limits.input_budget_tokens,
    );
    let mut prompt_snapshot = build_prompt_snapshot(ctx, context_budget_chars).await;
    persist::schedule_checkpoint_prune(ctx);
    // ResumePlan 目前恒为 Continue；保留 hook，未来若恢复逻辑需要 tail，可在这里恢复
    // `read_entries_tail(..., 64)` 预读。
    let _ = crate::core::compute_resume_plan(entry.as_ref(), &[]);
    let mut context_state = init_context_state_with_limits(
        &ctx.session_runtime.session,
        context_config,
        prompt_snapshot.system_text(),
        &initial_main_call.limits,
    )?;
    if let Err(err) = ctx
        .session_runtime
        .plan_runtime
        .attach_from_resume_state(context_state.resume_control.clone())
    {
        tracing::warn!(error = %err, "plan_runtime attach_from_resume_state failed; continuing with Chat mode");
    }
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()?
        .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
    let root_event_emitter = Arc::new(ScopedEventEmitter::new(
        ctx.global_services.event_bus.clone(),
        session_id.clone(),
    ));
    let session_stderr_ids = events::stderr::register_chat_session_stderr_listeners(
        &*ctx.global_services.event_bus,
        search_tools_printer,
        Some(session_id.as_str()),
        ctx.config.preflight.show_search_tools_ui,
        ctx.config.preflight.show_git_ui,
    );
    preflight::start_search_tools_preflight(&ctx.config, Arc::clone(&root_event_emitter));
    preflight::start_git_preflight(
        &ctx.config,
        Arc::clone(&root_event_emitter),
        ctx.scope_services.checkpoint_switcher.clone(),
    );

    if ctx
        .session_runtime
        .completion_subscriber_handle
        .lock()
        .is_none()
    {
        let handle = spawn_completion_subscriber(ctx);
        *ctx.session_runtime.completion_subscriber_handle.lock() = Some(handle);
    }

    let mut auto_turn_count: u32 = 0;
    // `--resume` must not replay an assistant-ended transcript. Its only zero-input
    // responsibility is resolving a persisted ask_question tail into a tool result,
    // which then becomes the causal input of the resumed request.
    let has_pending_resume_question = if resume {
        match has_resumable_tail_ask_question(&ctx.session_runtime.session) {
            Ok(pending) => pending,
            Err(error) => {
                warn!(error = %error, "failed to inspect pending ask_question state for --resume");
                false
            }
        }
    } else {
        false
    };
    let mut resume_without_input = should_auto_drain_resume(resume, has_pending_resume_question);
    let mut fatal_error: Option<AppError> = None;

    let exit_reason = loop {
        if ctx
            .session_runtime
            .hard_exit_requested
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            context_state.preheat.abort();
            break "hard_interrupt_exit";
        }

        let queued_follow_ups = !ctx.session_runtime.follow_up_queue.lock().is_empty();
        let auto_drain =
            resume_without_input || (queued_follow_ups && auto_turn_count < AUTO_TURN_BUDGET);
        if !auto_drain {
            if auto_turn_count >= AUTO_TURN_BUDGET && queued_follow_ups {
                eprintln!(
                    "\n[bg] auto-turn budget exhausted ({}); falling back to user input.",
                    AUTO_TURN_BUDGET
                );
            }
            auto_turn_count = 0;
        }

        let input = if auto_drain {
            String::new()
        } else {
            let raw = match rl.readline(&current_user_prompt(ctx)) {
                Ok(line) => line,
                Err(rustyline::error::ReadlineError::Eof) => {
                    println!("\n再见！");
                    context_state.preheat.abort();
                    break "chat_eof_exit";
                }
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    if ctx
                        .session_runtime
                        .hard_exit_requested
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        context_state.preheat.abort();
                        break "hard_interrupt_exit";
                    }
                    continue;
                }
                Err(rustyline::error::ReadlineError::Signal(rustyline::error::Signal::Resize)) => {
                    continue;
                }
                Err(error) => {
                    eprintln!("输入错误: {}", error);
                    context_state.preheat.abort();
                    break "chat_input_error_exit";
                }
            };
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                continue;
            } else {
                let (parsed, history_line) = match dispatch_chat_command(
                    ctx,
                    parse_chat_command(&trimmed),
                    &mut rl,
                    &mut context_state,
                    prompt_snapshot.system_text(),
                )
                .await
                {
                    ChatCommandOutcome::Continue {
                        line,
                        echo_user,
                        history_line,
                    } => {
                        if echo_user {
                            print!("{}{}", current_user_prompt(ctx), line);
                            println!();
                            io::stdout().flush().map_err(AppError::Io)?;
                        }
                        (line, history_line)
                    }
                    ChatCommandOutcome::Handled => continue,
                };
                let history_line = history_line.unwrap_or_else(|| parsed.clone());
                let _ = rl.add_history_entry(&history_line);
                parsed
            }
        };

        if input.is_empty() {
            auto_turn_count += 1;
            resume_without_input = false;
        } else {
            auto_turn_count = 0;
        }

        // The prior turn's snapshot may still be running while the user types.
        // Before this input can execute tools and mutate the worktree, finish it
        // so checkpoint order matches turn order without delaying agent_idle.
        drain_checkpoint_record_tasks(ctx, CHECKPOINT_SHUTDOWN_FLUSH_TIMEOUT).await;

        let turn_token = {
            let mut guard = ctx.session_runtime.cancel_token.lock();
            *guard = CancellationToken::new();
            guard.clone()
        };
        ctx.agent_registry
            .rearm_root(
                &ctx.session_runtime
                    .session
                    .current_session_id()?
                    .ok_or_else(|| AppError::Config("无当前会话".to_string()))?,
                turn_token.child_token(),
            )
            .map_err(|error| {
                AppError::Config(format!("agent_registry root rearm 失败: {error}"))
            })?;

        let outcome = run_chat_turn_with_snapshot(
            ctx,
            &input,
            &mut prompt_snapshot,
            &mut context_state,
            turn_token,
        )
        .await?;

        match outcome {
            AgentRunOutcome::Completed(_) => {}
            AgentRunOutcome::Interrupted(_) => {
                if let Err(error) = ctx.session_runtime.plan_runtime.park_executing_plan() {
                    tracing::warn!(
                        error = %error,
                        "failed to park executing plan after user interruption"
                    );
                }
                if ctx
                    .session_runtime
                    .hard_exit_requested
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    eprintln!("\n^C 已中断，正在退出...");
                    context_state.preheat.abort();
                    break "hard_interrupt_exit";
                }
                eprintln!("\n^C 已中断（partial 已保存）");
            }
            AgentRunOutcome::Failed(error) => {
                let fatal = rehydrate::is_fatal_error(&error);
                eprintln!("\n[错误] {}", error);
                if fatal {
                    eprintln!("(致命错误，退出对话)");
                    context_state.preheat.abort();
                    fatal_error = Some(error);
                    break "chat_fatal_exit";
                }
                eprintln!("{}", nonfatal_error_hint(&error));
                continue;
            }
        }

        println!();
    };

    drain_checkpoint_record_tasks(ctx, CHECKPOINT_SHUTDOWN_FLUSH_TIMEOUT).await;
    cleanup::cleanup_chat_session_resources(ctx, exit_reason).await;
    events::stderr::unregister_chat_session_stderr_listeners(
        &*ctx.global_services.event_bus,
        &session_stderr_ids,
    );
    if let Some(error) = fatal_error {
        return Err(error);
    }
    Ok(())
}

pub async fn run_chat_turn(
    ctx: &ChatContext,
    input: &str,
    system_text: &str,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    let input_message = if input.is_empty() {
        None
    } else {
        Some(ChatMessage::user(input))
    };
    run_chat_turn_with_message(ctx, input_message, system_text, context_state, turn_token).await
}

pub(crate) async fn run_chat_turn_with_snapshot(
    ctx: &ChatContext,
    input: &str,
    prompt_snapshot: &mut SystemPromptSnapshot,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    let input_message = (!input.is_empty()).then(|| ChatMessage::user(input));
    run_chat_turn_with_message_and_snapshot(
        ctx,
        input_message,
        prompt_snapshot,
        context_state,
        turn_token,
    )
    .await
}

pub(crate) async fn run_chat_turn_with_message_and_snapshot(
    ctx: &ChatContext,
    input_message: Option<ChatMessage>,
    prompt_snapshot: &mut SystemPromptSnapshot,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    ctx.refresh_resource_inventory_before_turn().await?;
    let previous_system_len = prompt_snapshot.system_text().len();
    if refresh_prompt_snapshot(ctx, context_state.context_budget_chars, prompt_snapshot).await {
        context_state
            .replace_system_prompt_chars(previous_system_len, prompt_snapshot.system_text().len());
    }
    let system_text = prompt_snapshot.system_text().to_string();
    run_chat_turn_with_message_and_tool_definitions(
        ctx,
        input_message,
        &system_text,
        prompt_snapshot.tool_definitions().to_vec(),
        context_state,
        turn_token,
    )
    .await
}

pub async fn run_chat_turn_with_message(
    ctx: &ChatContext,
    input_message: Option<ChatMessage>,
    system_text: &str,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    ctx.refresh_resource_inventory_before_turn().await?;
    let tool_definitions = build_tool_definitions(ctx).await;
    run_chat_turn_with_message_and_tool_definitions(
        ctx,
        input_message,
        system_text,
        tool_definitions,
        context_state,
        turn_token,
    )
    .await
}

async fn run_chat_turn_with_message_and_tool_definitions(
    ctx: &ChatContext,
    input_message: Option<ChatMessage>,
    system_text: &str,
    tool_definitions: Vec<serde_json::Value>,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    // A root user reply is the explicit boundary that may reopen an execution
    // paused on an unresolved P0. Never infer acknowledgement from model text.
    if input_message.is_some() {
        if let Err(error) = ctx
            .session_runtime
            .plan_runtime
            .refresh_code_review_budget_after_user_message()
        {
            tracing::warn!(error = %error, "failed to refresh code-review budget after user input");
        }
    }
    ctx.session_runtime
        .plan_runtime
        .attach_cancel_hook(turn_token.clone());
    let recovered_pending_question = if input_message.is_none() {
        rehydrate::resume_tail_ask_questions(ctx).await?
    } else {
        rehydrate::skip_tail_ask_questions_for_new_input(ctx)?
    };
    if recovered_pending_question {
        // The recovered or skipped tool result is already durable. Reload instead of manually
        // splicing it into the live context, so supersede+append and the normal append path
        // share one source of truth.
        *context_state = init_context_state(
            &ctx.session_runtime.session,
            &ctx.config.context,
            system_text,
        )?;
    }
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()?
        .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
    let root_event_emitter = Arc::new(ScopedEventEmitter::new(
        ctx.global_services.event_bus.clone(),
        session_id.clone(),
    ));

    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())?;
    let main_call = ctx.resolve_call(LlmScene::Main, entry.as_ref())?;
    let compaction_call = ctx.resolve_call(LlmScene::Compaction, entry.as_ref())?;
    // A session can switch models between turns. Recompute the input budget
    // before appending/sending this turn so compaction observes the selected
    // model's actual limits rather than the global fallback.
    context_state.apply_limits(&main_call.limits);
    // 会话标题优先走 title_model；若其未配置/未解析，则降级到主模型。
    // turn 折叠标题仍维持 title_call 语义，不在此处一起降级。
    let title_call = ctx.resolve_call(LlmScene::Title, entry.as_ref()).ok();
    let compaction_provider = compaction_call.provider_impl.clone();
    let compaction_output_limit = compaction_call.output_limit_for_request(None).0;
    let title_provider = title_call.as_ref().map(|c| c.provider_impl.clone());
    let (session_title_provider, session_title_model, session_title_output_limit) = title_call
        .as_ref()
        .map(|c| {
            (
                c.provider_impl.clone(),
                c.model.clone(),
                c.output_limit_for_request(None).0,
            )
        })
        .unwrap_or_else(|| {
            (
                main_call.provider_impl.clone(),
                main_call.model.clone(),
                main_call.output_limit_for_request(None).0,
            )
        });
    let title_model = title_call
        .as_ref()
        .map(|c| c.model.clone())
        .unwrap_or_default();
    let title_output_limit = title_call
        .as_ref()
        .and_then(|c| c.output_limit_for_request(None).0)
        .or_else(|| main_call.output_limit_for_request(None).0);
    let thinking_model_id = ctx.effective_model(entry.as_ref());
    // 让 reviewer / verifier 在下一次派发时跟上会话当前的模型。
    //
    // 这里记的必须是 catalog id（`fcodex/claude-opus-4-8`），不是 `main_call.model`
    // 那个发给 provider 的线上模型名（`claude-opus-4-8`）。真实解析点在
    // `plan_runtime/prod_reviewer.rs::resolve_subagent_runtime()` 与
    // `plan_runtime/verify.rs::dispatch()`：子 Agent 会拿这个 catalog id 回 catalog 重新
    // resolve 出成对的 `{provider_impl, wire_model}`。上一轮 404 事故就是因为只带了线上名，
    // 命中了另一个 provider 的同名条目；这条注释现在对应的是已落地的不变量，而不再只是意图。
    ctx.session_runtime
        .plan_runtime
        .set_session_model(&thinking_model_id);
    let thinking_level = Some(ctx.resolve_thinking_level(&thinking_model_id));
    let mut context_config = ctx.config.context.clone();
    context_config.compaction_model = compaction_call.model.clone();

    let planned_messages = drain_planned_turn_messages(ctx, input_message);
    if let Err(error) = ctx.global_services.model_catalog.with_catalog(|catalog| {
        validate_capabilities(
            catalog,
            &ctx.config.llm.default_model,
            LlmScene::Main,
            &main_call.model,
            &main_call.capabilities,
            &planned_messages,
        )
    }) {
        let _ = ctx
            .session_runtime
            .session
            .persist_context_observability(context_state);
        let error_message = error.to_string();
        let _ = root_event_emitter.emit(AgentEvent::AgentStart);
        let _ = root_event_emitter.emit(AgentEvent::AgentEnd {
            messages: Vec::new(),
            error: Some(error_message),
        });
        return Ok(AgentRunOutcome::Failed(error));
    }
    let mut messages = std::mem::take(&mut context_state.messages);
    let first_appended_id = append_planned_messages_with_rehydrate_retry(
        ctx,
        system_text,
        &context_config,
        &planned_messages,
        context_state,
        &mut messages,
    )?;
    maybe_emit_rule_session_title(
        &ctx.session_runtime.session,
        &planned_messages,
        &root_event_emitter,
    );
    maybe_spawn_semantic_session_title(
        &ctx.session_runtime.session,
        &planned_messages,
        session_title_provider,
        session_title_model,
        session_title_output_limit,
        root_event_emitter.clone(),
        session_id.clone(),
    );
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_user_append",
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count,
        turns = turn_count(&messages)
    );

    context_state.preheat.try_restart_if_pending(
        context_state.usage_ratio(),
        &messages,
        &context_state.transcript_path,
        PromptCacheKeyFamily::Compaction.key_for(&session_id),
        compaction_provider.clone(),
        compaction_output_limit,
        &context_config,
        Arc::clone(&root_event_emitter),
        Some(
            ctx.session_runtime
                .plan_runtime
                .control_snapshot(Some(main_call.wire_model())),
        ),
    );
    let boundary_env = BoundaryEnv {
        config: &context_config,
        work_dir: ctx.scope_services.agent_trail_dir.as_path(),
        session_id: &session_id,
        read_file_state: ctx.session_runtime.read_file_state.as_ref(),
    };
    let mut turn_start = first_appended_id
        .as_deref()
        .and_then(|id| {
            messages
                .iter()
                .position(|message| message.msg_id.as_deref() == Some(id))
        })
        .unwrap_or(messages.len());
    let _boundary_applied = check_before_request(
        context_state,
        &mut messages,
        &root_event_emitter,
        &boundary_env,
        &mut turn_start,
    )
    .await;
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_timing2_check",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = false,
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count
    );
    if let std::borrow::Cow::Owned(degraded) =
        degrade_unsupported_multimodal(&messages, &main_call.capabilities)
    {
        messages = degraded;
    }

    let render_cli_output = !ctx.session_runtime.suppress_cli_output;
    let renderer = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let unattended_execution = ctx
        .session_runtime
        .plan_runtime
        .executing_plan_id()
        .is_some();
    let config = AgentLoopConfig {
        // 长时间无人 PLAN/EXEC 不应因一次 Connect/429 直接把工作交还用户；
        // 普通交互仍尊重用户配置的短重试预算。
        max_attempts: if unattended_execution {
            ctx.config.llm.agent_max_attempts.max(10)
        } else {
            ctx.config.llm.agent_max_attempts
        },
        system_prompt: Some(system_text.to_string()),
        unattended_retry: false,
        max_tool_rounds: usize::MAX,
        retry_base_delay_ms: ctx.config.llm.agent_retry_base_delay_ms,
        thinking_level,
        session_id: session_id.clone(),
        tool_definitions,
        context_config: context_config.clone(),
        compaction_provider: Some(compaction_provider.clone()),
        compaction_output_limit,
        title_provider: title_provider.clone(),
        title_model,
        title_output_limit,
        agent_trail_dir: ctx
            .scope_services
            .agent_trail_dir
            .to_string_lossy()
            .to_string(),
        read_file_state: ctx.session_runtime.read_file_state.clone(),
        openai_files_runtime: ctx.openai_files_runtime_for(&main_call),
        checkpoint_store: ctx.scope_services.checkpoint_store.clone(),
        message_append_sink: Some(ctx.session_runtime.message_append_sink.clone()),
        parent_session_id: None,
        spawn_depth: 0,
        subagent_type: crate::core::agent_loop::SubagentType::User,
        plan_runtime: Some(ctx.session_runtime.plan_runtime.clone()),
        skill_set: Some(ctx.scope_services.skill_set.clone()),
        ephemeral_tail_provider: Some(runtime_tail_provider(ctx)),
    };
    let mut agent_loop = AgentLoop::new(
        main_call,
        ctx.global_services.primitive.clone(),
        ctx.global_services.event_bus.clone(),
        config,
        turn_token,
    );
    agent_loop = agent_loop.with_tool_registry(ctx.global_services.tool_registry.clone());
    if let Some(connectors) = ctx.global_services.connector_registry.clone() {
        agent_loop = agent_loop.with_connector_registry(connectors);
    }
    agent_loop = agent_loop.with_plugin_engine_config(crate::ext::PluginEngineConfig {
        quickjs_heap_mb: ctx.config.plugin.js_heap_mb,
        call_timeout_ms: ctx.config.plugin.call_timeout_ms,
        interrupt_budget: ctx.config.plugin.interrupt_budget,
        idle_ttl_ms: ctx.config.plugin.idle_ttl_ms,
    });
    if let Some(backend) = ctx.global_services.config_backend.clone() {
        agent_loop = agent_loop.with_config_backend(backend);
    }
    if let Some(backend) = ctx.global_services.package_install_backend.clone() {
        agent_loop = agent_loop.with_package_install_backend(backend);
    }
    agent_loop = agent_loop.with_bash_task_registry(ctx.session_runtime.bash_task_registry.clone());
    agent_loop = agent_loop.with_web_fetch_runtime(ctx.global_services.web_fetch_runtime.clone());
    agent_loop = agent_loop.with_web_search_runtime(ctx.global_services.web_search_runtime.clone());
    agent_loop = agent_loop.with_todos_runtime(ctx.session_runtime.todos_runtime.clone());
    agent_loop = agent_loop.with_session_manager(ctx.session_runtime.session.clone());
    agent_loop =
        agent_loop.with_shared_follow_up_queue(ctx.session_runtime.follow_up_queue.clone());
    agent_loop = agent_loop.with_shared_steering_queue(ctx.session_runtime.steering_queue.clone());
    agent_loop = agent_loop.with_completion_routes(ctx.session_runtime.completion_routes.clone());

    let previous_state = std::mem::replace(
        context_state,
        make_fallback_context_state(ctx, system_text, &context_config),
    );
    agent_loop.set_context_state(Some(previous_state));

    let listener_ids = if render_cli_output {
        let cli_turn_renderer = cli_turn_renderer::CliTurnRenderer::new(
            Arc::clone(&renderer),
            Arc::clone(&ctx.session_runtime.thinking_display),
            Some(session_id.clone()),
            ctx.config.llm.thinking.print_to_stderr,
            ctx.config.llm.tool_cli_verbosity,
        );
        Some(cli_turn_renderer.register(&*ctx.global_services.event_bus))
    } else {
        None
    };
    let thinking_persist_listener_ids = if render_cli_output && ctx.config.llm.thinking.persist {
        let transcript_path = ctx
            .session_runtime
            .session
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        Some(thinking_persist::register_thinking_persist_listeners(
            &*ctx.global_services.event_bus,
            transcript_path,
        ))
    } else {
        None
    };

    if render_cli_output {
        let active_plan = ctx.session_runtime.plan_runtime.active_plan();
        print!(
            "\n{}",
            agent_prompt_for_mode(
                &ctx.config.agent.id,
                ctx.session_runtime.plan_runtime.mode(),
                active_plan.as_ref(),
            )
        );
        io::stdout().flush().map_err(AppError::Io)?;
    }

    info!(
        target: "tomcat_chat_diag",
        phase = "chat_before_agent_run",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = true
    );
    let outcome = agent_loop.run(messages).await;
    if let Some(ids) = &thinking_persist_listener_ids {
        thinking_persist::unregister_thinking_persist_listeners(
            &*ctx.global_services.event_bus,
            ids,
        );
    }
    if let Some(listener_ids) = &listener_ids {
        cli_turn_renderer::CliTurnRenderer::unregister(
            &*ctx.global_services.event_bus,
            listener_ids,
        );
    }

    if render_cli_output {
        if let Some(remaining) = renderer.lock().flush() {
            print!("{}", remaining);
            let _ = io::stdout().flush();
        }
    }

    let mut next_state = agent_loop.take_context_state().unwrap_or_else(|| {
        init_context_state(&ctx.session_runtime.session, &context_config, system_text)
            .unwrap_or_else(|_| make_fallback_context_state(ctx, system_text, &context_config))
    });
    if let AgentRunOutcome::Failed(error) = &outcome {
        let _ = rehydrate::recover_context_state_after_failed_turn(
            ctx,
            &context_config,
            system_text,
            error,
            &mut next_state,
        );
    }
    *context_state = next_state;

    match &outcome {
        AgentRunOutcome::Completed(result) => {
            persist::persist_turn_result(
                ctx,
                context_state,
                result.new_messages.clone(),
                CheckpointKind::TurnEnd,
            )?;
        }
        AgentRunOutcome::Interrupted(result) => {
            persist::persist_turn_result(
                ctx,
                context_state,
                result.new_messages.clone(),
                CheckpointKind::Interrupt,
            )?;
        }
        AgentRunOutcome::Failed(_) => {
            let _ = ctx
                .session_runtime
                .session
                .persist_context_observability(context_state);
        }
    }

    Ok(outcome)
}
