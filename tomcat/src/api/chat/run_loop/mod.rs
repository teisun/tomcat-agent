use std::io::{self, Write as IoWrite};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{build_context_from_state, init_context_state};
use crate::core::session::read_entries_tail;
use crate::infra::error::AppError;
use crate::{AgentLoop, AgentLoopConfig, CheckpointKind};

use crate::core::plan_runtime;

use super::super::render::MarkdownRenderer;
use super::commands::{dispatch_chat_command, parse_chat_command, ChatCommandOutcome};
use super::context::ChatContext;
use super::prompt::{agent_prompt_for_mode, user_prompt_for_mode};
use super::{cli_turn_renderer, events, preflight};

mod background;
mod cleanup;
mod persist;
mod rehydrate;
mod thinking_persist;
mod workspace_state;

use self::background::{spawn_completion_subscriber, spawn_readline_waker};
use self::cleanup::ensure_session;
use self::persist::push_turn_message;
use self::rehydrate::{make_fallback_context_state, nonfatal_error_hint};
use self::workspace_state::compute_workspace_state;

#[cfg(test)]
pub(crate) use self::cleanup::cleanup_openai_files_on_session_end;
#[cfg(test)]
pub(crate) use self::persist::{
    build_turn_checkpoint_request, persist_turn_result, schedule_checkpoint_prune,
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

fn build_tool_definitions(ctx: &ChatContext) -> Vec<serde_json::Value> {
    plan_runtime::catalog::visible_tools_for_mode(&ctx.plan_runtime.mode())
}

const AUTO_TURN_BUDGET: u32 = 8;

pub async fn chat_loop(ctx: &ChatContext, resume: bool) -> Result<(), AppError> {
    ensure_session(ctx)?;

    let entry = ctx.session.get_session(ctx.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());

    if resume {
        println!("恢复会话: {}", ctx.session.current_session_key());
    }
    println!("tomcat 对话模式 (模型: {})", model);
    println!("输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。");
    println!("输入 /help 查看命令列表。\n");

    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))?;

    #[cfg(target_os = "macos")]
    // macOS 中文输入法在 `ExternalPrinter` 激活的输入路径下更容易出现回显异常。
    let search_tools_printer: Option<
        Arc<std::sync::Mutex<Box<dyn rustyline::ExternalPrinter + Send>>>,
    > = None;
    #[cfg(not(target_os = "macos"))]
    let search_tools_printer = rl.create_external_printer().ok().map(|printer| {
        Arc::new(std::sync::Mutex::new(
            Box::new(printer) as Box<dyn rustyline::ExternalPrinter + Send>,
        ))
    });

    let context_config = &ctx.config.context;
    let workspace_context = crate::core::llm::system_prompt::WorkspaceContext {
        agent_workspace_dir: ctx.agent_workspace_dir.to_string_lossy().to_string(),
        agent_definition_dir: ctx.agent_definition_dir.to_string_lossy().to_string(),
        agent_plans_dir: plan_runtime::file_store::plans_dir()
            .map(|path| crate::infra::platform::format_home_path(path.as_path()))
            .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
        agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
    };
    let workspace_state = compute_workspace_state(ctx);
    let system_text = crate::core::llm::system_prompt::build_system_prompt_with_state(
        workspace_context,
        workspace_state,
    );
    persist::schedule_checkpoint_prune(ctx);
    if let Some(path) = ctx.session.current_transcript_path()? {
        let tail = read_entries_tail(&path, 64).unwrap_or_default();
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &tail);
    } else {
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &[]);
    }
    let mut context_state = init_context_state(&ctx.session, context_config, &system_text)?;
    let session_stderr_ids =
        events::stderr::register_chat_session_stderr_listeners(&*ctx.event_bus, search_tools_printer);
    preflight::start_search_tools_preflight(&ctx.config, ctx.event_bus.clone());
    preflight::start_git_preflight(
        &ctx.config,
        ctx.event_bus.clone(),
        ctx.checkpoint_switcher.clone(),
    );

    if ctx.completion_subscriber_handle.lock().is_none() {
        let handle = spawn_completion_subscriber(ctx);
        *ctx.completion_subscriber_handle.lock() = Some(handle);
    }

    let mut auto_turn_count: u32 = 0;

    loop {
        let auto_drain = {
            let queue_len = ctx.follow_up_queue.lock().len();
            queue_len > 0 && auto_turn_count < AUTO_TURN_BUDGET
        };
        if !auto_drain {
            if auto_turn_count >= AUTO_TURN_BUDGET && !ctx.follow_up_queue.lock().is_empty() {
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
            let readline_waker = spawn_readline_waker(ctx.follow_up_signal.clone());
            let raw = match rl.readline(&user_prompt_for_mode(&ctx.plan_runtime.mode())) {
                Ok(line) => line,
                Err(rustyline::error::ReadlineError::Eof) => {
                    readline_waker.abort();
                    println!("\n再见！");
                    context_state.preheat.abort();
                    break;
                }
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    readline_waker.abort();
                    continue;
                }
                Err(rustyline::error::ReadlineError::Signal(
                    rustyline::error::Signal::Resize,
                )) => {
                    readline_waker.abort();
                    if !ctx.follow_up_queue.lock().is_empty() {
                        auto_turn_count = 0;
                        String::new()
                    } else {
                        continue;
                    }
                }
                Err(error) => {
                    readline_waker.abort();
                    eprintln!("输入错误: {}", error);
                    context_state.preheat.abort();
                    break;
                }
            };
            readline_waker.abort();
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                if !ctx.follow_up_queue.lock().is_empty() {
                    auto_turn_count = 0;
                } else {
                    continue;
                }
                String::new()
            } else {
                let parsed = match dispatch_chat_command(ctx, parse_chat_command(&trimmed), &mut rl)
                {
                    ChatCommandOutcome::Continue { line, echo_user } => {
                        if echo_user {
                            print!("{}{}", user_prompt_for_mode(&ctx.plan_runtime.mode()), line);
                            println!();
                            io::stdout().flush().map_err(AppError::Io)?;
                        }
                        line
                    }
                    ChatCommandOutcome::Handled => continue,
                };
                let _ = rl.add_history_entry(&parsed);
                parsed
            }
        };

        if input.is_empty() {
            auto_turn_count += 1;
        } else {
            auto_turn_count = 0;
        }

        let turn_token = {
            let mut guard = ctx.cancel_token.lock();
            *guard = CancellationToken::new();
            guard.clone()
        };

        let outcome =
            run_chat_turn(ctx, &input, &system_text, &mut context_state, turn_token).await?;

        match outcome {
            AgentRunOutcome::Completed(_) => {}
            AgentRunOutcome::Interrupted(_) => {
                eprintln!("\n^C 已中断（partial 已保存）");
            }
            AgentRunOutcome::Failed(error) => {
                let fatal = rehydrate::is_fatal_error(&error);
                eprintln!("\n[错误] {}", error);
                if fatal {
                    eprintln!("(致命错误，退出对话)");
                    context_state.preheat.abort();
                    cleanup::cleanup_openai_files_on_session_end(ctx, "chat_fatal_exit").await;
                    events::stderr::unregister_chat_session_stderr_listeners(
                        &*ctx.event_bus,
                        &session_stderr_ids,
                    );
                    return Err(error);
                }
                eprintln!("{}", nonfatal_error_hint(&error));
                continue;
            }
        }

        println!();
    }

    cleanup::cleanup_openai_files_on_session_end(ctx, "session_end").await;
    events::stderr::unregister_chat_session_stderr_listeners(&*ctx.event_bus, &session_stderr_ids);
    Ok(())
}

pub async fn run_chat_turn(
    ctx: &ChatContext,
    input: &str,
    system_text: &str,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    ctx.plan_runtime.attach_cancel_hook(turn_token.clone());

    let entry = ctx.session.get_session(ctx.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());
    let session_id = ctx
        .session
        .current_session_id()?
        .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
    let context_config = &ctx.config.context;

    context_state.on_message_appended(input.len());
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
        ctx.llm.clone(),
        context_config,
        ctx.event_bus.clone(),
    );
    check_before_request(context_state, &*ctx.event_bus).await;
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_timing2_check",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = false,
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count
    );

    let mut messages = build_context_from_state(context_state);

    let plan_mode = ctx.plan_runtime.mode();
    let system_text_with_reminder = match &plan_mode {
        plan_runtime::PlanMode::Planning => {
            format!(
                "{}{}",
                system_text,
                *plan_runtime::reminders::PLANNER_REMINDER
            )
        }
        plan_runtime::PlanMode::Executing { plan_id } => format!(
            "{}{}",
            system_text,
            plan_runtime::reminders::render_executor_reminder(plan_id)
        ),
        _ => system_text.to_string(),
    };
    messages.insert(0, ChatMessage::system(&system_text_with_reminder));
    if !input.is_empty() {
        push_turn_message(
            &mut messages,
            &ctx.message_append_sink,
            ChatMessage::user(input),
        )?;
    }
    {
        let mut queue = ctx.follow_up_queue.lock();
        if !queue.is_empty() {
            let drained: Vec<_> = queue.drain(..).collect();
            drop(queue);
            for message in drained {
                push_turn_message(&mut messages, &ctx.message_append_sink, message)?;
            }
        }
    }

    let renderer = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let config = AgentLoopConfig {
        max_attempts: 3,
        max_tool_rounds: usize::MAX,
        retry_base_delay_ms: 300,
        model,
        session_id,
        tool_definitions: build_tool_definitions(ctx),
        context_config: context_config.clone(),
        agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
        read_file_state: ctx.read_file_state.clone(),
        openai_files_runtime: ctx.openai_files_runtime.clone(),
        checkpoint_store: ctx.checkpoint_store.clone(),
        message_append_sink: Some(ctx.message_append_sink.clone()),
        parent_session_id: None,
        spawn_depth: 0,
        subagent_type: crate::core::agent_loop::SubagentType::User,
        review_kind: None,
        plan_runtime: Some(ctx.plan_runtime.clone()),
    };
    let mut agent_loop = AgentLoop::new(
        ctx.llm.clone(),
        ctx.primitive.clone(),
        ctx.event_bus.clone(),
        config,
        turn_token,
    );
    if let Some(backend) = ctx.config_backend.clone() {
        agent_loop = agent_loop.with_config_backend(backend);
    }
    agent_loop = agent_loop.with_bash_task_registry(ctx.bash_task_registry.clone());
    agent_loop = agent_loop.with_shared_follow_up_queue(ctx.follow_up_queue.clone());
    agent_loop = agent_loop.with_completion_routes(ctx.completion_routes.clone());

    let previous_state = std::mem::replace(
        context_state,
        make_fallback_context_state(ctx, system_text, context_config),
    );
    agent_loop.set_context_state(Some(previous_state));

    let cli_turn_renderer = cli_turn_renderer::CliTurnRenderer::new(
        Arc::clone(&renderer),
        Arc::clone(&ctx.thinking_display),
        ctx.config.llm.thinking.print_to_stderr,
        ctx.config.llm.tool_cli_verbosity,
    );
    let listener_ids = cli_turn_renderer.register(&*ctx.event_bus);
    let thinking_persist_listener_ids = if ctx.config.llm.thinking.persist {
        let transcript_path = ctx
            .session
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        Some(thinking_persist::register_thinking_persist_listeners(
            &*ctx.event_bus,
            transcript_path,
        ))
    } else {
        None
    };

    print!(
        "\n{}",
        agent_prompt_for_mode(&ctx.config.agent.id, &ctx.plan_runtime.mode())
    );
    io::stdout().flush().map_err(AppError::Io)?;

    info!(
        target: "tomcat_chat_diag",
        phase = "chat_before_agent_run",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = true
    );
    let outcome = agent_loop.run(messages).await;
    if let Some(ids) = &thinking_persist_listener_ids {
        thinking_persist::unregister_thinking_persist_listeners(&*ctx.event_bus, ids);
    }
    cli_turn_renderer::CliTurnRenderer::unregister(&*ctx.event_bus, &listener_ids);

    if let Some(remaining) = renderer.lock().flush() {
        print!("{}", remaining);
        let _ = io::stdout().flush();
    }

    let mut next_state = agent_loop.take_context_state().unwrap_or_else(|| {
        init_context_state(&ctx.session, context_config, system_text)
            .unwrap_or_else(|_| make_fallback_context_state(ctx, system_text, context_config))
    });
    if let AgentRunOutcome::Failed(error) = &outcome {
        let _ = rehydrate::try_rehydrate_context_state_after_append_invariant(
            ctx,
            context_config,
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
            let _ = ctx.session.persist_context_observability(context_state);
        }
    }

    Ok(outcome)
}
