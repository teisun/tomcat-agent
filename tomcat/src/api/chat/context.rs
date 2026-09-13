use std::future::Future;
use std::io::{self, Write as IoWrite};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::core::connector::{CompositeToolExecutor, ConnectorRegistry};
use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::ext::plugin::{PluginCatalog, PluginSource};
use crate::ext::{
    FunctionRegistry, HostApiDispatcher, PluginEngine, PluginEngineConfig, PluginFunctionInvoker,
    PluginManager, PluginRuntimeManager, PluginToolExecutor, RegisteredFunction,
    SharedPluginRuntimeManager,
};
use crate::infra::config::ThinkingDisplay;
use crate::infra::error::AppError;
use crate::infra::http_client::{
    build_outbound_client, clamp_timeout_within_budget, default_connect_timeout_for, has_proxy_env,
    OutboundClientErrorKind, OutboundClientOptions,
};
use crate::infra::{
    AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder, TracingAuditRecorder,
};
use crate::{
    resolve_agent_definition_dir, resolve_agent_trail_dir, resolve_model_thinking_path,
    resolve_plugins_dir, resolve_sessions_dir, resolve_workspace_roots_paths,
    session_key_for_agent, AppConfig, DefaultPrimitiveExecutor, DefaultToolRegistry,
    ModelPrefsStore, PrimitiveExecutor, SessionEntry, SessionManager, SessionMode, ThinkingLevel,
    Tool, ToolExecutor, ToolRegistry,
};

use crate::core::llm::{LlmScene, ResolvedCall};
use crate::core::plan_runtime;

use super::session_runtime::{GlobalServices, ScopeContainer, ScopeServices, SessionRuntime};
use super::{panels, permission};

#[derive(Debug, Clone, PartialEq, Eq)]
struct BashProductionPolicy {
    foreground_wait_ms: u64,
    max_output_chars: usize,
    persist_dir: std::path::PathBuf,
}

fn resolve_bash_production_policy(
    config: &AppConfig,
    agent_trail_dir: &std::path::Path,
) -> BashProductionPolicy {
    BashProductionPolicy {
        foreground_wait_ms: config.tools.bash.foreground_wait_ms,
        max_output_chars: config.tools.bash.max_output_chars,
        persist_dir: agent_trail_dir.join("tool-results"),
    }
}

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
    pub confirmation: Option<Arc<dyn UserConfirmationProvider>>,
    pub fetch_http_client: Option<reqwest::Client>,
    pub shared_agent_registry: Option<Arc<crate::core::agent_registry::AgentRegistry>>,
    pub shared_model_prefs: Option<Arc<ModelPrefsStore>>,
    pub skip_session_plugin_activation: bool,
    pub suppress_cli_output: bool,
    pub session_cwd_override: Option<std::path::PathBuf>,
}

impl ChatContextOverrides {
    pub fn with_ask_question_panel(mut self, panel: Arc<dyn panels::AskQuestionPanel>) -> Self {
        self.ask_question_panel = Some(panel);
        self
    }

    pub fn with_confirmation(mut self, confirmation: Arc<dyn UserConfirmationProvider>) -> Self {
        self.confirmation = Some(confirmation);
        self
    }

    pub fn with_fetch_http_client(mut self, client: reqwest::Client) -> Self {
        self.fetch_http_client = Some(client);
        self
    }

    pub fn with_shared_agent_registry(
        mut self,
        registry: Arc<crate::core::agent_registry::AgentRegistry>,
    ) -> Self {
        self.shared_agent_registry = Some(registry);
        self
    }

    pub fn with_shared_model_prefs(mut self, store: Arc<ModelPrefsStore>) -> Self {
        self.shared_model_prefs = Some(store);
        self
    }

    pub fn skip_session_plugin_activation(mut self) -> Self {
        self.skip_session_plugin_activation = true;
        self
    }

    pub fn suppress_cli_output(mut self) -> Self {
        self.suppress_cli_output = true;
        self
    }

    pub fn with_session_cwd_override(mut self, cwd: std::path::PathBuf) -> Self {
        self.session_cwd_override = Some(cwd);
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

fn build_model_prefs_store(config: &AppConfig) -> Result<Arc<ModelPrefsStore>, AppError> {
    let default_level = ThinkingLevel::parse_or_medium(&config.llm.thinking.level).0;
    Ok(Arc::new(ModelPrefsStore::load(
        resolve_model_thinking_path(config)?,
        default_level,
    )?))
}

fn connector_registry_for(
    config: &AppConfig,
    workspace_root: Option<&std::path::Path>,
) -> Result<Option<Arc<ConnectorRegistry>>, AppError> {
    config
        .connector
        .enabled
        .then(|| ConnectorRegistry::new(config, workspace_root))
        .transpose()
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

#[allow(clippy::too_many_arguments)]
fn scope_runtime_for(
    config: &AppConfig,
    resource_root: std::path::PathBuf,
    session_project_root: Option<std::path::PathBuf>,
    audit: Arc<dyn AuditRecorder>,
    llm_resolver: Arc<dyn crate::core::LlmResolver>,
    primitive: Arc<dyn PrimitiveExecutor>,
    bash_task_registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    session: Arc<SessionManager>,
    overrides: &ChatContextOverrides,
) -> Result<Arc<ScopeContainer>, AppError> {
    let disable_cache = overrides.fetch_http_client.is_some();
    let key = std::fs::canonicalize(&resource_root).unwrap_or(resource_root);
    let current_tokio_handle = tokio::runtime::Handle::try_current().ok();
    if !disable_cache {
        if let Some(existing) = scope_runtime_cache()
            .read()
            .get(&key)
            .and_then(Weak::upgrade)
        {
            if current_tokio_handle.is_none() || existing.dispatcher.has_tokio_handle() {
                return Ok(existing);
            }
        }
    }

    let mut cache_guard = (!disable_cache).then(|| scope_runtime_cache().write());
    if let Some(cache) = cache_guard.as_ref() {
        if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
            if current_tokio_handle.is_none() || existing.dispatcher.has_tokio_handle() {
                return Ok(existing);
            }
        }
    }

    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let deps = PluginRuntimeDeps {
        audit,
        event_bus: event_bus.clone(),
        fetch_http_client: overrides.fetch_http_client.clone(),
        llm_resolver,
        primitive,
        bash_task_registry,
        session,
    };
    let connector_registry = connector_registry_for(config, session_project_root.as_deref())?;
    let (tool_registry, function_registry, plugin_manager, plugin_function_invoker, dispatcher) =
        build_plugin_runtime(
            config,
            &key,
            deps,
            connector_registry
                .as_ref()
                .map(|registry| registry.mcp_executor()),
        )?;
    let shared = Arc::new(ScopeContainer {
        event_bus,
        tool_registry,
        function_registry,
        plugin_manager,
        plugin_function_invoker,
        connector_registry,
        dispatcher,
        skill_set: Arc::new(RwLock::new(crate::core::skill::SkillSet::default())),
        skill_discovery_handle: Arc::new(tokio::sync::Mutex::new(None)),
    });
    if let Some(cache) = cache_guard.as_mut() {
        cache.insert(key, Arc::downgrade(&shared));
    }
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
        #[cfg(test)]
        let _home_lock = crate::test_support::home_env_lock()
            .lock()
            .expect("test home env lock");
        #[cfg(test)]
        let _cwd_lock = crate::test_support::cwd_lock()
            .lock()
            .expect("test cwd lock");

        let sessions_path = resolve_sessions_dir(&config)?;
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let cwd_for_key = overrides.session_cwd_override.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        let session_key = session_key_for_agent(&config.agent.id, mode, &cwd_for_key);
        let session = SessionManager::new_scoped(sessions_path, session_key);
        let message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink> =
            Arc::new(session.clone());
        let session_cwd = Some(cwd_for_key.to_string_lossy().to_string());
        let agent_definition_dir = resolve_agent_definition_dir(&config)?;
        std::fs::create_dir_all(&agent_definition_dir).map_err(AppError::Io)?;
        let agent_trail_dir = resolve_agent_trail_dir(&config)?;
        std::fs::create_dir_all(&agent_trail_dir).map_err(AppError::Io)?;
        // CLI startup owns its cwd; persist it as the explicit project root rather
        // than later treating the backend process cwd as an implicit fallback.
        let current_session_entry = session
            .ensure_current_session_with_project_root(session_cwd.clone(), session_cwd.clone())?;
        session.pin_session(&current_session_entry.session_id);
        migrate_legacy_layer0_tool_results(&agent_definition_dir, &agent_trail_dir);

        let agent_workspace_dir =
            resolve_agent_workspace_dir(&current_session_entry, &agent_definition_dir);
        let cfg_path_snapshot =
            crate::api::cli::config_file_path().unwrap_or_else(|_| std::path::PathBuf::new());

        let model_catalog = crate::core::llm::SharedModelCatalog::load(&config)?;
        let model_prefs = match overrides.shared_model_prefs.clone() {
            Some(store) => store,
            None => build_model_prefs_store(&config)?,
        };
        let web_search_runtime = Arc::new(crate::core::tools::web_search::WebSearchRuntime::new(
            &config,
            model_catalog.clone(),
        )?);
        let web_fetch_runtime = Arc::new(crate::core::tools::web_fetch::WebFetchRuntime::new(
            &config,
            agent_trail_dir.join("tool-results"),
        )?);
        let llm_resolver: Arc<dyn crate::core::llm::LlmResolver> =
            Arc::new(crate::core::llm::DefaultLlmResolver::new(
                config.clone(),
                model_catalog.clone(),
                Arc::clone(&model_prefs),
            ));
        let _ = llm_resolver.resolve(crate::core::llm::LlmScene::Main, None)?;

        let audit_store = AuditStore::open_if_enabled(&config)?.map(Arc::new);
        let audit: Arc<dyn AuditRecorder> = match audit_store.as_ref() {
            Some(store) => Arc::new(FileAuditRecorder::new(Arc::clone(store))),
            None => Arc::new(TracingAuditRecorder),
        };
        let workspace_roots = resolve_workspace_roots_paths(&config)?;
        let base_confirmation: Arc<dyn UserConfirmationProvider> = overrides
            .confirmation
            .clone()
            .unwrap_or_else(|| Arc::new(CliConfirmation));

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
                base_confirmation,
                agent_workspace_dir.clone(),
                gate.clone(),
                session_grants.clone(),
                cfg_path_snapshot.clone(),
            ));

        let _ = workspace_roots;
        let bash_ast = crate::core::permission::BashAstChecker::new(false, vec![], vec![]);
        let bash_policy = resolve_bash_production_policy(&config, &agent_trail_dir);
        let bash_task_registry = crate::core::tools::primitive::build_bash_task_registry(
            &config.tools.bash,
            bash_policy.persist_dir.clone(),
            gate.clone(),
            confirmation.clone(),
            audit.clone(),
            bash_ast.clone(),
        );
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(
            DefaultPrimitiveExecutor::new(
                config.primitive.clone(),
                confirmation.clone(),
                audit.clone(),
                gate.clone(),
            )
            .with_bash_ast(bash_ast.clone())
            .with_bash_foreground_wait_ms(bash_policy.foreground_wait_ms)
            .with_bash_max_output_chars(bash_policy.max_output_chars)
            .with_bash_persist_dir(bash_policy.persist_dir.clone())
            .with_bash_task_registry(bash_task_registry.clone())
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

        let session_project_root = current_session_entry
            .project_root
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_dir())
            .and_then(|path| std::fs::canonicalize(path).ok());
        let resource_root = session_project_root
            .clone()
            // Older session records have no explicit root. Keep their established discovery
            // behavior, but do not use this fallback as an install target.
            .unwrap_or_else(|| agent_workspace_dir.clone());
        let checkpoint_switcher =
            checkpoint_store_for(agent_trail_dir.clone(), agent_workspace_dir.clone());
        let checkpoint_store: Arc<dyn crate::core::CheckpointStore> = checkpoint_switcher.clone();
        crate::core::skill::materialize_builtin_skills(&config)?;

        let session_arc = Arc::new(session.clone());
        let shared_scope_runtime = scope_runtime_for(
            &config,
            resource_root.clone(),
            session_project_root.clone(),
            audit.clone(),
            llm_resolver.clone(),
            primitive.clone(),
            bash_task_registry.clone(),
            session_arc.clone(),
            &overrides,
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
        if !overrides.skip_session_plugin_activation {
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
                    if plugin_manager_ref
                        .has_session_vm(&current_session_entry.session_id, &plugin_id)
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
        }
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));
        let hard_exit_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>> =
            Arc::new(Mutex::new(Vec::new()));
        let steering_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>> =
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
        plan_runtime.set_auto_checkpoint_on_build(config.plan.auto_checkpoint_on_build);
        plan_runtime.set_verify_gate_mode(config.plan.verify_gate.clone());
        plan_runtime.set_max_code_review_rounds(config.plan.max_code_review_rounds);
        plan_runtime.set_max_completion_gate_cycles(config.plan.max_completion_gate_cycles);
        plan_runtime.attach_workspace_root(agent_workspace_dir.clone());
        plan_runtime.attach_bash_task_registry(bash_task_registry.clone());
        plan_runtime.set_expose_skills_to_reviewer(config.skills.expose_to_reviewer);
        plan_runtime.attach_checkpoint_store(checkpoint_store.clone());
        plan_runtime.register_todos_panel(Arc::new(panels::CliTodosPanel));
        plan_runtime.attach_ask_question_panel(ask_question_panel);

        let mut package_install_context =
            crate::core::tools::package_install::PackageInstallContext::new(
                config.clone(),
                agent_workspace_dir.clone(),
                session_project_root.clone(),
                confirmation.clone(),
            )
            .with_gate(gate.clone())
            .with_plan_runtime(&plan_runtime);
        if let Some(store) = audit_store.clone() {
            package_install_context = package_install_context.with_audit_store(store);
        }
        let package_install_backend: Option<crate::core::agent_loop::SharedPackageInstallBackend> =
            Some(Arc::new(
                crate::core::tools::package_install::ChatPackageInstallBackend {
                    ctx: package_install_context,
                },
            ));

        let agent_registry = overrides.shared_agent_registry.unwrap_or_else(|| {
            crate::core::agent_registry::AgentRegistry::new().attach_event_bus(event_bus.clone())
        });
        let root_agent_guard = agent_registry
            .register_root(current_session_entry.session_id.clone())
            .map_err(|e| AppError::Config(format!("agent_registry root register 失败: {e}")))?;
        let skill_set = shared_scope_runtime.skill_set.clone();
        let skill_discovery_handle = shared_scope_runtime.skill_discovery_handle.clone();

        let reviewer_max_turns = std::env::var("TOMCAT_REVIEWER_MAX_TURNS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(config.reviewer.max_turns);
        // 只捕获 override；为空时由 dispatcher 在每次派发时取当前会话模型。
        let reviewer_model_override = config.reviewer.model_override.clone();
        let read_file_state = Arc::new(
            crate::core::tools::pipeline::read_state::ReadFileState::with_mutation_stamp_refresh(
                config.tools.read.refresh_mutation_stamp,
            ),
        );
        let prod_plan_reviewer = plan_runtime::prod_reviewer::ProdPlanReviewerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm_resolver: llm_resolver.clone(),
                model_catalog: model_catalog.clone(),
                model_prefs: model_prefs.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                refresh_mutation_stamp: config.tools.read.refresh_mutation_stamp,
                llm_files_config: config.llm.files.clone(),
                sessions_dir: session.sessions_dir().to_path_buf(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                bash_config: config.tools.bash.clone(),
                gate: gate.clone(),
                confirmation: confirmation.clone(),
                audit: audit.clone(),
                bash_ast: bash_ast.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model_override: reviewer_model_override.clone(),
                fallback_model: config.llm.default_model.clone(),
                max_turns: reviewer_max_turns,
            },
        );
        let prod_code_reviewer = plan_runtime::prod_reviewer::ProdCodeReviewerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm_resolver: llm_resolver.clone(),
                model_catalog: model_catalog.clone(),
                model_prefs: model_prefs.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                refresh_mutation_stamp: config.tools.read.refresh_mutation_stamp,
                llm_files_config: config.llm.files.clone(),
                sessions_dir: session.sessions_dir().to_path_buf(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                bash_config: config.tools.bash.clone(),
                gate: gate.clone(),
                confirmation: confirmation.clone(),
                audit: audit.clone(),
                bash_ast: bash_ast.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model_override: reviewer_model_override,
                fallback_model: config.llm.default_model.clone(),
                max_turns: reviewer_max_turns,
            },
        );
        let prod_explorer = plan_runtime::prod_reviewer::ProdExplorerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm_resolver: llm_resolver.clone(),
                model_catalog: model_catalog.clone(),
                model_prefs: model_prefs.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                refresh_mutation_stamp: config.tools.read.refresh_mutation_stamp,
                llm_files_config: config.llm.files.clone(),
                sessions_dir: session.sessions_dir().to_path_buf(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                bash_config: config.tools.bash.clone(),
                gate: gate.clone(),
                confirmation: confirmation.clone(),
                audit: audit.clone(),
                bash_ast: bash_ast.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                // explorer 没有独立模型配置，始终跟随会话模型。
                model_override: None,
                fallback_model: config.llm.default_model.clone(),
                max_turns: reviewer_max_turns,
            },
        );
        plan_runtime.attach_plan_reviewer(Arc::new(prod_plan_reviewer));
        plan_runtime.attach_code_reviewer(Arc::new(prod_code_reviewer));
        plan_runtime.attach_explorer(Arc::new(prod_explorer));
        let prod_verifier = plan_runtime::verify::ProdVerifierDispatcher::new(
            "chat_context",
            plan_runtime::verify::ProdVerifierDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm_resolver: llm_resolver.clone(),
                model_catalog: model_catalog.clone(),
                model_prefs: model_prefs.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                refresh_mutation_stamp: config.tools.read.refresh_mutation_stamp,
                llm_files_config: config.llm.files.clone(),
                sessions_dir: session.sessions_dir().to_path_buf(),
                web_fetch_runtime: web_fetch_runtime.clone(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                skill_set: skill_set.clone(),
                skills_config: config.skills.clone(),
                bash_config: config.tools.bash.clone(),
                gate: gate.clone(),
                confirmation: confirmation.clone(),
                audit: audit.clone(),
                bash_ast: bash_ast.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                // verifier 没有独立的 model_override 配置，直接跟随会话模型。
                model_override: None,
                fallback_model: config.llm.default_model.clone(),
            },
        );
        plan_runtime.attach_verifier(Arc::new(prod_verifier));

        {
            let appender_session = session.clone();
            plan_runtime.attach_transcript_appender(Arc::new(move |extra| {
                appender_session.append_custom_entry(extra)
            }));

            let plan_event_bus = event_bus.clone();
            let plan_event_session_id = current_session_entry.session_id.clone();
            plan_runtime.attach_transcript_event_notifier(Arc::new(move |mut payload| {
                if let Some(event_name) = payload
                    .get("event")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| name.starts_with("plan.") || name.starts_with("session."))
                {
                    let event_name = event_name.to_string();
                    if let Some(obj) = payload.as_object_mut() {
                        obj.remove("event");
                        if let Some(plan_id) = obj.remove("plan_id") {
                            obj.insert("planId".to_string(), plan_id);
                        }
                        obj.insert(
                            "type".to_string(),
                            serde_json::Value::String(event_name.clone()),
                        );
                        obj.insert(
                            "sessionId".to_string(),
                            serde_json::Value::String(plan_event_session_id.clone()),
                        );
                    }
                    if let Err(error) = plan_event_bus.emit_sync(
                        &event_name,
                        crate::infra::EventContext::new(event_name.clone(), payload)
                            .with_session_id(plan_event_session_id.clone()),
                    ) {
                        warn!(
                            error = %error,
                            event_name = %event_name,
                            "plan transcript event emit failed"
                        );
                    }
                }
            }));
        }

        if let Err(err) = plan_runtime.recover() {
            warn!(error = %err, "plan_runtime recover failed; continuing with Chat mode");
        }

        let thinking_display = Arc::new(std::sync::atomic::AtomicU8::new(
            initial_thinking_display.as_u8(),
        ));
        let global_services = GlobalServices {
            model_catalog: model_catalog.clone(),
            llm_resolver: llm_resolver.clone(),
            model_prefs,
            primitive: primitive.clone(),
            tool_registry: tool_registry.clone(),
            function_registry: function_registry.clone(),
            event_bus: event_bus.clone(),
            audit: audit.clone(),
            gate: gate.clone(),
            config_backend: config_backend.clone(),
            package_install_backend: package_install_backend.clone(),
            web_fetch_runtime: web_fetch_runtime.clone(),
            web_search_runtime: web_search_runtime.clone(),
            plugin_manager,
            plugin_function_invoker,
            connector_registry: shared_scope_runtime.connector_registry.clone(),
        };
        let scope_services = ScopeServices {
            scope_container: shared_scope_runtime.clone(),
            checkpoint_switcher: checkpoint_switcher.clone(),
            checkpoint_store: checkpoint_store.clone(),
            agent_workspace_dir: agent_workspace_dir.clone(),
            resource_root: resource_root.clone(),
            session_project_root: session_project_root.clone(),
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
            hard_exit_requested: hard_exit_requested.clone(),
            seen_resource_inventory_epoch: Arc::new(std::sync::atomic::AtomicU64::new(
                crate::api::chat::session_runtime::current_resource_inventory_epoch(),
            )),
            session_grants: session_grants.clone(),
            bash_task_registry: bash_task_registry.clone(),
            follow_up_queue: follow_up_queue.clone(),
            steering_queue: steering_queue.clone(),
            completion_routes: completion_routes.clone(),
            delivered_completion: delivered_completion.clone(),
            completion_subscriber_handle: completion_subscriber_handle.clone(),
            checkpoint_record_tasks: Arc::new(Mutex::new(Vec::new())),
            read_file_state: read_file_state.clone(),
            openai_files_runtime: Arc::new(Mutex::new(None)),
            thinking_display: thinking_display.clone(),
            todos_runtime: todos_runtime.clone(),
            plan_runtime: plan_runtime.clone(),
            suppress_cli_output: overrides.suppress_cli_output,
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

    pub(crate) fn resolve_thinking_level(&self, model_id: &str) -> ThinkingLevel {
        self.global_services
            .model_catalog
            .resolve_reasoning_level(self.global_services.model_prefs.as_ref(), model_id)
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
        call: &ResolvedCall,
    ) -> Option<Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>> {
        let session_id = self
            .session_runtime
            .session
            .current_session_id()
            .ok()
            .flatten()?;
        let runtime_key = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            call.provider,
            call.api,
            call.base_url.as_deref().unwrap_or_default(),
            call.key_source,
        );
        let mut cached = self.session_runtime.openai_files_runtime.lock();
        if let Some((cached_key, runtime)) = cached.as_ref() {
            if cached_key == &runtime_key {
                return Some(runtime.clone());
            }
        }
        let runtime = crate::core::llm::openai_files::build_runtime_for_provider(
            call.provider_impl.as_ref(),
            &self.config.llm.files,
            self.session_runtime.session.sessions_dir(),
            &session_id,
        )
        .map(Arc::new)?;
        *cached = Some((runtime_key, runtime.clone()));
        Some(runtime)
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

    pub(crate) async fn spawn_connector_startup_if_needed(&self) {
        let Some(connectors) = self.global_services.connector_registry.as_ref() else {
            return;
        };
        connectors.spawn_connect_all().await;
    }

    pub(crate) async fn spawn_skill_discovery_if_needed(&self) {
        if !self.config.skills.enabled || !self.scope_services.skill_set.read().is_empty() {
            return;
        }
        let mut handle = self.scope_services.skill_discovery_handle.lock().await;
        if handle.is_none() {
            *handle = Some(crate::core::skill::spawn_discovery_task(
                self.config.clone(),
                self.scope_services.resource_root.clone(),
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

    /// Refresh cacheable skill/plugin inventories only before a later user turn. Active plugin
    /// VMs are intentionally left alone by `refresh_plugin_catalog_inventory`; a new inventory
    /// must not replace live plugin code halfway through a conversation.
    pub(crate) async fn refresh_resource_inventory_before_turn(&self) -> Result<(), AppError> {
        let current = crate::api::chat::current_resource_inventory_epoch();
        let seen = self
            .session_runtime
            .seen_resource_inventory_epoch
            .load(std::sync::atomic::Ordering::Acquire);
        if current <= seen {
            return Ok(());
        }
        self.reload_skill_set().await;
        self.refresh_plugin_catalog_inventory().await?;
        self.session_runtime
            .seen_resource_inventory_epoch
            .store(current, std::sync::atomic::Ordering::Release);
        Ok(())
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
            crate::core::skill::discover(&self.config, &self.scope_services.resource_root)
        } else {
            crate::core::skill::SkillSet::default()
        };
        if skill_set.warnings.iter().any(|warning| {
            warning == "skills_discovery_roots_failed"
                || warning.starts_with("skills_root_unreadable:")
        }) {
            return self.skill_set_snapshot();
        }
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

        let catalog = PluginCatalog::discover(&self.config, &self.scope_services.resource_root)?;
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

        let function_catalog = refresh_host_function_registry(
            &self.config,
            &self.scope_services.resource_root,
            &self.global_services.function_registry,
        )?;
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
    fetch_http_client: Option<reqwest::Client>,
    llm_resolver: Arc<dyn crate::core::LlmResolver>,
    primitive: Arc<dyn PrimitiveExecutor>,
    bash_task_registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    session: Arc<SessionManager>,
}

fn canonicalize_or_keep(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn host_function_source_rank(source: PluginSource) -> u8 {
    match source {
        PluginSource::Project => 0,
        PluginSource::Agent => 1,
        PluginSource::Managed => 2,
    }
}

fn materialize_host_functions_from_catalog(
    registry: &FunctionRegistry,
    catalog: &PluginCatalog,
) -> Vec<String> {
    #[derive(Debug)]
    struct HostFunctionCandidate {
        order: usize,
        source: PluginSource,
        plugin_id: String,
        plugin_root: std::path::PathBuf,
        point: String,
        function: String,
    }

    let mut candidates = Vec::<HostFunctionCandidate>::new();
    for (plugin_id, entry) in catalog.iter() {
        for function in &entry.manifest.functions {
            candidates.push(HostFunctionCandidate {
                order: candidates.len(),
                source: entry.source,
                plugin_id: plugin_id.clone(),
                plugin_root: canonicalize_or_keep(&entry.plugin_root),
                point: function.point.clone(),
                function: function.function.clone(),
            });
        }
    }
    candidates.sort_by(|left, right| {
        host_function_source_rank(left.source)
            .cmp(&host_function_source_rank(right.source))
            .then(left.order.cmp(&right.order))
    });

    let mut warnings = Vec::new();
    let mut winners =
        std::collections::BTreeMap::<String, (PluginSource, RegisteredFunction)>::new();
    for candidate in candidates {
        let registered = RegisteredFunction {
            plugin_id: candidate.plugin_id.clone(),
            plugin_root: candidate.plugin_root.clone(),
            point: candidate.point.clone(),
            function: candidate.function.clone(),
        };
        if let Some((winner_source, winner)) = winners.get(&candidate.point) {
            let warning = if *winner_source == candidate.source {
                format!(
                    "function_point_conflict:{}:{} shadowed_by {} (source={}, policy=first_wins)",
                    candidate.point,
                    candidate.plugin_id,
                    winner.plugin_id,
                    candidate.source.as_str()
                )
            } else {
                format!(
                    "function_point_shadowed:{}:{}:{} shadowed_by {}:{}",
                    candidate.point,
                    candidate.source.as_str(),
                    candidate.plugin_id,
                    winner_source.as_str(),
                    winner.plugin_id
                )
            };
            warnings.push(warning);
            continue;
        }
        winners.insert(candidate.point.clone(), (candidate.source, registered));
    }

    registry.replace_all(winners.into_values().map(|(_, function)| function));
    warnings
}

fn refresh_host_function_registry(
    config: &AppConfig,
    agent_workspace_dir: &std::path::Path,
    registry: &FunctionRegistry,
) -> Result<PluginCatalog, AppError> {
    let mut catalog = PluginCatalog::discover(config, agent_workspace_dir)?;
    catalog
        .warnings
        .extend(materialize_host_functions_from_catalog(registry, &catalog));
    Ok(catalog)
}

fn build_plugin_runtime(
    config: &AppConfig,
    agent_workspace_dir: &std::path::Path,
    deps: PluginRuntimeDeps,
    mcp_executor: Option<Arc<dyn ToolExecutor>>,
) -> Result<PluginRuntimeParts, AppError> {
    let PluginRuntimeDeps {
        audit,
        event_bus,
        fetch_http_client,
        llm_resolver,
        primitive,
        bash_task_registry,
        session,
    } = deps;
    if plugin_runtime_disabled_via_env() {
        warn!("PI_PLUGIN_DISABLE enabled; skipping plugin runtime initialization");
        let plugin_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let executor: Arc<dyn ToolExecutor> = match mcp_executor {
            Some(mcp_executor) => CompositeToolExecutor::new(plugin_executor, mcp_executor),
            None => plugin_executor,
        };
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(executor, audit.clone()));
        let function_registry = Arc::new(FunctionRegistry::new());
        let dispatcher = Arc::new(
            HostApiDispatcher::new(event_bus.clone())
                .with_tools(tool_registry.clone())
                .with_session(session)
                .with_llm_resolver(llm_resolver)
                .with_primitive(primitive)
                .with_bash_task_registry(bash_task_registry.clone())
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

    let plugin_executor = PluginToolExecutor::new(Arc::downgrade(&plugin_manager));
    let executor: Arc<dyn ToolExecutor> = match mcp_executor {
        Some(mcp_executor) => CompositeToolExecutor::new(plugin_executor.clone(), mcp_executor),
        None => plugin_executor.clone(),
    };
    let function_registry = Arc::new(FunctionRegistry::new());
    let default_tool_registry = Arc::new(DefaultToolRegistry::new(executor.clone(), audit.clone()));
    let tool_registry: Arc<dyn ToolRegistry> = default_tool_registry.clone();
    let plugin_fetch_timeout = clamp_timeout_within_budget(
        config.tools.web_fetch.fetch_timeout_ms,
        config.plugin.call_timeout_ms,
    );
    let fetch_client = if let Some(client) = fetch_http_client {
        client
    } else {
        let mut options = OutboundClientOptions::new(config.llm.proxy.as_deref());
        options.use_public_ip_dns_resolver = true;
        options.redirect_policy = Some(reqwest::redirect::Policy::none());
        options.timeout = Some(plugin_fetch_timeout);
        options.connect_timeout = Some(default_connect_timeout_for(plugin_fetch_timeout));
        build_outbound_client(
            options,
            OutboundClientErrorKind::Tool,
            "创建 plugin net.fetch HTTP 客户端失败",
        )?
    };
    let explicit_fetch_proxy = config
        .llm
        .proxy
        .as_deref()
        .is_some_and(|proxy| !proxy.trim().is_empty());
    let ambient_fetch_proxy = !explicit_fetch_proxy && has_proxy_env();
    let dispatcher = Arc::new(
        HostApiDispatcher::new(event_bus.clone())
            .with_fetch_transport_diagnostics(
                plugin_fetch_timeout,
                explicit_fetch_proxy,
                ambient_fetch_proxy,
            )
            .with_tools(tool_registry.clone())
            .with_session(session.clone())
            .with_llm_resolver(llm_resolver)
            .with_primitive(primitive)
            .with_bash_task_registry(bash_task_registry)
            .with_plugin_manager(Arc::downgrade(&plugin_manager))
            .with_fetch_http_client(fetch_client)
            .with_fetch_max_body_bytes(config.tools.web_fetch.max_http_content_bytes)
            .with_audit(audit),
    );
    let function_invoker = PluginFunctionInvoker::new(Arc::downgrade(&plugin_manager));
    plugin_executor.attach_dispatcher(Arc::downgrade(&dispatcher));
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
    let host_function_catalog =
        refresh_host_function_registry(config, agent_workspace_dir, &function_registry)?;
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

    use super::{connector_registry_for, resolve_bash_production_policy, ChatContext};
    use crate::core::llm::{DefaultLlmResolver, LlmResolver, ModelCatalog};
    use crate::core::plan_runtime::prod_reviewer::resolve_subagent_runtime;
    use crate::{AppConfig, ModelPrefsStore, ThinkingLevel};

    fn model_prefs(path: &std::path::Path) -> Arc<ModelPrefsStore> {
        Arc::new(
            ModelPrefsStore::load(path.join("model-thinking.json"), ThinkingLevel::Medium)
                .expect("model preferences"),
        )
    }

    #[test]
    fn bash_production_policy_resolves_non_default_wait_output_and_path() {
        let mut cfg = AppConfig::default();
        cfg.tools.bash.foreground_wait_ms = 9_000;
        cfg.tools.bash.max_output_chars = 42_000;
        let trail = std::path::Path::new("/tmp/tomcat-policy-test/agent-trail");

        let policy = resolve_bash_production_policy(&cfg, trail);

        assert_eq!(policy.foreground_wait_ms, 9_000);
        assert_eq!(policy.max_output_chars, 42_000);
        assert_eq!(policy.persist_dir, trail.join("tool-results"));
        let registry =
            crate::core::tools::primitive::BashTaskRegistry::new(policy.persist_dir.clone())
                .with_foreground_wait_ms(policy.foreground_wait_ms);
        assert_eq!(registry.foreground_wait_ms(), 9_000);
    }

    #[test]
    fn connector_registry_constructed_when_enabled() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut enabled = AppConfig::default();
        enabled.connector.enabled = true;
        enabled.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        assert!(connector_registry_for(&enabled, Some(temp.path()))
            .expect("enabled connector registry")
            .is_some());

        let mut disabled = enabled.clone();
        disabled.connector.enabled = false;
        assert!(connector_registry_for(&disabled, Some(temp.path()))
            .expect("disabled connector registry")
            .is_none());
    }
    #[test]
    #[serial(env_lock)]
    fn child_agent_dispatch_runtime_preserves_resolved_model_provider_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.context.compaction_model = "deepseek-v4-pro".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
            cfg.clone(),
            catalog,
            model_prefs(dir.path()),
        ));

        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "stub");
        }

        let runtime = resolve_subagent_runtime(
            resolver.as_ref(),
            &cfg.context,
            &cfg.llm.files,
            dir.path(),
            "parent-session",
            "deepseek-v4-pro",
        )
        .expect("runtime should resolve");
        assert_eq!(runtime.context_config.compaction_model, "deepseek-v4-pro");
        assert!(
            runtime.compaction_provider.is_some(),
            "应把解析成功的 compaction provider 注入给子 Agent"
        );

        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
    }

    #[test]
    #[serial(env_lock)]
    fn child_agent_dispatch_runtime_preserves_fallback_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.llm.default_model = "deepseek-v4-pro".to_string();
        cfg.context.compaction_model = "gpt-5.4".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
            cfg.clone(),
            catalog,
            model_prefs(dir.path()),
        ));

        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "stub");
            std::env::remove_var("OPENAI_API_KEY");
        }

        let runtime = resolve_subagent_runtime(
            resolver.as_ref(),
            &cfg.context,
            &cfg.llm.files,
            dir.path(),
            "parent-session",
            "deepseek-v4-pro",
        )
        .expect("runtime should resolve");
        assert_eq!(runtime.context_config.compaction_model, "deepseek-v4-pro");
        assert!(
            runtime.compaction_provider.is_some(),
            "fallback 成功时也应把 fallback 后的 provider 一起注入"
        );

        unsafe {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
    }

    #[test]
    #[serial(env_lock)]
    fn child_agent_dispatch_runtime_keeps_compat_fallback_boundary_when_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.toml");
        let mut cfg = AppConfig::default();
        cfg.llm.default_model = "missing-compaction-model".to_string();
        cfg.context.compaction_model = "missing-compaction-model".to_string();
        let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, path).unwrap());
        let resolver: Arc<dyn LlmResolver> = Arc::new(DefaultLlmResolver::new(
            cfg.clone(),
            catalog,
            model_prefs(dir.path()),
        ));

        unsafe {
            std::env::set_var("OPENAI_API_KEY", "stub");
            std::env::remove_var("DEEPSEEK_API_KEY");
        }

        let runtime = resolve_subagent_runtime(
            resolver.as_ref(),
            &cfg.context,
            &cfg.llm.files,
            dir.path(),
            "parent-session",
            "gpt-5.4",
        )
        .expect("main model should still resolve");

        assert!(
            runtime.compaction_provider.is_none(),
            "无法解析 pair 时应保留兼容回退边界"
        );
        assert_eq!(
            runtime.context_config.compaction_model, "missing-compaction-model",
            "未解析成功时不应偷偷改写 compaction_model"
        );

        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    #[test]
    #[serial(env_lock)]
    fn resolve_thinking_level_uses_catalog_id_key_when_model_name_differs() {
        const ENV_KEY: &str = "TOMCAT_REASONING_LOOKUP_TEST_KEY";

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
        cfg.llm.default_model = "relay/gpt-sol".to_string();
        cfg.context.compaction_model = "relay/gpt-sol".to_string();
        std::fs::write(
            dir.path().join("models.toml"),
            format!(
                r#"
[[models]]
id = "relay/gpt-sol"
model_name = "gpt-sol"
api = "openai-responses"
provider = "relay"
api_key_env = "{env}"
base_url = "https://api.example.test"
thinking_format = "openai"
supported_reasoning_levels = ["low", "medium", "high", "xhigh"]
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
                env = ENV_KEY,
            ),
        )
        .unwrap();

        unsafe {
            std::env::set_var(ENV_KEY, "stub");
        }

        let ctx = ChatContext::from_config(cfg).expect("ctx");
        ctx.global_services
            .model_prefs
            .set_reasoning("relay/gpt-sol", ThinkingLevel::Xhigh)
            .expect("persist relay override");

        assert_eq!(
            ctx.resolve_thinking_level("relay/gpt-sol"),
            ThinkingLevel::Xhigh
        );
        assert_eq!(
            ctx.resolve_thinking_level("gpt-sol"),
            ThinkingLevel::High,
            "wire model_name key miss should still fall back to the configured default level"
        );

        unsafe {
            std::env::remove_var(ENV_KEY);
        }
    }

    #[test]
    #[serial(env_lock)]
    fn resolve_thinking_level_preserves_empty_supported_levels_toggle_default() {
        const ENV_KEY: &str = "TOMCAT_REASONING_TOGGLE_TEST_KEY";

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
        cfg.llm.default_model = "relay/toggle-only".to_string();
        cfg.context.compaction_model = "relay/toggle-only".to_string();
        std::fs::write(
            dir.path().join("models.toml"),
            format!(
                r#"
[[models]]
id = "relay/toggle-only"
model_name = "toggle-only"
api = "openai"
provider = "relay"
api_key_env = "{env}"
base_url = "https://api.example.test"
thinking_format = "doubao"
supported_reasoning_levels = []
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
                env = ENV_KEY,
            ),
        )
        .unwrap();

        unsafe {
            std::env::set_var(ENV_KEY, "stub");
        }

        let ctx = ChatContext::from_config(cfg).expect("ctx");
        ctx.global_services
            .model_prefs
            .set_reasoning("relay/toggle-only", ThinkingLevel::Low)
            .expect("persist toggle override");

        assert_eq!(
            ctx.resolve_thinking_level("relay/toggle-only"),
            ThinkingLevel::High,
            "empty supported_reasoning_levels should keep the existing Thinking On/Off fallback"
        );

        unsafe {
            std::env::remove_var(ENV_KEY);
        }
    }
}
