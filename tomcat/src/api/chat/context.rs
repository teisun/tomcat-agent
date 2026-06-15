use std::future::Future;
use std::io::{self, Write as IoWrite};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::ext::plugin::PluginCatalog;
use crate::ext::{
    FunctionRegistry, HostApiDispatcher, PluginEngine, PluginEngineConfig, PluginFunctionInvoker,
    PluginManager, PluginRuntimeManager, PluginToolExecutor, RegisteredFunction,
    SharedPluginRuntimeManager,
};
use crate::infra::config::ThinkingDisplay;
use crate::infra::error::AppError;
use crate::infra::{
    AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder, TracingAuditRecorder,
};
use crate::{
    resolve_agent_definition_dir, resolve_agent_trail_dir, resolve_plugins_dir,
    resolve_sessions_dir, resolve_workspace_roots_paths, session_key_for_agent, AppConfig,
    DefaultPrimitiveExecutor, DefaultToolRegistry, LlmProvider, PrimitiveExecutor, SessionEntry,
    SessionManager, SessionMode, Tool, ToolExecutor, ToolRegistry,
};

use crate::core::llm::LlmScene;
use crate::core::plan_runtime;

use super::session_runtime::{GlobalServices, ScopeContainer, ScopeServices, SessionRuntime};
use super::{panels, permission};

pub struct ChatContext {
    pub global_services: GlobalServices,
    pub scope_services: ScopeServices,
    pub session_runtime: SessionRuntime,
    pub config: AppConfig,
    pub agent_registry: Arc<crate::core::agent_registry::AgentRegistry>,
    _root_agent_guard: crate::core::agent_registry::RegistrationGuard,
}

#[derive(Default)]
pub struct ChatContextOverrides {
    pub ask_question_panel: Option<Arc<dyn panels::AskQuestionPanel>>,
}

impl ChatContextOverrides {
    pub fn with_ask_question_panel(mut self, panel: Arc<dyn panels::AskQuestionPanel>) -> Self {
        self.ask_question_panel = Some(panel);
        self
    }
}

fn git_available_for_checkpoints() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn resolve_agent_workspace_dir(
    session_entry: &SessionEntry,
    agent_definition_dir: &std::path::Path,
) -> std::path::PathBuf {
    if let Some(cwd) = session_entry.cwd.as_deref() {
        let path = std::path::PathBuf::from(cwd);
        if path.exists() {
            return path;
        }
        warn!(
            cwd = %cwd,
            "session cwd no longer exists; falling back to current shell directory"
        );
    }
    std::env::current_dir().unwrap_or_else(|_| agent_definition_dir.to_path_buf())
}

fn resolve_child_agent_compaction_runtime(
    llm_resolver: &dyn crate::core::llm::LlmResolver,
    base_context_config: &crate::infra::config::ContextConfig,
    entry: Option<&SessionEntry>,
) -> (
    crate::infra::config::ContextConfig,
    Option<Arc<dyn LlmProvider>>,
) {
    let mut context_config = base_context_config.clone();
    let session_override = entry
        .and_then(|e| e.model_override.as_deref())
        .filter(|model| !model.trim().is_empty());
    match llm_resolver.resolve(LlmScene::Compaction, session_override) {
        Ok(call) => {
            context_config.compaction_model = call.model;
            (context_config, Some(call.provider_impl))
        }
        Err(err) => {
            warn!(
                error = %err,
                "failed to resolve child-agent compaction runtime; falling back to main provider"
            );
            (context_config, None)
        }
    }
}

fn checkpoint_store_cache() -> &'static RwLock<
    std::collections::HashMap<
        std::path::PathBuf,
        std::sync::Weak<crate::core::SwitchingCheckpointStore>,
    >,
> {
    static CACHE: OnceLock<
        RwLock<
            std::collections::HashMap<
                std::path::PathBuf,
                std::sync::Weak<crate::core::SwitchingCheckpointStore>,
            >,
        >,
    > = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

fn checkpoint_store_for(
    agent_trail_dir: std::path::PathBuf,
    work_tree: std::path::PathBuf,
) -> Arc<crate::core::SwitchingCheckpointStore> {
    let key = std::fs::canonicalize(&work_tree).unwrap_or(work_tree);
    if let Some(existing) = checkpoint_store_cache()
        .read()
        .get(&key)
        .and_then(std::sync::Weak::upgrade)
    {
        return existing;
    }

    let mut cache = checkpoint_store_cache().write();
    if let Some(existing) = cache.get(&key).and_then(std::sync::Weak::upgrade) {
        return existing;
    }
    let store = Arc::new(crate::core::SwitchingCheckpointStore::new(
        agent_trail_dir,
        key.clone(),
        git_available_for_checkpoints(),
    ));
    cache.insert(key, Arc::downgrade(&store));
    store
}

fn scope_runtime_cache(
) -> &'static RwLock<std::collections::HashMap<std::path::PathBuf, Weak<ScopeContainer>>> {
    static CACHE: OnceLock<
        RwLock<std::collections::HashMap<std::path::PathBuf, Weak<ScopeContainer>>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

fn scope_runtime_for(
    config: &AppConfig,
    agent_workspace_dir: std::path::PathBuf,
    audit: Arc<dyn AuditRecorder>,
    llm: Arc<dyn LlmProvider>,
    llm_resolver: Arc<dyn crate::core::LlmResolver>,
    primitive: Arc<dyn PrimitiveExecutor>,
    session: Arc<SessionManager>,
) -> Result<Arc<ScopeContainer>, AppError> {
    let key = std::fs::canonicalize(&agent_workspace_dir).unwrap_or(agent_workspace_dir);
    if let Some(existing) = scope_runtime_cache()
        .read()
        .get(&key)
        .and_then(Weak::upgrade)
    {
        return Ok(existing);
    }

    let mut cache = scope_runtime_cache().write();
    if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
        return Ok(existing);
    }

    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let deps = PluginRuntimeDeps {
        audit,
        event_bus: event_bus.clone(),
        llm,
        llm_resolver,
        primitive,
        session,
    };
    let (tool_registry, function_registry, plugin_manager, plugin_function_invoker, dispatcher) =
        build_plugin_runtime(config, &key, deps)?;
    let shared = Arc::new(ScopeContainer {
        event_bus,
        tool_registry,
        function_registry,
        plugin_manager,
        plugin_function_invoker,
        dispatcher,
        skill_set: Arc::new(RwLock::new(crate::core::skill::SkillSet::default())),
        skill_discovery_handle: Arc::new(tokio::sync::Mutex::new(None)),
    });
    cache.insert(key, Arc::downgrade(&shared));
    Ok(shared)
}

fn block_on_plugin_future<F, T>(future: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>> + Send + 'static,
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return std::thread::spawn(move || handle.block_on(future))
            .join()
            .map_err(|_| AppError::Plugin("scope activation worker panicked".to_string()))?;
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|error| {
        AppError::Plugin(format!(
            "create runtime for scope activation failed: {error}"
        ))
    })?;
    runtime.block_on(future)
}

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        Self::from_config_with_mode_and_overrides(
            config,
            SessionMode::Claw,
            ChatContextOverrides::default(),
        )
    }

    pub fn from_config_with_mode(config: AppConfig, mode: SessionMode) -> Result<Self, AppError> {
        Self::from_config_with_mode_and_overrides(config, mode, ChatContextOverrides::default())
    }

    pub fn from_config_with_overrides(
        config: AppConfig,
        overrides: ChatContextOverrides,
    ) -> Result<Self, AppError> {
        Self::from_config_with_mode_and_overrides(config, SessionMode::Claw, overrides)
    }

    pub fn from_config_with_mode_and_overrides(
        config: AppConfig,
        mode: SessionMode,
        overrides: ChatContextOverrides,
    ) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        let sessions_path_for_appender = sessions_path.clone();
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let cwd_for_key = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let session_key = session_key_for_agent(&config.agent.id, mode, &cwd_for_key);
        let session = SessionManager::new_scoped(sessions_path, session_key);
        let message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink> =
            Arc::new(session.clone());
        let session_cwd = Some(cwd_for_key.to_string_lossy().to_string());
        let agent_definition_dir = resolve_agent_definition_dir(&config)?;
        std::fs::create_dir_all(&agent_definition_dir).map_err(AppError::Io)?;
        let agent_trail_dir = resolve_agent_trail_dir(&config)?;
        std::fs::create_dir_all(&agent_trail_dir).map_err(AppError::Io)?;
        let current_session_entry = session.ensure_current_session(session_cwd.clone())?;
        migrate_legacy_layer0_tool_results(&agent_definition_dir, &agent_trail_dir);

        let agent_workspace_dir =
            resolve_agent_workspace_dir(&current_session_entry, &agent_definition_dir);
        let cfg_path_snapshot =
            crate::api::cli::config_file_path().unwrap_or_else(|_| std::path::PathBuf::new());

        let model_catalog = Arc::new(crate::core::llm::ModelCatalog::load(&config)?);
        let web_search_runtime = Arc::new(crate::core::tools::web_search::WebSearchRuntime::new(
            &config,
            model_catalog.clone(),
        )?);
        let web_fetch_runtime = Arc::new(crate::core::tools::web_fetch::WebFetchRuntime::new(
            &config,
            agent_trail_dir.join("tool-results"),
        )?);
        let llm_resolver: Arc<dyn crate::core::llm::LlmResolver> = Arc::new(
            crate::core::llm::DefaultLlmResolver::new(config.clone(), model_catalog.clone()),
        );
        let llm: Arc<dyn LlmProvider> = crate::core::llm::resolve_llm(&config.llm)?;
        let openai_files_runtime = crate::core::llm::openai_files::build_runtime_for_provider(
            llm.as_ref(),
            &config.llm.files,
            session.sessions_dir(),
            &current_session_entry.session_id,
        )
        .map(Arc::new);

        let audit: Arc<dyn AuditRecorder> = match AuditStore::open_if_enabled(&config)? {
            Some(store) => Arc::new(FileAuditRecorder::new(Arc::new(store))),
            None => Arc::new(TracingAuditRecorder),
        };
        let workspace_roots = resolve_workspace_roots_paths(&config)?;
        let cli_confirmation: Arc<dyn UserConfirmationProvider> = Arc::new(CliConfirmation);

        let session_grants = crate::core::permission::SessionGrants::new();
        let agent_trail_readonly_dirs: Vec<std::path::PathBuf> = vec![
            Some(agent_trail_dir.clone()),
            crate::infra::config::resolve_sessions_dir(&config).ok(),
            crate::infra::config::resolve_log_dir(&config).ok(),
            crate::infra::config::resolve_audit_dir(&config).ok(),
            crate::infra::config::resolve_agent_dir(&config).ok(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let gate_cfg = crate::core::permission::GateConfig {
            agent_definition_dir: agent_definition_dir.clone(),
            workspace_roots: workspace_roots.clone(),
            agent_trail_readonly_dirs: agent_trail_readonly_dirs.clone(),
            user_path_rules: config.primitive.path_rules.clone(),
            user_bash_forbidden: config.primitive.bash_forbidden.clone(),
            user_bash_approval: config.primitive.bash_approval_required.clone(),
            auto_confirm: config.primitive.auto_confirm,
        };
        let gate: Arc<dyn crate::core::permission::PermissionGate> = Arc::new(
            crate::core::permission::DefaultPermissionGate::new(gate_cfg, session_grants.clone()),
        );

        let confirmation: Arc<dyn UserConfirmationProvider> =
            Arc::new(permission::cwd_lazy::CwdLazyPrompt::new(
                cli_confirmation,
                agent_workspace_dir.clone(),
                gate.clone(),
                session_grants.clone(),
                cfg_path_snapshot.clone(),
            ));

        let _ = workspace_roots;
        let bash_ast = crate::core::permission::BashAstChecker::new(false, vec![], vec![]);
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(
            DefaultPrimitiveExecutor::new(
                config.primitive.clone(),
                confirmation.clone(),
                audit.clone(),
                gate.clone(),
            )
            .with_bash_ast(bash_ast.clone())
            .with_write_normalize_crlf(config.tools.write.normalize_crlf),
        );

        let config_backend: Option<crate::core::agent_loop::SharedConfigBackend> =
            match crate::api::cli::config_file_path() {
                Ok(p) => Some(Arc::new(
                    crate::core::tools::config_tool::ChatConfigBackend {
                        ctx: crate::core::tools::config_tool::ConfigToolContext::new(
                            p,
                            confirmation.clone(),
                        )
                        .with_gate(gate.clone()),
                    },
                )),
                Err(_) => None,
            };

        let checkpoint_switcher =
            checkpoint_store_for(agent_trail_dir.clone(), agent_workspace_dir.clone());
        let checkpoint_store: Arc<dyn crate::core::CheckpointStore> = checkpoint_switcher.clone();

        let session_arc = Arc::new(session.clone());
        let shared_scope_runtime = scope_runtime_for(
            &config,
            agent_workspace_dir.clone(),
            audit.clone(),
            llm.clone(),
            llm_resolver.clone(),
            primitive.clone(),
            session_arc.clone(),
        )?;
        shared_scope_runtime.dispatcher.bind_session(
            &current_session_entry.session_id,
            Arc::downgrade(&session_arc),
        );
        let event_bus = shared_scope_runtime.event_bus.clone();
        let tool_registry = shared_scope_runtime.tool_registry.clone();
        let function_registry = shared_scope_runtime.function_registry.clone();
        let plugin_manager = shared_scope_runtime.plugin_manager.clone();
        let plugin_function_invoker = shared_scope_runtime.plugin_function_invoker.clone();
        if let Some(function_invoker) = plugin_function_invoker.as_ref() {
            web_search_runtime.set_plugin_invoker(crate::ext::ExtPluginSearchInvoker::new(
                function_registry.clone(),
                function_invoker.clone(),
            ));
        }
        if let Some(plugin_manager_ref) = plugin_manager.as_ref() {
            for plugin_id in plugin_manager_ref.list_loaded() {
                let Some(info) = plugin_manager_ref.get_plugin(&plugin_id) else {
                    continue;
                };
                if info.manifest.tools.is_empty() && info.loaded_at == 0 {
                    if let Err(err) = plugin_manager_ref.load_plugin(&info.plugin_root) {
                        warn!(
                            plugin = %plugin_id,
                            path = %info.plugin_root.display(),
                            error = %err,
                            "scope activation failed to pre-register legacy dynamic plugin"
                        );
                    }
                    if info.manifest.activation == crate::ext::PluginActivation::Lazy {
                        continue;
                    }
                }
                if info.manifest.activation != crate::ext::PluginActivation::Session {
                    continue;
                }
                if plugin_manager_ref.has_session_vm(&current_session_entry.session_id, &plugin_id)
                {
                    continue;
                }

                let pm = Arc::clone(plugin_manager_ref);
                let session_id = current_session_entry.session_id.clone();
                let plugin_id_for_start = plugin_id.clone();
                if let Err(err) = block_on_plugin_future(async move {
                    pm.start_session_vm(&session_id, &plugin_id_for_start)
                        .await
                        .map(|_| ())
                }) {
                    warn!(
                        plugin = %plugin_id,
                        session = %current_session_entry.session_id,
                        error = %err,
                        "scope activation failed to prestart session plugin"
                    );
                    continue;
                }
                if let Err(err) = plugin_manager_ref.dispatch_session_event(
                    &current_session_entry.session_id,
                    &plugin_id,
                    crate::infra::wire::vm::WIRE_SESSION_START,
                    serde_json::json!({}),
                    serde_json::json!({
                        "sessionId": current_session_entry.session_id.clone(),
                    }),
                ) {
                    warn!(
                        plugin = %plugin_id,
                        session = %current_session_entry.session_id,
                        error = %err,
                        "scope activation failed to deliver session_start"
                    );
                }
            }
        }
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));

        let bash_task_registry = Arc::new(
            crate::core::tools::primitive::BashTaskRegistry::new(
                agent_trail_dir.join("tool-results"),
            )
            .with_background_guard(
                crate::core::tools::primitive::bash_task::BackgroundBashGuard::new(
                    "__agent__",
                    gate.clone(),
                    confirmation.clone(),
                    audit.clone(),
                    bash_ast,
                ),
            ),
        );
        let follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>> =
            Arc::new(Mutex::new(Vec::new()));
        let completion_routes: crate::core::agent_loop::BackgroundCompletionRoutes =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let delivered_completion: Arc<
            Mutex<std::collections::HashSet<crate::core::tools::primitive::BashTaskId>>,
        > = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(None));

        let initial_thinking_display = resolve_initial_thinking_display(&config.llm.thinking);

        let todos_runtime = Arc::new(plan_runtime::todo_runtime::TodosRuntime::new(
            agent_trail_dir.clone(),
            current_session_entry.session_id.clone(),
        ));
        let plan_runtime = plan_runtime::PlanRuntime::new_with_session_id(
            session.current_session_key(),
            current_session_entry.session_id.clone(),
        );
        let ask_question_panel: Arc<dyn panels::AskQuestionPanel> = overrides
            .ask_question_panel
            .unwrap_or_else(|| Arc::new(panels::CliAskQuestionPanel));
        plan_runtime.set_ask_question_timeout_ms(Some(config.ask_question.timeout_ms));
        plan_runtime.set_auto_checkpoint_on_build(config.plan.auto_checkpoint_on_build);
        plan_runtime.set_verify_gate_mode(config.plan.verify_gate.clone());
        plan_runtime.set_max_code_review_rounds(config.plan.max_code_review_rounds);
        plan_runtime.set_expose_skills_to_reviewer(config.skills.expose_to_reviewer);
        plan_runtime.attach_checkpoint_store(checkpoint_store.clone());
        plan_runtime.register_todos_panel(Arc::new(panels::CliTodosPanel));
        plan_runtime.attach_ask_question_panel(ask_question_panel);

        let agent_registry =
            crate::core::agent_registry::AgentRegistry::new().attach_event_bus(event_bus.clone());
        let root_agent_guard = agent_registry
            .register_root(current_session_entry.session_id.clone())
            .map_err(|e| AppError::Config(format!("agent_registry root register 失败: {e}")))?;
        let skill_set = shared_scope_runtime.skill_set.clone();
        let skill_discovery_handle = shared_scope_runtime.skill_discovery_handle.clone();

        let reviewer_max_turns = std::env::var("TOMCAT_REVIEWER_MAX_TURNS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(config.reviewer.max_turns);
        let reviewer_model = config
            .reviewer
            .model_override
            .clone()
            .unwrap_or_else(|| config.llm.default_model.clone());
        let (child_agent_context_config, child_agent_compaction_provider) =
            resolve_child_agent_compaction_runtime(
                llm_resolver.as_ref(),
                &config.context,
                Some(&current_session_entry),
            );
        let read_file_state =
            Arc::new(crate::core::tools::pipeline::read_state::ReadFileState::default());
        let prod_reviewer = plan_runtime::prod_reviewer::ProdReviewerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm: llm.clone(),
                compaction_provider: child_agent_compaction_provider.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: child_agent_context_config.clone(),
                read_file_state: read_file_state.clone(),
                openai_files_runtime: openai_files_runtime.clone(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model: reviewer_model,
                max_turns: reviewer_max_turns,
            },
        );
        plan_runtime.attach_reviewer(Arc::new(prod_reviewer));
        let prod_verifier = plan_runtime::verify::ProdVerifierDispatcher::new(
            "chat_context",
            plan_runtime::verify::ProdVerifierDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm: llm.clone(),
                compaction_provider: child_agent_compaction_provider.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: child_agent_context_config.clone(),
                read_file_state: read_file_state.clone(),
                openai_files_runtime: openai_files_runtime.clone(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model: config.llm.default_model.clone(),
            },
        );
        plan_runtime.attach_verifier(Arc::new(prod_verifier));

        {
            let appender_session_key = session.current_session_key().to_string();
            plan_runtime.attach_transcript_appender(Arc::new(move |extra| {
                let sm = SessionManager::new_scoped(
                    sessions_path_for_appender.clone(),
                    appender_session_key.clone(),
                );
                sm.append_custom_entry(extra)
            }));
        }

        if let Err(err) = plan_runtime.recover() {
            warn!(error = %err, "plan_runtime recover failed; continuing with Chat mode");
        }

        let thinking_display = Arc::new(std::sync::atomic::AtomicU8::new(
            initial_thinking_display.as_u8(),
        ));
        let global_services = GlobalServices {
            llm: llm.clone(),
            model_catalog: model_catalog.clone(),
            llm_resolver: llm_resolver.clone(),
            primitive: primitive.clone(),
            tool_registry: tool_registry.clone(),
            function_registry: function_registry.clone(),
            event_bus: event_bus.clone(),
            audit: audit.clone(),
            gate: gate.clone(),
            config_backend: config_backend.clone(),
            web_fetch_runtime: web_fetch_runtime.clone(),
            web_search_runtime: web_search_runtime.clone(),
            plugin_manager,
            plugin_function_invoker,
        };
        let scope_services = ScopeServices {
            scope_container: shared_scope_runtime.clone(),
            checkpoint_switcher: checkpoint_switcher.clone(),
            checkpoint_store: checkpoint_store.clone(),
            agent_workspace_dir: agent_workspace_dir.clone(),
            agent_definition_dir: agent_definition_dir.clone(),
            agent_trail_dir: agent_trail_dir.clone(),
            cfg_path: cfg_path_snapshot.clone(),
            skill_set: skill_set.clone(),
            skill_discovery_handle: skill_discovery_handle.clone(),
        };
        let session_runtime = SessionRuntime {
            session: session.clone(),
            message_append_sink: message_append_sink.clone(),
            cancel_token: cancel_token.clone(),
            last_interrupt_at: last_interrupt_at.clone(),
            session_grants: session_grants.clone(),
            bash_task_registry: bash_task_registry.clone(),
            follow_up_queue: follow_up_queue.clone(),
            completion_routes: completion_routes.clone(),
            delivered_completion: delivered_completion.clone(),
            completion_subscriber_handle: completion_subscriber_handle.clone(),
            read_file_state: read_file_state.clone(),
            thinking_display: thinking_display.clone(),
            openai_files_runtime: openai_files_runtime.clone(),
            todos_runtime: todos_runtime.clone(),
            plan_runtime: plan_runtime.clone(),
        };

        Ok(Self {
            global_services,
            scope_services,
            session_runtime,
            config,
            agent_registry,
            _root_agent_guard: root_agent_guard,
        })
    }

    pub(crate) fn effective_model(&self, entry: Option<&SessionEntry>) -> String {
        entry
            .and_then(|e| e.model_override.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.config.llm.default_model)
            .to_string()
    }

    pub(crate) fn resolve_call(
        &self,
        scene: LlmScene,
        entry: Option<&SessionEntry>,
    ) -> Result<crate::core::llm::ResolvedCall, AppError> {
        let session_override = entry
            .and_then(|e| e.model_override.as_deref())
            .filter(|model| !model.trim().is_empty());
        self.global_services
            .llm_resolver
            .resolve(scene, session_override)
    }

    pub(crate) fn openai_files_runtime_for(
        &self,
        provider: &dyn LlmProvider,
    ) -> Option<Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>> {
        let session_id = self
            .session_runtime
            .session
            .current_session_id()
            .ok()
            .flatten()?;
        crate::core::llm::openai_files::build_runtime_for_provider(
            provider,
            &self.config.llm.files,
            self.session_runtime.session.sessions_dir(),
            &session_id,
        )
        .map(Arc::new)
    }

    pub(crate) fn shutdown_completion_subscriber(&self) {
        if let Some(handle) = self
            .session_runtime
            .completion_subscriber_handle
            .lock()
            .take()
        {
            handle.abort();
        }
    }

    pub(crate) fn skill_set_snapshot(&self) -> crate::core::skill::SkillSet {
        self.scope_services.skill_set.read().clone()
    }

    pub(crate) async fn spawn_skill_discovery_if_needed(&self) {
        if !self.config.skills.enabled || !self.scope_services.skill_set.read().is_empty() {
            return;
        }
        let mut handle = self.scope_services.skill_discovery_handle.lock().await;
        if handle.is_none() {
            *handle = Some(crate::core::skill::spawn_discovery_task(
                self.config.clone(),
                self.scope_services.agent_workspace_dir.clone(),
            ));
        }
    }

    pub(crate) async fn await_skill_discovery(&self) -> crate::core::skill::SkillSet {
        let handle = self
            .scope_services
            .skill_discovery_handle
            .lock()
            .await
            .take();
        if let Some(handle) = handle {
            match handle.await {
                Ok(skill_set) => {
                    *self.scope_services.skill_set.write() = skill_set.clone();
                    skill_set
                }
                Err(error) => {
                    let mut failed = crate::core::skill::SkillSet::default();
                    failed
                        .warnings
                        .push(format!("skills_discovery_join_failed:{error}"));
                    *self.scope_services.skill_set.write() = failed.clone();
                    failed
                }
            }
        } else {
            self.skill_set_snapshot()
        }
    }

    pub(crate) async fn reload_skill_set(&self) -> crate::core::skill::SkillSet {
        if let Some(handle) = self
            .scope_services
            .skill_discovery_handle
            .lock()
            .await
            .take()
        {
            handle.abort();
        }
        let skill_set = if self.config.skills.enabled {
            crate::core::skill::discover(&self.config, &self.scope_services.agent_workspace_dir)
        } else {
            crate::core::skill::SkillSet::default()
        };
        *self.scope_services.skill_set.write() = skill_set.clone();
        skill_set
    }

    pub(crate) async fn refresh_plugin_catalog_inventory(&self) -> Result<Vec<String>, AppError> {
        let Some(plugin_manager) = self.global_services.plugin_manager.as_ref() else {
            return Ok(Vec::new());
        };
        let current_session_id = self
            .session_runtime
            .session
            .current_session_id()
            .ok()
            .flatten();

        let catalog =
            PluginCatalog::discover(&self.config, &self.scope_services.agent_workspace_dir)?;
        let discovered_ids = catalog
            .iter()
            .map(|(plugin_id, _)| plugin_id.clone())
            .collect::<std::collections::HashSet<_>>();

        for existing_id in plugin_manager.list_loaded() {
            let Some(info) = plugin_manager.get_plugin(&existing_id) else {
                continue;
            };
            let has_session_vm = current_session_id
                .as_deref()
                .map(|session_id| plugin_manager.has_session_vm(session_id, &existing_id))
                .unwrap_or(false);
            if info.loaded_at != 0 || has_session_vm {
                continue;
            }
            if discovered_ids.contains(&existing_id) {
                continue;
            }
            self.global_services
                .tool_registry
                .unregister_plugin_tools(&existing_id);
            let _ = plugin_manager.unload_plugin(&existing_id);
        }

        for (plugin_id, entry) in catalog.iter() {
            let loaded = plugin_manager
                .get_plugin(plugin_id)
                .map(|info| info.loaded_at > 0)
                .unwrap_or(false);
            let has_session_vm = current_session_id
                .as_deref()
                .map(|session_id| plugin_manager.has_session_vm(session_id, plugin_id))
                .unwrap_or(false);
            if loaded || has_session_vm {
                continue;
            }

            plugin_manager.register_catalog_plugin(&entry.plugin_root, entry.manifest.clone())?;
            self.global_services
                .tool_registry
                .unregister_plugin_tools(plugin_id);
            for manifest_tool in &entry.manifest.tools {
                self.global_services
                    .tool_registry
                    .register_tool(
                        Tool {
                            name: manifest_tool.name.clone(),
                            label: manifest_tool.name.clone(),
                            description: manifest_tool.description.clone(),
                            parameters: manifest_tool.parameters.clone(),
                            plugin_id: plugin_id.clone(),
                            is_enabled: true,
                            created_at: 0,
                        },
                        plugin_id,
                    )
                    .await?;
            }
        }

        let function_catalog =
            refresh_host_function_registry(&self.config, &self.global_services.function_registry)?;
        let mut warnings = catalog.warnings.clone();
        warnings.extend(function_catalog.warnings.clone());
        warnings.extend(catalog.diagnostics.iter().map(|diagnostic| {
            format!(
                "plugin catalog ignored {}: {}",
                diagnostic.path.display(),
                diagnostic.reason
            )
        }));
        warnings.extend(function_catalog.diagnostics.iter().map(|diagnostic| {
            format!(
                "host function catalog ignored {}: {}",
                diagnostic.path.display(),
                diagnostic.reason
            )
        }));
        Ok(warnings)
    }
}

impl Drop for ChatContext {
    fn drop(&mut self) {
        self.shutdown_completion_subscriber();
    }
}

pub struct CliConfirmation;

#[async_trait::async_trait]
impl UserConfirmationProvider for CliConfirmation {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        println!("\n--- 操作确认 ---");
        let source_label = if plugin_id == "__agent__" {
            "host".to_string()
        } else {
            plugin_id.to_string()
        };
        println!("类型: {:?}  来源: {}", operation, source_label);
        if !preview.is_empty() {
            let lines: Vec<&str> = preview.lines().collect();
            let display = if lines.len() > 20 {
                format!(
                    "{}\n  ... ({} 行已省略)",
                    lines[..20].join("\n"),
                    lines.len() - 20
                )
            } else {
                preview.to_string()
            };
            println!("预览:\n{}", display);
        }
        print!("是否执行？[y/N] ");
        io::stdout().flush().map_err(AppError::Io)?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).map_err(AppError::Io)?;
        let answer = line.trim().to_lowercase();
        Ok(answer == "y" || answer == "yes")
    }

    async fn confirm_decision(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        suggested_root: Option<std::path::PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        if operation == PrimitiveOperation::Bash {
            return match self.confirm(operation, preview, plugin_id).await? {
                true => Ok(ConfirmDecision::AllowOnce),
                false => Ok(ConfirmDecision::Deny),
            };
        }

        let target = extract_path_from_preview(preview).unwrap_or_else(|| {
            suggested_root
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        });
        match permission::prompt::read_path_prompt(
            &target,
            suggested_root,
            Some(&format!("类型: {:?}  来源: {}", operation, plugin_id)),
        )
        .map_err(AppError::Io)?
        {
            permission::prompt::PathPromptChoice::AllowSession => Ok(ConfirmDecision::AllowOnce),
            permission::prompt::PathPromptChoice::PersistWorkspaceRoot { root } => {
                let cfg_path = crate::api::cli::config_file_path()?;
                crate::infra::config::append_workspace_root_to_disk(
                    &cfg_path,
                    root.to_string_lossy().into_owned(),
                )?;
                Ok(ConfirmDecision::AllowAndPersistRoot { root })
            }
            permission::prompt::PathPromptChoice::Cancel => Ok(ConfirmDecision::Deny),
        }
    }
}

fn extract_path_from_preview(preview: &str) -> Option<std::path::PathBuf> {
    preview
        .lines()
        .find_map(|line| line.strip_prefix("路径: "))
        .map(std::path::PathBuf::from)
}

type PluginRuntimeParts = (
    Arc<dyn ToolRegistry>,
    Arc<FunctionRegistry>,
    Option<Arc<PluginManager>>,
    Option<Arc<PluginFunctionInvoker>>,
    Arc<HostApiDispatcher>,
);

struct PluginRuntimeDeps {
    audit: Arc<dyn AuditRecorder>,
    event_bus: Arc<dyn EventBus>,
    llm: Arc<dyn LlmProvider>,
    llm_resolver: Arc<dyn crate::core::LlmResolver>,
    primitive: Arc<dyn PrimitiveExecutor>,
    session: Arc<SessionManager>,
}

fn canonicalize_or_keep(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn materialize_host_functions_from_catalog(registry: &FunctionRegistry, catalog: &PluginCatalog) {
    let registered = catalog
        .iter()
        .flat_map(|(plugin_id, entry)| {
            entry
                .manifest
                .functions
                .iter()
                .map(|function| RegisteredFunction {
                    plugin_id: plugin_id.clone(),
                    plugin_root: canonicalize_or_keep(&entry.plugin_root),
                    point: function.point.clone(),
                    function: function.function.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    registry.replace_all(registered);
}

fn refresh_host_function_registry(
    config: &AppConfig,
    registry: &FunctionRegistry,
) -> Result<PluginCatalog, AppError> {
    let catalog = PluginCatalog::discover_host_root(config)?;
    materialize_host_functions_from_catalog(registry, &catalog);
    Ok(catalog)
}

fn build_plugin_runtime(
    config: &AppConfig,
    agent_workspace_dir: &std::path::Path,
    deps: PluginRuntimeDeps,
) -> Result<PluginRuntimeParts, AppError> {
    let PluginRuntimeDeps {
        audit,
        event_bus,
        llm,
        llm_resolver,
        primitive,
        session,
    } = deps;
    if plugin_runtime_disabled_via_env() {
        warn!("PI_PLUGIN_DISABLE enabled; skipping plugin runtime initialization");
        let executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(executor, audit.clone()));
        let function_registry = Arc::new(FunctionRegistry::new());
        let dispatcher = Arc::new(
            HostApiDispatcher::new(event_bus.clone())
                .with_tools(tool_registry.clone())
                .with_session(session)
                .with_llm(llm)
                .with_llm_resolver(llm_resolver)
                .with_primitive(primitive)
                .with_audit(audit),
        );
        return Ok((tool_registry, function_registry, None, None, dispatcher));
    }

    let mut plugin_manager = Arc::new(PluginManager::new(event_bus.clone()));
    let plugin_manager_strong_count = Arc::strong_count(&plugin_manager);
    let inner = Arc::get_mut(&mut plugin_manager).ok_or_else(|| {
        AppError::Plugin(format!(
            "plugin_manager unexpectedly shared before runtime init (strong_count={})",
            plugin_manager_strong_count
        ))
    })?;
    inner.set_plugin_engine(PluginEngine::global(Some(PluginEngineConfig {
        quickjs_heap_mb: config.plugin.js_heap_mb,
        call_timeout_ms: config.plugin.call_timeout_ms,
        interrupt_budget: config.plugin.interrupt_budget,
        idle_ttl_ms: config.plugin.idle_ttl_ms,
    }))?);
    let runtime_manager: SharedPluginRuntimeManager =
        Arc::new(PluginRuntimeManager::with_idle_ttl(
            std::time::Duration::from_millis(config.plugin.idle_ttl_ms),
        ));
    inner.set_plugin_runtime_manager(runtime_manager);
    inner.set_audit_recorder(audit.clone());
    inner.set_event_channel_capacity(config.plugin.event_channel_capacity);
    inner.set_confirm_permissions(Arc::new(|_| Ok(true)));

    let executor = PluginToolExecutor::new(Arc::downgrade(&plugin_manager));
    let function_registry = Arc::new(FunctionRegistry::new());
    let default_tool_registry = Arc::new(DefaultToolRegistry::new(executor.clone(), audit.clone()));
    let tool_registry: Arc<dyn ToolRegistry> = default_tool_registry.clone();
    let mut fetch_client_builder = reqwest::Client::builder()
        .dns_resolver(Arc::new(crate::infra::net_guard::PublicIpDnsResolver))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_millis(
            config.tools.web_fetch.fetch_timeout_ms,
        ));
    if let Some(proxy_url) = config.llm.proxy.as_deref() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| AppError::Config(format!("代理 URL 无效 {}: {}", proxy_url, e)))?;
        fetch_client_builder = fetch_client_builder.proxy(proxy);
    } else {
        fetch_client_builder = fetch_client_builder.no_proxy();
    }
    let fetch_client = fetch_client_builder
        .build()
        .map_err(|e| AppError::Tool(format!("创建 plugin net.fetch HTTP 客户端失败: {}", e)))?;
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_tools(tool_registry.clone())
            .with_session(session.clone())
            .with_llm(llm)
            .with_llm_resolver(llm_resolver)
            .with_primitive(primitive)
            .with_plugin_manager(Arc::downgrade(&plugin_manager))
            .with_fetch_http_client(fetch_client)
            .with_fetch_max_body_bytes(config.tools.web_fetch.max_http_content_bytes)
            .with_audit(audit),
    );
    let function_invoker = PluginFunctionInvoker::new(Arc::downgrade(&plugin_manager));
    executor.attach_dispatcher(Arc::downgrade(&dispatcher));
    function_invoker.attach_dispatcher(Arc::downgrade(&dispatcher));
    plugin_manager.set_tool_registry(tool_registry.clone());
    plugin_manager.set_function_registry(function_registry.clone());
    plugin_manager.set_host_dispatcher(dispatcher.clone());

    let catalog = PluginCatalog::discover(config, agent_workspace_dir)?;
    for (plugin_id, entry) in catalog.iter() {
        if let Err(err) =
            plugin_manager.register_catalog_plugin(&entry.plugin_root, entry.manifest.clone())
        {
            warn!(
                plugin = %plugin_id,
                path = %entry.plugin_root.display(),
                error = %err,
                "catalog register plugin failed; continuing without this plugin"
            );
            continue;
        }
        for manifest_tool in &entry.manifest.tools {
            let tool = Tool {
                name: manifest_tool.name.clone(),
                label: manifest_tool.name.clone(),
                description: manifest_tool.description.clone(),
                parameters: manifest_tool.parameters.clone(),
                plugin_id: plugin_id.clone(),
                is_enabled: true,
                created_at: 0,
            };
            if let Err(err) = default_tool_registry.register_tool_local(tool, plugin_id) {
                warn!(
                    plugin = %plugin_id,
                    tool = %manifest_tool.name,
                    error = %err,
                    "catalog materialize static tool failed; continuing without this tool"
                );
            }
        }
    }
    for diagnostic in &catalog.diagnostics {
        warn!(
            path = %diagnostic.path.display(),
            reason = %diagnostic.reason,
            "plugin catalog scan ignored invalid entry"
        );
    }
    let host_function_catalog = refresh_host_function_registry(config, &function_registry)?;
    for diagnostic in &host_function_catalog.diagnostics {
        warn!(
            path = %diagnostic.path.display(),
            reason = %diagnostic.reason,
            "host function catalog scan ignored invalid entry"
        );
    }

    let plugins_dir = resolve_plugins_dir(config)?;
    for entry in &config.plugin.auto_load {
        let configured = std::path::PathBuf::from(entry);
        let load_path = if let Some(catalog_entry) = catalog.get(entry) {
            catalog_entry.plugin_root.clone()
        } else if configured.is_absolute() || configured.exists() {
            configured
        } else {
            plugins_dir.join(entry)
        };
        if let Err(err) = plugin_manager.load_plugin(&load_path) {
            warn!(
                plugin = %entry,
                path = %load_path.display(),
                error = %err,
                "auto-load plugin failed; continuing without this plugin"
            );
        }
    }

    Ok((
        tool_registry,
        function_registry,
        Some(plugin_manager),
        Some(function_invoker),
        dispatcher,
    ))
}

fn plugin_runtime_disabled_via_env() -> bool {
    match std::env::var("PI_PLUGIN_DISABLE") {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[allow(dead_code)]
struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
        _session_id: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        Err(AppError::Tool(format!(
            "对话模式下不支持插件工具执行: {}",
            tool.name
        )))
    }
}

pub(crate) fn resolve_initial_thinking_display(
    thinking: &crate::infra::config::ThinkingConfig,
) -> ThinkingDisplay {
    match std::env::var("PI_CHAT_SHOW_THINKING") {
        Ok(v) => parse_thinking_display_override(&v).unwrap_or_else(|| {
            warn!(
                target: "tomcat::chat_context",
                value = %v,
                fallback = ?thinking.show,
                "unknown PI_CHAT_SHOW_THINKING override; falling back to config"
            );
            thinking.show
        }),
        Err(_) => thinking.show,
    }
}

fn parse_thinking_display_override(raw: &str) -> Option<ThinkingDisplay> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "minimal" => Some(ThinkingDisplay::Minimal),
        "summary" => Some(ThinkingDisplay::Summary),
        "full" => Some(ThinkingDisplay::Full),
        // 兼容历史 bool 环境变量：0/false -> summary；1/true -> full。
        "0" | "false" | "no" | "off" | "" => Some(ThinkingDisplay::Summary),
        "1" | "true" | "yes" | "on" => Some(ThinkingDisplay::Full),
        _ => None,
    }
}

fn migrate_legacy_layer0_tool_results(
    agent_definition_dir: &std::path::Path,
    agent_trail_dir: &std::path::Path,
) {
    let legacy_root = agent_definition_dir.join("workspace");
    if !legacy_root.exists() {
        return;
    }
    let target_root = agent_trail_dir.join("tool-results");
    if let Ok(entries) = std::fs::read_dir(&legacy_root) {
        let _ = std::fs::create_dir_all(&target_root);
        for entry in entries.flatten() {
            let from = entry.path();
            let name = entry.file_name();
            let to = target_root.join(name);
            if to.exists() {
                continue;
            }
            let _ = std::fs::rename(&from, &to);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serial_test::serial;

    use super::resolve_child_agent_compaction_runtime;
    use crate::core::llm::{DefaultLlmResolver, LlmResolver, ModelCatalog};
    use crate::AppConfig;

    #[test]
    #[serial(env_lock)]
    fn child_agent_compaction_runtime_preserves_resolved_model_provider_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.context.compaction_model = "deepseek-v4-pro".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> =
            Arc::new(DefaultLlmResolver::new(cfg.clone(), catalog));

        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "stub");
        }

        let (context_config, provider) =
            resolve_child_agent_compaction_runtime(resolver.as_ref(), &cfg.context, None);
        assert_eq!(context_config.compaction_model, "deepseek-v4-pro");
        assert!(
            provider.is_some(),
            "应把解析成功的 compaction provider 注入给子 Agent"
        );

        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
    }

    #[test]
    #[serial(env_lock)]
    fn child_agent_compaction_runtime_preserves_fallback_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.llm.default_model = "deepseek-v4-pro".to_string();
        cfg.context.compaction_model = "gpt-5.4".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> =
            Arc::new(DefaultLlmResolver::new(cfg.clone(), catalog));

        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "stub");
            std::env::remove_var("OPENAI_API_KEY");
        }

        let (context_config, provider) =
            resolve_child_agent_compaction_runtime(resolver.as_ref(), &cfg.context, None);
        assert_eq!(context_config.compaction_model, "deepseek-v4-pro");
        assert!(
            provider.is_some(),
            "fallback 成功时也应把 fallback 后的 provider 一起注入"
        );

        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
    }

    #[test]
    #[serial(env_lock)]
    fn child_agent_compaction_runtime_keeps_compat_fallback_boundary_when_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.llm.default_model = "deepseek-v4-pro".to_string();
        cfg.context.compaction_model = "gpt-5.4".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> =
            Arc::new(DefaultLlmResolver::new(cfg.clone(), catalog));

        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }

        let (context_config, provider) =
            resolve_child_agent_compaction_runtime(resolver.as_ref(), &cfg.context, None);

        assert!(provider.is_none(), "无法解析 pair 时应保留兼容回退边界");
        assert_eq!(
            context_config.compaction_model, "gpt-5.4",
            "未解析成功时不应偷偷改写 compaction_model"
        );
    }
}
