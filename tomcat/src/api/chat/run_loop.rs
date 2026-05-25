use std::io::{self, Write as IoWrite};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{build_context_from_state, init_context_state};
use crate::core::session::read_entries_tail;
use crate::infra::error::AppError;
use crate::infra::EventBus;
use crate::{
    AgentLoop, AgentLoopConfig, CheckpointKind, CheckpointRecordRequest, compound_turn_id,
    resolve_workspace_roots_paths,
};

use crate::core::plan_runtime;

use super::commands::{ChatCommandOutcome, dispatch_chat_command, parse_chat_command};
use super::context::ChatContext;
use super::prompt::{agent_prompt_for_mode, user_prompt_for_mode};
use super::{cli_turn_renderer, events, preflight};
use super::super::render::MarkdownRenderer;

fn build_tool_definitions(ctx: &ChatContext) -> Vec<serde_json::Value> {
    plan_runtime::catalog::visible_tools_for_mode(&ctx.plan_runtime.mode())
}

fn compute_workspace_state(ctx: &ChatContext) -> crate::core::llm::system_prompt::WorkspaceState {
    use crate::core::llm::system_prompt::{
        PathRuleSummary, WorkspaceRootDescriptor, WorkspaceState,
    };
    use crate::core::permission::PathRuleMode;
    use std::collections::HashSet;

    let cfg = &ctx.config;
    let agent_definition_dir = ctx.agent_definition_dir.clone();
    let workspace_roots = resolve_workspace_roots_paths(cfg).unwrap_or_default();
    let agent_plans_dir = crate::infra::config::resolve_plans_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let agent_trail_readonly_dirs: Vec<std::path::PathBuf> = vec![
        Some(ctx.agent_trail_dir.clone()),
        crate::infra::config::resolve_sessions_dir(cfg).ok(),
        crate::infra::config::resolve_log_dir(cfg).ok(),
        crate::infra::config::resolve_audit_dir(cfg).ok(),
        crate::infra::config::resolve_agent_dir(cfg).ok(),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut entry_meta: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();
    for e in &cfg.workspace.entries {
        if !e.path.trim().is_empty() {
            let key = crate::infra::platform::normalize_path(&e.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| e.path.clone());
            entry_meta.insert(key, (e.alias.clone(), e.description.clone()));
        }
    }

    let agent_definition_canon = agent_definition_dir.to_string_lossy().to_string();
    let workspace_root_set: HashSet<String> = workspace_roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let session_set: HashSet<String> = ctx
        .session_grants
        .snapshot()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let er = ctx.gate.effective_roots();
    let mut read_write: Vec<WorkspaceRootDescriptor> = Vec::new();
    let mut seen_rw: HashSet<String> = HashSet::new();
    for p in er.read_write {
        let s = p.to_string_lossy().to_string();
        if !seen_rw.insert(s.clone()) {
            continue;
        }
        let label = if s == agent_definition_canon {
            "agent_definition_dir"
        } else if workspace_root_set.contains(&s) {
            "agent_workspace_root"
        } else if session_set.contains(&s) {
            "session_grant"
        } else {
            "workspace_root"
        };
        let (alias, description) = entry_meta.get(&s).cloned().unwrap_or((None, None));
        read_write.push(WorkspaceRootDescriptor {
            path: s,
            label: label.to_string(),
            alias,
            description,
        });
    }

    let mut read_only: Vec<WorkspaceRootDescriptor> = Vec::new();
    let mut seen_ro: HashSet<String> = HashSet::new();
    let agent_trail_set: HashSet<String> = agent_trail_readonly_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    for p in er.read_only {
        let s = p.to_string_lossy().to_string();
        if !seen_ro.insert(s.clone()) {
            continue;
        }
        let label = if agent_trail_set.contains(&s) {
            "agent_trail_dir"
        } else if agent_plans_dir.as_deref() == Some(&s) {
            "agent_plans_dir"
        } else {
            "path_rule_readonly"
        };
        read_only.push(WorkspaceRootDescriptor {
            path: s,
            label: label.to_string(),
            alias: None,
            description: None,
        });
    }

    let user_paths: HashSet<String> = cfg
        .primitive
        .path_rules
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let mut path_rules: Vec<PathRuleSummary> = Vec::new();
    for r in ctx.gate.effective_path_rules() {
        path_rules.push(PathRuleSummary {
            path: r.path.clone(),
            mode: match r.mode {
                PathRuleMode::Deny => "deny".to_string(),
                PathRuleMode::Readonly => "readonly".to_string(),
            },
            builtin: !user_paths.contains(&r.path),
        });
    }

    WorkspaceState {
        read_write,
        read_only,
        path_rules,
    }
}

const AUTO_TURN_BUDGET: u32 = 8;

fn spawn_completion_subscriber(ctx: &ChatContext) -> tokio::task::JoinHandle<()> {
    use crate::core::tools::primitive::{BackgroundTaskLifecycleEvent, BashTaskStatus};

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
                        let mut g = delivered.lock();
                        if g.contains(&task_id) {
                            continue;
                        }
                        g.insert(task_id.clone());
                    }
                    let should_push = {
                        let mut g = routes.lock();
                        match g.get(&task_id).copied() {
                            Some(crate::core::agent_loop::CompletionRoute::ToolWillDeliver)
                            | Some(crate::core::agent_loop::CompletionRoute::Delivered) => false,
                            _ => {
                                g.insert(
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
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        target: "tomcat_chat_diag",
                        phase = "completion_subscriber_lagged",
                        skipped = n,
                        "lifecycle broadcast subscriber lagged; some events skipped"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn spawn_readline_waker(
    signal: Arc<tokio::sync::Notify>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        signal.notified().await;
        wake_blocking_readline();
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn wake_blocking_readline() {
    // `rustyline` 会把 SIGWINCH 转成 `ReadlineError::Signal(Signal::Resize)`；借它把阻塞中的
    // `readline()` 温和唤醒，让 chat loop 立刻进入 auto-drain，而不是等用户再按一次回车。
    unsafe {
        libc::raise(libc::SIGWINCH);
    }
}

#[cfg(any(not(unix), target_os = "macos"))]
// macOS 下优先保证 IME 输入稳定，宁可退回“等用户下一次交互再 drain”。
fn wake_blocking_readline() {}

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
    let search_tools_printer = rl.create_external_printer().ok().map(|p| {
        Arc::new(std::sync::Mutex::new(
            Box::new(p) as Box<dyn rustyline::ExternalPrinter + Send>
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
    schedule_checkpoint_prune(ctx);
    if let Some(path) = ctx.session.current_transcript_path()? {
        let tail = read_entries_tail(&path, 64).unwrap_or_default();
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &tail);
    } else {
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &[]);
    }
    let mut context_state = init_context_state(&ctx.session, context_config, &system_text)?;
    let session_stderr_ids = events::stderr::register_chat_session_stderr_listeners(
        &*ctx.event_bus,
        search_tools_printer,
    );
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
        let auto_drain: bool = {
            let qlen = ctx.follow_up_queue.lock().len();
            qlen > 0 && auto_turn_count < AUTO_TURN_BUDGET
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
                Err(e) => {
                    readline_waker.abort();
                    eprintln!("输入错误: {}", e);
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
            AgentRunOutcome::Failed(e) => {
                let is_fatal = is_fatal_error(&e);
                eprintln!("\n[错误] {}", e);
                if is_fatal {
                    eprintln!("(致命错误，退出对话)");
                    context_state.preheat.abort();
                    cleanup_openai_files_on_session_end(ctx, "chat_fatal_exit").await;
                    events::stderr::unregister_chat_session_stderr_listeners(
                        &*ctx.event_bus,
                        &session_stderr_ids,
                    );
                    return Err(e);
                }
                eprintln!("(可重试，请继续输入)\n");
                continue;
            }
        }

        println!();
    }

    cleanup_openai_files_on_session_end(ctx, "session_end").await;
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
            format!("{}{}", system_text, *plan_runtime::reminders::PLANNER_REMINDER)
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
        messages.push(ChatMessage::user(input));
    }
    {
        let mut q = ctx.follow_up_queue.lock();
        if !q.is_empty() {
            messages.extend(q.drain(..));
        }
    }

    let renderer = Arc::new(parking_lot::Mutex::new(MarkdownRenderer::new()));
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

    let prev_state = std::mem::replace(
        context_state,
        make_fallback_context_state(ctx, system_text, context_config),
    );
    agent_loop.set_context_state(Some(prev_state));

    let cli_turn_renderer = cli_turn_renderer::CliTurnRenderer::new(
        Arc::clone(&renderer),
        Arc::clone(&ctx.show_thinking),
        ctx.config.llm.thinking.print_to_stderr,
        ctx.config.llm.tool_cli_verbosity,
    );
    let listener_ids = cli_turn_renderer.register(&*ctx.event_bus);
    let thinking_persist_listener_ids = if ctx.config.llm.thinking.persist {
        let transcript_path = ctx
            .session
            .current_transcript_path()?
            .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
        Some(register_thinking_persist_listeners(
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
        unregister_thinking_persist_listeners(&*ctx.event_bus, ids);
    }
    cli_turn_renderer::CliTurnRenderer::unregister(&*ctx.event_bus, &listener_ids);

    if let Some(remaining) = renderer.lock().flush() {
        print!("{}", remaining);
        let _ = io::stdout().flush();
    }

    *context_state = agent_loop.take_context_state().unwrap_or_else(|| {
        init_context_state(&ctx.session, context_config, system_text)
            .unwrap_or_else(|_| make_fallback_context_state(ctx, system_text, context_config))
    });

    match &outcome {
        AgentRunOutcome::Completed(result) => {
            persist_turn_result(
                ctx,
                context_state,
                result.new_messages.clone(),
                CheckpointKind::TurnEnd,
            )?;
        }
        AgentRunOutcome::Interrupted(result) => {
            persist_turn_result(
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

fn make_fallback_context_state(
    ctx: &ChatContext,
    system_text: &str,
    context_config: &crate::infra::ContextConfig,
) -> crate::core::ContextState {
    crate::core::ContextState {
        messages: Vec::new(),
        estimate_context_chars: system_text.len(),
        context_budget_chars: crate::infra::config::compute_context_budget_chars(context_config),
        context_budget_tokens: context_config
            .context_window
            .saturating_sub(context_config.max_output_tokens),
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: ctx
            .session
            .current_transcript_path()
            .ok()
            .flatten()
            .unwrap_or_default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

fn is_fatal_error(e: &AppError) -> bool {
    matches!(e, AppError::Config(_))
}

pub(crate) fn schedule_checkpoint_prune(ctx: &ChatContext) {
    let store = ctx.checkpoint_store.clone();
    let retention = crate::core::RetentionPolicy {
        retention_max: ctx.config.checkpoint.retention_max,
        retention_days: ctx.config.checkpoint.retention_days,
    };
    std::thread::spawn(move || {
        if let Err(err) = store.prune(retention) {
            warn!(error = %err, "checkpoint prune failed");
        }
    });
}

pub(crate) fn persist_turn_result(
    ctx: &ChatContext,
    context_state: &mut crate::core::ContextState,
    new_messages: Vec<crate::core::llm::ChatMessage>,
    kind: CheckpointKind,
) -> Result<Vec<String>, AppError> {
    let mut appended_row_ids = Vec::new();
    for msg in new_messages {
        let row_id = ctx.session.append_message(serde_json::to_value(&msg)?)?;
        let mut cm = msg;
        cm.msg_id = Some(row_id);
        appended_row_ids.push(cm.msg_id.clone().unwrap_or_default());
        context_state.messages.push(cm);
    }
    ctx.session.persist_context_observability(context_state)?;
    maybe_record_turn_checkpoint(ctx, kind, &appended_row_ids);
    Ok(appended_row_ids)
}

fn maybe_record_turn_checkpoint(
    ctx: &ChatContext,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) {
    let Ok(Some(session_id)) = ctx.session.current_session_id() else {
        return;
    };
    let Some(request) = build_turn_checkpoint_request(&session_id, kind, appended_row_ids) else {
        return;
    };
    if let Err(err) = ctx.checkpoint_store.record(request) {
        warn!(error = %err, "checkpoint record failed");
    }
}

pub(crate) fn build_turn_checkpoint_request(
    session_id: &str,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) -> Option<CheckpointRecordRequest> {
    let (Some(start_id), Some(end_id)) = (appended_row_ids.first(), appended_row_ids.last()) else {
        return None;
    };
    Some(CheckpointRecordRequest {
        session_id: session_id.to_string(),
        turn_id: compound_turn_id(start_id, end_id),
        kind,
        message_anchor: Some(end_id.clone()),
        notes: None,
    })
}

#[derive(Default)]
struct ThinkingPersistState {
    text: String,
    signature: Option<String>,
}

pub(crate) struct ThinkingPersistListenerIds {
    msg_update: crate::infra::event_bus::EventListenerId,
    msg_end: crate::infra::event_bus::EventListenerId,
}

pub(crate) fn register_thinking_persist_listeners(
    bus: &dyn EventBus,
    transcript_path: std::path::PathBuf,
) -> ThinkingPersistListenerIds {
    let state = Arc::new(Mutex::new(ThinkingPersistState::default()));

    let state_for_update = Arc::clone(&state);
    let msg_update = bus.on(
        crate::infra::wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |evt: crate::infra::event_bus::EventContext| {
            let event = match evt.payload.get("assistantMessageEvent") {
                Some(e) => e,
                None => return Ok(()),
            };
            if event.get("kind").and_then(|v| v.as_str()) != Some("thinking_delta") {
                return Ok(());
            }
            let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if delta.is_empty() {
                return Ok(());
            }
            let mut st = state_for_update.lock();
            st.text.push_str(delta);
            if let Some(sig) = event.get("signature").and_then(|v| v.as_str()) {
                st.signature = Some(sig.to_string());
            }
            Ok(())
        }),
    );

    let state_for_end = Arc::clone(&state);
    let msg_end = bus.on(
        crate::infra::wire::WIRE_MESSAGE_END,
        Box::new(move |_evt: crate::infra::event_bus::EventContext| {
            let (text, signature) = {
                let mut st = state_for_end.lock();
                if st.text.is_empty() {
                    return Ok(());
                }
                (std::mem::take(&mut st.text), st.signature.take())
            };
            let entry = crate::core::session::TranscriptEntry::ThinkingTrace(
                crate::core::session::ThinkingTraceEntry {
                    id: None,
                    parent_id: None,
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    text,
                    signature,
                },
            );
            if let Err(e) = crate::core::session::append_entry(&transcript_path, &entry) {
                warn!(error = %e, "append thinking_trace entry failed");
            }
            Ok(())
        }),
    );

    ThinkingPersistListenerIds {
        msg_update,
        msg_end,
    }
}

pub(crate) fn unregister_thinking_persist_listeners(
    bus: &dyn EventBus,
    ids: &ThinkingPersistListenerIds,
) {
    bus.off(ids.msg_update);
    bus.off(ids.msg_end);
}

fn ensure_session(ctx: &ChatContext) -> Result<(), AppError> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let _ = ctx.session.ensure_current_session(cwd)?;
    Ok(())
}

pub(crate) async fn cleanup_openai_files_on_session_end(
    ctx: &ChatContext,
    reason: &str,
) {
    let Some(runtime) = ctx.openai_files_runtime.as_ref() else {
        return;
    };
    let summary = runtime.cleanup_registered_files(reason).await;
    if summary.total == 0 {
        return;
    }
    if summary.failed > 0 {
        warn!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            failed = summary.failed,
            "openai files cleanup finished with failures"
        );
    } else {
        info!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            "openai files cleanup completed"
        );
    }
}
