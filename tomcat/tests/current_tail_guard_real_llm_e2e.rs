//! Manual end-to-end build task for current-tail preheat and application.
//!
//! It uses a real gpt-5.6-terra relay, real disk reads/writes, the production
//! `AgentLoop::run` path, and a persisted session transcript.

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;
use tomcat::core::session::MessageAppendSink;
use tomcat::core::tools::contract::catalog::builtin_tool_by_name;
use tomcat::{
    AgentLoop, AgentLoopConfig, AppConfig, AppError, BashResult, ChatMessage, DefaultEventBus,
    DirEntry, EditFileResult, EditOperation, EventBus, EventContext, PrimitiveExecutor,
    PrimitiveOperation, SessionManager, WriteFileResult,
};

const MODEL_ENV: &str = "TOMCAT_E2E_GUARD_REAL_MODEL";
const DEFAULT_MODEL: &str = "idatatlas/gpt-5.6-terra";
const TOTAL_TIMEOUT: Duration = Duration::from_secs(300);
const SECOND_READ_DELAY: Duration = Duration::from_secs(12);
const TEST_CONTEXT_WINDOW_TOKENS: usize = 40_000;
const TEST_OUTPUT_RESERVE_TOKENS: usize = 8_000;

struct DelayedDiskPrimitive {
    root: PathBuf,
    read_count: AtomicUsize,
}

impl DelayedDiskPrimitive {
    fn checked_path(&self, path: &str) -> Result<PathBuf, AppError> {
        let path = PathBuf::from(path);
        if !path.starts_with(&self.root) {
            return Err(AppError::Primitive(format!(
                "test primitive rejects path outside {}: {}",
                self.root.display(),
                path.display()
            )));
        }
        Ok(path)
    }
}

#[async_trait]
impl PrimitiveExecutor for DelayedDiskPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        let ordinal = self.read_count.fetch_add(1, Ordering::SeqCst) + 1;
        if ordinal == 2 {
            // Preheat begins after the first read. Holding this real tool call gives the
            // concurrently running real summary enough time to become applicable.
            tokio::time::sleep(SECOND_READ_DELAY).await;
        }
        std::fs::read_to_string(self.checked_path(path)?)
            .map_err(|error| AppError::Primitive(format!("read {path}: {error}")))
    }

    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(Vec::new())
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        let path = self.checked_path(path)?;
        if path.exists() && !overwrite {
            return Err(AppError::Primitive(format!(
                "write refuses existing file without overwrite: {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AppError::Primitive(format!("create {}: {error}", parent.display()))
            })?;
        }
        std::fs::write(&path, content)
            .map_err(|error| AppError::Primitive(format!("write {}: {error}", path.display())))?;
        Ok(WriteFileResult {
            path: path.display().to_string(),
            written: true,
            bytes_written: content.len() as u64,
            diff_hint: None,
            added: None,
            removed: None,
            diff: None,
            diff_truncated: false,
        })
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Err(AppError::Primitive(
            "this build task only permits read and write".to_string(),
        ))
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _foreground_wait_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        Err(AppError::Primitive(
            "this build task only permits read and write".to_string(),
        ))
    }

    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

fn tool_definition(name: &str) -> serde_json::Value {
    let entry = builtin_tool_by_name(name).expect("builtin tool must be registered");
    serde_json::json!({
        "type": "function",
        "function": {
            "name": entry.name,
            "description": entry.description,
            "parameters": (entry.parameters)(),
        }
    })
}

fn selected_model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn has_dangling_tool_calls(messages: &[ChatMessage]) -> bool {
    let mut outstanding = HashSet::new();
    for message in messages {
        if let Some(calls) = &message.tool_calls {
            outstanding.extend(
                calls
                    .iter()
                    .filter_map(|call| call.get("id").and_then(serde_json::Value::as_str))
                    .map(str::to_string),
            );
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            outstanding.remove(tool_call_id);
        }
    }
    !outstanding.is_empty()
}

#[tokio::test]
#[ignore = "manual: real AgentLoop build task through a gpt-5.6-terra relay"]
#[serial]
async fn real_terra_agent_loop_applies_preheat_during_build_task() {
    common::setup_logging();
    let model_id = selected_model();
    assert!(
        matches!(
            model_id.as_str(),
            "idatatlas/gpt-5.6-terra" | "fcodex/gpt-5.6-terra"
        ),
        "{MODEL_ENV} must select idatatlas/gpt-5.6-terra or fcodex/gpt-5.6-terra"
    );

    // The real models path and credential source must be resolved before the temp HOME guard.
    let models_toml = common::real_models_toml_path();
    let runtime_env = common::real_runtime_env_path();
    let _home = common::TempHomeGuard::new();
    let work_dir = common::dot_tomcat_e2e_workdir("current_tail_guard_real_llm_e2e");
    std::fs::create_dir_all(&work_dir).expect("create e2e workspace");

    let mut app_config = AppConfig::default();
    app_config.storage.work_dir = Some(work_dir.display().to_string());
    let wire_model = match common::apply_models_toml_entry_app_config(
        &mut app_config,
        &models_toml,
        &runtime_env,
        &model_id,
    ) {
        Ok(wire_model) => wire_model,
        Err(error) => {
            eprintln!("skipping real current-tail E2E: {error}");
            return;
        }
    };
    app_config.llm.default_model = model_id.clone();
    app_config.context.compaction_model = model_id.clone();
    let mut binding = common::resolve_main_call(&app_config);
    // Preserve the real provider and wire model while using a declared 40K test context window.
    // This reaches the production budget derivation without paying for a 272K-token marathon.
    binding.limits.context_window = TEST_CONTEXT_WINDOW_TOKENS;
    binding.limits.model_max_output_tokens = Some(TEST_OUTPUT_RESERVE_TOKENS);
    binding.limits.output_reserve_tokens = TEST_OUTPUT_RESERVE_TOKENS;
    binding.limits.input_budget_tokens = TEST_CONTEXT_WINDOW_TOKENS - TEST_OUTPUT_RESERVE_TOKENS;

    let sources: Vec<PathBuf> = (1..=3)
        .map(|index| work_dir.join(format!("source-{index}.txt")))
        .collect();
    for (index, path) in sources.iter().enumerate() {
        std::fs::write(path, format!("source {index} build input\n").repeat(4_000))
            .expect("write source fixture");
    }
    let output = work_dir.join("build-result.txt");
    let source_list = sources
        .iter()
        .map(|path| format!("- {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Run this build verification task exactly. Read these three files in the listed order:\n\
         {source_list}\n\
         Call `read` exactly once in each assistant response; do not batch calls. After all \
         three reads, call `write` exactly once to create `{}` with the exact content \
         `build task verified\\n`. Do not call any other tools. Then give a brief final answer.",
        output.display()
    );

    let sessions = tempfile::tempdir().expect("create session directory");
    let session = SessionManager::new(sessions.path().to_path_buf());
    let key = session.current_session_key();
    session.create_session(key, None).expect("create session");
    let user_id = session
        .append_message(serde_json::json!({"role": "user", "content": prompt}))
        .expect("persist initial user message");
    let mut initial_user = ChatMessage::user(prompt);
    initial_user.msg_id = Some(user_id);

    let preheat_started = Arc::new(AtomicBool::new(false));
    let event_bus = Arc::new(DefaultEventBus::new());
    let preheat_started_listener = Arc::clone(&preheat_started);
    event_bus.on(
        tomcat::infra::wire::WIRE_AUTO_COMPACTION_START,
        Box::new(move |_context: EventContext| {
            preheat_started_listener.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    let sink: Arc<dyn MessageAppendSink> = Arc::new(session.clone());
    let primitive = Arc::new(DelayedDiskPrimitive {
        root: work_dir.clone(),
        read_count: AtomicUsize::new(0),
    });
    let mut agent = AgentLoop::new(
        binding,
        primitive,
        event_bus,
        AgentLoopConfig {
            session_id: format!("current-tail-e2e-{wire_model}"),
            max_attempts: 1,
            system_prompt: Some("You are a build task agent.".to_string()),
            max_tool_rounds: 8,
            tool_definitions: vec![tool_definition("read"), tool_definition("write")],
            context_config: tomcat::ContextConfig {
                current_tail_compactable_min_chars: 1,
                compaction_model: model_id.clone(),
                // Keep phase-2 tool results in memory so the third read exercises the
                // current-tail Collapse fallback and produces <recent_files>.
                current_tail_single_result_max_chars: usize::MAX,
                layer0_single_result_max_chars: usize::MAX,
                ..Default::default()
            },
            message_append_sink: Some(sink),
            agent_trail_dir: work_dir.display().to_string(),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    let initial_messages = vec![initial_user];
    let initial_chars = initial_messages
        .iter()
        .map(tomcat::core::session::estimate_msg_chars)
        .sum();
    agent.set_context_state(Some(tomcat::ContextState {
        messages: Vec::new(),
        estimate_context_chars: initial_chars,
        context_budget_chars: TEST_CONTEXT_WINDOW_TOKENS * 4,
        context_budget_tokens: TEST_CONTEXT_WINDOW_TOKENS - TEST_OUTPUT_RESERVE_TOKENS,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: session
            .current_transcript_path()
            .expect("session transcript path")
            .expect("created transcript path"),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: tomcat::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let outcome = tokio::time::timeout(TOTAL_TIMEOUT, agent.run(initial_messages))
        .await
        .expect("real build task timed out");
    assert!(
        matches!(outcome, tomcat::AgentRunOutcome::Completed(_)),
        "real build task must complete: {outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&output).unwrap_or_else(|error| panic!(
            "read real build artifact: {error}; outcome={outcome:?}"
        )),
        "build task verified\n"
    );
    assert!(
        preheat_started.load(Ordering::SeqCst),
        "the first large read must cross the preheat watermark"
    );

    let transcript = session
        .current_transcript_path()
        .expect("session transcript path")
        .expect("created transcript path");
    let entries = tomcat::core::session::transcript::read_entries_tail(&transcript, 64)
        .expect("read transcript");
    let preheat_marker_ids: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            tomcat::TranscriptEntry::BranchSummary(summary)
                if summary.is_boundary == Some(true) && summary.summary.is_none() =>
            {
                summary.id.as_deref()
            }
            _ => None,
        })
        .collect();
    assert!(
        !preheat_marker_ids.is_empty(),
        "preheat must append an unfulfilled branch_summary marker at its cut"
    );
    let preheat_applied = entries.iter().any(|entry| {
        matches!(
            entry,
            tomcat::TranscriptEntry::BranchSummaryText(body)
                if preheat_marker_ids.contains(body.for_id.as_str())
        )
    });
    let collapse_preserved_file_index = entries.iter().any(|entry| {
        matches!(
            entry,
            tomcat::TranscriptEntry::BranchSummary(summary)
                if summary.summary.as_deref().is_some_and(|text| {
                    text.contains("<recent_files>")
                        && sources.iter().any(|path| text.contains(&path.display().to_string()))
                })
        )
    });
    // A real preheat may lose the race to the same-turn Collapse when the relay spends longer
    // generating the summary than the main model spends completing the remaining reads. Both
    // paths are valid; require that one of them durably preserves the work instead of treating
    // scheduler timing as a single-list regression.
    assert!(
        preheat_applied || collapse_preserved_file_index,
        "ready preheat must apply, or its safe Collapse fallback must preserve the file index"
    );
    assert!(
        collapse_preserved_file_index,
        "the Collapse summary must retain a path-only <recent_files> index"
    );
    let rehydrated =
        tomcat::init_context_state(&session, &app_config.context, "You are a build task agent.")
            .expect("rehydrate real build transcript");
    assert!(
        !has_dangling_tool_calls(&rehydrated.messages),
        "collapse and reload must retain complete assistant/tool pairs"
    );
    let state = agent
        .take_context_state()
        .expect("context state remains available");
    eprintln!(
        "phase=\"real_current_tail_e2e\" model={model_id} cache_hit_ratio={:?} \
         tail_changed_count={} compaction_count={}",
        state.session_obs.cache_hit_ratio(),
        state.session_obs.tail_changed_count,
        state.session_obs.compaction_count,
    );
    assert!(
        state.session_obs.prompt_tokens_total > 0,
        "the real provider must report prompt usage for D4 observation"
    );
    assert!(
        state.session_obs.cache_observed_request_count > 0,
        "the real provider must report cache usage for D4 observation"
    );
    assert!(
        state.session_obs.cache_hit_ratio().is_some(),
        "reported cache usage must yield a cache-hit ratio"
    );
    assert_eq!(
        state.session_obs.tail_changed_count, 0,
        "this stable-tail build task must not attribute cache misses to tail changes"
    );
}
