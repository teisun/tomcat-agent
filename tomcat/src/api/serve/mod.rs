pub mod ask_question;
pub mod commands;
pub mod control;
pub mod event_pump;
pub mod ndjson;
pub mod registry;
pub mod schema;
pub mod stdin;
pub mod types;
pub mod writer;

#[cfg(test)]
mod test_support;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::api::chat::run_chat_turn;
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::{
    ensure_work_dir_structure, resolve_sessions_dir, session_key_for_agent, AppConfig, AppError,
    SessionManager, SessionMode,
};

use ask_question::ServeAskQuestionBridge;
use registry::{ChatContextRegistry, SessionSlot, SessionTurnState};
use types::{NewSessionParams, ServeSessionMode};
use writer::{WriterConfig, WriterHandle};

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
    pub initialized: AtomicBool,
}

impl ServeState {
    fn new(cfg: AppConfig, writer: WriterHandle) -> Arc<Self> {
        let registry = Arc::new(ChatContextRegistry::new(cfg.serve.max_sessions));
        let ask_question = ServeAskQuestionBridge::new(writer.clone());
        Arc::new(Self {
            cfg,
            registry,
            writer,
            ask_question,
            initialized: AtomicBool::new(false),
        })
    }
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
    let writer = writer::spawn_stdout_writer(WriterConfig::from(&cfg.serve));
    let state = ServeState::new(cfg, writer);
    let initial_slot =
        create_session_slot(Arc::clone(&state), NewSessionParams::default(), false).await?;
    state.registry.insert(Arc::clone(&initial_slot))?;
    register_slot_hooks(&state, &initial_slot);
    stdin::run_stdio_loop(state).await
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
        .with_session_cwd_override(cwd_path.clone());
    let ctx = ChatContext::from_config_with_mode_and_overrides(state.cfg.clone(), mode, overrides)?;
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

pub(crate) async fn run_slot_turn(
    slot: Arc<SessionSlot>,
    input: String,
) -> Result<crate::AgentRunOutcome, AppError> {
    let turn_token = {
        let mut guard = slot.ctx.session_runtime.cancel_token.lock();
        *guard = tokio_util::sync::CancellationToken::new();
        guard.clone()
    };

    let (mut context_state, system_text, context_budget_chars) = {
        let mut guard = slot.turn_state.lock();
        let state = guard
            .take()
            .ok_or_else(|| AppError::Config("serve session turn state missing".to_string()))?;
        (
            state.context_state,
            state.system_text,
            state.context_budget_chars,
        )
    };

    let next_system_text =
        crate::api::chat::build_system_text(&slot.ctx, context_budget_chars).await;
    crate::api::chat::sync_context_state_system_prompt_len(
        &mut context_state,
        system_text.len(),
        next_system_text.len(),
    );
    let outcome = run_chat_turn(
        &slot.ctx,
        &input,
        &next_system_text,
        &mut context_state,
        turn_token,
    )
    .await?;
    {
        let mut guard = slot.turn_state.lock();
        *guard = Some(SessionTurnState {
            context_state,
            system_text: next_system_text,
            context_budget_chars,
        });
    }
    Ok(outcome)
}
