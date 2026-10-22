//! `tomcat serve` 的 stdio 传输层与会话调度层。
//!
//! Phase 1 负责：
//! - `tomcat serve --stdio` 的命令/控制/事件帧编排
//! - 多会话 `sessionId` 路由
//! - `ask_question` 回环桥接
//! - schema / TypeScript 工件导出
//!
//! `AgentLoop`、`EventBus`、`ChatContext` 等核心能力保持复用，避免在传输层复制业务逻辑。

pub mod ask_question;
pub mod commands;
pub mod confirmation;
mod connection;
pub mod control;
mod controller_route;
pub mod event_pump;
mod fanout_event_bus;
pub mod ndjson;
mod ownership;
pub mod registry;
mod rewind_and_resend;
mod runtime;
pub mod schema;
pub(crate) mod service;
mod session_delete;
mod session_files;
mod session_job;
mod slash;
pub mod stdin;
pub mod types;
mod ui_preferences;
pub mod writer;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::api::chat::{
    drain_checkpoint_record_tasks, recover_context_state_after_failed_turn,
    spawn_completion_subscriber, ChatContext, ChatContextOverrides,
};
use crate::core::llm::{ChatMessage, LlmScene};
use crate::core::tools::primitive::BashTaskStatus;
use crate::infra::{wire, ScopedEventEmitter};
use crate::{
    ensure_work_dir_structure, resolve_model_thinking_path, resolve_sessions_dir,
    session_key_for_agent, AppConfig, AppError, ModelPrefsStore, SessionManager, SessionMode,
    ThinkingLevel,
};

pub(crate) use connection::ServeConnection as ServeState;
use registry::{SessionSlot, SessionTurnState};
pub(crate) use runtime::ServeRuntime;
use types::{NewSessionParams, ServeSessionMode};
use writer::WriterConfig;

const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
pub struct ServeCliArgs {
    pub stdio: bool,
    pub ws: bool,
    pub print_schema: bool,
}

pub(crate) fn build_shared_model_prefs(cfg: &AppConfig) -> Result<Arc<ModelPrefsStore>, AppError> {
    let default_level = ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0;
    Ok(Arc::new(ModelPrefsStore::load(
        resolve_model_thinking_path(cfg)?,
        default_level,
    )?))
}

use crate::infra::i18n::tr;

pub(crate) fn run_serve(args: ServeCliArgs, cfg: &AppConfig) -> Result<(), AppError> {
    if args.print_schema {
        let out_dir = schema::write_schema_bundle(cfg)?;
        println!("{}", out_dir.display());
        return Ok(());
    }

    let transport = if args.ws {
        crate::ServeTransport::Ws
    } else if args.stdio {
        crate::ServeTransport::Stdio
    } else {
        cfg.serve.transport
    };

    match transport {
        crate::ServeTransport::Stdio => {
            let runtime = tokio::runtime::Runtime::new().map_err(|error| {
                AppError::Config(tr("serve.runtimeFailed", &[("detail", &error.to_string())]))
            })?;
            runtime.block_on(run_stdio(cfg.clone()))
        }
        crate::ServeTransport::Ws => Err(AppError::Config(tr("serve.wsDeferred", &[]))),
    }
}

async fn run_stdio(cfg: AppConfig) -> Result<(), AppError> {
    ensure_work_dir_structure(&cfg)?;
    let shared_model_prefs = build_shared_model_prefs(&cfg)?;
    let writer = writer::spawn_stdout_writer(WriterConfig::from(&cfg.serve));
    let state = ServeState::new(
        cfg,
        crate::api::cli::config_file_path()?,
        writer,
        shared_model_prefs,
    )?;
    let initial_slot =
        create_session_slot(Arc::clone(&state), NewSessionParams::default(), false).await?;
    state.registry.insert(Arc::clone(&initial_slot))?;
    register_slot_hooks(&state, &initial_slot);
    let outcome = stdin::run_stdio_loop(Arc::clone(&state)).await;
    let cleanup = control::shutdown_all_sessions(state).await;
    match (outcome, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub(crate) fn default_mode(cfg: &AppConfig) -> Result<SessionMode, AppError> {
    let env_override = std::env::var("TOMCAT_SESSION_MODE").ok();
    crate::resolve_session_mode(&cfg.session.default_mode, env_override.as_deref())
}

/// 进程启动时跑一遍附件目录的家务活。
///
/// 握手响应写入 stdout 后异步执行一次，因此不阻塞初始化或交互热路径：
///
/// 1. 删掉旧版本遗留的后端草稿目录 —— 草稿改由扩展层持有后它已无人读取
/// 2. 回收超期租约（上次会话没发出去、进程又崩了的那些字节）
/// 3. 把派生缓存压回上限以内
/// 4. 将超过保留期的历史 diff 压缩成摘要
///
/// 全部失败都只记日志：家务活做不成的后果是多占点磁盘，不该阻止用户开始工作。
pub(crate) fn run_attachment_housekeeping(state: &ServeState) {
    let Some(manager) = scoped_session_manager(state) else {
        return;
    };
    manager.discard_legacy_draft_dir();
    if let Ok(dir) = resolve_sessions_dir(&state.cfg) {
        crate::core::checkpoint::file_baselines::prune(
            &dir,
            state.cfg.checkpoint.retention_days,
            std::time::SystemTime::now(),
        );
    }
    let store = manager.attachment_store();
    match store.gc_pending(crate::core::session::attachments::PENDING_BLOB_TTL) {
        Ok(report) if report.leases_released > 0 => {
            tracing::info!(
                "serve: attachment gc released {} expired lease(s)",
                report.leases_released,
            );
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("serve: attachment gc failed: {error}"),
    }

    match store
        .evict_rebuildable_over_budget(crate::core::session::attachments::REBUILDABLE_MAX_BYTES)
    {
        Ok(evicted) if evicted > 0 => {
            tracing::info!("serve: evicted {evicted} bytes of rebuildable attachment data");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("serve: attachment eviction failed: {error}"),
    }
    match store.sweep_orphan_blobs(
        || {
            let mark = manager.run_incremental_attachment_housekeeping()?;
            if mark.compacted_displays > 0 {
                tracing::info!(
                    "serve: compacted {} expired tool-display sidecar record(s)",
                    mark.compacted_displays
                );
            }
            let live = mark.live_blob_references;
            tracing::debug!(
                bytes_scanned = live.bytes_scanned,
                transcripts_scanned = live.transcripts_scanned,
                "serve: refreshed changed transcript references for attachment housekeeping"
            );
            Ok(live.shas)
        },
        crate::core::session::attachments::ORPHAN_BLOB_GRACE,
    ) {
        Ok(report) if report.blobs_deleted > 0 => tracing::info!(
            "serve: attachment sweep reclaimed {} orphan blob(s)",
            report.blobs_deleted
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!("serve: attachment orphan sweep failed: {error}"),
    }
}

/// 按当前配置构造会话管理器，用于「还没有任何会话」时也需要访问磁盘布局的场合。
fn scoped_session_manager(state: &ServeState) -> Option<SessionManager> {
    let sessions_dir = resolve_sessions_dir(&state.cfg).ok()?;
    let session_key = match state.current_scope() {
        Some(scope) => scope.key,
        None if state.shared => return None,
        None => session_key_for_agent(
            &state.cfg.agent.id,
            default_mode(&state.cfg).ok()?,
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ),
    };
    Some(SessionManager::new_scoped(sessions_dir, session_key))
}

/// 附件目录的绝对路径，握手时就能给出。
///
/// 宿主要用它配 webview 的 `localResourceRoots`，而这个配置只有在 webview 首次渲染
/// 之前写好才不会引发重载 —— 所以它必须来自 `initialize`，不能等到某个会话的
/// `get_state`：那时 webview 早就渲染完了，再改资源根就是一次可见的白屏。
pub(crate) fn attachment_root(state: &ServeState) -> Option<PathBuf> {
    Some(
        scoped_session_manager(state)?
            .attachment_store()
            .root()
            .to_path_buf(),
    )
}

pub(crate) fn normalize_session_mode(
    cfg: &AppConfig,
    explicit: Option<ServeSessionMode>,
) -> Result<SessionMode, AppError> {
    match explicit {
        Some(mode) => Ok(mode.into_core_mode()),
        None => default_mode(cfg),
    }
}

fn resolve_session_paths(
    params: &NewSessionParams,
    shared: bool,
) -> Result<(PathBuf, Option<PathBuf>), AppError> {
    let Some(raw_cwd) = params.cwd.as_deref() else {
        if shared {
            return Err(AppError::Config("cwd_required".into()));
        }
        return Ok((
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            None,
        ));
    };
    if raw_cwd.trim().is_empty() {
        return Err(AppError::Config(tr("serve.cwdEmpty", &[])));
    }
    let supplied = PathBuf::from(raw_cwd);
    if !supplied.is_absolute() {
        return Err(AppError::Config(tr("serve.cwdAbsolute", &[])));
    }
    let cwd = crate::normalize_path(raw_cwd)?;
    if !cwd.is_dir() {
        return Err(AppError::Config(tr(
            "serve.cwdMissing",
            &[("path", &cwd.display().to_string())],
        )));
    }
    Ok((cwd.clone(), Some(crate::core::session::project_root(&cwd))))
}

pub(crate) fn create_detached_session(
    state: &ServeState,
    params: NewSessionParams,
) -> Result<crate::SessionEntry, AppError> {
    let mode = normalize_session_mode(&state.cfg, params.mode)?;
    let (cwd_path, project_root) = resolve_session_paths(&params, state.shared)?;
    let sessions_dir = resolve_sessions_dir(&state.cfg)?;
    std::fs::create_dir_all(&sessions_dir).map_err(AppError::Io)?;
    let session_key = session_key_for_agent(&state.cfg.agent.id, mode, &cwd_path);
    let session_manager = SessionManager::new_scoped(sessions_dir, session_key.clone());
    session_manager.create_detached_session_with_project_root(
        &session_key,
        Some(cwd_path.to_string_lossy().to_string()),
        project_root.map(|path| path.to_string_lossy().to_string()),
    )
}

pub(crate) async fn create_session_slot(
    state: Arc<ServeState>,
    params: NewSessionParams,
    force_new: bool,
) -> Result<Arc<SessionSlot>, AppError> {
    create_or_load_session_slot(state, params, force_new, None, None).await
}

pub(super) async fn load_session_slot(
    state: Arc<ServeState>,
    params: NewSessionParams,
    session_id: &str,
) -> Result<Arc<SessionSlot>, AppError> {
    create_or_load_session_slot(state, params, false, Some(session_id), None).await
}

async fn create_or_load_session_slot(
    state: Arc<ServeState>,
    params: NewSessionParams,
    force_new: bool,
    session_id: Option<&str>,
    session_mcp: Option<&crate::core::connector::mcp::manager::SessionMcpConfig>,
) -> Result<Arc<SessionSlot>, AppError> {
    let mode = normalize_session_mode(&state.cfg, params.mode)?;
    let (cwd_path, project_root) = resolve_session_paths(&params, state.shared)?;
    if let Some(directory) = state.skills_dir.as_deref() {
        crate::core::skill::discovery::validate_connection_root(directory)?;
    }
    let sessions_dir = resolve_sessions_dir(&state.cfg)?;
    std::fs::create_dir_all(&sessions_dir).map_err(AppError::Io)?;
    let session_key = session_key_for_agent(&state.cfg.agent.id, mode, &cwd_path);
    let session_manager = SessionManager::new_scoped(sessions_dir, session_key);
    let cwd_string = Some(cwd_path.to_string_lossy().to_string());
    let current_entry = if let Some(id) = session_id {
        session_manager
            .get_session_by_id(id)?
            .filter(|entry| entry.session_key == session_manager.current_session_key())
            .ok_or_else(|| AppError::Config("unknown_session".into()))?
    } else if force_new {
        session_manager.create_session_with_origin(
            session_manager.current_session_key(),
            cwd_string.clone(),
            project_root.map(|path| path.to_string_lossy().to_string()),
            state.source(),
        )?
    } else {
        session_manager.ensure_current_session_with_project_root(
            cwd_string.clone(),
            project_root.map(|path| path.to_string_lossy().to_string()),
        )?
    };
    let result = async {
        session_manager.pin_session(&current_entry.session_id)?;

        let mut overrides = ChatContextOverrides::default()
            .with_session_id_override(current_entry.session_id.clone())
            .suppress_cli_output()
            .with_shared_agent_registry(Arc::clone(&state.shared_agent_registry))
            .with_shared_model_prefs(Arc::clone(&state.shared_model_prefs))
            .with_confirmation(
                state
                    .confirmation
                    .provider_for_session(current_entry.session_id.clone()),
            )
            .with_session_cwd_override(cwd_path.clone());
        overrides.child_environment = state.child_environment();
        let current_config =
            crate::infra::config::load_model_selection_config(&state.config_path, &state.cfg)?;
        let ctx =
            ChatContext::from_config_with_mode_and_overrides(current_config, mode, overrides)?;
        if let Some(config) = session_mcp {
            let prepared = ctx.prepare_session_mcp(
                config,
                &current_entry.session_id,
                state.child_environment(),
            )?;
            ctx.replace_session_mcp(prepared).await?;
        }
        state.shared_event_bus.register_session_bus(
            current_entry.session_id.clone(),
            ctx.global_services.event_bus.clone(),
        );
        ask_question::bind_panel_for_controller(&state, &ctx, &current_entry.session_id);
        if state.shared {
            ctx.bind_connection_skill_dir(state.skills_dir.clone());
            ctx.sync_resource_inventory().await?;
        } else if ctx.config.skills.enabled {
            ctx.spawn_skill_discovery_if_needed().await;
            let _ = ctx.await_skill_discovery().await;
        }
        let initial_model = match ctx.resolve_metadata(LlmScene::Main, Some(&current_entry)) {
            Ok(model) => Some(model),
            Err(AppError::ModelUnavailable { .. }) => None,
            Err(error) => return Err(error),
        };
        let context_budget_chars = crate::infra::config::compute_context_budget_chars_from_tokens(
            initial_model
                .as_ref()
                .map(|model| model.limits.input_budget_tokens)
                .unwrap_or_else(|| {
                    crate::infra::config::fallback_input_budget_tokens(&ctx.config.context)
                }),
        );
        let prompt_snapshot =
            crate::api::chat::build_prompt_snapshot(&ctx, context_budget_chars).await;
        let mut context_state = crate::core::session::manager::init_context_state(
            &ctx.session_runtime.session,
            &ctx.config.context,
            prompt_snapshot.system_text(),
        )?;
        if let Some(model) = initial_model {
            context_state.apply_limits(&model.limits);
        }
        if let Err(err) = ctx
            .session_runtime
            .plan_runtime
            .attach_from_resume_state(context_state.resume_control.clone())
        {
            tracing::warn!(error = %err, "serve session resume state attach failed");
        }
        let origin = ctx
            .session_runtime
            .session
            .read_session_header_for_session(&current_entry.session_id)?
            .and_then(|header| header.origin);
        let ctx = Arc::new(ctx);
        let mut slot = SessionSlot::new(
            current_entry.session_id.clone(),
            ctx,
            mode,
            cwd_string,
            SessionTurnState {
                context_state,
                prompt_snapshot,
                context_budget_chars,
            },
        );
        slot.origin = origin;
        Ok(Arc::new(slot))
    }
    .await;
    if result.is_err() {
        state
            .shared_event_bus
            .unregister_session_bus(&current_entry.session_id);
        session_manager.release_session_usage();
        if force_new {
            match session_manager.delete_session(&current_entry.session_id) {
                Ok(outcome) => {
                    for warning in outcome.warnings {
                        tracing::warn!(%warning, "created session cleanup incomplete");
                    }
                }
                Err(cleanup) => {
                    return Err(AppError::Config(format!(
                        "{}; created session rollback failed: {cleanup}",
                        result.as_ref().err().expect("failed initialization")
                    )))
                }
            }
        }
    }
    result
}

pub(crate) fn register_slot_hooks(state: &Arc<ServeState>, slot: &Arc<SessionSlot>) {
    if !state.shared {
        *slot.controller.lock() = Some(state.connection_id);
    }
    if !state.shared && state.active_session_id().is_none() {
        let _ = state.set_active_session(&slot.session_id);
    }
    let runtime = Arc::downgrade(&state.runtime);
    let session_id = slot.session_id.clone();
    let event_ids = event_pump::register_session_event_pump_with(
        slot,
        Arc::new(move |payload| {
            if let Some(runtime) = runtime.upgrade() {
                runtime.emit_event(&session_id, payload);
            }
        }),
    );
    slot.listener_ids.lock().extend(event_ids);
    let ask_listener = state.ask_question.register_request_listener(
        slot.session_id.clone(),
        slot.ctx.global_services.event_bus.clone(),
    );
    slot.listener_ids.lock().push(ask_listener);
    if slot
        .ctx
        .session_runtime
        .completion_subscriber_handle
        .lock()
        .is_none()
    {
        let handle = spawn_completion_subscriber(&slot.ctx);
        *slot.ctx.session_runtime.completion_subscriber_handle.lock() = Some(handle);
    }
    if slot.background_task_listener.lock().is_none() {
        *slot.background_task_listener.lock() = Some(spawn_background_task_listener(slot));
    }
    match crate::api::chat::has_resumable_tail_ask_question(&slot.ctx.session_runtime.session) {
        Ok(true) => {
            slot.resume_pending_ask_question
                .store(true, Ordering::SeqCst);
            if state.initialized.load(Ordering::SeqCst) {
                resume_pending_ask_question(state, slot);
            }
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(error = %error, "serve failed to inspect pending ask_question state");
        }
    }
}

/// Re-arm a restart-recovered ask_question only once the frontend has
/// completed its initialize handshake. Before that handshake, a control frame
/// can be lost because no webview has installed its receiver yet.
pub(crate) fn resume_pending_ask_question(state: &Arc<ServeState>, slot: &Arc<SessionSlot>) {
    if state.shared && !state.controls(slot) {
        return;
    }
    if !slot
        .resume_pending_ask_question
        .swap(false, Ordering::SeqCst)
    {
        return;
    }
    // This controller is initialized but cannot re-arm an interactive panel.
    // Consume the deferred-init flag so it cannot leave an idle slot undeletable;
    // the unanswered question stays in history for the existing new-input path.
    if slot
        .ctx
        .session_runtime
        .plan_runtime
        .ask_question_panel()
        .is_none()
    {
        return;
    }
    let state = Arc::clone(state);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        if let Err(error) = crate::api::serve::commands::start_turn(
            state,
            slot,
            None,
            None,
            crate::api::serve::commands::TurnAck::Silent,
        )
        .await
        {
            tracing::warn!(error = %error, "serve pending ask_question auto-resume failed");
        }
    });
}

fn spawn_background_task_listener(slot: &Arc<SessionSlot>) -> tokio::task::JoinHandle<()> {
    let registry = slot.ctx.session_runtime.bash_task_registry.clone();
    let emitter = ScopedEventEmitter::new(
        slot.ctx.global_services.event_bus.clone(),
        slot.session_id.clone(),
    );
    let mut rx = registry.subscribe_lifecycle();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let exit_code = match event.final_status {
                        BashTaskStatus::Finished { exit_code } => exit_code,
                        BashTaskStatus::Stopped => -1,
                        BashTaskStatus::Running | BashTaskStatus::DrainingOutput => continue,
                    };
                    let payload = serde_json::json!({
                        "type": wire::WIRE_BACKGROUND_TASK_FINISHED,
                        "taskId": event.task_id,
                        "exitCode": exit_code,
                        "logPath": event.log_path,
                        "command": event.command,
                    });
                    let _ = emitter.emit_payload(wire::WIRE_BACKGROUND_TASK_FINISHED, payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        target: "tomcat_chat_diag",
                        phase = "serve_background_task_listener_lagged",
                        skipped = skipped,
                        "serve background task listener lagged; some lifecycle events skipped"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

struct TurnStateLease {
    context_state: Option<crate::ContextState>,
    context_budget_chars: usize,
    slot: Arc<SessionSlot>,
    prompt_snapshot: crate::core::llm::SystemPromptSnapshot,
}

impl TurnStateLease {
    fn acquire(slot: Arc<SessionSlot>) -> Result<Self, AppError> {
        let mut guard = slot.turn_state.lock();
        let state = guard.take().ok_or_else(|| {
            AppError::Config(crate::infra::i18n::tr("serve.turnStateMissing", &[]))
        })?;
        drop(guard);
        Ok(Self {
            context_state: Some(state.context_state),
            context_budget_chars: state.context_budget_chars,
            slot,
            prompt_snapshot: state.prompt_snapshot,
        })
    }

    fn context_state_mut(&mut self) -> &mut crate::ContextState {
        self.context_state
            .as_mut()
            .expect("turn state lease should always hold context_state")
    }

    fn prompt_snapshot_and_context_state_mut(
        &mut self,
    ) -> (
        &mut crate::core::llm::SystemPromptSnapshot,
        &mut crate::ContextState,
    ) {
        let context_state = self
            .context_state
            .as_mut()
            .expect("turn state lease should always hold context_state");
        (&mut self.prompt_snapshot, context_state)
    }
}

impl Drop for TurnStateLease {
    fn drop(&mut self) {
        let Some(context_state) = self.context_state.take() else {
            return;
        };
        let mut guard = self.slot.turn_state.lock();
        *guard = Some(SessionTurnState {
            context_state,
            prompt_snapshot: self.prompt_snapshot.clone(),
            context_budget_chars: self.context_budget_chars,
        });
    }
}

/// Cancel a foreground turn and wait for its actual exit. Unlike shutdown, a timeout
/// retains the handle and must never abort a writer then report the session as stopped.
pub(super) async fn cancel_and_wait(
    state: &runtime::ServeRuntime,
    slot: &Arc<SessionSlot>,
) -> Result<(), &'static str> {
    if slot.is_command_job_running() {
        return Err("busy");
    }
    if slot.is_turn_running() {
        state
            .ask_question
            .cancel_live_session(&slot.session_id, "rewind");
        state
            .confirmation
            .cancel_live_session(&slot.session_id, "rewind");
        slot.ctx.session_runtime.cancel_token.lock().cancel();
        slot.ctx.agent_registry.cascade_abort(&slot.session_id);
    }
    let handle = slot.run_task.lock().take();
    if let Some(mut handle) = handle {
        if tokio::time::timeout(SESSION_SHUTDOWN_TIMEOUT, &mut handle)
            .await
            .is_err()
        {
            let mut running = slot.run_task.lock();
            if slot.is_busy() {
                *running = Some(handle);
            }
            return Err("stop_timeout");
        }
    }
    if slot.is_busy() {
        return Err("stop_timeout");
    }
    if !drain_checkpoint_record_tasks(&slot.ctx, SESSION_SHUTDOWN_TIMEOUT).await {
        return Err("stop_timeout");
    }
    if let Some(turn) = slot.turn_state.lock().as_mut() {
        turn.context_state.preheat.abort();
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotCleanup {
    Close,
    Delete,
    Shutdown,
}

impl SlotCleanup {
    fn as_str(self) -> &'static str {
        match self {
            Self::Close => "close_session",
            Self::Delete => "delete_session",
            Self::Shutdown => "serve_stdio_shutdown",
        }
    }
}

pub(crate) async fn cleanup_session_slot(
    state: &runtime::ServeRuntime,
    slot: &Arc<SessionSlot>,
    cleanup: SlotCleanup,
) -> Result<(), AppError> {
    let reason = cleanup.as_str();
    slot.ctx.session_runtime.cancel_token.lock().cancel();
    // Private transports must finish retiring before the slot can be forgotten.
    slot.ctx.stop_session_mcp().await?;
    slot.ctx.agent_registry.cascade_abort(&slot.session_id);
    let background_task_listener = { slot.background_task_listener.lock().take() };
    if let Some(handle) = background_task_listener {
        handle.abort();
        let _ = handle.await;
    }

    if let Some(plugin_manager) = slot.ctx.global_services.plugin_manager.clone() {
        match tokio::time::timeout(
            SESSION_SHUTDOWN_TIMEOUT,
            plugin_manager.end_session(&slot.session_id),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    reason = reason,
                    session_id = %slot.session_id,
                    error = %error,
                    "serve session plugin cleanup failed"
                );
                if cleanup == SlotCleanup::Delete {
                    return Err(error);
                }
            }
            Err(_) => {
                tracing::warn!(
                    reason = reason,
                    session_id = %slot.session_id,
                    timeout_ms = SESSION_SHUTDOWN_TIMEOUT.as_millis(),
                    "serve session plugin cleanup timed out"
                );
                if cleanup == SlotCleanup::Delete {
                    return Err(AppError::Config("stop_timeout".into()));
                }
            }
        }
    }

    let handle = { slot.run_task.lock().take() };
    if let Some(mut handle) = handle {
        match tokio::time::timeout(SESSION_SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(joined) => {
                if let Err(error) = joined {
                    tracing::warn!(
                        reason = reason,
                        session_id = %slot.session_id,
                        error = %error,
                        "serve session task join failed during cleanup"
                    );
                }
            }
            Err(_) => {
                tracing::warn!(
                    reason = reason,
                    session_id = %slot.session_id,
                    timeout_ms = SESSION_SHUTDOWN_TIMEOUT.as_millis(),
                    "serve session task join timed out; aborting task"
                );
                if cleanup == SlotCleanup::Delete {
                    *slot.run_task.lock() = Some(handle);
                    return Err(AppError::Config("stop_timeout".into()));
                }
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    let drained = drain_checkpoint_record_tasks(&slot.ctx, SESSION_SHUTDOWN_TIMEOUT).await;
    if !drained && cleanup == SlotCleanup::Delete {
        return Err(AppError::Config("stop_timeout".into()));
    }
    event_pump::unregister_session_event_pump(slot);
    state.ask_question.clear_session(&slot.session_id);
    state
        .confirmation
        .cancel_live_session(&slot.session_id, reason);
    state
        .shared_event_bus
        .unregister_session_bus(&slot.session_id);
    slot.ctx.shutdown_completion_subscriber();
    slot.ctx.agent_registry.unregister(&slot.session_id);
    slot.ctx.session_runtime.session.release_session_usage();
    if cleanup != SlotCleanup::Shutdown {
        state.remove_slot(&slot.session_id);
    }
    Ok(())
}

pub(crate) async fn run_slot_turn(
    slot: Arc<SessionSlot>,
    input_message: Option<ChatMessage>,
    turn_token: tokio_util::sync::CancellationToken,
) -> Result<crate::AgentRunOutcome, AppError> {
    let mut turn_state = TurnStateLease::acquire(Arc::clone(&slot))?;
    let result = async {
        let (prompt_snapshot, context_state) = turn_state.prompt_snapshot_and_context_state_mut();
        slot.ctx.ready_session_mcp(&turn_token).await?;
        crate::api::chat::run_chat_turn_with_message_and_snapshot(
            &slot.ctx,
            input_message,
            prompt_snapshot,
            context_state,
            turn_token,
        )
        .await
    }
    .await;
    if let Err(error) = &result {
        let system_text = turn_state.prompt_snapshot.system_text().to_string();
        let _ = recover_context_state_after_failed_turn(
            &slot.ctx,
            &slot.ctx.config.context,
            &system_text,
            error,
            turn_state.context_state_mut(),
        );
        let _ = slot
            .ctx
            .session_runtime
            .session
            .persist_context_observability(turn_state.context_state_mut());
    }
    result
}
