use std::io::{self, Write as IoWrite};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::llm::resolver::validate_capabilities;
use crate::core::llm::{degrade_unsupported_multimodal, ChatMessage, LlmScene};
use crate::core::session::manager::{
    build_context_from_state, estimate_msg_chars, init_context_state,
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

use self::background::spawn_completion_subscriber;
use self::cleanup::ensure_session;
use self::persist::push_turn_message;
use self::rehydrate::{make_fallback_context_state, nonfatal_error_hint};
pub(crate) use self::rehydrate::{recover_context_state_after_failed_turn, render_error_message};
use self::session_title::{maybe_emit_rule_session_title, maybe_spawn_semantic_session_title};
use self::workspace_state::compute_workspace_state;

#[cfg(test)]
pub(crate) use self::cleanup::cleanup_plugin_sessions_on_session_end;

#[cfg(test)]
pub(crate) use self::cleanup::cleanup_openai_files_on_session_end;
#[cfg(test)]
pub(crate) use self::persist::{
    build_turn_checkpoint_request, checkpoint_warn_line, persist_turn_result,
    schedule_checkpoint_prune,
};
#[cfg(test)]
pub(crate) use self::rehydrate::{
    is_append_message_chain_invariant, is_fatal_error, try_rehydrate_context_state_after_append_invariant,
};
#[cfg(test)]
pub(crate) use self::thinking_persist::{
    register_thinking_persist_listeners, unregister_thinking_persist_listeners,
};

async fn build_tool_definitions(ctx: &ChatContext) -> Vec<serde_json::Value> {
    let skill_set = ctx.skill_set_snapshot();
    let allow_load_skill = ctx.config.skills.enabled && !skill_set.visible_skills().is_empty();
    let mut tools = plan_runtime::catalog::visible_tools_for_mode_with_policy(
        &ctx.session_runtime.plan_runtime.mode(),
        allow_load_skill,
    );
    match ctx.global_services.tool_registry.list_tools(None).await {
        Ok(plugin_tools) => {
            tools.extend(
                plugin_tools
                    .iter()
                    .map(crate::core::tools::contract::registry::tool_to_function_definition),
            );
        }
        Err(err) => {
            warn!(error = %err, "list plugin tools for LLM definitions failed; continuing with builtins only");
        }
    }
    tools
}

pub(crate) async fn build_system_text(ctx: &ChatContext, context_budget_chars: usize) -> String {
    let skill_set = ctx.skill_set_snapshot();
    let allow_load_skill = ctx.config.skills.enabled && !skill_set.visible_skills().is_empty();
    let plugin_tool_lines = match ctx.global_services.tool_registry.list_tools(None).await {
        Ok(plugin_tools) => plugin_tools
            .iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(err) => {
            warn!(error = %err, "list plugin tools for system prompt failed; omitting plugin tool lines");
            String::new()
        }
    };
    let builtin_tool_lines =
        crate::core::tools::contract::catalog::render_core_identity_tool_lines_with_policy(
            allow_load_skill,
        );
    let tool_lines = if plugin_tool_lines.is_empty() {
        builtin_tool_lines
    } else {
        format!("{builtin_tool_lines}\n{plugin_tool_lines}")
    };
    let workspace_context = crate::core::llm::system_prompt::WorkspaceContext {
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
        tool_lines: Some(tool_lines),
    };
    let workspace_state = compute_workspace_state(ctx);
    crate::core::llm::system_prompt::build_system_prompt_with_state_and_skills(
        workspace_context,
        workspace_state,
        Some(&skill_set),
        Some(&ctx.config.skills),
        context_budget_chars,
    )
}

pub(crate) fn sync_context_state_system_prompt_len(
    context_state: &mut crate::core::ContextState,
    old_len: usize,
    new_len: usize,
) {
    if old_len == new_len {
        return;
    }
    if new_len >= old_len {
        context_state.estimate_context_chars += new_len - old_len;
    } else {
        context_state.estimate_context_chars = context_state
            .estimate_context_chars
            .saturating_sub(old_len - new_len);
    }
    context_state.invalidate_api_usage();
}

const AUTO_TURN_BUDGET: u32 = 8;

fn current_user_prompt(ctx: &ChatContext) -> String {
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
        .ok()
        .flatten();
    user_prompt_for_mode_with_model(
        &ctx.session_runtime.plan_runtime.mode(),
        &ctx.effective_model(entry.as_ref()),
    )
}

fn append_failed_turn_message(
    context_state: &mut crate::core::ContextState,
    message: ChatMessage,
    account_chars: bool,
) {
    if account_chars {
        context_state.on_message_appended(estimate_msg_chars(&message));
    }
    context_state.messages.push(message);
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

type PlannedAppendOutcome = (Vec<ChatMessage>, Vec<(ChatMessage, bool)>);

fn append_planned_messages_with_rehydrate_retry(
    ctx: &ChatContext,
    system_text: &str,
    system_text_with_reminder: &str,
    context_config: &crate::infra::ContextConfig,
    planned_messages: &[ChatMessage],
    context_state: &mut crate::core::ContextState,
) -> Result<PlannedAppendOutcome, AppError> {
    let mut next_pending_idx = 0usize;
    let mut retried_after_rehydrate = false;
    loop {
        let mut messages = build_context_from_state(context_state);
        let mut appended_messages = Vec::new();
        messages.insert(0, ChatMessage::system(system_text_with_reminder));

        let mut append_error = None;
        for message in planned_messages.iter().skip(next_pending_idx) {
            if let Err(err) = push_turn_message(
                &mut messages,
                &ctx.session_runtime.message_append_sink,
                message.clone(),
            ) {
                append_error = Some(err);
                break;
            }
            context_state.on_message_appended(estimate_msg_chars(message));
            appended_messages.push((message.clone(), false));
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
                next_pending_idx += appended_messages.len();
                retried_after_rehydrate = true;
                continue;
            }
            return Err(err);
        }

        return Ok((messages, appended_messages));
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
    let context_budget_chars = crate::infra::config::compute_context_budget_chars(context_config);
    let mut system_text = build_system_text(ctx, context_budget_chars).await;
    persist::schedule_checkpoint_prune(ctx);
    // ResumePlan 目前恒为 Continue；保留 hook，未来若恢复逻辑需要 tail，可在这里恢复
    // `read_entries_tail(..., 64)` 预读。
    let _ = crate::core::compute_resume_plan(entry.as_ref(), &[]);
    let mut context_state =
        init_context_state(&ctx.session_runtime.session, context_config, &system_text)?;
    if let Err(err) = ctx
        .session_runtime
        .plan_runtime
        .attach_from_event(context_state.latest_plan_event.clone())
    {
        tracing::warn!(error = %err, "plan_runtime attach_from_event failed; continuing with Chat mode");
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
        let auto_drain = queued_follow_ups && auto_turn_count < AUTO_TURN_BUDGET;
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
                let (parsed, history_line) =
                    match dispatch_chat_command(ctx, parse_chat_command(&trimmed), &mut rl).await {
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
        } else {
            auto_turn_count = 0;
        }

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

        let next_system_text = build_system_text(ctx, context_budget_chars).await;
        sync_context_state_system_prompt_len(
            &mut context_state,
            system_text.len(),
            next_system_text.len(),
        );
        system_text = next_system_text;

        let outcome =
            run_chat_turn(ctx, &input, &system_text, &mut context_state, turn_token).await?;

        match outcome {
            AgentRunOutcome::Completed(_) => {}
            AgentRunOutcome::Interrupted(_) => {
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

pub async fn run_chat_turn_with_message(
    ctx: &ChatContext,
    input_message: Option<ChatMessage>,
    system_text: &str,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    ctx.session_runtime
        .plan_runtime
        .attach_cancel_hook(turn_token.clone());
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
    // 会话标题优先走 title_model；若其未配置/未解析，则降级到主模型。
    // turn 折叠标题仍维持 title_call 语义，不在此处一起降级。
    let title_call = ctx.resolve_call(LlmScene::Title, entry.as_ref()).ok();
    let main_provider = main_call.provider_impl.clone();
    let compaction_provider = compaction_call.provider_impl.clone();
    let title_provider = title_call.as_ref().map(|c| c.provider_impl.clone());
    let (session_title_provider, session_title_model) = title_call
        .as_ref()
        .map(|c| (c.provider_impl.clone(), c.model.clone()))
        .unwrap_or_else(|| (main_provider.clone(), main_call.model.clone()));
    let model = main_call.model.clone();
    let title_model = title_call
        .as_ref()
        .map(|c| c.model.clone())
        .unwrap_or_default();
    let thinking_model_id = ctx.effective_model(entry.as_ref());
    let thinking_level = Some(ctx.resolve_thinking_level(&thinking_model_id));
    let mut context_config = ctx.config.context.clone();
    context_config.compaction_model = compaction_call.model.clone();

    let plan_mode = ctx.session_runtime.plan_runtime.mode();
    let system_text_with_reminder = match &plan_mode {
        plan_runtime::PlanState::Planning => {
            format!(
                "{}{}",
                system_text,
                *plan_runtime::reminders::PLANNER_REMINDER
            )
        }
        plan_runtime::PlanState::Executing { plan_id } => format!(
            "{}{}",
            system_text,
            plan_runtime::reminders::render_executor_reminder(plan_id)
        ),
        _ => system_text.to_string(),
    };
    let planned_messages = drain_planned_turn_messages(ctx, input_message);
    let (messages, appended_messages) = append_planned_messages_with_rehydrate_retry(
        ctx,
        system_text,
        &system_text_with_reminder,
        &context_config,
        &planned_messages,
        context_state,
    )?;
    maybe_emit_rule_session_title(
        &ctx.session_runtime.session,
        &appended_messages,
        &root_event_emitter,
    );
    maybe_spawn_semantic_session_title(
        &ctx.session_runtime.session,
        &appended_messages,
        session_title_provider,
        session_title_model,
        root_event_emitter.clone(),
        session_id.clone(),
    );
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_user_append",
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count,
        turns = context_state.turn_count()
    );

    context_state.preheat.try_restart_if_pending(
        context_state.usage_ratio(),
        &context_state.messages,
        &context_state.transcript_path,
        compaction_provider.clone(),
        &context_config,
        Arc::clone(&root_event_emitter),
    );
    check_before_request(context_state, &root_event_emitter).await;
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_timing2_check",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = false,
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count
    );
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
        for (message, account_chars) in appended_messages {
            append_failed_turn_message(context_state, message, account_chars);
        }
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
    let mut messages = messages;
    if let std::borrow::Cow::Owned(degraded) =
        degrade_unsupported_multimodal(&messages, &main_call.capabilities)
    {
        messages = degraded;
    }

    let render_cli_output = !ctx.session_runtime.suppress_cli_output;
    let renderer = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let config = AgentLoopConfig {
        max_attempts: ctx.config.llm.agent_max_attempts,
        max_tool_rounds: usize::MAX,
        retry_base_delay_ms: ctx.config.llm.agent_retry_base_delay_ms,
        model,
        thinking_level,
        session_id: session_id.clone(),
        tool_definitions: build_tool_definitions(ctx).await,
        context_config: context_config.clone(),
        compaction_provider: Some(compaction_provider.clone()),
        title_provider: title_provider.clone(),
        title_model,
        agent_trail_dir: ctx
            .scope_services
            .agent_trail_dir
            .to_string_lossy()
            .to_string(),
        read_file_state: ctx.session_runtime.read_file_state.clone(),
        openai_files_runtime: ctx.openai_files_runtime_for(main_provider.as_ref()),
        checkpoint_store: ctx.scope_services.checkpoint_store.clone(),
        message_append_sink: Some(ctx.session_runtime.message_append_sink.clone()),
        parent_session_id: None,
        spawn_depth: 0,
        subagent_type: crate::core::agent_loop::SubagentType::User,
        review_kind: None,
        plan_runtime: Some(ctx.session_runtime.plan_runtime.clone()),
        skill_set: Some(ctx.scope_services.skill_set.clone()),
    };
    let mut agent_loop = AgentLoop::new(
        main_provider,
        ctx.global_services.primitive.clone(),
        ctx.global_services.event_bus.clone(),
        config,
        turn_token,
    );
    agent_loop = agent_loop.with_tool_registry(ctx.global_services.tool_registry.clone());
    if let Some(backend) = ctx.global_services.config_backend.clone() {
        agent_loop = agent_loop.with_config_backend(backend);
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
        print!(
            "\n{}",
            agent_prompt_for_mode(
                &ctx.config.agent.id,
                &ctx.session_runtime.plan_runtime.mode()
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
