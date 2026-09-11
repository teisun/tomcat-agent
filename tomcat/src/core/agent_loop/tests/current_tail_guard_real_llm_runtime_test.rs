//! Manual real-provider coverage for the internal current-tail guard seams.
//!
//! Run once per relay:
//! `TOMCAT_E2E_GUARD_REAL_MODEL=idatatlas/gpt-5.6-terra cargo test -p tomcat \
//! current_tail_guard_real_llm_runtime -- --ignored --nocapture`
//! and repeat with `fcodex/gpt-5.6-terra`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard::{self, GuardRoute};
use super::super::types::AgentLoop;
use super::super::AgentLoopConfig;
use super::mocks::MockPrimitiveExecutor;
use crate::core::compaction::preheat::{Preheat, PreheatOutcome};
use crate::core::llm::{
    DefaultLlmResolver, LlmResolver, LlmScene, MessageKind, ModelCatalog, ResolvedCall,
    ThinkingLevel,
};
use crate::core::session::manager::{estimate_msg_chars, ContextState};
use crate::core::session::ModelPrefsStore;
use crate::infra::config::{AppConfig, ContextConfig, LogConfig};
use crate::infra::event_bus::DefaultEventBus;
use crate::{init_context_state, ChatMessage, SessionManager};

const MODEL_ENV: &str = "TOMCAT_E2E_GUARD_REAL_MODEL";
const DEFAULT_MODEL: &str = "idatatlas/gpt-5.6-terra";
const PREHEAT_TIMEOUT: Duration = Duration::from_secs(120);

fn selected_model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn real_models_path() -> PathBuf {
    std::env::var_os("TOMCAT_E2E_MODELS_TOML")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".tomcat/models.toml")
        })
}

fn inject_model_api_key_or_skip(models_path: &Path, model_id: &str) -> Option<()> {
    let contents = match std::fs::read_to_string(models_path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!(
                "skipping current_tail_guard_real_llm_runtime: cannot read {}: {error}",
                models_path.display()
            );
            return None;
        }
    };
    let document: toml::Value = match toml::from_str(&contents) {
        Ok(document) => document,
        Err(error) => {
            eprintln!(
                "skipping current_tail_guard_real_llm_runtime: cannot parse {}: {error}",
                models_path.display()
            );
            return None;
        }
    };
    let Some(entry) = document
        .get("models")
        .and_then(toml::Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|entry| entry.get("id").and_then(toml::Value::as_str) == Some(model_id))
        })
    else {
        eprintln!(
            "skipping current_tail_guard_real_llm_runtime: model {model_id} is absent from {}",
            models_path.display()
        );
        return None;
    };
    let Some(api_key_env) = entry.get("api_key_env").and_then(toml::Value::as_str) else {
        eprintln!(
            "skipping current_tail_guard_real_llm_runtime: model {model_id} has no api_key_env"
        );
        return None;
    };
    if std::env::var(api_key_env)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Some(());
    }

    let env_path = models_path
        .parent()
        .map(|parent| parent.join("assets/.env"))
        .unwrap_or_default();
    let key = dotenvy::from_path_iter(&env_path)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .find_map(|(key, value)| (key == api_key_env).then_some(value))
        })
        .filter(|value| !value.trim().is_empty());
    let Some(key) = key else {
        eprintln!(
            "skipping current_tail_guard_real_llm_runtime: {api_key_env} is not exported or set in {}",
            env_path.display()
        );
        return None;
    };
    // The ignored test is serialised because this process-wide environment value may be consumed
    // by the provider constructor. Never overwrite a value supplied by the caller.
    unsafe { std::env::set_var(api_key_env, key) };
    Some(())
}

fn resolve_terra_binding_or_skip(model_id: &str) -> Option<ResolvedCall> {
    if !matches!(model_id, "idatatlas/gpt-5.6-terra" | "fcodex/gpt-5.6-terra") {
        eprintln!(
            "skipping current_tail_guard_real_llm_runtime: {MODEL_ENV} must select one of the two gpt-5.6-terra relays"
        );
        return None;
    }
    let models_path = real_models_path();
    inject_model_api_key_or_skip(&models_path, model_id)?;

    let mut config = AppConfig::default();
    config.llm.default_model = model_id.to_string();
    config.context.compaction_model = model_id.to_string();
    let catalog = match ModelCatalog::load_from_path(&config, models_path) {
        Ok(catalog) => Arc::new(catalog),
        Err(error) => {
            eprintln!(
                "skipping current_tail_guard_real_llm_runtime: model catalog failed: {error}"
            );
            return None;
        }
    };
    let prefs_path = std::env::temp_dir().join(format!(
        "tomcat-current-tail-real-llm-prefs-{}-{}.json",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let prefs = match ModelPrefsStore::load(prefs_path, ThinkingLevel::High) {
        Ok(prefs) => Arc::new(prefs),
        Err(error) => {
            eprintln!(
                "skipping current_tail_guard_real_llm_runtime: model preferences failed: {error}"
            );
            return None;
        }
    };
    let resolver = DefaultLlmResolver::new(config, catalog, prefs);
    match resolver.resolve(LlmScene::Main, Some(model_id)) {
        Ok(binding) => Some(binding),
        Err(error) => {
            eprintln!(
                "skipping current_tail_guard_real_llm_runtime: provider resolve failed: {error}"
            );
            None
        }
    }
}

fn make_agent(binding: ResolvedCall, config: ContextConfig, trail_dir: &Path) -> AgentLoop {
    AgentLoop::new(
        binding,
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "current-tail-guard-real-llm".to_string(),
            agent_trail_dir: trail_dir.display().to_string(),
            context_config: config,
            ..Default::default()
        },
        CancellationToken::new(),
    )
}

fn assistant_read_call(id: &str, path: &str) -> ChatMessage {
    ChatMessage::assistant_with_tool_calls(
        Some("read the requested file"),
        vec![serde_json::json!({
            "id": id,
            "type": "function",
            "function": {"name": "read", "arguments": format!(r#"{{"path":"{path}"}}"#)},
        })],
    )
}

async fn require_successful_preheat(agent: &mut AgentLoop) {
    let outcome = agent
        .context_state
        .as_mut()
        .expect("preheat requires context state")
        .preheat
        .await_result(PREHEAT_TIMEOUT)
        .await;
    let result = match outcome {
        PreheatOutcome::Completed(result) => result,
        PreheatOutcome::Exhausted => {
            panic!("real current-tail preheat exhausted all summary attempts")
        }
        PreheatOutcome::Failed => panic!("real current-tail preheat task failed"),
        PreheatOutcome::NotReady => {
            panic!("real current-tail preheat did not finish before timeout")
        }
    };
    agent
        .context_state
        .as_mut()
        .expect("preheat requires context state")
        .preheat
        .restore_completed(result);
}

#[tokio::test]
#[ignore = "manual: real gpt-5.6-terra preheat/apply/resume guard verification"]
#[serial(env_lock)]
async fn real_terra_preheat_apply_and_resume_first_request_guard() {
    let _ = crate::infra::logging::init_logging(&LogConfig::default(), None);
    let model_id = selected_model();
    let Some(binding) = resolve_terra_binding_or_skip(&model_id) else {
        return;
    };
    let temp = tempfile::tempdir().expect("create real-llm test directory");
    let context_config = ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 10_000,
        compaction_model: model_id.clone(),
        ..Default::default()
    };

    // Phase 1: a Fits request above 50% starts a real preheat.
    let mut user = ChatMessage::user("Summarize the current implementation state.\n".repeat(120));
    user.msg_id = Some("preheat-user".to_string());
    let mut assistant = assistant_read_call("preheat-read", "src/lib.rs");
    assistant.msg_id = Some("preheat-assistant".to_string());
    let mut tool = ChatMessage::tool("preheat-read", &"source line\n".repeat(1_000));
    tool.msg_id = Some("preheat-tool".to_string());
    let mut messages = vec![ChatMessage::system("system"), user, assistant, tool];
    let chars: usize = messages.iter().skip(1).map(estimate_msg_chars).sum();
    let mut agent = make_agent(binding.clone(), context_config.clone(), temp.path());
    agent.start_idx = 1;
    agent.context_tail_start = 1;
    agent.set_context_state(Some(ContextState {
        messages: Vec::new(),
        estimate_context_chars: chars,
        context_budget_chars: 20_000,
        context_budget_tokens: 5_000,
        last_api_usage: None,
        post_usage_appended_chars: chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let phase_one = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .expect("start real preheat")
    .expect("phase one must make a guard decision");
    assert_eq!(phase_one.route, GuardRoute::Fits);
    assert!(
        !agent.context_state.as_ref().unwrap().preheat.is_idle(),
        "a Fits payload above the 50% watermark must start preheat"
    );
    require_successful_preheat(&mut agent).await;
    let summary_result = match agent.context_state.as_mut().unwrap().preheat.poll_result() {
        PreheatOutcome::Completed(result) => result,
        _ => panic!("the restored real preheat result must remain available for inspection"),
    };
    assert!(
        summary_result
            .summary_text
            .contains("<verbatim_user_messages>\nCopied verbatim")
            && !summary_result.summary_text.contains("<verbatim_user_messages>\nCopied verbatim from the user. If anything below contradicts these, these win.\n(none)"),
        "the real summary must contain captured user wording, not an empty verbatim block"
    );
    assert!(
        summary_result.summary_text.contains("<recent_files>"),
        "the real summary must carry the read/edit/write file index on the preheat path"
    );
    agent
        .context_state
        .as_mut()
        .unwrap()
        .preheat
        .restore_completed(summary_result);

    // Phase 2: the ready preheat is applied at its recorded round boundary.
    let mut tail_assistant = assistant_read_call("tail-read", "src/main.rs");
    tail_assistant.msg_id = Some("tail-assistant".to_string());
    let mut tail_tool = ChatMessage::tool("tail-read", &"tail source line\n".repeat(100));
    tail_tool.msg_id = Some("tail-tool".to_string());
    messages.extend([tail_assistant, tail_tool]);
    agent.context_state.as_mut().unwrap().estimate_context_chars = 24_000;
    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .expect("apply ready preheat");
    assert_eq!(messages[1].kind, MessageKind::CompactionSummary);
    assert_eq!(
        messages[2].role,
        crate::core::llm::ChatMessageRole::Assistant,
        "the first raw entry after a preheat summary must begin the next complete tool round"
    );
    assert!(
        !messages[1]
            .text_content()
            .unwrap_or_default()
            .contains("## Constraints & Preferences"),
        "the real summary must not recreate the removed model-authored constraints section"
    );
    assert!(
        !crate::core::session::has_dangling_tool_calls_in_messages(&messages),
        "a preheat boundary must preserve paired assistant/tool messages"
    );

    // Phase 3: hydrate a raw, over-budget transcript and exercise the guard as the first request
    // would. The real provider produces the collapse summary.
    let sessions = tempfile::tempdir().expect("create resume sessions directory");
    let manager = SessionManager::new(sessions.path().to_path_buf());
    let key = manager.current_session_key();
    manager
        .create_session(key, None)
        .expect("create resume session");
    let raw_user = ChatMessage::user("resume a raw oversized tool transcript");
    let raw_assistant = ChatMessage::assistant_with_tool_calls(
        Some("a non-replayable write completed"),
        vec![serde_json::json!({
            "id": "resume-write",
            "type": "function",
            "function": {
                "name": "write",
                "arguments": r#"{"path":"src/main.rs","content":"updated"}"#,
            },
        })],
    );
    let raw_tool = ChatMessage::tool("resume-write", &"raw source line\n".repeat(1_500));
    manager
        .append_message(serde_json::to_value(raw_user).expect("serialize resume user"))
        .expect("append resume user");
    manager
        .append_message(serde_json::to_value(raw_assistant).expect("serialize resume assistant"))
        .expect("append resume assistant");
    manager
        .append_message(serde_json::to_value(raw_tool).expect("serialize resume tool"))
        .expect("append resume tool");

    let mut resumed_state =
        init_context_state(&manager, &context_config, "system").expect("hydrate raw transcript");
    resumed_state.context_budget_chars = 4_000;
    resumed_state.context_budget_tokens = 1_000;
    let mut resumed_messages = vec![ChatMessage::system("system")];
    resumed_messages.extend(resumed_state.messages.clone());
    let mut resumed_agent = make_agent(binding, context_config, temp.path());
    resumed_agent.start_idx = 1;
    resumed_agent.context_tail_start = 1;
    resumed_agent.set_context_state(Some(resumed_state));
    current_tail_guard::maybe_reduce_before_next_llm(&mut resumed_agent, &mut resumed_messages)
        .await
        .expect("proactively reduce hydrated first request");

    assert!(
        !resumed_agent
            .context_state
            .as_ref()
            .unwrap()
            .is_over_budget(),
        "the first-request guard must reduce raw resumed context before a provider request"
    );
    assert_eq!(resumed_messages[1].kind, MessageKind::CompactionSummary);
}
