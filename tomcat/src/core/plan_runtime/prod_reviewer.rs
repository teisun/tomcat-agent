//! plan reviewer / code reviewer 的生产 dispatcher。
//!
//! 两者都通过 `spawn_subagent_internal` 同步派发，但子 Agent 类型、prompt、tool 白名单、
//! 结构化摘要类型都彼此独立；这里只保留共享的依赖注入和 spawn 机械流程。

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use tracing::warn;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome, SubagentType};
use crate::core::agent_registry::{AgentRegistry, SubagentOutcome, SubagentOutcomeLabel};
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::system_prompt::render_available_skills_prompt;
use crate::core::llm::{
    ChatMessage, LlmProvider, LlmResolver, LlmScene, ResolvedCall, SharedModelCatalog,
};
use crate::core::plan_runtime::code_reviewer::{
    build_code_review_prompt, changed_files_since, code_review_system_prompt_text,
    code_reviewer_allowed_tools_with_policy, collect_git_changed_files, CodeReviewPromptInput,
    CodeReviewSummary,
};
use crate::core::plan_runtime::explorer::{
    build_explorer_prompt, explorer_system_prompt_text, ExplorerReport, ExplorerTask,
    EXPLORER_ALLOWED_TOOLS,
};
use crate::core::plan_runtime::plan_reviewer::{
    build_review_prompt, plan_reviewer_allowed_tools_with_policy, reviewer_system_prompt_text,
    PlanReviewSummary,
};
use crate::core::plan_runtime::review::{
    count_assistant_turns, extract_review_text, parse_review_block, resolve_internal_tools,
};
use crate::core::plan_runtime::{
    CodeReviewDispatchInfo, CodeReviewerDispatcher, ExplorerDispatcher, PlanReviewerDispatcher,
    PlanRuntime,
};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::{CheckpointStore, ModelPrefsStore};
use crate::infra::config::{ContextConfig, LlmFilesConfig};
use crate::infra::event_bus::EventBus;

/// 生产 plan reviewer dispatcher。装配点：`ChatContext::from_config`。
pub struct ProdPlanReviewerDispatcher {
    origin: &'static str,
    deps: Option<ProdReviewerDeps>,
}

/// 生产 code reviewer dispatcher。装配点：`ChatContext::from_config`。
pub struct ProdCodeReviewerDispatcher {
    origin: &'static str,
    deps: Option<ProdReviewerDeps>,
}

/// 完整依赖集合；构造时一次性注入。dispatch 仅 clone Arc 拷贝到 spawn 闭包。
pub struct ProdReviewerDeps {
    pub agent_registry: Arc<AgentRegistry>,
    pub parent_session_id: String,
    pub llm_resolver: Arc<dyn LlmResolver>,
    /// Shared catalog and per-model preferences resolve the same clamped
    /// effort used by the main agent loop.
    pub model_catalog: SharedModelCatalog,
    pub model_prefs: Arc<ModelPrefsStore>,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub event_bus: Arc<dyn EventBus>,
    pub agent_trail_dir: String,
    pub checkpoint_store: Arc<dyn CheckpointStore>,
    pub context_config: ContextConfig,
    /// 每个子 Agent 的 read 去重表必须独立；这里只传同一份配置，而非共享状态。
    pub refresh_mutation_stamp: bool,
    pub llm_files_config: LlmFilesConfig,
    pub sessions_dir: std::path::PathBuf,
    pub agent_workspace_dir: std::path::PathBuf,
    pub skill_set: Arc<parking_lot::RwLock<crate::core::skill::SkillSet>>,
    pub skills_config: crate::infra::config::SkillsConfig,
    pub bash_config: crate::infra::config::ToolsBashConfig,
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
    pub confirmation: Arc<dyn crate::core::tools::contract::confirmation::UserConfirmationProvider>,
    pub audit: Arc<dyn crate::infra::AuditRecorder>,
    pub bash_ast: crate::core::permission::BashAstChecker,
    /// Weak 引用避免与 `PlanRuntime` 内部 dispatcher 字段形成 cycle。
    pub plan_runtime: Weak<PlanRuntime>,
    /// `[reviewer].model_override`。为空时在**每次派发**时取当前会话模型，
    /// 而不是启动时定死 `config.llm.default_model`——否则你在 UI 上换了模型，
    /// reviewer 还在用旧的那个。
    pub model_override: Option<String>,
    /// 会话模型也拿不到时（例如第一回合之前就派发）的最后兜底。
    pub fallback_model: String,
    /// 子 AgentLoop `max_tool_rounds`（`TOMCAT_REVIEWER_MAX_TURNS` 默认 64）。
    pub max_turns: u32,
}

impl ProdReviewerDeps {
    fn resolve_model(&self, plan_runtime: &PlanRuntime) -> String {
        resolve_dispatch_model(
            self.model_override.as_deref(),
            plan_runtime.session_model().as_deref(),
            &self.fallback_model,
        )
    }

    fn resolve_thinking_level(&self, model_id: &str) -> crate::core::llm::ThinkingLevel {
        self.model_catalog
            .resolve_reasoning_level(self.model_prefs.as_ref(), model_id)
    }
}

pub(crate) struct ResolvedSubagentRuntime {
    pub main_call: ResolvedCall,
    pub context_config: ContextConfig,
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,
    pub compaction_output_limit: Option<u32>,
    pub openai_files_runtime: Option<Arc<OpenAiFilesRuntime>>,
}

fn resolve_subagent_compaction_runtime(
    llm_resolver: &dyn LlmResolver,
    base_context_config: &ContextConfig,
) -> (ContextConfig, Option<Arc<dyn LlmProvider>>, Option<u32>) {
    let mut context_config = base_context_config.clone();
    match llm_resolver.resolve(LlmScene::Compaction, None) {
        Ok(call) => {
            let output_limit = call.output_limit_for_request(None).0;
            context_config.compaction_model = call.model;
            (context_config, Some(call.provider_impl), output_limit)
        }
        Err(err) => {
            warn!(
                error = %err,
                "failed to resolve child-agent compaction runtime; falling back to main provider"
            );
            (context_config, None, None)
        }
    }
}

pub(crate) fn resolve_subagent_runtime(
    llm_resolver: &dyn LlmResolver,
    base_context_config: &ContextConfig,
    llm_files_config: &LlmFilesConfig,
    sessions_dir: &std::path::Path,
    session_id: &str,
    model_id: &str,
) -> Result<ResolvedSubagentRuntime, crate::infra::error::AppError> {
    let main_call = llm_resolver.resolve(LlmScene::Main, Some(model_id))?;
    let (context_config, compaction_provider, compaction_output_limit) =
        resolve_subagent_compaction_runtime(llm_resolver, base_context_config);
    let openai_files_runtime = crate::core::llm::openai_files::build_runtime_for_provider(
        main_call.provider_impl.as_ref(),
        llm_files_config,
        sessions_dir,
        session_id,
    )
    .map(Arc::new);
    Ok(ResolvedSubagentRuntime {
        main_call,
        context_config,
        compaction_provider,
        compaction_output_limit,
        openai_files_runtime,
    })
}

/// 子 Agent 派发模型的解析顺序：显式 override > 当前会话模型 > 启动兜底。
///
/// 会话模型放在配置兜底之前，是因为你在 UI 上换模型是当下的、明确的意图；
/// 启动时读到的 `llm.default_model` 只是缺省值。空字符串一律当作"没配"。
pub(crate) fn resolve_dispatch_model(
    model_override: Option<&str>,
    session_model: Option<&str>,
    fallback: &str,
) -> String {
    model_override
        .filter(|m| !m.is_empty())
        .or(session_model.filter(|m| !m.is_empty()))
        .unwrap_or(fallback)
        .to_string()
}

impl ProdPlanReviewerDispatcher {
    pub fn stub(origin: &'static str) -> Self {
        Self { origin, deps: None }
    }

    pub fn new(origin: &'static str, deps: ProdReviewerDeps) -> Self {
        Self {
            origin,
            deps: Some(deps),
        }
    }
}

#[async_trait]
impl PlanReviewerDispatcher for ProdPlanReviewerDispatcher {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        _allow_review_edit: bool,
    ) -> PlanReviewSummary {
        let Some(deps) = self.deps.as_ref() else {
            return PlanReviewSummary::aborted_with(format!(
                "[{}] 生产 plan reviewer 子 Agent 未注入依赖（stub 模式）",
                self.origin
            ));
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return PlanReviewSummary::aborted_with(format!(
                "[{}] PlanRuntime 已被 drop，plan reviewer 取消派发",
                self.origin
            ));
        };

        let plan_path = match plan_runtime.resolved_plan_path(plan_id) {
            Ok(path) => path,
            Err(err) => return PlanReviewSummary::aborted_with(err),
        };
        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let initial_user_message =
            build_review_prompt(plan_id, plan_text, &plan_path, workspace_root);
        let turns_limit = deps.max_turns.max(1);

        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let refresh_mutation_stamp = deps.refresh_mutation_stamp;
        let shared_skill_set = Arc::clone(&deps.skill_set);
        let skill_set = deps.skill_set.read().clone();
        let expose_skills =
            plan_runtime.expose_skills_to_reviewer() && !skill_set.visible_skills().is_empty();
        let tool_defs =
            resolve_internal_tools(&plan_reviewer_allowed_tools_with_policy(expose_skills));
        let model_id = deps.resolve_model(&plan_runtime);
        let thinking_level = Some(deps.resolve_thinking_level(&model_id));
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;
        let runtime = match resolve_subagent_runtime(
            deps.llm_resolver.as_ref(),
            &deps.context_config,
            &deps.llm_files_config,
            &deps.sessions_dir,
            &parent_session_id,
            &model_id,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                let mut s = PlanReviewSummary::aborted_with(format!(
                    "[{}] plan reviewer 模型 `{}` 解析失败：{}",
                    self.origin, model_id, err
                ));
                s.reviewer_turns_limit = turns_limit;
                s.reviewer_stop_reason = "model_unresolved".into();
                return s;
            }
        };
        let context_budget_chars = crate::infra::config::compute_context_budget_chars_from_tokens(
            runtime.main_call.limits.input_budget_tokens,
        );
        let skill_prompt = if expose_skills {
            render_available_skills_prompt(&skill_set, context_budget_chars, &deps.skills_config)
        } else {
            None
        };
        let plan_id_for_closure = plan_id.to_string();
        let plan_runtime_for_loop = Arc::clone(&plan_runtime);
        let compaction_provider = runtime.compaction_provider.clone();
        let compaction_output_limit = runtime
            .compaction_output_limit
            .or_else(|| runtime.main_call.output_limit_for_request(None).0);
        let context_config = runtime.context_config.clone();
        let openai_files_runtime = runtime.openai_files_runtime.clone();
        let binding = runtime.main_call;
        let transcript_model = binding.catalog_id().to_string();

        let (tx, rx) = tokio::sync::oneshot::channel::<PlanReviewSummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::PlanReviewer,
                move |spawn_ctx| async move {
                    let read_file_state = Arc::new(ReadFileState::with_mutation_stamp_refresh(
                        refresh_mutation_stamp,
                    ));
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();
                    let transcript_root = agent_trail_dir.clone();
                    let transcript_sink =
                        crate::core::session::subagent_transcript::open_subagent_transcript(
                            &transcript_root,
                            &child_session_id,
                            SubagentType::PlanReviewer,
                            &transcript_model,
                            &parent_session_id_for_closure,
                        );
                    let initial_message_sink = transcript_sink.clone();
                    let transcript_path =
                        crate::core::session::subagent_transcript::format_subagent_transcript_path(
                            &transcript_root,
                            &child_session_id,
                        );
                    plan_runtime.write_plan_review_started_transcript(
                        &plan_id_for_closure,
                        Some(&child_session_id),
                        transcript_path.as_deref(),
                    );

                    let mut system_text = format!(
                        "{}\n(max_turns budget: {} reasoning turns)\n",
                        reviewer_system_prompt_text(),
                        turns_limit
                    );
                    if let Some(skill_prompt) = skill_prompt.as_ref() {
                        system_text.push('\n');
                        system_text.push_str(skill_prompt);
                    }
                    let cfg = AgentLoopConfig {
                        max_attempts: crate::infra::config::DEFAULT_AGENT_MAX_ATTEMPTS,
                        unattended_retry: true,
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms:
                            crate::infra::config::DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
                        thinking_level,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_provider,
                        compaction_output_limit,
                        title_provider: None,
                        title_model: String::new(),
                        title_output_limit: None,
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        message_append_sink: transcript_sink,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::PlanReviewer,
                        plan_runtime: Some(plan_runtime_for_loop),
                        skill_set: if expose_skills {
                            Some(Arc::clone(&shared_skill_set))
                        } else {
                            None
                        },
                        ephemeral_tail_provider: None,
                    };
                    let mut agent_loop =
                        AgentLoop::new(binding, primitive, event_bus, cfg, cancel_token.clone());
                    // PlanReviewer tool whitelist does not include `bash`, so it does not need
                    // a dedicated BashTaskRegistry.
                    let initial_messages = vec![
                        ChatMessage::system(&system_text),
                        ChatMessage::user(&initial_user_message),
                    ];
                    crate::core::session::subagent_transcript::persist_initial_messages(
                        initial_message_sink.as_ref(),
                        &initial_messages,
                    );
                    let run_outcome = agent_loop.run(initial_messages).await;

                    let (summary, label) = build_plan_summary_from_outcome(
                        origin,
                        &child_session_id,
                        turns_limit,
                        run_outcome,
                    );
                    let mut summary = summary;
                    crate::core::session::subagent_transcript::append_subagent_transcript_hint(
                        &mut summary.summary,
                        &transcript_root,
                        &child_session_id,
                    );
                    let _ = tx.send(summary.clone());

                    SubagentOutcome {
                        child_session_id: child_session_id.clone(),
                        subagent_type: SubagentType::PlanReviewer,
                        outcome_label: label,
                        error_message: if summary.aborted {
                            Some(summary.summary.clone())
                        } else {
                            None
                        },
                    }
                },
            )
            .await;

        match spawn_result {
            Ok(_) => match rx.await {
                Ok(summary) => summary,
                Err(_) => PlanReviewSummary::aborted_with(format!(
                    "[{}] plan reviewer 子 Agent 退出但 summary channel 提前关闭",
                    self.origin
                )),
            },
            Err(e) => {
                let mut s = PlanReviewSummary::aborted_with(format!(
                    "[{}] reviewer spawn 失败：{e}",
                    self.origin
                ));
                s.reviewer_turns_limit = turns_limit;
                s.reviewer_stop_reason = "spawn_error".into();
                s
            }
        }
    }
}

impl ProdCodeReviewerDispatcher {
    pub fn stub(origin: &'static str) -> Self {
        Self { origin, deps: None }
    }

    pub fn new(origin: &'static str, deps: ProdReviewerDeps) -> Self {
        Self {
            origin,
            deps: Some(deps),
        }
    }
}

#[async_trait]
impl CodeReviewerDispatcher for ProdCodeReviewerDispatcher {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        review_state: &crate::core::plan_runtime::file_store::PlanFileFrontmatter,
        dispatch: &CodeReviewDispatchInfo,
    ) -> CodeReviewSummary {
        let Some(deps) = self.deps.as_ref() else {
            return CodeReviewSummary::aborted_with(format!(
                "[{}] 生产 code reviewer 子 Agent 未注入依赖（stub 模式）",
                self.origin
            ));
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return CodeReviewSummary::aborted_with(format!(
                "[{}] PlanRuntime 已被 drop，code reviewer 取消派发",
                self.origin
            ));
        };

        let plan_path = match plan_runtime.resolved_plan_path(plan_id) {
            Ok(path) => path,
            Err(err) => return CodeReviewSummary::aborted_with(err),
        };
        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let changed_files = collect_git_changed_files(deps.agent_workspace_dir.as_path()).await;
        let previous_dispatch_ms = dispatch
            .is_incremental
            .then_some(review_state.code_review_baseline_ms)
            .flatten();
        let is_incremental = dispatch.is_incremental;
        let delta_files = previous_dispatch_ms
            .map(|since_ms| {
                changed_files_since(deps.agent_workspace_dir.as_path(), &changed_files, since_ms)
            })
            .unwrap_or_default();
        let initial_user_message = build_code_review_prompt(CodeReviewPromptInput {
            plan_id,
            plan_text,
            plan_path: &plan_path,
            workspace_root,
            changed_files: &changed_files,
            delta_files: &delta_files,
            round: dispatch.round,
            is_incremental,
            open_findings: &review_state.code_review_open_findings,
            disputed_findings: &review_state.code_review_disputed_findings,
        });
        let turns_limit = deps.max_turns.max(1);

        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let refresh_mutation_stamp = deps.refresh_mutation_stamp;
        let shared_skill_set = Arc::clone(&deps.skill_set);
        let skill_set = deps.skill_set.read().clone();
        let bash_config = deps.bash_config.clone();
        let gate = Arc::clone(&deps.gate);
        let confirmation = Arc::clone(&deps.confirmation);
        let audit = Arc::clone(&deps.audit);
        let bash_ast = deps.bash_ast.clone();
        let expose_skills =
            plan_runtime.expose_skills_to_reviewer() && !skill_set.visible_skills().is_empty();
        let tool_defs =
            resolve_internal_tools(&code_reviewer_allowed_tools_with_policy(expose_skills));
        let model_id = deps.resolve_model(&plan_runtime);
        let thinking_level = Some(deps.resolve_thinking_level(&model_id));
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;
        let runtime = match resolve_subagent_runtime(
            deps.llm_resolver.as_ref(),
            &deps.context_config,
            &deps.llm_files_config,
            &deps.sessions_dir,
            &parent_session_id,
            &model_id,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                let mut s = CodeReviewSummary::aborted_with(format!(
                    "[{}] code reviewer 模型 `{}` 解析失败：{}",
                    self.origin, model_id, err
                ));
                s.reviewer_turns_limit = turns_limit;
                s.reviewer_stop_reason = "model_unresolved".into();
                return s;
            }
        };
        let context_budget_chars = crate::infra::config::compute_context_budget_chars_from_tokens(
            runtime.main_call.limits.input_budget_tokens,
        );
        let skill_prompt = if expose_skills {
            render_available_skills_prompt(&skill_set, context_budget_chars, &deps.skills_config)
        } else {
            None
        };
        let plan_id_for_closure = plan_id.to_string();
        let plan_text_for_validation = plan_text.to_owned();
        let dispatch = dispatch.clone();
        let compaction_provider = runtime.compaction_provider.clone();
        let compaction_output_limit = runtime
            .compaction_output_limit
            .or_else(|| runtime.main_call.output_limit_for_request(None).0);
        let context_config = runtime.context_config.clone();
        let openai_files_runtime = runtime.openai_files_runtime.clone();
        let binding = runtime.main_call;
        let transcript_model = binding.catalog_id().to_string();

        let (tx, rx) = tokio::sync::oneshot::channel::<CodeReviewSummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::CodeReviewer,
                move |spawn_ctx| async move {
                    let read_file_state = Arc::new(ReadFileState::with_mutation_stamp_refresh(
                        refresh_mutation_stamp,
                    ));
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();
                    let transcript_root = agent_trail_dir.clone();
                    let bash_task_registry =
                        crate::core::tools::primitive::build_bash_task_registry(
                            &bash_config,
                            std::path::PathBuf::from(&transcript_root)
                                .join("tool-results")
                                .join(format!("subagent-{}", spawn_ctx.subagent_type.as_str()))
                                .join(&child_session_id),
                            gate.clone(),
                            confirmation.clone(),
                            audit.clone(),
                            bash_ast.clone(),
                        );
                    let transcript_sink =
                        crate::core::session::subagent_transcript::open_subagent_transcript(
                            &transcript_root,
                            &child_session_id,
                            SubagentType::CodeReviewer,
                            &transcript_model,
                            &parent_session_id_for_closure,
                        );
                    let initial_message_sink = transcript_sink.clone();
                    let transcript_path =
                        crate::core::session::subagent_transcript::format_subagent_transcript_path(
                            &transcript_root,
                            &child_session_id,
                        );
                    plan_runtime.write_code_review_started_transcript(
                        &plan_id_for_closure,
                        dispatch.round,
                        &dispatch.review_attempt_id,
                        &dispatch.tool_call_id,
                        Some(&child_session_id),
                        transcript_path.as_deref(),
                    );

                    let mut system_text = format!(
                        "{}\n(max_turns budget: {} reasoning turns)\n",
                        code_review_system_prompt_text(),
                        turns_limit
                    );
                    if let Some(skill_prompt) = skill_prompt.as_ref() {
                        system_text.push('\n');
                        system_text.push_str(skill_prompt);
                    }
                    let cfg = AgentLoopConfig {
                        max_attempts: crate::infra::config::DEFAULT_AGENT_MAX_ATTEMPTS,
                        unattended_retry: true,
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms:
                            crate::infra::config::DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
                        thinking_level,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_provider,
                        compaction_output_limit,
                        title_provider: None,
                        title_model: String::new(),
                        title_output_limit: None,
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        message_append_sink: transcript_sink,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::CodeReviewer,
                        plan_runtime: None,
                        skill_set: if expose_skills {
                            Some(Arc::clone(&shared_skill_set))
                        } else {
                            None
                        },
                        ephemeral_tail_provider: None,
                    };
                    let mut agent_loop =
                        AgentLoop::new(binding, primitive, event_bus, cfg, cancel_token.clone())
                            .with_bash_task_registry(bash_task_registry);
                    let initial_messages = vec![
                        ChatMessage::system(&system_text),
                        ChatMessage::user(&initial_user_message),
                    ];
                    crate::core::session::subagent_transcript::persist_initial_messages(
                        initial_message_sink.as_ref(),
                        &initial_messages,
                    );
                    let run_outcome = agent_loop.run(initial_messages).await;

                    let (summary, label) = build_code_summary_from_outcome(
                        origin,
                        &child_session_id,
                        turns_limit,
                        &plan_text_for_validation,
                        run_outcome,
                    );
                    let mut summary = summary;
                    crate::core::session::subagent_transcript::append_subagent_transcript_hint(
                        &mut summary.summary,
                        &transcript_root,
                        &child_session_id,
                    );
                    let _ = tx.send(summary.clone());

                    SubagentOutcome {
                        child_session_id: child_session_id.clone(),
                        subagent_type: SubagentType::CodeReviewer,
                        outcome_label: label,
                        error_message: if summary.aborted {
                            Some(summary.summary.clone())
                        } else {
                            None
                        },
                    }
                },
            )
            .await;

        match spawn_result {
            Ok(_) => match rx.await {
                Ok(summary) => summary,
                Err(_) => CodeReviewSummary::aborted_with(format!(
                    "[{}] code reviewer 子 Agent 退出但 summary channel 提前关闭",
                    self.origin
                )),
            },
            Err(e) => {
                let mut s = CodeReviewSummary::aborted_with(format!(
                    "[{}] code reviewer spawn 失败：{e}",
                    self.origin
                ));
                s.reviewer_turns_limit = turns_limit;
                s.reviewer_stop_reason = "spawn_error".into();
                s
            }
        }
    }
}

fn build_plan_summary_from_outcome(
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (PlanReviewSummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_review_text(&result);
            match parse_review_block(&text) {
                Some(parsed) => {
                    let mut s = PlanReviewSummary::from_parsed(parsed);
                    s.reviewer_turns_used = turns_used;
                    s.reviewer_turns_limit = turns_limit;
                    s.reviewer_stop_reason = if turns_used >= turns_limit {
                        "max_turns".into()
                    } else {
                        "completed".into()
                    };
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Completed)
                }
                None => {
                    let mut s = PlanReviewSummary::aborted_with(format!(
                        "[{origin}] reviewer 输出不符合 <review> 契约（child={child_session_id}）"
                    ));
                    s.reviewer_turns_used = turns_used;
                    s.reviewer_turns_limit = turns_limit;
                    s.reviewer_stop_reason = "parse_error".into();
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Failed)
                }
            }
        }
        AgentRunOutcome::Interrupted(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let mut s = PlanReviewSummary::aborted_with(format!(
                "[{origin}] reviewer 被父 abort / cancel（child={child_session_id}）"
            ));
            s.reviewer_turns_used = turns_used;
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "parent_abort".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Interrupted)
        }
        AgentRunOutcome::Failed(e) => {
            let mut s =
                PlanReviewSummary::aborted_with(format!("[{origin}] reviewer 子 Agent 失败：{e}"));
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "llm_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}

fn build_code_summary_from_outcome(
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    plan_text: &str,
    outcome: AgentRunOutcome,
) -> (CodeReviewSummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_review_text(&result);
            match parse_review_block(&text) {
                Some(parsed) => {
                    let mut s = CodeReviewSummary::from_parsed(parsed, plan_text);
                    s.reviewer_turns_used = turns_used;
                    s.reviewer_turns_limit = turns_limit;
                    s.reviewer_stop_reason = if turns_used >= turns_limit {
                        "max_turns".into()
                    } else {
                        "completed".into()
                    };
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Completed)
                }
                None => {
                    let mut s = CodeReviewSummary::aborted_with(format!(
                        "[{origin}] reviewer 输出不符合 <review> 契约（child={child_session_id}）"
                    ));
                    s.reviewer_turns_used = turns_used;
                    s.reviewer_turns_limit = turns_limit;
                    s.reviewer_stop_reason = "parse_error".into();
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Failed)
                }
            }
        }
        AgentRunOutcome::Interrupted(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let mut s = CodeReviewSummary::aborted_with(format!(
                "[{origin}] reviewer 被父 abort / cancel（child={child_session_id}）"
            ));
            s.reviewer_turns_used = turns_used;
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "parent_abort".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Interrupted)
        }
        AgentRunOutcome::Failed(e) => {
            let mut s =
                CodeReviewSummary::aborted_with(format!("[{origin}] reviewer 子 Agent 失败：{e}"));
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "llm_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}

/// 生产 explorer dispatcher（`dispatch_agent` 的后端）。装配点：`ChatContext::from_config`。
pub struct ProdExplorerDispatcher {
    origin: &'static str,
    deps: Option<ProdReviewerDeps>,
}

impl ProdExplorerDispatcher {
    pub fn stub(origin: &'static str) -> Self {
        Self { origin, deps: None }
    }

    pub fn new(origin: &'static str, deps: ProdReviewerDeps) -> Self {
        Self {
            origin,
            deps: Some(deps),
        }
    }
}

#[async_trait]
impl ExplorerDispatcher for ProdExplorerDispatcher {
    async fn dispatch(&self, task: &ExplorerTask) -> ExplorerReport {
        let Some(deps) = self.deps.as_ref() else {
            return ExplorerReport::aborted_with(
                &task.id,
                format!(
                    "[{}] explorer 子 Agent 未注入依赖（stub 模式）",
                    self.origin
                ),
            );
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return ExplorerReport::aborted_with(
                &task.id,
                format!("[{}] PlanRuntime 已被 drop，explorer 取消派发", self.origin),
            );
        };

        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let initial_user_message = build_explorer_prompt(task, workspace_root);
        let turns_limit = deps.max_turns.max(1);

        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let refresh_mutation_stamp = deps.refresh_mutation_stamp;
        let bash_config = deps.bash_config.clone();
        let gate = Arc::clone(&deps.gate);
        let confirmation = Arc::clone(&deps.confirmation);
        let audit = Arc::clone(&deps.audit);
        let bash_ast = deps.bash_ast.clone();
        let tool_defs = resolve_internal_tools(EXPLORER_ALLOWED_TOOLS);
        let model_id = deps.resolve_model(&plan_runtime);
        let thinking_level = Some(deps.resolve_thinking_level(&model_id));
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;
        let task_id = task.id.clone();
        let task_id_for_closure = task_id.clone();
        let runtime = match resolve_subagent_runtime(
            deps.llm_resolver.as_ref(),
            &deps.context_config,
            &deps.llm_files_config,
            &deps.sessions_dir,
            &parent_session_id,
            &model_id,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                let mut r = ExplorerReport::aborted_with(
                    &task_id,
                    format!(
                        "[{}] explorer 模型 `{}` 解析失败：{}",
                        self.origin, model_id, err
                    ),
                );
                r.turns_limit = turns_limit;
                r.stop_reason = "model_unresolved".into();
                return r;
            }
        };
        let compaction_provider = runtime.compaction_provider.clone();
        let compaction_output_limit = runtime
            .compaction_output_limit
            .or_else(|| runtime.main_call.output_limit_for_request(None).0);
        let context_config = runtime.context_config.clone();
        let openai_files_runtime = runtime.openai_files_runtime.clone();
        let binding = runtime.main_call;
        let transcript_model = binding.catalog_id().to_string();

        let (tx, rx) = tokio::sync::oneshot::channel::<ExplorerReport>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::Explorer,
                move |spawn_ctx| async move {
                    let read_file_state = Arc::new(ReadFileState::with_mutation_stamp_refresh(
                        refresh_mutation_stamp,
                    ));
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();
                    let transcript_root = agent_trail_dir.clone();
                    let bash_task_registry =
                        crate::core::tools::primitive::build_bash_task_registry(
                            &bash_config,
                            std::path::PathBuf::from(&transcript_root)
                                .join("tool-results")
                                .join(format!("subagent-{}", spawn_ctx.subagent_type.as_str()))
                                .join(&child_session_id),
                            gate.clone(),
                            confirmation.clone(),
                            audit.clone(),
                            bash_ast.clone(),
                        );
                    let transcript_sink =
                        crate::core::session::subagent_transcript::open_subagent_transcript(
                            &transcript_root,
                            &child_session_id,
                            SubagentType::Explorer,
                            &transcript_model,
                            &parent_session_id_for_closure,
                        );
                    let initial_message_sink = transcript_sink.clone();
                    let transcript_path =
                        crate::core::session::subagent_transcript::format_subagent_transcript_path(
                            &transcript_root,
                            &child_session_id,
                        );
                    plan_runtime.write_explorer_started_transcript(
                        &task_id_for_closure,
                        Some(&child_session_id),
                        transcript_path.as_deref(),
                    );

                    let system_text = format!(
                        "{}\n(max_turns budget: {} reasoning turns)\n",
                        explorer_system_prompt_text(),
                        turns_limit
                    );
                    let cfg = AgentLoopConfig {
                        max_attempts: crate::infra::config::DEFAULT_AGENT_MAX_ATTEMPTS,
                        unattended_retry: true,
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms:
                            crate::infra::config::DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
                        thinking_level,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_provider,
                        compaction_output_limit,
                        title_provider: None,
                        title_model: String::new(),
                        title_output_limit: None,
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        message_append_sink: transcript_sink,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::Explorer,
                        plan_runtime: None,
                        skill_set: None,
                        ephemeral_tail_provider: None,
                    };
                    let mut agent_loop =
                        AgentLoop::new(binding, primitive, event_bus, cfg, cancel_token.clone())
                            .with_bash_task_registry(bash_task_registry);
                    let initial_messages = vec![
                        ChatMessage::system(&system_text),
                        ChatMessage::user(&initial_user_message),
                    ];
                    crate::core::session::subagent_transcript::persist_initial_messages(
                        initial_message_sink.as_ref(),
                        &initial_messages,
                    );
                    let run_outcome = agent_loop.run(initial_messages).await;

                    let (report, label) = build_explorer_report_from_outcome(
                        origin,
                        &task_id_for_closure,
                        &child_session_id,
                        turns_limit,
                        run_outcome,
                    );
                    let _ = tx.send(report.clone());

                    SubagentOutcome {
                        child_session_id: child_session_id.clone(),
                        subagent_type: SubagentType::Explorer,
                        outcome_label: label,
                        error_message: if report.aborted {
                            Some(report.report.clone())
                        } else {
                            None
                        },
                    }
                },
            )
            .await;

        match spawn_result {
            Ok(_) => match rx.await {
                Ok(report) => report,
                Err(_) => ExplorerReport::aborted_with(
                    &task_id,
                    format!(
                        "[{}] explorer 子 Agent 退出但 report channel 提前关闭",
                        self.origin
                    ),
                ),
            },
            Err(e) => {
                let mut r = ExplorerReport::aborted_with(
                    &task_id,
                    format!("[{}] explorer spawn 失败：{e}", self.origin),
                );
                r.turns_limit = turns_limit;
                r.stop_reason = "spawn_error".into();
                r
            }
        }
    }
}

fn build_explorer_report_from_outcome(
    origin: &'static str,
    task_id: &str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (ExplorerReport, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_review_text(&result);
            let report = ExplorerReport {
                id: task_id.to_string(),
                aborted: text.trim().is_empty(),
                report: if text.trim().is_empty() {
                    format!("[{origin}] explorer 未产出任何结论（child={child_session_id}）")
                } else {
                    text
                },
                turns_used,
                turns_limit,
                stop_reason: if turns_used >= turns_limit {
                    "max_turns".into()
                } else {
                    "completed".into()
                },
                child_session_id: child_session_id.to_string(),
            };
            let label = if report.aborted {
                SubagentOutcomeLabel::Failed
            } else {
                SubagentOutcomeLabel::Completed
            };
            (report, label)
        }
        AgentRunOutcome::Interrupted(result) => {
            let mut r = ExplorerReport::aborted_with(
                task_id,
                format!("[{origin}] explorer 被父 abort / cancel（child={child_session_id}）"),
            );
            r.turns_used = count_assistant_turns(&result.new_messages);
            r.turns_limit = turns_limit;
            r.stop_reason = "parent_abort".into();
            r.child_session_id = child_session_id.to_string();
            (r, SubagentOutcomeLabel::Interrupted)
        }
        AgentRunOutcome::Failed(e) => {
            let mut r = ExplorerReport::aborted_with(
                task_id,
                format!("[{origin}] explorer 子 Agent 失败：{e}"),
            );
            r.turns_limit = turns_limit;
            r.stop_reason = "llm_error".into();
            r.child_session_id = child_session_id.to_string();
            (r, SubagentOutcomeLabel::Failed)
        }
    }
}
