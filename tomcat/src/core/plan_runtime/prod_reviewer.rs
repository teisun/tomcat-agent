//! plan reviewer / code reviewer 的生产 dispatcher。
//!
//! 两者都通过 `spawn_subagent_internal` 同步派发，但子 Agent 类型、prompt、tool 白名单、
//! 结构化摘要类型都彼此独立；这里只保留共享的依赖注入和 spawn 机械流程。

use std::sync::{Arc, Weak};

use async_trait::async_trait;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome, SubagentType};
use crate::core::agent_registry::{AgentRegistry, SubagentOutcome, SubagentOutcomeLabel};
use crate::core::llm::system_prompt::render_available_skills_prompt;
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::plan_runtime::code_reviewer::{
    build_code_review_prompt, code_review_system_prompt_text,
    code_reviewer_allowed_tools_with_policy, collect_git_diff_context, CodeReviewSummary,
};
use crate::core::plan_runtime::plan_reviewer::{
    build_review_prompt, plan_reviewer_allowed_tools_with_policy, reviewer_system_prompt_text,
    PlanReviewSummary,
};
use crate::core::plan_runtime::review::{
    count_assistant_turns, extract_review_text, parse_review_block, resolve_internal_tools,
};
use crate::core::plan_runtime::{CodeReviewerDispatcher, PlanReviewerDispatcher, PlanRuntime};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::CheckpointStore;
use crate::infra::config::ContextConfig;
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
    pub llm: Arc<dyn LlmProvider>,
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub event_bus: Arc<dyn EventBus>,
    pub agent_trail_dir: String,
    pub checkpoint_store: Arc<dyn CheckpointStore>,
    pub context_config: ContextConfig,
    pub read_file_state: Arc<ReadFileState>,
    pub openai_files_runtime: Option<Arc<OpenAiFilesRuntime>>,
    pub agent_workspace_dir: std::path::PathBuf,
    pub skill_set: Arc<parking_lot::RwLock<crate::core::skill::SkillSet>>,
    pub skills_config: crate::infra::config::SkillsConfig,
    /// Weak 引用避免与 `PlanRuntime` 内部 dispatcher 字段形成 cycle。
    pub plan_runtime: Weak<PlanRuntime>,
    pub model: String,
    /// 子 AgentLoop `max_tool_rounds`（`TOMCAT_REVIEWER_MAX_TURNS` 默认 64）。
    pub max_turns: u32,
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

        let llm = Arc::clone(&deps.llm);
        let compaction_provider = deps.compaction_provider.clone();
        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let context_config = deps.context_config.clone();
        let context_budget_chars =
            crate::infra::config::compute_context_budget_chars(&context_config);
        let read_file_state = Arc::clone(&deps.read_file_state);
        let openai_files_runtime = deps.openai_files_runtime.clone();
        let shared_skill_set = Arc::clone(&deps.skill_set);
        let skill_set = deps.skill_set.read().clone();
        let expose_skills =
            plan_runtime.expose_skills_to_reviewer() && !skill_set.visible_skills().is_empty();
        let tool_defs =
            resolve_internal_tools(&plan_reviewer_allowed_tools_with_policy(expose_skills));
        let skill_prompt = if expose_skills {
            render_available_skills_prompt(
                &skill_set,
                context_budget_chars,
                &deps.skills_config,
            )
        } else {
            None
        };
        let plan_runtime_for_loop = Arc::clone(&plan_runtime);
        let model = deps.model.clone();
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;

        let (tx, rx) = tokio::sync::oneshot::channel::<PlanReviewSummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::PlanReviewer,
                move |spawn_ctx| async move {
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();
                    let transcript_root = agent_trail_dir.clone();
                    let transcript_sink =
                        crate::core::session::subagent_transcript::open_subagent_transcript(
                            &transcript_root,
                            &child_session_id,
                            SubagentType::PlanReviewer,
                            &model,
                            &parent_session_id_for_closure,
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
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms:
                            crate::infra::config::DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
                        model,
                        thinking_level: None,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_provider,
                        title_provider: None,
                        title_model: String::new(),
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
                    };
                    let mut agent_loop =
                        AgentLoop::new(llm, primitive, event_bus, cfg, cancel_token.clone());
                    let initial_messages = vec![
                        ChatMessage::system(&system_text),
                        ChatMessage::user(&initial_user_message),
                    ];
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
                let mut s =
                    PlanReviewSummary::aborted_with(format!("[{}] reviewer spawn 失败：{e}", self.origin));
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
    async fn dispatch(&self, plan_id: &str, plan_text: &str) -> CodeReviewSummary {
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
        let (diff_stat, changed_files) = collect_git_diff_context(deps.agent_workspace_dir.as_path());
        let initial_user_message = build_code_review_prompt(
            plan_id,
            plan_text,
            &plan_path,
            workspace_root,
            &diff_stat,
            &changed_files,
        );
        let turns_limit = deps.max_turns.max(1);

        let llm = Arc::clone(&deps.llm);
        let compaction_provider = deps.compaction_provider.clone();
        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let context_config = deps.context_config.clone();
        let context_budget_chars =
            crate::infra::config::compute_context_budget_chars(&context_config);
        let read_file_state = Arc::clone(&deps.read_file_state);
        let openai_files_runtime = deps.openai_files_runtime.clone();
        let shared_skill_set = Arc::clone(&deps.skill_set);
        let skill_set = deps.skill_set.read().clone();
        let expose_skills =
            plan_runtime.expose_skills_to_reviewer() && !skill_set.visible_skills().is_empty();
        let tool_defs =
            resolve_internal_tools(&code_reviewer_allowed_tools_with_policy(expose_skills));
        let skill_prompt = if expose_skills {
            render_available_skills_prompt(
                &skill_set,
                context_budget_chars,
                &deps.skills_config,
            )
        } else {
            None
        };
        let plan_runtime_for_loop = Arc::clone(&plan_runtime);
        let model = deps.model.clone();
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;

        let (tx, rx) = tokio::sync::oneshot::channel::<CodeReviewSummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::CodeReviewer,
                move |spawn_ctx| async move {
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();
                    let transcript_root = agent_trail_dir.clone();
                    let transcript_sink =
                        crate::core::session::subagent_transcript::open_subagent_transcript(
                            &transcript_root,
                            &child_session_id,
                            SubagentType::CodeReviewer,
                            &model,
                            &parent_session_id_for_closure,
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
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms:
                            crate::infra::config::DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
                        model,
                        thinking_level: None,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_provider,
                        title_provider: None,
                        title_model: String::new(),
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        message_append_sink: transcript_sink,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::CodeReviewer,
                        plan_runtime: Some(plan_runtime_for_loop),
                        skill_set: if expose_skills {
                            Some(Arc::clone(&shared_skill_set))
                        } else {
                            None
                        },
                    };
                    let mut agent_loop =
                        AgentLoop::new(llm, primitive, event_bus, cfg, cancel_token.clone());
                    let initial_messages = vec![
                        ChatMessage::system(&system_text),
                        ChatMessage::user(&initial_user_message),
                    ];
                    let run_outcome = agent_loop.run(initial_messages).await;

                    let (summary, label) = build_code_summary_from_outcome(
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
            s.reviewer_stop_reason = "spawn_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}

fn build_code_summary_from_outcome(
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (CodeReviewSummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_review_text(&result);
            match parse_review_block(&text) {
                Some(parsed) => {
                    let mut s = CodeReviewSummary::from_parsed(parsed);
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
            s.reviewer_stop_reason = "spawn_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}
