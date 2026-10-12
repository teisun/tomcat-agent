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

/// Translates a durable plan transcript entry into the camelCase live-event
/// contract before it crosses the serve boundary.
fn normalize_plan_transcript_event_for_live_delivery(
    mut payload: serde_json::Value,
    session_id: &str,
) -> Option<(String, serde_json::Value)> {
    let event_name = payload
        .get("event")
        .and_then(serde_json::Value::as_str)
        .filter(|name| name.starts_with("plan.") || name.starts_with("session."))
        .map(str::to_owned)?;
    let obj = payload.as_object_mut()?;
    obj.remove("event");
    if let Some(plan_id) = obj.remove("plan_id") {
        obj.insert("planId".to_string(), plan_id);
    }
    if let Some(skip_reason) = obj.remove("skip_reason") {
        obj.insert("skipReason".to_string(), skip_reason);
    }
    if let Some(idle_nudges) = obj.remove("idle_nudges") {
        obj.insert("idleNudges".to_string(), idle_nudges);
    }
    if let Some(remaining_work) = obj.remove("remaining_work") {
        obj.insert("remainingWork".to_string(), remaining_work);
    }
    obj.insert(
        "type".to_string(),
        serde_json::Value::String(event_name.clone()),
    );
    obj.insert(
        "sessionId".to_string(),
        serde_json::Value::String(session_id.to_owned()),
    );
    Some((event_name, payload))
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

impl ChatContext {
    pub(crate) fn instruction_catalog(
        &self,
    ) -> Vec<crate::core::project_instructions::InstructionCard> {
        use crate::core::project_instructions::{InstructionCard, InstructionKind};
        let mut cards: Vec<_> = self
            .project_commands()
            .files
            .into_iter()
            .map(|f| f.card)
            .collect();
        if self.config.skills.enabled {
            let snapshot = self.skill_set_snapshot();
            for skill in snapshot.by_name.values() {
                if matches!(
                    self.global_services
                        .gate
                        .check(PrimitiveOperation::Read, &skill.file_path.to_string_lossy()),
                    Ok(crate::core::permission::PermissionDecision::Deny { .. }) | Err(_)
                ) {
                    continue;
                }
                cards.push(InstructionCard {
                    id: format!("skill:{}", skill.name),
                    kind: InstructionKind::Skill,
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    source: skill.source.as_str().into(),
                    path: skill.file_path.to_string_lossy().into_owned(),
                });
            }
        }
        cards
    }

    pub(crate) fn resolve_instruction(
        &self,
        id: &str,
        kind: crate::core::project_instructions::InstructionKind,
    ) -> Result<crate::core::llm::ContextReference, String> {
        use crate::core::llm::{ContextRefKind, ContextReference};
        use crate::core::project_instructions::{read_body, InstructionKind};
        let (path, label, body) = match kind {
            InstructionKind::Command => {
                let commands = self.project_commands();
                let file = commands
                    .files
                    .iter()
                    .find(|f| f.card.id == id)
                    .ok_or_else(|| tr("runtime.commandMissing", &[("id", id)]))?;
                let root = self
                    .scope_services
                    .resource_root
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                (
                    file.card.path.clone(),
                    format!("/{}", file.card.name),
                    read_body(&root, &file.file_path, self.global_services.gate.as_ref())?,
                )
            }
            InstructionKind::Skill => {
                if !self.config.skills.enabled {
                    return Err(tr("runtime.skillsDisabled", &[]));
                }
                let name = id
                    .strip_prefix("skill:")
                    .ok_or_else(|| tr("runtime.skillIdInvalid", &[]))?;
                let snapshot = self.skill_set_snapshot();
                let skill = snapshot
                    .resolve_any(name)
                    .ok_or_else(|| tr("runtime.skillMissing", &[("name", name)]))?;
                let base = skill.base_dir.canonicalize().map_err(|e| e.to_string())?;
                let file = base.join("SKILL.md");
                (
                    skill.file_path.to_string_lossy().into_owned(),
                    format!("/{name}"),
                    read_body(&base, &file, self.global_services.gate.as_ref())?
                        .trim()
                        .to_string(),
                )
            }
        };
        Ok(ContextReference {
            ref_kind: match kind {
                InstructionKind::Command => ContextRefKind::Command,
                InstructionKind::Skill => ContextRefKind::Skill,
            },
            path,
            label,
            line_start: None,
            line_end: None,
            text: Some(body),
            resource_id: Some(id.into()),
        })
    }

    pub(crate) fn project_commands(&self) -> crate::core::project_instructions::Discovery {
        crate::core::project_instructions::discover(
            &self.scope_services.resource_root,
            &self.config.workspace.project_resource_dir,
            false,
            self.global_services.gate.as_ref(),
        )
    }

    pub(crate) fn project_rules(&self, budget: usize) -> (String, Vec<String>, usize) {
        let root = self
            .scope_services
            .resource_root
            .canonicalize()
            .unwrap_or_else(|_| self.scope_services.resource_root.clone());
        let rules = crate::core::project_instructions::discover(
            &root,
            &self.config.workspace.project_resource_dir,
            true,
            self.global_services.gate.as_ref(),
        );
        crate::core::project_instructions::render_rules(
            &rules,
            &root,
            self.global_services.gate.as_ref(),
            budget,
        )
    }

    pub(crate) fn instruction_summary(&self) -> String {
        let commands = self.project_commands();
        let entry = self
            .session_runtime
            .session
            .get_session(self.session_runtime.session.current_session_key())
            .ok()
            .flatten();
        let budget = self
            .resolve_call(LlmScene::Main, entry.as_ref())
            .map(|call| {
                crate::infra::config::compute_context_budget_chars_from_tokens(
                    call.limits.input_budget_tokens,
                )
            })
            .unwrap_or(320_000);
        let (_, errors, loaded) = self.project_rules(budget);
        let summarize = |key: &str, count: usize, diagnostics: &[String]| {
            let mut text = tr(
                key,
                &[
                    ("count", &count.to_string()),
                    ("skipped", &diagnostics.len().to_string()),
                ],
            );
            for error in diagnostics.iter().take(20) {
                text.push_str(&format!("\n      {error}"));
            }
            if diagnostics.len() > 20 {
                text.push_str(&tr(
                    "runtime.moreDiagnostics",
                    &[("count", &(diagnostics.len() - 20).to_string())],
                ));
            }
            text
        };
        format!(
            "{}\n{}",
            summarize(
                "runtime.commandsSummary",
                commands.files.len(),
                &commands.diagnostics
            ),
            summarize("runtime.rulesSummary", loaded, &errors)
        )
    }
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
        .then(|| ConnectorRegistry::create_and_start(config, workspace_root))
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

#[derive(Clone, PartialEq, Eq, Hash)]
struct ScopeRuntimeCacheKey {
    resource_root: std::path::PathBuf,
    // Equal discovery directories do not imply equal connector scopes: a
    // default Serve session has no explicit Workspace MCP configuration.
    project_root: Option<std::path::PathBuf>,
}

fn scope_runtime_cache(
) -> &'static RwLock<std::collections::HashMap<ScopeRuntimeCacheKey, Weak<ScopeContainer>>> {
    static CACHE: OnceLock<
        RwLock<std::collections::HashMap<ScopeRuntimeCacheKey, Weak<ScopeContainer>>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(std::collections::HashMap::new()))
}

/// End only VMs already owned by this process; cleanup must never construct a new chat session.
pub(crate) fn end_cached_plugin_session(session_id: &str) -> Result<(), AppError> {
    let managers: Vec<_> = scope_runtime_cache()
        .read()
        .values()
        .filter_map(Weak::upgrade)
        .filter_map(|scope| scope.plugin_manager.clone())
        .collect();
    if managers.is_empty() {
        return Ok(());
    }
    let session_id = session_id.to_owned();
    block_on_plugin_future(async move {
        for manager in managers {
            manager.end_session(&session_id).await?;
        }
        Ok(())
    })
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
    let cache_key = ScopeRuntimeCacheKey {
        resource_root: key.clone(),
        project_root: session_project_root.clone(),
    };
    let current_tokio_handle = tokio::runtime::Handle::try_current().ok();
    if !disable_cache {
        if let Some(existing) = scope_runtime_cache()
            .read()
            .get(&cache_key)
            .and_then(Weak::upgrade)
        {
            if current_tokio_handle.is_none() || existing.dispatcher.has_tokio_handle() {
                return Ok(existing);
            }
        }
    }

    let mut cache_guard = (!disable_cache).then(|| scope_runtime_cache().write());
    if let Some(cache) = cache_guard.as_ref() {
        if let Some(existing) = cache.get(&cache_key).and_then(Weak::upgrade) {
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
    let (connector_registry, connector_load_error) =
        match connector_registry_for(config, session_project_root.as_deref()) {
            Ok(registry) => (registry, None),
            Err(error) => {
                tracing::warn!(error = %error, "MCP connectors unavailable; chat continues");
                (None, Some(error.to_string()))
            }
        };
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
        inventory_sync: tokio::sync::Mutex::new(()),
        event_bus,
        tool_registry,
        function_registry,
        plugin_manager,
        plugin_function_invoker,
        connector_registry,
        connector_load_error,
        dispatcher,
        skill_set: Arc::new(RwLock::new(crate::core::skill::SkillSet::default())),
        skill_discovery_handle: Arc::new(tokio::sync::Mutex::new(None)),
    });
    if let Some(cache) = cache_guard.as_mut() {
        cache.insert(cache_key, Arc::downgrade(&shared));
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
            .map_err(|_| AppError::Plugin(crate::infra::i18n::tr("runtime.scopeWorker", &[])))?;
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
        session.pin_session(&current_session_entry.session_id)?;
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
            if let Some(manager) = plugin_manager.clone() {
                let session_id = current_session_entry.session_id.clone();
                let (_, warnings) = block_on_plugin_future(async move {
                    Ok(activate_session_plugins(manager, &session_id).await)
                })?;
                for warning in warnings {
                    warn!("{warning}");
                }
            }
        }
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));
        let hard_exit_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>> =
            Arc::new(Mutex::new(Vec::new()));
        let steering_queue =
            Arc::new(Mutex::new(crate::core::agent_loop::SteeringInbox::default()));
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
            .map_err(|e| {
                AppError::Config(tr(
                    "runtime.rootRegisterFailed",
                    &[("detail", &e.to_string())],
                ))
            })?;
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
            plan_runtime.attach_transcript_event_notifier(Arc::new(move |payload| {
                if let Some((event_name, payload)) =
                    normalize_plan_transcript_event_for_live_delivery(
                        payload,
                        &plan_event_session_id,
                    )
                {
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
        self.resolve_call(LlmScene::Main, entry)
            .map(|call| call.catalog_id)
            .unwrap_or_default()
    }

    pub(crate) fn resolve_thinking_level(&self, model_id: &str) -> ThinkingLevel {
        self.global_services
            .model_catalog
            .resolve_reasoning_level(self.global_services.model_prefs.as_ref(), model_id)
    }

    pub(crate) fn resolve_speed(&self, model_id: &str) -> Option<crate::core::llm::Speed> {
        self.global_services
            .model_catalog
            .resolve_speed(self.global_services.model_prefs.as_ref(), model_id)
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
        if skill_set.discovery_failed() {
            return self.skill_set_snapshot();
        }
        *self.scope_services.skill_set.write() = skill_set.clone();
        skill_set
    }
}

impl Drop for ChatContext {
    fn drop(&mut self) {
        self.shutdown_completion_subscriber();
    }
}

use crate::infra::i18n::tr;

pub struct CliConfirmation;

#[async_trait::async_trait]
impl UserConfirmationProvider for CliConfirmation {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        println!("{}", tr("terminal.confirm.title", &[]));
        let source_label = if plugin_id == "__agent__" {
            "host".to_string()
        } else {
            plugin_id.to_string()
        };
        println!(
            "{}",
            tr(
                "terminal.confirm.source",
                &[
                    ("operation", &format!("{operation:?}")),
                    ("source", &source_label)
                ]
            )
        );
        if !preview.is_empty() {
            let lines: Vec<&str> = preview.lines().collect();
            let display = if lines.len() > 20 {
                tr(
                    "terminal.confirm.omitted",
                    &[
                        ("text", &lines[..20].join("\n")),
                        ("count", &(lines.len() - 20).to_string()),
                    ],
                )
            } else {
                preview.to_string()
            };
            println!("{}", tr("terminal.confirm.preview", &[("text", &display)]));
        }
        print!("{}", tr("terminal.confirm.ask", &[]));
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
        target: Option<std::path::PathBuf>,
        suggested_root: Option<std::path::PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        let target = match target {
            Some(target) if operation != PrimitiveOperation::Bash => target,
            _ => {
                return match self.confirm(operation, preview, plugin_id).await? {
                    true => Ok(ConfirmDecision::AllowOnce),
                    false => Ok(ConfirmDecision::Deny),
                };
            }
        };
        match permission::prompt::read_path_prompt(
            &target,
            suggested_root,
            Some(&tr(
                "terminal.confirm.source",
                &[
                    ("operation", &format!("{operation:?}")),
                    ("source", plugin_id),
                ],
            )),
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

pub(super) fn materialize_host_functions_from_catalog(
    registry: &FunctionRegistry,
    catalog: &PluginCatalog,
) -> Vec<String> {
    let (functions, warnings) = collect_host_functions_from_catalog(catalog);
    registry.replace_all(functions);
    warnings
}

pub(super) fn collect_host_functions_from_catalog(
    catalog: &PluginCatalog,
) -> (Vec<RegisteredFunction>, Vec<String>) {
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

    (
        winners
            .into_values()
            .map(|(_, function)| function)
            .collect(),
        warnings,
    )
}

pub(super) fn register_catalog_entry(
    manager: &PluginManager,
    entry: &crate::ext::plugin::CatalogEntry,
) -> Result<Vec<Tool>, AppError> {
    manager.register_catalog_plugin(&entry.plugin_root, entry.manifest.clone())?;
    manager.set_catalog_fingerprint(&entry.manifest.id, entry.fingerprint);
    Ok(entry
        .manifest
        .tools
        .iter()
        .map(|tool| Tool {
            name: tool.name.clone(),
            label: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
            plugin_id: entry.manifest.id.clone(),
            is_enabled: true,
            created_at: 0,
        })
        .collect())
}

pub(super) async fn activate_session_plugins(
    manager: Arc<PluginManager>,
    session_id: &str,
) -> (bool, Vec<String>) {
    let mut worked = false;
    let mut warnings = Vec::new();
    for id in manager.list_loaded() {
        let Some(info) = manager.get_plugin(&id) else {
            continue;
        };
        // Keep legacy dynamic discovery identical to new-session startup. A
        // Lazy plugin with no manifest tools can still run this short script.
        if info.manifest.tools.is_empty() && info.loaded_at == 0 {
            worked = true;
            let pm = manager.clone();
            let root = info.plugin_root.clone();
            match tokio::task::spawn_blocking(move || pm.load_plugin(&root)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warnings.push(format!("scope activation failed for {id}: {error}"))
                }
                Err(error) => {
                    warnings.push(format!("scope activation worker failed for {id}: {error}"))
                }
            }
            if info.manifest.activation == crate::ext::PluginActivation::Lazy {
                continue;
            }
        }
        if info.manifest.activation != crate::ext::PluginActivation::Session
            || manager.has_session_vm(session_id, &id)
        {
            continue;
        }
        worked = true;
        if let Err(error) = manager.start_session_vm(session_id, &id).await {
            warnings.push(format!("scope activation failed to start {id}: {error}"));
            continue;
        }
        if let Err(error) = manager.dispatch_session_event(
            session_id,
            &id,
            crate::infra::wire::vm::WIRE_SESSION_START,
            serde_json::json!({}),
            serde_json::json!({"sessionId": session_id}),
        ) {
            warnings.push(format!(
                "scope activation failed to deliver session_start to {id}: {error}"
            ));
        }
    }
    (worked, warnings)
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
            &tr("runtime.fetchClientFailed", &[]),
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
        match register_catalog_entry(&plugin_manager, entry) {
            Ok(tools) => {
                for tool in tools {
                    if let Err(error) = default_tool_registry.register_tool_local(tool, plugin_id) {
                        warn!(plugin = %plugin_id, %error, "catalog materialize static tool failed");
                    }
                }
            }
            Err(error) => warn!(plugin = %plugin_id, %error, "catalog register plugin failed"),
        }
    }
    for diagnostic in &catalog.diagnostics {
        warn!(
            path = %diagnostic.path.display(),
            reason = %diagnostic.reason,
            "plugin catalog scan ignored invalid entry"
        );
    }
    for warning in materialize_host_functions_from_catalog(&function_registry, &catalog) {
        warn!("{warning}");
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
        Err(AppError::Tool(tr(
            "runtime.pluginExecutionUnavailable",
            &[("name", &tool.name)],
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
#[path = "tests/context_test.rs"]
mod tests;
