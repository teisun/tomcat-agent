//! internal verifier 子 Agent（plan-exec-code-verification.md PR-V1/V2）。
//!
//! 设计口径：
//! - 通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`] 派发；
//! - 工具白名单固定为 `{read, search_files, list_dir, bash}`；
//! - 输出必须是 `<verify>...</verify>` block，解析为 [`VerifySummary`]；
//! - `VERIFIER_MAX_TURNS` 固定 64，不进 TOML。

use std::path::Path;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::core::agent_loop::{
    AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, SubagentType,
};
use crate::core::agent_registry::{AgentRegistry, SubagentOutcome, SubagentOutcomeLabel};
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::plan_runtime::review::resolve_internal_tools;
use crate::core::plan_runtime::{PlanRuntime, VerifierDispatcher};
use crate::core::prompts::{load as load_prompt, render as render_prompt, PromptKey};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::CheckpointStore;
use crate::infra::config::ContextConfig;
use crate::infra::event_bus::EventBus;

pub const VERIFIER_MAX_TURNS: u32 = 64;

/// verifier 子 Agent allowed tools 硬白名单。
pub(crate) const VERIFIER_ALLOWED_TOOLS: &[&str] = &["read", "search_files", "list_dir", "bash"];

pub(crate) fn verifier_allowed_tools_with_policy(expose_skills: bool) -> Vec<&'static str> {
    let mut tools = VERIFIER_ALLOWED_TOOLS.to_vec();
    if expose_skills {
        tools.push("load_skill");
    }
    tools
}

pub fn verifier_system_prompt_text() -> &'static str {
    load_prompt(PromptKey::Verifier)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyCheck {
    pub name: String,
    #[serde(default)]
    pub command: String,
    pub result: String,
    #[serde(default)]
    pub output_excerpt: String,
}

/// verifier 摘要（`update_plan` tool result 与 `plan.verify` transcript 共用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifySummary {
    #[serde(default)]
    pub checks: Vec<VerifyCheck>,
    pub verdict: String,
    pub summary: String,
    #[serde(default)]
    pub verifier_turns_used: u32,
    #[serde(default)]
    pub verifier_turns_limit: u32,
    #[serde(default)]
    pub verifier_stop_reason: String,
    #[serde(default)]
    pub child_session_id: String,
}

impl VerifySummary {
    /// dispatcher 未注入时的占位返回。
    pub fn placeholder_pending() -> Self {
        Self {
            checks: Vec::new(),
            verdict: "aborted".into(),
            summary: "verifier 子 Agent 未注入，返回占位摘要".into(),
            verifier_turns_used: 0,
            verifier_turns_limit: VERIFIER_MAX_TURNS,
            verifier_stop_reason: "not_dispatched".into(),
            child_session_id: String::new(),
        }
    }

    pub fn aborted_with(reason: impl Into<String>) -> Self {
        Self {
            checks: Vec::new(),
            verdict: "aborted".into(),
            summary: reason.into(),
            verifier_turns_used: 0,
            verifier_turns_limit: VERIFIER_MAX_TURNS,
            verifier_stop_reason: "aborted".into(),
            child_session_id: String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "checks": self.checks,
            "verdict": self.verdict,
            "summary": self.summary,
            "verifier_turns_used": self.verifier_turns_used,
            "verifier_turns_limit": self.verifier_turns_limit,
            "verifier_stop_reason": self.verifier_stop_reason,
            "child_session_id": self.child_session_id,
        })
    }
}

/// 严格解析 `<verify>...</verify>` 块。失败返回 None；多块取最后一个。
pub fn parse_verify_block(text: &str) -> Option<VerifySummary> {
    let last_block = find_last_verify_block(text)?;
    let mut summary: VerifySummary = serde_yaml::from_str(last_block).ok()?;
    normalize_summary(&mut summary)?;
    Some(summary)
}

fn normalize_summary(summary: &mut VerifySummary) -> Option<()> {
    summary.verdict = summary.verdict.trim().to_ascii_lowercase();
    if !matches!(
        summary.verdict.as_str(),
        "pass" | "fail" | "partial" | "aborted"
    ) {
        return None;
    }
    if summary.summary.len() > 600 {
        summary.summary.truncate(600);
    }
    for check in &mut summary.checks {
        check.result = check.result.trim().to_ascii_lowercase();
        if !matches!(check.result.as_str(), "pass" | "fail" | "skip") {
            return None;
        }
        if check.output_excerpt.len() > 500 {
            check.output_excerpt.truncate(500);
        }
    }
    Some(())
}

fn find_last_verify_block(text: &str) -> Option<&str> {
    let start_tag = "<verify>";
    let end_tag = "</verify>";
    let mut last_start = None;
    let mut search_from = 0;
    while let Some(s) = text[search_from..].find(start_tag) {
        last_start = Some(search_from + s);
        search_from = search_from + s + start_tag.len();
    }
    let start = last_start?;
    let body_start = start + start_tag.len();
    let end_rel = text[body_start..].find(end_tag)?;
    Some(&text[body_start..body_start + end_rel])
}

/// 规范化 runtime gate 语义。
///
/// - `pass` 但无 command → 降为 `skip`
/// - 关键检查（build/test/lint）全部 skip → `partial`
pub fn normalize_for_gate(summary: &mut VerifySummary) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut saw_key_check = false;
    let mut saw_key_non_skip = false;
    for check in &mut summary.checks {
        let key_probe = format!("{} {}", check.name, check.command).to_ascii_lowercase();
        let is_key = key_probe.contains("test")
            || key_probe.contains("build")
            || key_probe.contains("lint")
            || key_probe.contains("clippy");
        if is_key {
            saw_key_check = true;
        }
        if check.result == "pass" && check.command.trim().is_empty() {
            check.result = "skip".into();
            warnings.push(format!(
                "check `{}` 声称 pass 但 command 为空，已降级为 skip",
                check.name
            ));
        }
        if is_key && check.result != "skip" {
            saw_key_non_skip = true;
        }
    }
    if summary.verdict != "fail"
        && summary.verdict != "aborted"
        && saw_key_check
        && !saw_key_non_skip
    {
        summary.verdict = "partial".into();
        warnings.push("关键 build/test/lint 检查均未实际跑通，verdict 已降级为 partial".into());
    }
    warnings
}

/// 构造 verifier brief。
pub fn build_verify_prompt(
    plan_id: &str,
    plan_text: &str,
    plan_path: &Path,
    workspace_root: Option<&Path>,
) -> String {
    let plan_path = crate::infra::platform::format_home_path(plan_path);
    let workspace_hint = workspace_root
        .map(|path| {
            format!(
                "         - Project/workspace root (start inspection here first): `{}`\n\
                 - Access note: reads or bash may still require runtime authorization depending on workspace grants.\n",
                crate::infra::platform::format_home_path(path)
            )
        })
        .unwrap_or_default();
    render_prompt(
        PromptKey::VerifierBrief,
        &[
            ("plan_id", plan_id),
            ("plan_path", &plan_path),
            ("workspace_hint", &workspace_hint),
            ("plan_text", plan_text),
        ],
    )
}

pub struct ProdVerifierDispatcher {
    origin: &'static str,
    deps: Option<ProdVerifierDeps>,
}

pub struct ProdVerifierDeps {
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
    pub plan_runtime: Weak<PlanRuntime>,
    pub model: String,
}

impl ProdVerifierDispatcher {
    pub fn stub(origin: &'static str) -> Self {
        Self { origin, deps: None }
    }

    pub fn new(origin: &'static str, deps: ProdVerifierDeps) -> Self {
        Self {
            origin,
            deps: Some(deps),
        }
    }
}

#[async_trait]
impl VerifierDispatcher for ProdVerifierDispatcher {
    async fn dispatch(
        &self,
        plan_id: &str,
        plan_text: &str,
    ) -> VerifySummary {
        let Some(deps) = self.deps.as_ref() else {
            return VerifySummary::aborted_with(format!(
                "[{}] 生产 verifier 子 Agent 未注入依赖（stub 模式）",
                self.origin
            ));
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return VerifySummary::aborted_with(format!(
                "[{}] PlanRuntime 已被 drop，verifier 取消派发",
                self.origin
            ));
        };

        let plan_path = match plan_runtime.resolved_plan_path(plan_id) {
            Ok(path) => path,
            Err(err) => return VerifySummary::aborted_with(err),
        };
        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let initial_user_message =
            build_verify_prompt(plan_id, plan_text, &plan_path, workspace_root);
        let turns_limit = VERIFIER_MAX_TURNS;

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
        let tool_defs = resolve_internal_tools(&verifier_allowed_tools_with_policy(expose_skills));
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
        let mut system_text = format!(
            "{}\n(max_turns budget: {} reasoning turns)\n",
            verifier_system_prompt_text(),
            turns_limit
        );
        if let Some(skill_prompt) = skill_prompt.as_ref() {
            system_text.push('\n');
            system_text.push_str(skill_prompt);
        }

        let (tx, rx) = tokio::sync::oneshot::channel::<VerifySummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::Verifier,
                move |spawn_ctx| async move {
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = spawn_ctx.cancel_token.clone();

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
                        message_append_sink: None,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::Verifier,
                        review_kind: None,
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

                    let (summary, label) = build_summary_from_outcome(
                        origin,
                        &child_session_id,
                        turns_limit,
                        run_outcome,
                    );
                    let _ = tx.send(summary.clone());

                    SubagentOutcome {
                        child_session_id: child_session_id.clone(),
                        subagent_type: SubagentType::Verifier,
                        outcome_label: label,
                        error_message: if summary.verdict == "aborted" {
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
                Err(_) => VerifySummary::aborted_with(format!(
                    "[{}] verifier 子 Agent 退出但 summary channel 提前关闭",
                    self.origin
                )),
            },
            Err(e) => {
                let mut s = VerifySummary::aborted_with(format!(
                    "[{}] verifier spawn 失败：{e}",
                    self.origin
                ));
                s.verifier_turns_limit = turns_limit;
                s.verifier_stop_reason = "spawn_error".into();
                s
            }
        }
    }
}

pub(crate) fn build_summary_from_outcome(
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (VerifySummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let exhausted_budget =
                exhausted_turn_budget_without_terminal_completion(&result, turns_used, turns_limit);
            let text = extract_verify_text(&result);
            match parse_verify_block(&text) {
                Some(mut s) => {
                    s.verifier_turns_used = turns_used;
                    s.verifier_turns_limit = turns_limit;
                    s.child_session_id = child_session_id.to_string();
                    if exhausted_budget {
                        s.verdict = "aborted".into();
                        s.verifier_stop_reason = "max_turns".into();
                        append_budget_exhausted_note(&mut s.summary, turns_limit);
                        (s, SubagentOutcomeLabel::Failed)
                    } else {
                        s.verifier_stop_reason = "completed".into();
                        (s, SubagentOutcomeLabel::Completed)
                    }
                }
                None => {
                    let mut s = if exhausted_budget {
                        VerifySummary::aborted_with(format!(
                            "[{origin}] verifier 在 {turns_limit} 轮预算内未正常收口（child={child_session_id}）"
                        ))
                    } else {
                        VerifySummary::aborted_with(format!(
                            "[{origin}] verifier 输出不符合 <verify> 契约（child={child_session_id}）"
                        ))
                    };
                    s.verifier_turns_used = turns_used;
                    s.verifier_turns_limit = turns_limit;
                    s.verifier_stop_reason = if exhausted_budget {
                        "max_turns".into()
                    } else {
                        "parse_error".into()
                    };
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Failed)
                }
            }
        }
        AgentRunOutcome::Interrupted(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let mut s = VerifySummary::aborted_with(format!(
                "[{origin}] verifier 被父 abort / cancel（child={child_session_id}）"
            ));
            s.verifier_turns_used = turns_used;
            s.verifier_turns_limit = turns_limit;
            s.verifier_stop_reason = "parent_abort".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Interrupted)
        }
        AgentRunOutcome::Failed(e) => {
            let mut s =
                VerifySummary::aborted_with(format!("[{origin}] verifier 子 Agent 失败：{e}"));
            s.verifier_turns_limit = turns_limit;
            s.verifier_stop_reason = "spawn_error".into();
            s.child_session_id = child_session_id.to_string();
            (s, SubagentOutcomeLabel::Failed)
        }
    }
}

fn extract_verify_text(result: &AgentRunResult) -> String {
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

fn exhausted_turn_budget_without_terminal_completion(
    result: &AgentRunResult,
    turns_used: u32,
    turns_limit: u32,
) -> bool {
    if turns_used > turns_limit {
        return true;
    }
    if turns_used < turns_limit {
        return false;
    }
    !ended_with_terminal_assistant_message(&result.new_messages)
}

fn ended_with_terminal_assistant_message(messages: &[ChatMessage]) -> bool {
    use crate::core::llm::ChatMessageRole;
    matches!(
        messages.last().map(|msg| &msg.role),
        Some(ChatMessageRole::Assistant)
    )
}

fn append_budget_exhausted_note(summary: &mut String, turns_limit: u32) {
    let note = format!(
        "[runtime override] verifier exhausted the {turns_limit}-turn budget before normal completion."
    );
    if summary.is_empty() {
        *summary = note;
    } else if !summary.contains(&note) {
        summary.push(' ');
        summary.push_str(&note);
    }
    if summary.len() > 600 {
        summary.truncate(600);
    }
}
