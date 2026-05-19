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

use crate::api::chat::plan_runtime::review::{
    build_review_prompt, parse_review_block, resolve_internal_tools, ReviewSummary,
    REVIEWER_SYSTEM_PROMPT,
};
use crate::api::chat::plan_runtime::{PlanRuntime, ReviewerDispatcher};
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

/// reviewer 子 Agent allowed_tools 硬白名单（reviewer.md §5.2）。
/// 任何模式下都不含 `create_plan` / `bash` / `write` / `dispatch_agent` / `checkpoint`。
///
/// 注：本仓库的 catalog 把 grep/find 合并为 `search_files`（按 mode/glob/regex 分发），
/// 这里以 catalog 的实际工具名为准。
pub(crate) const REVIEWER_ALLOWED_TOOLS: &[&str] = &[
    "read",
    "search_files",
    "list_dir",
    "todos",
    "update_plan",
    "edit",
];

/// 生产 reviewer dispatcher。装配点：[`crate::api::chat::ChatContext::from_config`]。
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
        allow_review_edit: bool,
        _abort_signal: Arc<AtomicBool>,
    ) -> ReviewSummary {
        debug_assert!(
            allow_review_edit,
            "生产路径恒为 true；false 仅 Mock 单测注入"
        );

        let Some(deps) = self.deps.as_ref() else {
            return ReviewSummary::aborted_with(format!(
                "[{}] 生产 reviewer 子 Agent 未注入依赖（stub 模式）",
                self.origin
            ));
        };
        let Some(plan_runtime) = deps.plan_runtime.upgrade() else {
            return ReviewSummary::aborted_with(format!(
                "[{}] PlanRuntime 已被 drop，reviewer 取消派发",
                self.origin
            ));
        };

        let plan_path = crate::api::chat::plan_runtime::file_store::plan_path_for_id(plan_id)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(format!("~/.tomcat/plans/{plan_id}.plan.md"))
            });
        let workspace_root = std::env::current_dir().ok();
        let initial_user_message =
            build_review_prompt(plan_id, plan_text, &plan_path, workspace_root.as_deref());
        let tool_defs = resolve_internal_tools(REVIEWER_ALLOWED_TOOLS);
        let turns_limit = deps.max_turns.max(1);

        // spawn 闭包需要 'static + Send，所有依赖一次性 clone。
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

                    let system_text = format!(
                        "{}\n(max_turns budget: {} reasoning turns)\n",
                        REVIEWER_SYSTEM_PROMPT, turns_limit
                    );
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
                        subagent_type: SubagentType::Reviewer,
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
                let mut s = ReviewSummary::aborted_with(format!(
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

fn build_summary_from_outcome(
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
                    let mut s = ReviewSummary::aborted_with(format!(
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
            let mut s = ReviewSummary::aborted_with(format!(
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
                ReviewSummary::aborted_with(format!("[{origin}] reviewer 子 Agent 失败：{e}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::chat::plan_runtime::review::resolve_internal_tools;
    use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;

    #[tokio::test]
    async fn prod_reviewer_stub_returns_aborted_with_origin() {
        let d = ProdReviewerDispatcher::stub("test_origin");
        let r = d
            .dispatch("demo", "noop", true, Arc::new(AtomicBool::new(false)))
            .await;
        assert!(r.aborted);
        assert!(r.summary.contains("test_origin"));
        assert!(!r.applied_changes);
    }

    /// reviewer.md §11 RV-T1：reviewer 没有任何独立 catalog schema，
    /// LLM 永远见不到 `reviewer` / `review` 名字的工具入口。
    #[test]
    fn reviewer_not_in_catalog() {
        for entry in BUILTIN_TOOL_CATALOG.iter() {
            assert_ne!(entry.name, "reviewer", "catalog 不应暴露 `reviewer` 工具");
            assert_ne!(entry.name, "review", "catalog 不应暴露 `review` 工具");
        }
    }

    /// reviewer.md §11 RV-T3：reviewer 允许的工具集恒不含 create_plan/bash/write/
    /// dispatch_agent/checkpoint，即便它们出现在 catalog 里。
    #[test]
    fn reviewer_default_allowed_tools_no_create_plan() {
        let tools = resolve_internal_tools(REVIEWER_ALLOWED_TOOLS);
        let names: std::collections::BTreeSet<String> = tools
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains("create_plan"));
        assert!(!names.contains("bash"));
        assert!(!names.contains("write"));
        assert!(!names.contains("dispatch_agent"));
        assert!(!names.contains("checkpoint"));
        // 应该至少包含改稿权这套
        assert!(names.contains("update_plan"));
        assert!(names.contains("edit"));
        assert!(names.contains("read"));
    }

    /// reviewer.md §9：max_turns 默认 64，落到 ReviewSummary.reviewer_turns_limit；
    /// stop_reason 默认 "spawn_error"（dispatcher 未注入依赖时的 path）。
    #[test]
    fn reviewer_max_turns_default_is_64() {
        let config = crate::infra::config::ReviewerConfig::default();
        assert_eq!(config.max_turns, 64);
    }
}
