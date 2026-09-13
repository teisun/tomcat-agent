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
pub mod control;
pub mod event_pump;
mod fanout_event_bus;
pub mod ndjson;
pub mod registry;
pub mod schema;
pub mod stdin;
pub mod types;
pub mod writer;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::api::chat::{
    drain_checkpoint_record_tasks, recover_context_state_after_failed_turn,
    spawn_completion_subscriber, ChatContext, ChatContextOverrides,
};
use crate::core::agent_registry::AgentRegistry;
use crate::core::llm::{ChatMessage, LlmScene};
use crate::core::tools::primitive::BashTaskStatus;
use crate::infra::{wire, ScopedEventEmitter};
use crate::{
    ensure_work_dir_structure, resolve_model_thinking_path, resolve_sessions_dir,
    session_key_for_agent, AppConfig, AppError, ModelPrefsStore, SessionManager, SessionMode,
    ThinkingLevel,
};

use ask_question::ServeAskQuestionBridge;
use confirmation::ServeConfirmationBridge;
use fanout_event_bus::FanoutEventBus;
use registry::{ChatContextRegistry, SessionSlot, SessionTurnState};
use types::{NewSessionParams, ServeSessionMode};
use writer::{WriterConfig, WriterHandle};

const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
pub struct ServeCliArgs {
    pub stdio: bool,
    pub ws: bool,
    pub print_schema: bool,
}

pub(crate) struct ServeState {
    pub cfg: AppConfig,
    pub registry: Arc<ChatContextRegistry>,
    pub writer: WriterHandle,
    pub ask_question: ServeAskQuestionBridge,
    pub confirmation: ServeConfirmationBridge,
    pub shared_model_catalog: crate::core::llm::SharedModelCatalog,
    pub shared_model_prefs: Arc<ModelPrefsStore>,
    pub shared_agent_registry: Arc<AgentRegistry>,
    pub shared_event_bus: Arc<FanoutEventBus>,
    pub initialized: AtomicBool,
}

impl ServeState {
    fn new(
        cfg: AppConfig,
        writer: WriterHandle,
        shared_model_prefs: Arc<ModelPrefsStore>,
    ) -> Result<Arc<Self>, AppError> {
        let registry = Arc::new(ChatContextRegistry::new(cfg.serve.max_sessions));
        let ask_question = ServeAskQuestionBridge::new(writer.clone());
        let confirmation = ServeConfirmationBridge::new(writer.clone());
        let shared_event_bus = Arc::new(FanoutEventBus::new());
        let shared_agent_registry = AgentRegistry::new().attach_event_bus(shared_event_bus.clone());
        let shared_model_catalog = crate::core::llm::SharedModelCatalog::load(&cfg)?;
        Ok(Arc::new(Self {
            cfg,
            registry,
            writer,
            ask_question,
            confirmation,
            shared_model_catalog,
            shared_model_prefs,
            shared_agent_registry,
            shared_event_bus,
            initialized: AtomicBool::new(false),
        }))
    }
}

pub(crate) fn build_shared_model_prefs(cfg: &AppConfig) -> Result<Arc<ModelPrefsStore>, AppError> {
    let default_level = ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0;
    Ok(Arc::new(ModelPrefsStore::load(
        resolve_model_thinking_path(cfg)?,
        default_level,
    )?))
}

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
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| AppError::Config(format!("创建 serve runtime 失败: {error}")))?;
            runtime.block_on(run_stdio(cfg.clone()))
        }
        crate::ServeTransport::Ws => Err(AppError::Config(
            "serve transport ws is deferred to Phase 2".to_string(),
        )),
    }
}

async fn run_stdio(cfg: AppConfig) -> Result<(), AppError> {
    ensure_work_dir_structure(&cfg)?;
    let shared_model_prefs = build_shared_model_prefs(&cfg)?;
    let writer = writer::spawn_stdout_writer(WriterConfig::from(&cfg.serve));
    let state = ServeState::new(cfg, writer, shared_model_prefs)?;
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
    let mut live = match manager.run_incremental_attachment_housekeeping() {
        Ok(mark) => {
            if mark.compacted_displays > 0 {
                tracing::info!(
                    "serve: compacted {} expired tool-display sidecar record(s)",
                    mark.compacted_displays
                );
            }
            mark.live_blob_references
        }
        Err(error) => {
            tracing::warn!("serve: startup attachment housekeeping mark pass failed: {error}");
            return;
        }
    };
    tracing::debug!(
        bytes_scanned = live.bytes_scanned,
        transcripts_scanned = live.transcripts_scanned,
        "serve: refreshed changed transcript references for attachment housekeeping"
    );
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
    // `gc_pending` may have released expired leases. Re-read just the lease directory
    // (not every transcript) and rebuild the live set, so stale markers do not
    // artificially extend an orphan's grace period by one more startup.
    match store.collect_pending_blob_shas() {
        Ok(pending) => live.refresh_pending_blob_shas(pending),
        Err(error) => {
            tracing::warn!("serve: could not refresh live attachment leases: {error}");
            return;
        }
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
        &live.shas,
        crate::core::session::attachments::ORPHAN_BLOB_GRACE,
    ) {
        Ok(report) if report.blobs_deleted > 0 => tracing::info!(
            "serve: attachment sweep reclaimed {} orphan blob(s) after scanning {} bytes",
            report.blobs_deleted,
            live.bytes_scanned
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!("serve: attachment orphan sweep failed: {error}"),
    }
}

/// 按当前配置构造会话管理器，用于「还没有任何会话」时也需要访问磁盘布局的场合。
fn scoped_session_manager(state: &ServeState) -> Option<SessionManager> {
    let sessions_dir = resolve_sessions_dir(&state.cfg).ok()?;
    let session_key = session_key_for_agent(
        &state.cfg.agent.id,
        default_mode(&state.cfg).ok()?,
        &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    );
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
) -> Result<(PathBuf, Option<PathBuf>), AppError> {
    let Some(raw_cwd) = params.cwd.as_deref() else {
        return Ok((
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            None,
        ));
    };
    if raw_cwd.trim().is_empty() {
        return Err(AppError::Config(
            "new_session.cwd 不能为空字符串".to_string(),
        ));
    }
    let supplied = PathBuf::from(raw_cwd);
    if !supplied.is_absolute() {
        return Err(AppError::Config(
            "new_session.cwd 必须是绝对目录".to_string(),
        ));
    }
    let cwd = crate::normalize_path(raw_cwd)?;
    if !cwd.is_dir() {
        return Err(AppError::Config(format!(
            "new_session.cwd 必须是存在的目录: {}",
            cwd.display()
        )));
    }
    Ok((cwd.clone(), Some(crate::core::session::project_root(&cwd))))
}

pub(crate) fn create_detached_session(
    state: &ServeState,
    params: NewSessionParams,
) -> Result<crate::SessionEntry, AppError> {
    let mode = normalize_session_mode(&state.cfg, params.mode)?;
    let (cwd_path, project_root) = resolve_session_paths(&params)?;
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
    let mode = normalize_session_mode(&state.cfg, params.mode)?;
    let (cwd_path, project_root) = resolve_session_paths(&params)?;
    let sessions_dir = resolve_sessions_dir(&state.cfg)?;
    std::fs::create_dir_all(&sessions_dir).map_err(AppError::Io)?;
    let session_key = session_key_for_agent(&state.cfg.agent.id, mode, &cwd_path);
    let session_manager = SessionManager::new_scoped(sessions_dir, session_key);
    let cwd_string = Some(cwd_path.to_string_lossy().to_string());
    let current_entry = if force_new {
        session_manager.new_current_session_with_project_root(
            cwd_string.clone(),
            project_root.map(|path| path.to_string_lossy().to_string()),
        )?
    } else {
        session_manager.ensure_current_session_with_project_root(
            cwd_string.clone(),
            project_root.map(|path| path.to_string_lossy().to_string()),
        )?
    };
    session_manager.pin_session(&current_entry.session_id);

    let overrides = ChatContextOverrides::default()
        .suppress_cli_output()
        .with_shared_agent_registry(Arc::clone(&state.shared_agent_registry))
        .with_shared_model_prefs(Arc::clone(&state.shared_model_prefs))
        .with_confirmation(
            state
                .confirmation
                .provider_for_session(current_entry.session_id.clone()),
        )
        .with_session_cwd_override(cwd_path.clone());
    let ctx = ChatContext::from_config_with_mode_and_overrides(state.cfg.clone(), mode, overrides)?;
    state.shared_event_bus.register_session_bus(
        current_entry.session_id.clone(),
        ctx.global_services.event_bus.clone(),
    );
    let ask_panel = state.ask_question.panel_for_session(
        ctx.global_services.event_bus.clone(),
        &current_entry.session_id,
    );
    ctx.session_runtime
        .plan_runtime
        .attach_ask_question_panel(ask_panel);
    if ctx.config.skills.enabled {
        ctx.spawn_skill_discovery_if_needed().await;
        let _ = ctx.await_skill_discovery().await;
    }
    let initial_main_call = ctx.resolve_call(LlmScene::Main, Some(&current_entry))?;
    let context_budget_chars = crate::infra::config::compute_context_budget_chars_from_tokens(
        initial_main_call.limits.input_budget_tokens,
    );
    let prompt_snapshot = crate::api::chat::build_prompt_snapshot(&ctx, context_budget_chars).await;
    let context_state = crate::core::session::manager::init_context_state_with_limits(
        &ctx.session_runtime.session,
        &ctx.config.context,
        prompt_snapshot.system_text(),
        &initial_main_call.limits,
    )?;
    if let Err(err) = ctx
        .session_runtime
        .plan_runtime
        .attach_from_resume_state(context_state.resume_control.clone())
    {
        tracing::warn!(error = %err, "plan_runtime attach_from_resume_state failed during serve slot init");
    }
    let ctx = Arc::new(ctx);
    Ok(Arc::new(SessionSlot::new(
        current_entry.session_id.clone(),
        ctx,
        mode,
        cwd_string,
        SessionTurnState {
            context_state,
            prompt_snapshot,
            context_budget_chars,
        },
    )))
}

pub(crate) fn register_slot_hooks(state: &Arc<ServeState>, slot: &Arc<SessionSlot>) {
    let event_ids = event_pump::register_session_event_pump(slot, state.writer.clone());
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
    if !slot
        .resume_pending_ask_question
        .swap(false, Ordering::SeqCst)
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
        let state = guard
            .take()
            .ok_or_else(|| AppError::Config("serve session turn state missing".to_string()))?;
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

pub(crate) async fn cleanup_session_slot(
    state: &ServeState,
    slot: &Arc<SessionSlot>,
    remove_from_registry: bool,
    reason: &str,
) -> Result<(), AppError> {
    slot.ctx.session_runtime.cancel_token.lock().cancel();
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
            }
            Err(_) => {
                tracing::warn!(
                    reason = reason,
                    session_id = %slot.session_id,
                    timeout_ms = SESSION_SHUTDOWN_TIMEOUT.as_millis(),
                    "serve session plugin cleanup timed out"
                );
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
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    drain_checkpoint_record_tasks(&slot.ctx, SESSION_SHUTDOWN_TIMEOUT).await;
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
    if remove_from_registry {
        state.registry.remove(&slot.session_id);
    }
    Ok(())
}

pub(crate) async fn run_slot_turn(
    slot: Arc<SessionSlot>,
    input_message: Option<ChatMessage>,
    turn_token: tokio_util::sync::CancellationToken,
) -> Result<crate::AgentRunOutcome, AppError> {
    let mut turn_state = TurnStateLease::acquire(Arc::clone(&slot))?;
    let result = {
        let (prompt_snapshot, context_state) = turn_state.prompt_snapshot_and_context_state_mut();
        crate::api::chat::run_chat_turn_with_message_and_snapshot(
            &slot.ctx,
            input_message,
            prompt_snapshot,
            context_state,
            turn_token,
        )
        .await
    };
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
