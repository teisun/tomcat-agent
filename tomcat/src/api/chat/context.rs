use std::io::{self, Write as IoWrite};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::ThinkingDisplay;
use crate::infra::error::AppError;
use crate::infra::{
    AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder, TracingAuditRecorder,
};
use crate::{
    resolve_agent_definition_dir, resolve_agent_trail_dir, resolve_sessions_dir,
    resolve_workspace_roots_paths, AppConfig, DefaultPrimitiveExecutor, DefaultToolRegistry,
    LlmProvider, PrimitiveExecutor, SessionEntry, SessionManager, Tool, ToolExecutor, ToolRegistry,
};

use crate::core::llm::LlmScene;
use crate::core::plan_runtime;

use super::{panels, permission};

pub struct ChatContext {
    pub session: SessionManager,
    pub message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink>,
    pub llm: Arc<dyn LlmProvider>,
    pub model_catalog: Arc<crate::core::llm::ModelCatalog>,
    pub llm_resolver: Arc<dyn crate::core::llm::LlmResolver>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditRecorder>,
    pub checkpoint_switcher: Arc<crate::core::SwitchingCheckpointStore>,
    pub checkpoint_store: Arc<dyn crate::core::CheckpointStore>,
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
    pub agent_workspace_dir: std::path::PathBuf,
    pub agent_definition_dir: std::path::PathBuf,
    pub agent_trail_dir: std::path::PathBuf,
    pub cfg_path: std::path::PathBuf,
    pub session_grants: crate::core::permission::SessionGrants,
    pub config_backend: Option<crate::core::agent_loop::SharedConfigBackend>,
    pub bash_task_registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    pub follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    pub completion_routes: crate::core::agent_loop::BackgroundCompletionRoutes,
    pub follow_up_signal: Arc<tokio::sync::Notify>,
    pub delivered_completion:
        Arc<Mutex<std::collections::HashSet<crate::core::tools::primitive::BashTaskId>>>,
    pub completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
    pub read_file_state: Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    pub thinking_display: Arc<std::sync::atomic::AtomicU8>,
    pub openai_files_runtime: Option<Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    pub web_fetch_runtime: Arc<crate::core::tools::web_fetch::WebFetchRuntime>,
    pub web_search_runtime: Arc<crate::core::tools::web_search::WebSearchRuntime>,
    pub plan_runtime: Arc<plan_runtime::PlanRuntime>,
    pub skill_set: Arc<RwLock<crate::core::skill::SkillSet>>,
    pub skill_discovery_handle:
        Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<crate::core::skill::SkillSet>>>>,
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

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        Self::from_config_with_overrides(config, ChatContextOverrides::default())
    }

    pub fn from_config_with_overrides(
        config: AppConfig,
        overrides: ChatContextOverrides,
    ) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        let sessions_path_for_appender = sessions_path.clone();
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);
        let message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink> =
            Arc::new(session.clone());
        let session_cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        let current_session_entry = session.ensure_current_session(session_cwd.clone())?;

        let agent_definition_dir = resolve_agent_definition_dir(&config)?;
        std::fs::create_dir_all(&agent_definition_dir).map_err(AppError::Io)?;
        let agent_trail_dir = resolve_agent_trail_dir(&config)?;
        std::fs::create_dir_all(&agent_trail_dir).map_err(AppError::Io)?;
        migrate_legacy_layer0_tool_results(&agent_definition_dir, &agent_trail_dir);

        let agent_workspace_dir =
            std::env::current_dir().unwrap_or_else(|_| agent_definition_dir.clone());
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
            session.current_session_key(),
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

        let checkpoint_switcher = Arc::new(crate::core::SwitchingCheckpointStore::new(
            agent_trail_dir.clone(),
            agent_workspace_dir.clone(),
            git_available_for_checkpoints(),
        ));
        let checkpoint_store: Arc<dyn crate::core::CheckpointStore> = checkpoint_switcher.clone();

        let tool_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(tool_executor, audit.clone()));

        let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
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
        let follow_up_signal = Arc::new(tokio::sync::Notify::new());
        let delivered_completion: Arc<
            Mutex<std::collections::HashSet<crate::core::tools::primitive::BashTaskId>>,
        > = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(None));

        let initial_thinking_display = resolve_initial_thinking_display(&config.llm.thinking);

        let plan_runtime = plan_runtime::PlanRuntime::new_with_session_id(
            session.current_session_key(),
            current_session_entry.session_id.clone(),
        );
        let ask_question_panel: Arc<dyn panels::AskQuestionPanel> = overrides
            .ask_question_panel
            .unwrap_or_else(|| Arc::new(panels::CliAskQuestionPanel));
        plan_runtime.set_ask_question_timeout_ms(Some(config.ask_question.timeout_ms));
        plan_runtime.set_todos_persist_base(Some(agent_trail_dir.clone()));
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
        let skill_set = Arc::new(RwLock::new(crate::core::skill::SkillSet::default()));
        let skill_discovery_handle = Arc::new(tokio::sync::Mutex::new(None));

        let reviewer_max_turns = std::env::var("TOMCAT_REVIEWER_MAX_TURNS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(config.reviewer.max_turns);
        let reviewer_model = config
            .reviewer
            .model_override
            .clone()
            .unwrap_or_else(|| config.llm.default_model.clone());
        let read_file_state =
            Arc::new(crate::core::tools::pipeline::read_state::ReadFileState::default());
        let prod_reviewer = plan_runtime::prod_reviewer::ProdReviewerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm: llm.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
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
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
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
            plan_runtime.attach_transcript_appender(Arc::new(move |extra| {
                let sm = SessionManager::new(sessions_path_for_appender.clone());
                sm.append_custom_entry(extra)
            }));
        }

        if let Err(err) = plan_runtime.recover() {
            warn!(error = %err, "plan_runtime recover failed; continuing with Chat mode");
        }

        Ok(Self {
            session,
            message_append_sink,
            llm,
            model_catalog,
            llm_resolver,
            config,
            primitive,
            tool_registry,
            event_bus,
            audit,
            checkpoint_switcher,
            checkpoint_store,
            cancel_token,
            last_interrupt_at,
            agent_workspace_dir,
            agent_definition_dir,
            agent_trail_dir,
            cfg_path: cfg_path_snapshot,
            session_grants,
            config_backend,
            bash_task_registry,
            follow_up_queue,
            completion_routes,
            follow_up_signal,
            delivered_completion,
            completion_subscriber_handle,
            gate,
            read_file_state,
            thinking_display: Arc::new(std::sync::atomic::AtomicU8::new(
                initial_thinking_display.as_u8(),
            )),
            openai_files_runtime,
            web_fetch_runtime,
            web_search_runtime,
            plan_runtime,
            skill_set,
            skill_discovery_handle,
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
        self.llm_resolver.resolve(scene, session_override)
    }

    pub(crate) fn openai_files_runtime_for(
        &self,
        provider: &dyn LlmProvider,
    ) -> Option<Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>> {
        crate::core::llm::openai_files::build_runtime_for_provider(
            provider,
            &self.config.llm.files,
            self.session.sessions_dir(),
            self.session.current_session_key(),
        )
        .map(Arc::new)
    }

    pub(crate) fn shutdown_completion_subscriber(&self) {
        if let Some(handle) = self.completion_subscriber_handle.lock().take() {
            handle.abort();
        }
    }

    pub(crate) fn skill_set_snapshot(&self) -> crate::core::skill::SkillSet {
        self.skill_set.read().clone()
    }

    pub(crate) async fn spawn_skill_discovery_if_needed(&self) {
        if !self.config.skills.enabled || !self.skill_set.read().is_empty() {
            return;
        }
        let mut handle = self.skill_discovery_handle.lock().await;
        if handle.is_none() {
            *handle = Some(crate::core::skill::spawn_discovery_task(
                self.config.clone(),
                self.agent_workspace_dir.clone(),
            ));
        }
    }

    pub(crate) async fn await_skill_discovery(&self) -> crate::core::skill::SkillSet {
        let handle = self.skill_discovery_handle.lock().await.take();
        if let Some(handle) = handle {
            match handle.await {
                Ok(skill_set) => {
                    *self.skill_set.write() = skill_set.clone();
                    skill_set
                }
                Err(error) => {
                    let mut failed = crate::core::skill::SkillSet::default();
                    failed
                        .warnings
                        .push(format!("skills_discovery_join_failed:{error}"));
                    *self.skill_set.write() = failed.clone();
                    failed
                }
            }
        } else {
            self.skill_set_snapshot()
        }
    }

    pub(crate) async fn reload_skill_set(&self) -> crate::core::skill::SkillSet {
        if let Some(handle) = self.skill_discovery_handle.lock().await.take() {
            handle.abort();
        }
        let skill_set = if self.config.skills.enabled {
            crate::core::skill::discover(&self.config, &self.agent_workspace_dir)
        } else {
            crate::core::skill::SkillSet::default()
        };
        *self.skill_set.write() = skill_set.clone();
        skill_set
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

struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
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
