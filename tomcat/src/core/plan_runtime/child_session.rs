//! One-shot child session assembly. Role-specific dispatchers own the task and
//! result contracts; this module owns the loop, its private state and seeds.
use std::sync::Arc;

use super::explorer::explorer_system_prompt_text;
use super::plan_reviewer::reviewer_system_prompt_text;
use super::prod_reviewer::{ProdReviewerDeps, ResolvedSubagentRuntime};
use super::{todo_runtime::TodosRuntime, PlanRuntime};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::agent_registry::SubagentSpawnContext;
use crate::core::llm::system_prompt::render_available_skills_prompt;
use crate::core::llm::{ChatMessage, ThinkingLevel};
use crate::core::session::subagent_transcript as transcript;
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::ContextState;

pub(super) struct ChildSession {
    agent: AgentLoop,
    brief: ChatMessage,
    shell: Option<Arc<crate::core::tools::primitive::BashTaskRegistry>>,
    pub(super) transcript_hint: Option<String>,
    pub(super) sink: Option<Arc<dyn crate::core::session::MessageAppendSink>>,
}

impl ChildSession {
    pub(super) fn new(
        deps: &ProdReviewerDeps,
        runtime: ResolvedSubagentRuntime,
        spawn: SubagentSpawnContext,
        level: ThinkingLevel,
        review_plan: Option<Arc<PlanRuntime>>,
        brief: String,
    ) -> Self {
        let root = &deps.agent_trail_dir;
        let id = &spawn.child_session_id;
        let binding = runtime.main_call;
        let tokens = binding.limits.input_budget_tokens;
        let turns = deps.max_turns.max(1);
        let sink = transcript::open_subagent_transcript(
            root,
            id,
            spawn.subagent_type,
            binding.catalog_id(),
            &spawn.parent_session_id,
        );
        let path = sink
            .as_ref()
            .and_then(|_| transcript::subagent_transcript_path(root, id));
        let transcript_hint = path
            .as_ref()
            .and_then(|_| transcript::format_subagent_transcript_path(root, id));
        let todos = Arc::new(TodosRuntime::open(root.into(), id.clone(), None));
        let (system, skill_set, skill_prompt) = if let Some(plan) = &review_plan {
            let skills = deps.skill_set.read();
            let expose = plan.expose_skills_to_reviewer() && !skills.visible_skills().is_empty();
            let prompt = expose
                .then(|| {
                    render_available_skills_prompt(
                        &skills,
                        crate::infra::config::compute_context_budget_chars_from_tokens(tokens),
                        &deps.skills_config,
                    )
                })
                .flatten();
            (
                reviewer_system_prompt_text(),
                expose.then(|| deps.skill_set.clone()),
                prompt,
            )
        } else {
            (explorer_system_prompt_text(), None, None)
        };
        let tools = super::child_tools::for_role(spawn.subagent_type, skill_set.is_some())
            .expect("only Explorer and Plan Reviewer use ChildSession");
        let uses_bash = tools.iter().any(|t| t["function"]["name"] == "bash");
        let mut system = format!("{system}\n(max_turns budget: {turns} reasoning turns)\n");
        if let Some(prompt) = skill_prompt {
            system.push('\n');
            system.push_str(&prompt);
        }
        let state =
            ContextState::fresh(tokens, system.len() + brief.len(), path.unwrap_or_default());
        let mut seeds = [ChatMessage::system(&system), ChatMessage::user(brief)];
        transcript::persist_initial_messages(sink.as_ref(), &mut seeds);
        let [_, brief] = seeds;
        let bus = Arc::new(super::child_activity::ChildActivity {
            bus: deps.event_bus.clone(),
            registry: deps.agent_registry.clone(),
            parent: spawn.parent_session_id.clone(),
            child: id.clone(),
        });
        let cfg = AgentLoopConfig {
            max_attempts: deps.agent_max_attempts,
            system_prompt: Some(system),
            max_tool_rounds: turns as usize,
            retry_base_delay_ms: deps.agent_retry_base_delay_ms,
            thinking_level: Some(level),
            speed: deps
                .model_catalog
                .resolve_speed(deps.model_prefs.as_ref(), binding.catalog_id()),
            session_id: id.clone(),
            tool_definitions: tools,
            context_config: runtime.context_config,
            compaction_provider: runtime.compaction_provider,
            compaction_output_limit: runtime
                .compaction_output_limit
                .or_else(|| binding.output_limit_for_request(None).0),
            compaction_input_budget: runtime.compaction_input_budget,
            title_provider: None,
            title_model: String::new(),
            title_output_limit: None,
            agent_trail_dir: root.clone(),
            read_file_state: Arc::new(ReadFileState::with_mutation_stamp_refresh(
                deps.refresh_mutation_stamp,
            )),
            openai_files_runtime: runtime.openai_files_runtime,
            checkpoint_store: deps.checkpoint_store.clone(),
            message_append_sink: sink.clone(),
            parent_session_id: Some(spawn.parent_session_id),
            spawn_depth: spawn.spawn_depth,
            subagent_type: spawn.subagent_type,
            plan_runtime: review_plan,
            skill_set,
            ephemeral_tail_provider: None,
        };
        let mut agent = AgentLoop::new(
            binding,
            deps.primitive.clone(),
            bus,
            cfg,
            spawn.cancel_token,
        )
        .with_todos_runtime(todos);
        let shell = if uses_bash {
            let registry = crate::core::tools::primitive::build_bash_task_registry(
                &deps.bash_config,
                std::path::Path::new(root)
                    .join("tool-results")
                    .join(format!("subagent-{}", spawn.subagent_type.as_str()))
                    .join(id),
                deps.gate.clone(),
                deps.confirmation.clone(),
                deps.audit.clone(),
                deps.bash_ast.clone(),
                Some(deps.agent_workspace_dir.clone()),
            );
            registry.set_environment(deps.primitive.child_environment());
            agent = agent.with_bash_task_registry(registry.clone());
            Some(registry)
        } else {
            None
        };
        agent.set_context_state(Some(state));
        Self {
            agent,
            brief,
            shell,
            sink,
            transcript_hint,
        }
    }

    pub(super) async fn run(mut self) -> AgentRunOutcome {
        use futures_util::FutureExt;
        // Keep the loop's resources accessible while unwinding a provider/tool panic.
        let outcome = std::panic::AssertUnwindSafe(self.agent.run(vec![self.brief]))
            .catch_unwind()
            .await;
        let mut cleanup_error = None;
        if let Some(shell) = self.shell {
            use crate::core::tools::primitive::BashTaskStatus;
            let tasks = shell
                .list()
                .into_iter()
                .filter(|t| {
                    matches!(
                        t.status,
                        BashTaskStatus::Running | BashTaskStatus::DrainingOutput
                    )
                })
                .collect::<Vec<_>>();
            for task in &tasks {
                if let Err(err) = shell.stop(&task.task_id).await {
                    cleanup_error = Some(err);
                }
            }
            for task in tasks {
                if let Err(err) = shell.wait_for_finish(&task.task_id).await {
                    cleanup_error = Some(err);
                }
            }
        }
        if let Some(mut state) = self.agent.take_context_state() {
            state.preheat.abort_and_join().await;
        }
        match outcome {
            Ok(outcome) => cleanup_error.map_or(outcome, AgentRunOutcome::Failed),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown cause");
                let cleanup = cleanup_error
                    .map(|e| format!("; cleanup: {e}"))
                    .unwrap_or_default();
                AgentRunOutcome::Failed(crate::AppError::Internal(format!(
                    "child panicked: {message}{cleanup}"
                )))
            }
        }
    }
}
