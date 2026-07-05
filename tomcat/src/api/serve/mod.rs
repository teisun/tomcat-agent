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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::api::chat::run_chat_turn_with_message;
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::core::agent_registry::AgentRegistry;
use crate::core::llm::ChatMessage;
use crate::{
    ensure_work_dir_structure, resolve_model_thinking_path, resolve_sessions_dir,
    session_key_for_agent, AppConfig, AppError, ModelThinkingStore, SessionManager, SessionMode,
    ThinkingLevel,
};

use ask_question::ServeAskQuestionBridge;
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
    pub shared_model_thinking: Arc<ModelThinkingStore>,
    pub shared_agent_registry: Arc<AgentRegistry>,
    pub shared_event_bus: Arc<FanoutEventBus>,
    pub initialized: AtomicBool,
}

impl ServeState {
    fn new(
        cfg: AppConfig,
        writer: WriterHandle,
        shared_model_thinking: Arc<ModelThinkingStore>,
    ) -> Arc<Self> {
        let registry = Arc::new(ChatContextRegistry::new(cfg.serve.max_sessions));
        let ask_question = ServeAskQuestionBridge::new(writer.clone());
        let shared_event_bus = Arc::new(FanoutEventBus::new());
        let shared_agent_registry = AgentRegistry::new().attach_event_bus(shared_event_bus.clone());
        Arc::new(Self {
            cfg,
            registry,
            writer,
            ask_question,
            shared_model_thinking,
            shared_agent_registry,
            shared_event_bus,
            initialized: AtomicBool::new(false),
        })
    }
}

pub(crate) fn build_shared_model_thinking(
    cfg: &AppConfig,
) -> Result<Arc<ModelThinkingStore>, AppError> {
    let default_level = ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0;
    Ok(Arc::new(ModelThinkingStore::load(
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
    let shared_model_thinking = build_shared_model_thinking(&cfg)?;
    let writer = writer::spawn_stdout_writer(WriterConfig::from(&cfg.serve));
    let state = ServeState::new(cfg, writer, shared_model_thinking);
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

pub(crate) fn normalize_session_mode(
    cfg: &AppConfig,
    explicit: Option<ServeSessionMode>,
) -> Result<SessionMode, AppError> {
    match explicit {
        Some(mode) => Ok(mode.into_core_mode()),
        None => default_mode(cfg),
    }
}

pub(crate) async fn create_session_slot(
    state: Arc<ServeState>,
    params: NewSessionParams,
    force_new: bool,
) -> Result<Arc<SessionSlot>, AppError> {
    let mode = normalize_session_mode(&state.cfg, params.mode)?;
    let cwd_path = params
        .cwd
        .as_deref()
        .map(crate::normalize_path)
        .transpose()?
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let sessions_dir = resolve_sessions_dir(&state.cfg)?;
    std::fs::create_dir_all(&sessions_dir).map_err(AppError::Io)?;
    let session_key = session_key_for_agent(&state.cfg.agent.id, mode, &cwd_path);
    let session_manager = SessionManager::new_scoped(sessions_dir, session_key);
    let cwd_string = Some(cwd_path.to_string_lossy().to_string());
    let current_entry = if force_new {
        session_manager.new_current_session(cwd_string.clone())?
    } else {
        session_manager.ensure_current_session(cwd_string.clone())?
    };
    session_manager.pin_session(&current_entry.session_id);

    let overrides = ChatContextOverrides::default()
        .suppress_cli_output()
        .with_shared_agent_registry(Arc::clone(&state.shared_agent_registry))
        .with_shared_model_thinking(Arc::clone(&state.shared_model_thinking))
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
    let context_budget_chars =
        crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let system_text = crate::api::chat::build_system_text(&ctx, context_budget_chars).await;
    let context_state = crate::init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        &system_text,
    )?;
    if let Err(err) = ctx
        .session_runtime
        .plan_runtime
        .attach_from_event(context_state.latest_plan_event.clone())
    {
        tracing::warn!(error = %err, "plan_runtime attach_from_event failed during serve slot init");
    }
    let ctx = Arc::new(ctx);
    Ok(Arc::new(SessionSlot::new(
        current_entry.session_id.clone(),
        ctx,
        mode,
        cwd_string,
        SessionTurnState {
            context_state,
            system_text,
            context_budget_chars,
        },
    )))
}

pub(crate) fn register_slot_hooks(state: &ServeState, slot: &Arc<SessionSlot>) {
    let event_ids = event_pump::register_session_event_pump(slot, state.writer.clone());
    slot.listener_ids.lock().extend(event_ids);
    let ask_listener = state.ask_question.register_request_listener(
        slot.session_id.clone(),
        slot.ctx.global_services.event_bus.clone(),
    );
    slot.listener_ids.lock().push(ask_listener);
}

struct TurnStateLease {
    context_state: Option<crate::ContextState>,
    context_budget_chars: usize,
    slot: Arc<SessionSlot>,
    system_text: String,
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
            system_text: state.system_text,
        })
    }

    fn context_budget_chars(&self) -> usize {
        self.context_budget_chars
    }

    fn context_state_mut(&mut self) -> &mut crate::ContextState {
        self.context_state
            .as_mut()
            .expect("turn state lease should always hold context_state")
    }

    fn replace_system_text(&mut self, system_text: String) {
        self.system_text = system_text;
    }

    fn system_text_len(&self) -> usize {
        self.system_text.len()
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
            system_text: std::mem::take(&mut self.system_text),
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

    event_pump::unregister_session_event_pump(slot);
    state.ask_question.clear_session(&slot.session_id);
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
    input_message: ChatMessage,
    turn_token: tokio_util::sync::CancellationToken,
) -> Result<crate::AgentRunOutcome, AppError> {
    let mut turn_state = TurnStateLease::acquire(Arc::clone(&slot))?;
    let next_system_text =
        crate::api::chat::build_system_text(&slot.ctx, turn_state.context_budget_chars()).await;
    let previous_system_text_len = turn_state.system_text_len();
    crate::api::chat::sync_context_state_system_prompt_len(
        turn_state.context_state_mut(),
        previous_system_text_len,
        next_system_text.len(),
    );
    turn_state.replace_system_text(next_system_text.clone());
    run_chat_turn_with_message(
        &slot.ctx,
        Some(input_message),
        &next_system_text,
        turn_state.context_state_mut(),
        turn_token,
    )
    .await
}
