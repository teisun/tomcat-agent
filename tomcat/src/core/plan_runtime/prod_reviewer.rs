//! 生产 `ReviewerDispatcher` 实现（reviewer.md RV-A~RV-E / plan-runtime.md §P4）。
//!
//! 通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`] 派一个
//! `SubagentType::Reviewer` 子 [`AgentLoop`]，子 catalog 硬编码为
//! `{read, grep, find, todos, update_plan, edit}`（reviewer.md §5.2 / §5.5），父
//! abort_signal 通过 watcher 桥接为子 `CancellationToken`。
//!
//! - **改稿权固定开启**：`allow_review_edit` 形参在生产路径恒为 `true`；Mock 单测可
//!   注入 `false` 验证只读路径。
//! - **避免 Arc cycle**：dispatcher 持 `Weak<PlanRuntime>`；`PlanRuntime` 持
//!   `Arc<dyn ReviewerDispatcher>`，drop 时反向 dangling 不会泄漏内存。
//! - **解析失败不阻 create_plan**：parse 失败 → `ReviewSummary::aborted_with`，
//!   `reviewer_stop_reason = "parse_error"`。
//! - **transcript turn 计数**：reviewer 结束时把 `reviewer_turns_used / limit / stop_reason`
//!   写进返回的 `ReviewSummary`，由调用方落 `plan.review` 自定义事件 + `ToolResult.review`。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{
    AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, SubagentType,
};
use crate::core::agent_registry::{AgentRegistry, SubagentOutcome, SubagentOutcomeLabel};
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::plan_runtime::review::{
    build_code_review_prompt, build_review_prompt, code_review_system_prompt_text,
    parse_review_block, resolve_internal_tools, reviewer_allowed_tools_with_policy,
    reviewer_system_prompt_text, ReviewKind, ReviewSummary,
};
use crate::core::plan_runtime::{PlanRuntime, ReviewerDispatcher};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::CheckpointStore;
use crate::infra::config::ContextConfig;
use crate::infra::event_bus::EventBus;

/// 生产 reviewer dispatcher。装配点：`ChatContext::from_config`。
pub struct ProdReviewerDispatcher {
    origin: &'static str,
    deps: Option<ProdReviewerDeps>,
}

/// 完整依赖集合；构造时一次性注入。dispatch 仅 clone Arc 拷贝至 spawn 闭包。
pub struct ProdReviewerDeps {
    pub agent_registry: Arc<AgentRegistry>,
    pub parent_session_id: String,
    pub llm: Arc<dyn LlmProvider>,
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
    /// Weak 引用避免与 `PlanRuntime::reviewer: Arc<dyn ReviewerDispatcher>` 形成 cycle。
    pub plan_runtime: Weak<PlanRuntime>,
    pub model: String,
    /// 子 AgentLoop `max_tool_rounds`（reviewer.md §9，`TOMCAT_REVIEWER_MAX_TURNS` 默认 64）。
    pub max_turns: u32,
}

impl ProdReviewerDispatcher {
    /// 构造未注入依赖的 stub 版本（早期装配阶段 / 测试占位）。
    pub fn stub(origin: &'static str) -> Self {
        Self { origin, deps: None }
    }

    /// 构造完整生产实现。
    pub fn new(origin: &'static str, deps: ProdReviewerDeps) -> Self {
        Self {
            origin,
            deps: Some(deps),
        }
    }
}

#[async_trait]
impl ReviewerDispatcher for ProdReviewerDispatcher {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
        kind: ReviewKind,
        _allow_review_edit: bool,
        _abort_signal: Arc<AtomicBool>,
    ) -> ReviewSummary {
        let Some(deps) = self.deps.as_ref() else {
            return ReviewSummary::aborted_with_kind(
                kind,
                format!(
                    "[{}] 生产 reviewer 子 Agent 未注入依赖（stub 模式）",
                    self.origin
                ),
            );
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return ReviewSummary::aborted_with_kind(
                kind,
                format!("[{}] PlanRuntime 已被 drop，reviewer 取消派发", self.origin),
            );
        };

        let plan_path = match plan_runtime.resolved_plan_path(plan_id) {
            Ok(path) => path,
            Err(err) => return ReviewSummary::aborted_with_kind(kind, err),
        };
        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let initial_user_message = match kind {
            ReviewKind::Plan => build_review_prompt(plan_id, plan_text, &plan_path, workspace_root),
            ReviewKind::Code => {
                let (diff_stat, changed_files) =
                    collect_git_diff_context(deps.agent_workspace_dir.as_path());
                build_code_review_prompt(
                    plan_id,
                    plan_text,
                    &plan_path,
                    workspace_root,
                    &diff_stat,
                    &changed_files,
                )
            }
        };
        let turns_limit = deps.max_turns.max(1);

        // spawn 闭包需要 'static + Send，所有依赖一次性 clone。
        let llm = Arc::clone(&deps.llm);
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
            resolve_internal_tools(&reviewer_allowed_tools_with_policy(kind, expose_skills));
        let skill_prompt = if expose_skills {
            crate::core::llm::system_prompt::render_available_skills_prompt(
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

        // 通过 oneshot 把 `ReviewSummary` 从 spawn 闭包传回；`SubagentOutcome.error_message`
        // 不足以承载结构化 finding / turn 计数。
        let (tx, rx) = tokio::sync::oneshot::channel::<ReviewSummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::Reviewer,
                move |spawn_ctx| async move {
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = CancellationToken::new();

                    // CascadeAbort 桥接：父 abort_signal 翻起 → cancel 子 token。
                    // 100ms 粒度对人感来说足够（reviewer 长跑数秒到数分钟）。
                    let token_for_watcher = cancel_token.clone();
                    let abort_clone = Arc::clone(&spawn_ctx.abort_signal);
                    let watcher = tokio::spawn(async move {
                        loop {
                            if abort_clone.load(std::sync::atomic::Ordering::Acquire) {
                                token_for_watcher.cancel();
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    });

                    let mut system_text = format!(
                        "{}\n(max_turns budget: {} reasoning turns)\n",
                        reviewer_system_prompt(kind),
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
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        compaction_llm: None,
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        message_append_sink: None,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::Reviewer,
                        review_kind: Some(kind),
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
                    watcher.abort();

                    let (summary, label) = build_summary_from_outcome(
                        kind,
                        origin,
                        &child_session_id,
                        turns_limit,
                        run_outcome,
                    );
                    let _ = tx.send(summary.clone());

                    SubagentOutcome {
                        child_session_id: child_session_id.clone(),
                        subagent_type: SubagentType::Reviewer,
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
                Err(_) => ReviewSummary::aborted_with(format!(
                    "[{}] reviewer 子 Agent 退出但 summary channel 提前关闭",
                    self.origin
                )),
            },
            Err(e) => {
                let mut s = ReviewSummary::aborted_with_kind(
                    kind,
                    format!("[{}] reviewer spawn 失败：{e}", self.origin),
                );
                s.reviewer_turns_limit = turns_limit;
                s.reviewer_stop_reason = "spawn_error".into();
                s
            }
        }
    }
}

fn build_summary_from_outcome(
    kind: ReviewKind,
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (ReviewSummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_review_text(&result);
            match parse_review_block(&text) {
                Some(mut s) => {
                    s.kind = kind;
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
                    let mut s = ReviewSummary::aborted_with_kind(
                        kind,
                        format!(
                            "[{origin}] reviewer 输出不符合 <review> 契约（child={child_session_id}）"
                        ),
                    );
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
            let mut s = ReviewSummary::aborted_with_kind(
                kind,
                format!("[{origin}] reviewer 被父 abort / cancel（child={child_session_id}）"),
            );
            s.reviewer_turns_used = turns_used;
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "parent_abort".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Interrupted)
        }
        AgentRunOutcome::Failed(e) => {
            let mut s = ReviewSummary::aborted_with_kind(
                kind,
                format!("[{origin}] reviewer 子 Agent 失败：{e}"),
            );
            s.reviewer_turns_limit = turns_limit;
            s.reviewer_stop_reason = "spawn_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}

/// reviewer 最终消息体——优先取 `final_text`（reasoning_loop 累计），fallback 到
/// `new_messages` 中最后一条 Assistant 文本。
fn extract_review_text(result: &AgentRunResult) -> String {
    if !result.final_text.trim().is_empty() {
        return result.final_text.clone();
    }
    use crate::core::llm::ChatMessageRole;
    for msg in result.new_messages.iter().rev() {
        if matches!(msg.role, ChatMessageRole::Assistant) {
            if let Some(text) = msg.text_content() {
                if !text.trim().is_empty() {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

fn count_assistant_turns(messages: &[ChatMessage]) -> u32 {
    use crate::core::llm::ChatMessageRole;
    messages
        .iter()
        .filter(|m| matches!(m.role, ChatMessageRole::Assistant))
        .count() as u32
}

fn reviewer_system_prompt(kind: ReviewKind) -> &'static str {
    match kind {
        ReviewKind::Plan => reviewer_system_prompt_text(),
        ReviewKind::Code => code_review_system_prompt_text(),
    }
}

fn collect_git_diff_context(workspace_root: &std::path::Path) -> (String, Vec<String>) {
    use std::collections::BTreeSet;
    let diff_stat = run_git_capture(workspace_root, &["diff", "--stat", "--no-ext-diff", "HEAD"])
        .unwrap_or_default();

    let mut changed_files = BTreeSet::new();
    for line in run_git_lines(
        workspace_root,
        &["diff", "--name-only", "--no-ext-diff", "HEAD"],
    ) {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }
    for line in run_git_lines(
        workspace_root,
        &["ls-files", "--others", "--exclude-standard"],
    ) {
        if !line.is_empty() {
            changed_files.insert(line);
        }
    }

    (diff_stat, changed_files.into_iter().collect())
}

fn run_git_capture(workspace_root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_lines(workspace_root: &std::path::Path, args: &[&str]) -> Vec<String> {
    run_git_capture(workspace_root, args)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
