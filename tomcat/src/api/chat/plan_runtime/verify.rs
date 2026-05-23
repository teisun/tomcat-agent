//! internal verifier 子 Agent（plan-exec-code-verification.md PR-V1/V2）。
//!
//! 设计口径：
//! - 通过 [`crate::core::agent_registry::AgentRegistry::spawn_subagent_internal`] 派发；
//! - 工具白名单固定为 `{read, search_files, list_dir, bash}`；
//! - 输出必须是 `<verify>...</verify>` block，解析为 [`VerifySummary`]；
//! - `VERIFIER_MAX_TURNS` 固定 64，不进 TOML。

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::api::chat::plan_runtime::review::resolve_internal_tools;
use crate::api::chat::plan_runtime::{PlanRuntime, VerifierDispatcher};
use crate::core::agent_loop::{
    AgentLoop, AgentLoopConfig, AgentRunOutcome, AgentRunResult, SubagentType,
};
use crate::core::agent_registry::{AgentRegistry, SubagentOutcome, SubagentOutcomeLabel};
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::{ChatMessage, LlmProvider};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::CheckpointStore;
use crate::infra::config::ContextConfig;
use crate::infra::event_bus::EventBus;

pub const VERIFIER_MAX_TURNS: u32 = 64;

/// verifier 子 Agent allowed tools 硬白名单。
pub(crate) const VERIFIER_ALLOWED_TOOLS: &[&str] = &["read", "search_files", "list_dir", "bash"];

pub const VERIFIER_SYSTEM_PROMPT: &str = r#"
You are an internal verification subagent. You are NOT the user-facing agent.

You receive a verification brief in the initial user message with the plan, paths,
constraints, and current workspace root. Treat that brief as the source of truth.

Goal:
- Verify the deliverable after all plan todos are marked completed.
- Prefer reproducible checks tied to the changed area.
- Be adversarial enough to catch obvious misses, but stay within the granted tools.

Tool policy:
- Allowed tools are runtime-filtered. Expect only: read, search_files, list_dir, bash.
- create_plan, update_plan, todos, edit, write, dispatch_agent, checkpoint and any
  other write-capable tool are NEVER available.
- If you cannot verify because the environment blocks you, report that explicitly.

Command discovery:
1. Reuse concrete commands already present in the plan body or brief.
2. Otherwise inspect nearby manifests first (package.json, Cargo.toml, pyproject.toml,
   Makefile, justfile, go.mod, pom.xml, etc.).
3. Then inspect nearby README / CONTRIBUTING docs.
4. Do not guess default repo-wide commands without first reading a manifest or doc.
5. Prefer the smallest scoped check that meaningfully exercises the deliverable.

Verification workflow:
1. Read the plan file and inspect the workspace around the target area.
2. Choose a small set of reproducible checks (build / test / lint / smoke) based on
   explicit commands you discovered.
3. Run them with bash, capture concise evidence, and note skips when blocked.
4. Include at least one adversarial probe when relevant (for example run a focused
   failing-path smoke check, read the changed file to confirm the expected symbol,
   or inspect a nearby test target when execution is unavailable).
5. End with the exact output block below as the final assistant message.

Output contract (must be the final assistant message):

<verify>
checks:
  - name: "<short label>"
    command: "<exact command or read/search description>"
    result: pass|fail|skip
    output_excerpt: "<short evidence snippet>"
verdict: pass|fail|partial|aborted
summary: <what you verified, what failed/skipped, and why>
</verify>

Rules:
1. Report only what you actually observed.
2. Use `result: skip` for environment blocks or missing commands; do not fake a pass.
3. `verdict: fail` is reserved for observed failures. Use `partial` when the checks are
   inconclusive but not failed.
4. Keep `summary` under 600 chars and each `output_excerpt` under 500 chars.
"#;

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
    format!(
        "Verify the finished PlanFile (plan_id = `{plan_id}`).\n\
         Artifact paths (use these first; do not guess alternate locations):\n\
         - Exact plan file path: `{}`\n\
         {}\
         Scope:\n\
         - Treat all todos as already marked completed; you are checking whether that is justified.\n\
         - Read the exact plan file path above before broad search if you need to confirm current disk content.\n\
         - Discover commands in this order: plan body / brief, nearby manifest, nearby README, then minimal inferred smoke.\n\
         - Do NOT default to repo-wide `npm test`, workspace-wide `cargo test`, or whole-repo `pytest` without reading a manifest/doc first.\n\
         - Prefer the smallest scoped build/test/lint command that matches the changed area.\n\
         - End with the required <verify> output block.\n\n\
         ----- BEGIN PLAN -----\n{plan_text}\n----- END PLAN -----\n"
        ,
        plan_path,
        workspace_hint,
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
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub event_bus: Arc<dyn EventBus>,
    pub agent_trail_dir: String,
    pub checkpoint_store: Arc<dyn CheckpointStore>,
    pub context_config: ContextConfig,
    pub read_file_state: Arc<ReadFileState>,
    pub openai_files_runtime: Option<Arc<OpenAiFilesRuntime>>,
    pub agent_workspace_dir: std::path::PathBuf,
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
        _abort_signal: Arc<AtomicBool>,
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

        let plan_path = crate::api::chat::plan_runtime::file_store::plan_path_for_id(plan_id)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(format!("~/.tomcat/plans/{plan_id}.plan.md"))
            });
        let workspace_root = Some(deps.agent_workspace_dir.as_path());
        let initial_user_message =
            build_verify_prompt(plan_id, plan_text, &plan_path, workspace_root);
        let tool_defs = resolve_internal_tools(VERIFIER_ALLOWED_TOOLS);
        let turns_limit = VERIFIER_MAX_TURNS;

        let llm = Arc::clone(&deps.llm);
        let primitive = Arc::clone(&deps.primitive);
        let event_bus = Arc::clone(&deps.event_bus);
        let agent_trail_dir = deps.agent_trail_dir.clone();
        let checkpoint_store = Arc::clone(&deps.checkpoint_store);
        let context_config = deps.context_config.clone();
        let read_file_state = Arc::clone(&deps.read_file_state);
        let openai_files_runtime = deps.openai_files_runtime.clone();
        let plan_runtime_for_loop = Arc::clone(&plan_runtime);
        let model = deps.model.clone();
        let parent_session_id = deps.parent_session_id.clone();
        let parent_session_id_for_closure = parent_session_id.clone();
        let origin = self.origin;
        let system_text = format!(
            "{}\n(max_turns budget: {} reasoning turns)\n",
            VERIFIER_SYSTEM_PROMPT, turns_limit
        );

        let (tx, rx) = tokio::sync::oneshot::channel::<VerifySummary>();

        let spawn_result = deps
            .agent_registry
            .spawn_subagent_internal(
                &parent_session_id,
                SubagentType::Verifier,
                move |spawn_ctx| async move {
                    let child_session_id = spawn_ctx.child_session_id.clone();
                    let cancel_token = CancellationToken::new();

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

                    let cfg = AgentLoopConfig {
                        max_attempts: 3,
                        max_tool_rounds: turns_limit as usize,
                        retry_base_delay_ms: 300,
                        model,
                        session_id: child_session_id.clone(),
                        tool_definitions: tool_defs,
                        context_config,
                        agent_trail_dir,
                        read_file_state,
                        openai_files_runtime,
                        checkpoint_store,
                        parent_session_id: Some(parent_session_id_for_closure.clone()),
                        spawn_depth: spawn_ctx.spawn_depth,
                        subagent_type: SubagentType::Verifier,
                        plan_runtime: Some(plan_runtime_for_loop),
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

fn build_summary_from_outcome(
    origin: &'static str,
    child_session_id: &str,
    turns_limit: u32,
    outcome: AgentRunOutcome,
) -> (VerifySummary, SubagentOutcomeLabel) {
    match outcome {
        AgentRunOutcome::Completed(result) => {
            let turns_used = count_assistant_turns(&result.new_messages);
            let text = extract_verify_text(&result);
            match parse_verify_block(&text) {
                Some(mut s) => {
                    s.verifier_turns_used = turns_used;
                    s.verifier_turns_limit = turns_limit;
                    s.verifier_stop_reason = if turns_used >= turns_limit {
                        "max_turns".into()
                    } else {
                        "completed".into()
                    };
                    s.child_session_id = child_session_id.to_string();
                    (s, SubagentOutcomeLabel::Completed)
                }
                None => {
                    let mut s = VerifySummary::aborted_with(format!(
                        "[{origin}] verifier 输出不符合 <verify> 契约（child={child_session_id}）"
                    ));
                    s.verifier_turns_used = turns_used;
                    s.verifier_turns_limit = turns_limit;
                    s.verifier_stop_reason = "parse_error".into();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::chat::plan_runtime::review::resolve_internal_tools;
    use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;

    #[test]
    fn parse_verify_block_happy_path() {
        let text = r#"
noise
<verify>
checks:
  - name: unit
    command: cargo test -p tomcat verify
    result: pass
    output_excerpt: ok
verdict: pass
summary: all good
</verify>
"#;
        let summary = parse_verify_block(text).unwrap();
        assert_eq!(summary.verdict, "pass");
        assert_eq!(summary.summary, "all good");
        assert_eq!(summary.checks.len(), 1);
        assert_eq!(summary.checks[0].result, "pass");
    }

    #[test]
    fn parse_verify_block_picks_last_block() {
        let text = r#"
<verify>
checks: []
verdict: partial
summary: old
</verify>
<verify>
checks: []
verdict: fail
summary: new
</verify>
"#;
        let summary = parse_verify_block(text).unwrap();
        assert_eq!(summary.verdict, "fail");
        assert_eq!(summary.summary, "new");
    }

    #[test]
    fn parse_verify_block_rejects_unknown_verdict() {
        let text = r#"
<verify>
checks: []
verdict: maybe
summary: nope
</verify>
"#;
        assert!(parse_verify_block(text).is_none());
    }

    #[test]
    fn verify_summary_round_trips_to_json() {
        let summary = VerifySummary {
            checks: vec![VerifyCheck {
                name: "unit".into(),
                command: "cargo test".into(),
                result: "pass".into(),
                output_excerpt: "ok".into(),
            }],
            verdict: "pass".into(),
            summary: "looks good".into(),
            verifier_turns_used: 2,
            verifier_turns_limit: 64,
            verifier_stop_reason: "completed".into(),
            child_session_id: "child-1".into(),
        };
        let json = summary.to_json();
        assert_eq!(json["verdict"], "pass");
        assert_eq!(json["summary"], "looks good");
        assert_eq!(json["checks"][0]["command"], "cargo test");
        assert_eq!(json["verifier_turns_limit"], 64);
    }

    #[test]
    fn verifier_system_prompt_contains_contract() {
        let prompt = VERIFIER_SYSTEM_PROMPT;
        assert!(prompt.contains("<verify>"));
        assert!(prompt.contains("pass|fail|partial|aborted"));
        assert!(prompt.contains("read, search_files, list_dir, bash"));
        assert!(prompt.contains("Do not guess default repo-wide commands"));
    }

    #[test]
    fn verifier_not_in_catalog() {
        for entry in BUILTIN_TOOL_CATALOG.iter() {
            assert_ne!(entry.name, "verifier");
            assert_ne!(entry.name, "verify");
        }
    }

    #[test]
    fn verifier_allowed_tools_do_not_include_write_paths() {
        let tools = resolve_internal_tools(VERIFIER_ALLOWED_TOOLS);
        let names: std::collections::BTreeSet<String> = tools
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains("read"));
        assert!(names.contains("search_files"));
        assert!(names.contains("list_dir"));
        assert!(names.contains("bash"));
        assert!(!names.contains("create_plan"));
        assert!(!names.contains("update_plan"));
        assert!(!names.contains("write"));
        assert!(!names.contains("edit"));
    }

    #[test]
    fn verifier_max_turns_default_is_64() {
        assert_eq!(VERIFIER_MAX_TURNS, 64);
    }
}
