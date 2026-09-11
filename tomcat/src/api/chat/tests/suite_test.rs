use super::super::*;
use crate::api::chat::run_loop::cleanup_plugin_sessions_on_session_end;
use crate::api::chat::run_loop::{
    build_tool_definitions, compose_planned_turn_messages, rebuild_turn_messages,
};
use crate::core::session::manager::init_context_state;
use crate::SessionEntry;
use crate::{
    AgentRunOutcome, AppConfig, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind,
    CheckpointMeta, CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, ListOptions,
    RestoreOptions, RetentionPolicy, SessionManager,
};
use serde_json::json;
use serial_test::serial;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn spawn_single_response_server(
    status: u16,
    body: &'static str,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let reason = match status {
                200 => "OK",
                404 => "Not Found",
                _ => "Unknown",
            };
            let resp = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                reason,
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://{}", addr), hits, handle)
}

struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by serial + home_env_lock.
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => {
                // SAFETY: restore original env during test teardown.
                unsafe { std::env::set_var(self.key, prev) };
            }
            None => {
                // SAFETY: clear test-only env during teardown.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

struct CurrentDirGuard {
    _lock: crate::test_support::TestLockGuard<'static>,
    previous: PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn write_session_plugin_fixture(workspace: &Path, plugin_id: &str, activation: &str) {
    let plugin_dir = workspace.join(".tomcat").join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_dir).expect("create plugin fixture dir");
    let manifest = json!({
        "id": plugin_id,
        "name": plugin_id,
        "version": "0.1.0",
        "description": format!("fixture {plugin_id}"),
        "author": "tests",
        "main": "main.js",
        "requiredPermissions": [],
        "requiredApiVersion": "1.0",
        "tags": [],
        "tools": [],
        "events": ["session_start"],
        "activation": activation
    });
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("serialize plugin manifest"),
    )
    .expect("write plugin manifest");
    fs::write(
        plugin_dir.join("main.js"),
        r#"
pi.on("session_start", function () {});
__pi_start_event_loop();
"#,
    )
    .expect("write plugin main");
}

// The request catalog is mode-stable; handlers, not catalog visibility,
// enforce plan-mode policy.
fn build_tool_definitions_default_view() -> Vec<serde_json::Value> {
    crate::core::plan_runtime::catalog::all_tools_with_policy(false)
}

#[test]
fn compose_planned_turn_messages_keeps_real_user_prompt_last() {
    let follow_up = crate::core::llm::ChatMessage::user(
        "<background-task-finished task_id=\"t-1\" exit_code=\"0\">done</background-task-finished>",
    );
    let planned = compose_planned_turn_messages("real user prompt", vec![follow_up.clone()]);
    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].text_content(), follow_up.text_content());
    assert_eq!(planned[1].text_content(), Some("real user prompt"));
}

#[test]
fn compose_planned_turn_messages_preserves_auto_turn_follow_up_order() {
    let first = crate::core::llm::ChatMessage::user("follow-up-a");
    let second = crate::core::llm::ChatMessage::user("follow-up-b");
    let planned = compose_planned_turn_messages("", vec![first.clone(), second.clone()]);
    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].text_content(), first.text_content());
    assert_eq!(planned[1].text_content(), second.text_content());
}

#[test]
fn rebuild_turn_messages_uses_context_state_and_keeps_current_input_at_tail() {
    let summary =
        crate::ChatMessage::compaction_summary("summary replaces old history", "summary-id");
    let mut placeholder = crate::ChatMessage::tool(
        "old-tool-call",
        "[Previous tool result replaced to save context space]",
    );
    placeholder.msg_id = Some("placeholder-id".to_string());
    let state = crate::core::session::manager::ContextState {
        messages: vec![summary, placeholder],
        estimate_context_chars: 100,
        context_budget_chars: 4_000,
        context_budget_tokens: 1_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    let mut current_input = crate::ChatMessage::user("current user input");
    current_input.msg_id = Some("current-input-id".to_string());

    let rebuilt = rebuild_turn_messages("system prompt", &state, &[(current_input.clone(), false)]);

    assert_eq!(rebuilt.len(), 4);
    assert_eq!(rebuilt[0].role, crate::core::llm::ChatMessageRole::System);
    assert_eq!(
        rebuilt[1].text_content(),
        Some("summary replaces old history"),
        "the summary must immediately follow the system prompt"
    );
    assert_eq!(
        rebuilt[2].text_content(),
        Some("[Previous tool result replaced to save context space]"),
        "the rebuild must use the L0-rewritten context, not stale source text"
    );
    assert_eq!(
        rebuilt.last().and_then(crate::ChatMessage::text_content),
        Some("current user input"),
        "the already-persisted current user input must remain at the tail"
    );
    assert_eq!(
        rebuilt.last().and_then(|message| message.msg_id.as_deref()),
        Some("current-input-id"),
        "rebuild must reuse the persisted message rather than append a duplicate"
    );
    assert!(
        rebuilt
            .iter()
            .all(|message| message.text_content() != Some("old full history")),
        "covered history must not reappear after rebuild"
    );
}

#[test]
fn build_tool_definitions_is_non_empty() {
    let defs = build_tool_definitions_default_view();
    assert!(defs.len() >= 4);
    for d in &defs {
        assert!(d["function"]["name"].is_string());
    }
}

#[test]
fn build_tool_definitions_contains_all_primitives() {
    let defs = build_tool_definitions_default_view();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(names.contains(&"read".to_string()));
    assert!(!names.contains(&"read_file".to_string()));
    assert!(names.contains(&"write".to_string()));
    assert!(!names.contains(&"write_file".to_string()));
    assert!(names.contains(&"edit".to_string()));
    assert!(!names.contains(&"edit_file".to_string()));
    assert!(names.contains(&"bash".to_string()));
    assert!(!names.contains(&"execute_bash".to_string()));
    assert!(names.contains(&"list_dir".to_string()));
}

#[test]
fn build_tool_definitions_contains_config_tools() {
    let defs = build_tool_definitions_default_view();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(
        names.contains(&"config_get".to_string()),
        "config_get tool must be registered (PR-7)"
    );
    assert!(
        names.contains(&"config_set".to_string()),
        "config_set tool must be registered (PR-7)"
    );
}

#[test]
fn build_tool_definitions_default_view_includes_plan_tools_for_handler_policy() {
    let defs = build_tool_definitions_default_view();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
        assert!(
            names.contains(&plan_tool.to_string()),
            "stable tool surface must include {plan_tool}, got: {names:?}"
        );
    }
}

#[tokio::test]
async fn tool_catalog_is_identical_across_all_mode_and_executing_combinations() {
    const ENV_KEY: &str = "TOMCAT_STABLE_TOOL_SURFACE_TEST_KEY";
    let (_dir, ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let chat = build_tool_definitions(&ctx).await;

    ctx.session_runtime
        .plan_runtime
        .enter_plan()
        .expect("enter Plan mode");
    let planning = build_tool_definitions(&ctx).await;

    ctx.session_runtime
        .plan_runtime
        .exit_plan()
        .expect("return to Chat mode");
    ctx.session_runtime.plan_runtime.seed_active_plan_for_test(
        "executing-catalog-test".to_string(),
        crate::core::plan_runtime::file_store::PlanFileState::Executing,
    );
    let executing = build_tool_definitions(&ctx).await;

    assert_eq!(
        serde_json::to_string(&chat).expect("serialize tools"),
        serde_json::to_string(&planning).expect("serialize tools"),
        "the actual LLM tool array must not change on Chat → Plan"
    );
    assert_eq!(
        serde_json::to_string(&chat).expect("serialize tools"),
        serde_json::to_string(&executing).expect("serialize tools"),
        "the actual LLM tool array must not change when a plan becomes executing"
    );
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
#[serial(env_lock)]
async fn mcp_lifecycle_never_changes_prompt_snapshot() {
    const ENV_KEY: &str = "TOMCAT_MCP_PREFIX_STABILITY_TEST_KEY";
    let _api_key = EnvGuard::set(ENV_KEY, "stub");
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace directory");

    let mut cfg = AppConfig::default();
    cfg.connector.enabled = true;
    cfg.storage.work_dir = Some(dir.path().join("work").to_string_lossy().into_owned());
    crate::test_support::write_models_override(
        dir.path().join("work").as_path(),
        &[crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)],
    );
    let mcp_config_path = dir.path().join("work").join("mcp.json");
    fs::create_dir_all(mcp_config_path.parent().expect("MCP config parent"))
        .expect("MCP config directory");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/fake_stdio_server.mjs");
    fs::write(
        &mcp_config_path,
        json!({
            "mcpServers": {
                "fake": {
                    "command": "node",
                    "args": [fixture],
                }
            }
        })
        .to_string(),
    )
    .expect("write MCP config");

    let ctx = ChatContext::from_config_with_overrides(
        cfg,
        crate::api::chat::ChatContextOverrides::default().with_session_cwd_override(workspace),
    )
    .expect("chat context");
    let budget = crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let connecting = crate::api::chat::build_prompt_snapshot(&ctx, budget).await;
    let connectors = ctx
        .global_services
        .connector_registry
        .as_ref()
        .expect("connector registry")
        .clone();

    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < ready_deadline {
        if !connectors.mcp_manager().list_servers().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !connectors.mcp_manager().list_servers().is_empty(),
        "fake MCP must become ready to exercise the lifecycle transition; statuses={:?}",
        connectors
            .mcp_manager()
            .statuses()
            .into_iter()
            .map(|status| format!("{}:{}", status.name, status.state.code()))
            .collect::<Vec<_>>()
    );
    let ready = crate::api::chat::build_prompt_snapshot(&ctx, budget).await;

    connectors.deny("fake").expect("disconnect fake MCP");
    assert!(
        connectors.mcp_manager().list_servers().is_empty(),
        "denied source must disappear from the live deferred catalog"
    );
    let disconnected = crate::api::chat::build_prompt_snapshot(&ctx, budget).await;

    for snapshot in [&ready, &disconnected] {
        assert_eq!(
            snapshot.system_text(),
            connecting.system_text(),
            "MCP lifecycle state may not change the system prompt prefix"
        );
        assert_eq!(
            snapshot.tool_definitions(),
            connecting.tool_definitions(),
            "MCP lifecycle state may not change the function-definition prefix"
        );
        assert_eq!(
            snapshot.signature(),
            connecting.signature(),
            "MCP lifecycle state may not change the cache signature"
        );
    }
    assert!(
        connecting
            .tool_definitions()
            .iter()
            .any(|definition| definition["function"]["name"] == "tool_search"),
        "the constant discovery entry remains available before connection"
    );
    assert!(
        connecting.tool_definitions().iter().all(|definition| {
            definition["function"]["name"]
                .as_str()
                .is_none_or(|name| name != "mcp__fake__capture")
        }),
        "the concrete MCP catalog must never enter the prompt-facing tool array"
    );
}

#[tokio::test]
async fn request_prefix_is_byte_identical_across_turns() {
    const ENV_KEY: &str = "TOMCAT_RUNTIME_TAIL_PREFIX_TEST_KEY";
    let (dir, ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let budget = crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let snapshot = crate::api::chat::build_prompt_snapshot(&ctx, budget).await;
    let tail_provider = crate::api::chat::run_loop::runtime_tail_provider(&ctx);
    let stable_history = vec![
        crate::ChatMessage::system(snapshot.system_text()),
        crate::ChatMessage::user("first user request"),
    ];
    let capture = |history: &[crate::ChatMessage]| {
        let mut messages = history.to_vec();
        let mut tail = crate::ChatMessage::user(tail_provider.render_ephemeral_tail());
        tail.kind = crate::core::llm::MessageKind::EphemeralTail;
        messages.push(tail);
        crate::ChatRequest {
            messages,
            model: "gpt-5.4".to_string(),
            temperature: None,
            max_tokens: None,
            resolved_output_limit: None,
            diagnostic_request_id: None,
            stream: Some(true),
            model_override: None,
            thinking_level: None,
            cache_key: Some("prefix-test:main".to_string()),
            tools: Some(snapshot.tool_definitions().to_vec()),
        }
    };

    let initial = capture(&stable_history);
    ctx.session_runtime.session_grants.add(
        dir.path().join("granted-after-turn-start"),
        crate::core::permission::GrantTrigger::UserConfirm,
    );
    let after_grant = capture(&stable_history);
    ctx.session_runtime
        .plan_runtime
        .enter_plan()
        .expect("enter Plan mode");
    let in_plan = capture(&stable_history);
    ctx.session_runtime
        .plan_runtime
        .exit_plan()
        .expect("return to Chat mode");
    ctx.session_runtime.plan_runtime.seed_active_plan_for_test(
        "executing-prefix-test".to_string(),
        crate::core::plan_runtime::file_store::PlanFileState::Executing,
    );
    let executing = capture(&stable_history);
    let mut appended_history = stable_history.clone();
    appended_history.push(crate::ChatMessage::assistant("first response"));
    appended_history.push(crate::ChatMessage::user("second user request"));
    let after_history_append = capture(&appended_history);

    for request in [&after_grant, &in_plan, &executing, &after_history_append] {
        assert_eq!(
            serde_json::to_string(&initial.tools).expect("serialize tools"),
            serde_json::to_string(&request.tools).expect("serialize tools"),
            "runtime state must not alter the tools cache prefix"
        );
        assert_eq!(
            initial.messages[0].text_content(),
            request.messages[0].text_content(),
            "runtime state must not alter the system cache prefix"
        );
        assert_eq!(
            request.messages.last().map(|message| message.kind),
            Some(crate::core::llm::MessageKind::EphemeralTail)
        );
    }
    for request in [&after_grant, &in_plan, &executing] {
        assert_eq!(
            serde_json::to_string(&request.messages[..stable_history.len()])
                .expect("serialize stable history"),
            serde_json::to_string(&initial.messages[..stable_history.len()])
                .expect("serialize stable history"),
            "permissions and plan lifecycle may only change the ephemeral tail"
        );
    }
    assert_ne!(
        initial
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content),
        after_grant
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content)
    );
    assert_ne!(
        after_grant
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content),
        in_plan
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content)
    );
    assert_ne!(
        in_plan
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content),
        executing
            .messages
            .last()
            .and_then(crate::ChatMessage::text_content)
    );
    assert_eq!(
        serde_json::to_string(&after_history_append.messages[..stable_history.len()])
            .expect("serialize persisted prefix"),
        serde_json::to_string(&initial.messages[..stable_history.len()])
            .expect("serialize persisted prefix"),
        "appending a turn must retain the original persisted prefix byte-for-byte"
    );
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn chat_message_assistant_with_tool_calls_has_tool_calls() {
    use crate::ChatMessage;
    let tc_json = vec![serde_json::json!({
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "read",
            "arguments": r#"{"path":"/tmp/x"}"#
        }
    })];
    let msg = ChatMessage::assistant_with_tool_calls(Some("thinking..."), tc_json);
    assert!(msg.tool_calls.is_some());
    let tc_val = msg.tool_calls.as_ref().unwrap();
    assert_eq!(tc_val.len(), 1);
    assert_eq!(tc_val[0]["function"]["name"], "read");
}

#[test]
fn chat_message_assistant_tool_calls_null_content_when_empty() {
    use crate::ChatMessage;
    let tc_json = vec![serde_json::json!({
        "id": "call_2",
        "type": "function",
        "function": {
            "name": "list_dir",
            "arguments": r#"{"path":"."}"#
        }
    })];
    let msg = ChatMessage::assistant_with_tool_calls(None, tc_json);
    assert!(msg.content.is_none());
    assert!(msg.tool_calls.is_some());
}

#[test]
fn effective_model_uses_session_override() {
    let entry = SessionEntry {
        session_key: crate::DEFAULT_SESSION_KEY.to_string(),
        session_id: "s1".into(),
        updated_at: 0,
        session_file: None,
        cwd: None,
        thinking_level: None,
        model_override: Some("gpt-5.2".to_string()),
        input_tokens: None,
        output_tokens: None,
        compaction_count: None,
        compaction_tokens_freed: None,
        tool_result_chars_persisted: None,
        context_utilization_ratio: None,
        last_checkpoint_id: None,
        title: None,
    };
    let config = AppConfig::default();
    let model = entry
        .model_override
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.llm.default_model);
    assert_eq!(model, "gpt-5.2");
}

#[test]
fn effective_model_uses_global_when_no_override() {
    let entry = SessionEntry {
        session_key: crate::DEFAULT_SESSION_KEY.to_string(),
        session_id: "s2".into(),
        updated_at: 0,
        session_file: None,
        cwd: None,
        thinking_level: None,
        model_override: None,
        input_tokens: None,
        output_tokens: None,
        compaction_count: None,
        compaction_tokens_freed: None,
        tool_result_chars_persisted: None,
        context_utilization_ratio: None,
        last_checkpoint_id: None,
        title: None,
    };
    let config = AppConfig::default();
    let model = entry
        .model_override
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.llm.default_model);
    assert_eq!(model, config.llm.default_model);
}

#[test]
fn ensure_session_creates_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    assert!(mgr.get_session(key).unwrap().is_none());

    if mgr.get_session(key).unwrap().is_none() {
        mgr.create_session(key, None).unwrap();
    }
    assert!(mgr.get_session(key).unwrap().is_some());
}

/// T-017 硬验收：`AgentRunOutcome::Interrupted` 的持久化路径必须与 `Completed`
/// 一致——partial assistant + 已完成 tool_result 均落到 transcript JSONL。
///
/// 本测试不启动完整 `chat_loop`（依赖 rustyline / runtime），而是锁定
/// `chat_loop` 中"Completed/Interrupted 共用 `append_message` 循环"这一契约：
/// 给定 `AgentRunResult.new_messages`，SessionManager.append_message 能按
/// 顺序把每条消息 append 到 JSONL，读回后内容 / 角色完全对得上。
#[test]
fn interrupt_persists_transcript_hard_ack() {
    use crate::core::agent_loop::AgentRunResult;
    use crate::core::llm::ChatMessage;
    use std::io::{BufRead, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    // 模拟中断时 AgentLoop::run 返回的 Interrupted 载荷：
    // - 1 条 partial assistant（承载 content_buf 截至中断的 delta）
    // - 1 条已完成的 tool_result（对应中断前已收到的 tool call）
    let tc_json = vec![serde_json::json!({
        "id": "call_1",
        "type": "function",
        "function": { "name": "read", "arguments": r#"{"path":"/x"}"# }
    })];
    let partial = AgentRunResult {
        final_text: "thinking about foo...".to_string(),
        new_messages: vec![
            ChatMessage::assistant_with_tool_calls(Some("thinking about foo..."), tc_json),
            ChatMessage::tool("call_1", "result_of_read"),
        ],
    };

    // 模拟 chat_loop 中 Completed/Interrupted 共用的持久化循环：
    for msg in &partial.new_messages {
        let json = serde_json::to_value(msg).expect("msg serialize");
        mgr.append_message(json).expect("append_message");
    }

    let path = mgr
        .current_transcript_path()
        .unwrap()
        .expect("transcript should exist");
    let file = std::fs::File::open(&path).expect("open transcript");
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();

    assert!(
        lines.len() >= 2,
        "transcript 应至少含 2 行（assistant + tool），实际 {} 行",
        lines.len()
    );

    let last_two: Vec<serde_json::Value> = lines
        .iter()
        .rev()
        .take(2)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .collect();
    // TranscriptEntry 顶层 wrap 了 Message 类型，实际 ChatMessage 在 .message 下
    let tool_msg = last_two[0].get("message").unwrap();
    let assistant_msg = last_two[1].get("message").unwrap();

    assert_eq!(
        assistant_msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "assistant",
        "倒数第二行应为 partial assistant"
    );
    assert!(
        assistant_msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("thinking about foo"),
        "partial assistant 应含 content_buf 累积文本"
    );
    assert_eq!(
        tool_msg.get("role").and_then(|v| v.as_str()).unwrap_or(""),
        "tool",
        "最后一行应为已完成 tool_result（中断前 tool 已跑完）"
    );
    assert_eq!(
        tool_msg
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "call_1",
        "tool_call_id 应与 assistant 发起的调用匹配"
    );
}

#[test]
fn build_turn_checkpoint_request_uses_first_and_last_row_ids() {
    let request = build_turn_checkpoint_request(
        "sess-1",
        CheckpointKind::TurnEnd,
        &[
            "msg_user".to_string(),
            "msg_assistant".to_string(),
            "msg_tool".to_string(),
        ],
    )
    .expect("row ids should produce checkpoint request");

    assert_eq!(request.session_id, "sess-1");
    assert_eq!(request.turn_id, "msg_user::msg_tool");
    assert_eq!(request.message_anchor.as_deref(), Some("msg_tool"));
    assert!(matches!(request.kind, CheckpointKind::TurnEnd));
}

#[test]
fn build_turn_checkpoint_request_skips_empty_turns() {
    assert!(
        build_turn_checkpoint_request("sess-1", CheckpointKind::Interrupt, &[]).is_none(),
        "空 turn 不应尝试 record checkpoint"
    );
}

#[test]
fn checkpoint_warn_line_is_single_line() {
    let line = checkpoint_warn_line(&CheckpointError::CommandFailed(
        "git add failed:\nline one\nline two".to_string(),
    ));
    assert!(!line.contains('\n'));
    assert!(line.contains("checkpoint record failed"));
}

#[test]
fn checkpoint_warn_line_mentions_backoff_for_timeout() {
    let line = checkpoint_warn_line(&CheckpointError::CommandTimedOut(
        "git status timed out after 30s (work_tree=/tmp/demo, captured output omitted 12 bytes)"
            .to_string(),
    ));
    assert!(!line.contains('\n'));
    assert!(line.contains("temporarily reducing checkpoint frequency"));
    assert!(line.contains("git status timed out after 30s"));
}

struct FailingRecordStore {
    timeout: bool,
    message: String,
}

impl CheckpointStore for FailingRecordStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        if self.timeout {
            Err(CheckpointError::CommandTimedOut(self.message.clone()))
        } else {
            Err(CheckpointError::CommandFailed(self.message.clone()))
        }
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

#[test]
fn record_failure_does_not_break_turn() {
    const ENV_KEY: &str = "TOMCAT_CHAT_CKPT_FAIL_OPEN_KEY";

    let (_dir, mut ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.scope_services.checkpoint_store = Arc::new(FailingRecordStore {
        timeout: true,
        message:
            "git status timed out after 30s (work_tree=/tmp/demo, captured output omitted 12 bytes)"
                .to_string(),
    });
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let messages = vec![crate::ChatMessage::assistant(
        "checkpoint failure should be nonfatal",
    )];

    let appended_ids =
        persist_turn_result(&ctx, &mut state, messages, crate::CheckpointKind::TurnEnd).unwrap();

    assert_eq!(appended_ids.len(), 1, "checkpoint 失败不应影响消息落盘");
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("checkpoint failure should be nonfatal"),
        "checkpoint 失败后仍应保留本轮 assistant 消息"
    );

    let detail_log = ctx
        .scope_services
        .agent_trail_dir
        .join("logs")
        .join("checkpoint-record-errors.log");
    let detail = std::fs::read_to_string(detail_log).unwrap();
    assert!(detail.contains("session_id="));
    assert!(detail.contains("TurnEnd"));
    assert!(detail.contains("CommandTimedOut"));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

struct SlowRecordStore {
    calls: Arc<AtomicUsize>,
    sleep: Duration,
}

impl CheckpointStore for SlowRecordStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        std::thread::sleep(self.sleep);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CheckpointId::null())
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn checkpoint_recording_does_not_block_turn_persistence_on_runtime() {
    const ENV_KEY: &str = "TOMCAT_CHAT_CKPT_BACKGROUND_RECORD_KEY";

    let (_dir, mut ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let calls = Arc::new(AtomicUsize::new(0));
    ctx.scope_services.checkpoint_store = Arc::new(SlowRecordStore {
        calls: Arc::clone(&calls),
        sleep: Duration::from_millis(500),
    });
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();

    let started = Instant::now();
    let appended_ids = persist_turn_result(
        &ctx,
        &mut state,
        vec![crate::ChatMessage::assistant(
            "durable before background checkpoint",
        )],
        CheckpointKind::TurnEnd,
    )
    .expect("checkpoint scheduling must not break turn persistence");

    assert_eq!(appended_ids.len(), 1);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "checkpoint record must not delay returning control to the turn"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "record runs after persist_turn_result returns"
    );

    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background checkpoint should eventually run");

    unsafe { std::env::remove_var(ENV_KEY) };
}

struct PruneSpyStore {
    calls: Arc<AtomicUsize>,
    sleep: Duration,
}

impl CheckpointStore for PruneSpyStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        Ok(CheckpointId::null())
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        std::thread::sleep(self.sleep);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }
}

#[test]
fn startup_prune_scheduled_without_blocking_readline() {
    const ENV_KEY: &str = "TOMCAT_CHAT_PRUNE_TEST_KEY";

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(
        dir.path(),
        &[crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)],
    );

    // SAFETY: 单测内部设置独立 env key，结束后立即清理。
    unsafe { std::env::set_var(ENV_KEY, "stub") };
    let mut ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let prune_calls = Arc::new(AtomicUsize::new(0));
    ctx.scope_services.checkpoint_store = Arc::new(PruneSpyStore {
        calls: prune_calls.clone(),
        sleep: Duration::from_millis(150),
    });

    let started = Instant::now();
    schedule_checkpoint_prune(&ctx);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "schedule_checkpoint_prune 应立即返回，不阻塞 readline 主线程"
    );

    let deadline = Instant::now() + Duration::from_secs(1);
    while prune_calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        prune_calls.load(Ordering::SeqCst),
        1,
        "后台线程应触发一次 prune"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn chat_context_attaches_cli_ask_question_panel() {
    const ENV_KEY: &str = "TOMCAT_CHAT_ASKQ_PANEL_KEY";

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(
        dir.path(),
        &[crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)],
    );

    // SAFETY: 测试使用独立 env key，作用域结束后立即清理。
    unsafe { std::env::set_var(ENV_KEY, "stub") };
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    assert!(
        ctx.session_runtime
            .plan_runtime
            .ask_question_panel()
            .is_some(),
        "CLI ChatContext 应默认挂载 AskQuestionPanel，避免真 LLM 调 ask_question 时直接报工具不可用"
    );
    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[derive(Default)]
struct RecordSpyState {
    requests: Vec<CheckpointRecordRequest>,
    observed_leaf_ids: Vec<Option<String>>,
}

struct RecordSpyStore {
    transcript_path: std::path::PathBuf,
    state: Arc<Mutex<RecordSpyState>>,
}

impl CheckpointStore for RecordSpyStore {
    fn record(&self, request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        let observed_leaf = crate::core::session::read_entries_tail(&self.transcript_path, 1)
            .ok()
            .and_then(|entries| entries.into_iter().next_back())
            .and_then(|entry| match entry {
                crate::core::TranscriptEntry::Message(me) => me.id,
                _ => None,
            });
        let mut guard = self.state.lock().unwrap();
        guard.requests.push(request);
        guard.observed_leaf_ids.push(observed_leaf);
        Ok(CheckpointId::new(format!(
            "ck-spy-{}",
            guard.requests.len()
        )))
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<crate::CheckpointDiff, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Err(CheckpointError::Unsupported("not used in test".to_string()))
    }

    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

fn checkpoint_recording_test_context(
    env_key: &str,
) -> (tempfile::TempDir, ChatContext, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(
        dir.path(),
        &[crate::test_support::TestModelOverride::gpt54_openai_responses(env_key)],
    );

    // SAFETY: 测试使用独立 env key，作用域结束后由调用方清理。
    unsafe { std::env::set_var(env_key, "stub") };
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_key = ctx
        .session_runtime
        .session
        .current_session_key()
        .to_string();
    ctx.session_runtime
        .session
        .create_session(&session_key, None)
        .unwrap();
    let transcript_path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    (dir, ctx, transcript_path)
}

fn chat_turn_test_context(
    entries: &[crate::test_support::TestModelOverride<'_>],
) -> (tempfile::TempDir, ChatContext, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(dir.path(), entries);

    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_key = ctx
        .session_runtime
        .session
        .current_session_key()
        .to_string();
    ctx.session_runtime
        .session
        .create_session(&session_key, None)
        .unwrap();
    let transcript_path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .unwrap()
        .expect("transcript path");
    (dir, ctx, transcript_path)
}

fn pdf_user_message() -> crate::ChatMessage {
    let pdf_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        b"%PDF-1.4\n%%EOF\n",
    );
    crate::ChatMessage::user_with_parts(vec![
        crate::ChatMessageContentPart::text("Read this PDF"),
        crate::ChatMessageContentPart::file_base64_data("brief.pdf", "application/pdf", pdf_b64)
            .expect("build pdf part"),
    ])
}

fn image_user_message() -> crate::ChatMessage {
    let image_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"fake-png");
    crate::ChatMessage::user_with_parts(vec![
        crate::ChatMessageContentPart::text("Inspect this image"),
        crate::ChatMessageContentPart::image_base64_data("image/png", image_b64)
            .expect("build image part"),
    ])
}

#[test]
fn append_message_chain_invariant_is_nonfatal() {
    let err = crate::AppError::invariant("append_message_chain", "tool tail broken");
    assert!(
        super::super::is_append_message_chain_invariant(&err),
        "append_message_chain invariant 应命中恢复分支识别"
    );
    assert!(
        !super::super::is_fatal_error(&err),
        "append_message_chain invariant 不应被视为 chat fatal error"
    );
}

#[test]
fn append_message_chain_rehydrate_reloads_context_from_transcript() {
    const ENV_KEY: &str = "TOMCAT_CHAT_APPEND_REHYDRATE_KEY";

    let (_dir, ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "outer tool call",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "bash", "arguments": r#"{"command":"echo hi"}"# }
            }]
        }))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "tool",
            "tool_call_id": "call_1",
            "content": "[interrupted]"
        }))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "user",
            "content": "nested prompt"
        }))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "inner done"
        }))
        .unwrap();

    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    state.messages = vec![crate::ChatMessage::assistant_with_tool_calls(
        Some("outer tool call"),
        vec![serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "bash", "arguments": r#"{"command":"echo hi"}"# }
        })],
    )];

    let changed = super::super::try_rehydrate_context_state_after_append_invariant(
        &ctx,
        &ctx.config.context,
        "sys",
        &crate::AppError::invariant(
            "append_message_chain",
            "tool must follow assistant+tool_calls or tool",
        ),
        &mut state,
    );

    assert!(
        changed,
        "append_message_chain invariant 应触发一次 context rehydrate"
    );
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("inner done"),
        "rehydrate 后应以磁盘 transcript 的最后一条 assistant 为准"
    );
    assert!(
        state
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")
                && m.text_content() == Some("[interrupted]")),
        "rehydrate 后应带回磁盘上已补齐的 interrupted tool result"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn append_message_chain_rehydrate_falls_back_when_transcript_reload_fails() {
    const ENV_KEY: &str = "TOMCAT_CHAT_APPEND_REHYDRATE_FALLBACK_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    state.messages = vec![crate::ChatMessage::assistant_with_tool_calls(
        Some("outer tool call"),
        vec![serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "bash", "arguments": r#"{"command":"echo hi"}"# }
        })],
    )];
    std::fs::remove_file(&transcript_path).unwrap();

    let changed = super::super::try_rehydrate_context_state_after_append_invariant(
        &ctx,
        &ctx.config.context,
        "sys",
        &crate::AppError::invariant(
            "append_message_chain",
            "tool must follow assistant+tool_calls or tool",
        ),
        &mut state,
    );

    assert!(
        changed,
        "append_message_chain invariant 仍应触发恢复 helper"
    );
    assert!(
        state.messages.is_empty(),
        "rehydrate 失败时应退回空消息 fallback，避免继续携带坏掉的 dangling tool_calls"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn append_message_chain_failed_turn_recovery_does_not_append_error_or_mark_turn_failed() {
    const ENV_KEY: &str = "TOMCAT_CHAT_APPEND_REHYDRATE_NO_TRANSCRIPT_MUTATION_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"user","content":"retry-tail"}))
        .unwrap();

    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let changed = crate::api::chat::run_loop::recover_context_state_after_failed_turn(
        &ctx,
        &ctx.config.context,
        "sys",
        &crate::AppError::invariant(
            "append_message_chain",
            "tool must follow assistant+tool_calls or tool",
        ),
        &mut state,
    );

    assert!(changed, "append_message_chain 失败也应走恢复 helper");
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("retry-tail"),
        "append invariant 分支只重载 transcript，不应把 user tail 标成失败"
    );
    let entries = crate::core::session::read_entries_tail(&transcript_path, 8).unwrap();
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(entry, crate::core::TranscriptEntry::Error(_))),
        "append invariant 恢复不应追加结构化 Error 记录"
    );
    let trailing_user = entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            crate::core::TranscriptEntry::Message(me)
                if me.message.get("role").and_then(serde_json::Value::as_str) == Some("user") =>
            {
                Some(me)
            }
            _ => None,
        })
        .expect("expected trailing user message");
    assert_eq!(
        trailing_user
            .message
            .get("superseded")
            .and_then(serde_json::Value::as_bool),
        None
    );
    assert_eq!(
        trailing_user
            .message
            .get("turn_failed")
            .and_then(serde_json::Value::as_bool),
        None
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn non_append_invariant_does_not_rehydrate_context() {
    const ENV_KEY: &str = "TOMCAT_CHAT_APPEND_REHYDRATE_NOOP_KEY";

    let (_dir, ctx, _transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    state.messages = vec![crate::ChatMessage::user("keep me")];

    let changed = super::super::try_rehydrate_context_state_after_append_invariant(
        &ctx,
        &ctx.config.context,
        "sys",
        &crate::AppError::Permission("deny".to_string()),
        &mut state,
    );

    assert!(
        !changed,
        "非 append_message_chain 错误不应进入 rehydrate 恢复路径"
    );
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("keep me"),
        "非目标错误应保持当前 context_state 不变"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn failed_turn_recovery_supersedes_trailing_user_tail_and_rehydrates() {
    const ENV_KEY: &str = "TOMCAT_CHAT_FAILED_TAIL_RECOVERY_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"q1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry-1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry-2"}))
        .unwrap();

    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let changed = crate::api::chat::run_loop::recover_context_state_after_failed_turn(
        &ctx,
        &ctx.config.context,
        "sys",
        &crate::AppError::Llm("gateway 403".to_string()),
        &mut state,
    );

    assert!(changed, "failed turn should trigger a context recovery");
    let texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|message| message.text_content().map(str::to_string))
        .collect();
    assert_eq!(texts, vec!["q1".to_string(), "a1".to_string()]);

    let entries = crate::core::session::read_entries_tail(&transcript_path, 8).unwrap();
    let superseded_flags: Vec<(Option<bool>, Option<bool>)> = entries
        .iter()
        .filter_map(|entry| match entry {
            crate::core::TranscriptEntry::Message(me) => Some((
                me.message
                    .get("superseded")
                    .and_then(serde_json::Value::as_bool),
                me.message
                    .get("turn_failed")
                    .and_then(serde_json::Value::as_bool),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        superseded_flags,
        vec![
            (None, None),
            (None, None),
            (Some(true), Some(true)),
            (Some(true), Some(true)),
        ]
    );
    let error_entry = entries
        .iter()
        .find_map(|entry| match entry {
            crate::core::TranscriptEntry::Error(error) => Some(error),
            _ => None,
        })
        .expect("failed turn recovery should append an error entry");
    assert_eq!(error_entry.status_code, None);
    assert_eq!(error_entry.summary, "gateway 403");
    assert!(error_entry.detail.contains("gateway 403"));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn failed_turn_recovery_is_idempotent_for_the_same_failed_tail() {
    const ENV_KEY: &str = "TOMCAT_CHAT_FAILED_TAIL_RECOVERY_IDEMPOTENT_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"q1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(json!({"role":"assistant","content":"a1"}))
        .unwrap();
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry-once"}))
        .unwrap();

    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let error = crate::AppError::Llm("gateway 403".to_string());
    assert!(
        crate::api::chat::run_loop::recover_context_state_after_failed_turn(
            &ctx,
            &ctx.config.context,
            "sys",
            &error,
            &mut state,
        )
    );
    assert!(
        crate::api::chat::run_loop::recover_context_state_after_failed_turn(
            &ctx,
            &ctx.config.context,
            "sys",
            &error,
            &mut state,
        )
    );

    let texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|message| message.text_content().map(str::to_string))
        .collect();
    assert_eq!(texts, vec!["q1".to_string(), "a1".to_string()]);

    let entries = crate::core::session::read_entries_tail(&transcript_path, 8).unwrap();
    let error_count = entries
        .iter()
        .filter(|entry| matches!(entry, crate::core::TranscriptEntry::Error(_)))
        .count();
    assert_eq!(
        error_count, 1,
        "同一 failed tail 重复恢复不应重复追加 Error"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn failed_turn_recovery_summarizes_403_html_and_redacts_sensitive_detail() {
    const ENV_KEY: &str = "TOMCAT_CHAT_FAILED_TURN_403_SUMMARY_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry this"}))
        .unwrap();
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let err = crate::infra::error::llm_http_status_error(
        "openai-responses",
        403,
        r#"<html><body><h1>403 Forbidden</h1><p>Host: PS-SHA-01JfN78</p><p>Request-Id: req_turn2</p><p>Authorization: Bearer sk-secret</p><p>api_key="raw-key"</p><p>token=raw-token</p><p>Cookie: SID=raw-cookie; Path=/</p><p>Set-Cookie: refresh=raw-set-cookie; HttpOnly</p><p>x-api-key: raw-x-key</p><p>password=raw-password</p><p>secret="raw-secret"</p><p>Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==</p><p>direct-key sk-direct-secret-1234567890</p></body></html>"#,
    );

    assert_eq!(
        crate::api::chat::render_error_message(&err),
        "API 错误 403 · PS-SHA-01JfN78 · Request-Id req_turn2"
    );

    let changed = crate::api::chat::run_loop::recover_context_state_after_failed_turn(
        &ctx,
        &ctx.config.context,
        "sys",
        &err,
        &mut state,
    );
    assert!(changed, "403 failed turn should trigger recovery");

    let error_entry = crate::core::session::read_entries_tail(&transcript_path, 8)
        .unwrap()
        .into_iter()
        .find_map(|entry| match entry {
            crate::core::TranscriptEntry::Error(error) => Some(error),
            _ => None,
        })
        .expect("expected structured error entry");
    assert_eq!(
        error_entry.summary,
        "API 错误 403 · PS-SHA-01JfN78 · Request-Id req_turn2"
    );
    assert_eq!(
        error_entry.failure_kind.as_deref(),
        Some("authentication"),
        "an isolated 403 must not be misclassified as billing"
    );
    assert_eq!(error_entry.failure_domain.as_deref(), Some("account"));
    assert!(error_entry.detail.contains("Request-Id: req_turn2"));
    assert!(error_entry.detail.contains("Host: PS-SHA-01JfN78"));
    assert!(error_entry.detail.contains("Authorization: [REDACTED]"));
    assert!(error_entry.detail.contains("api_key=\"[REDACTED]\""));
    assert!(error_entry.detail.contains("token=[REDACTED]"));
    assert!(error_entry.detail.contains("Cookie: [REDACTED]"));
    assert!(error_entry.detail.contains("Set-Cookie: [REDACTED]"));
    assert!(error_entry.detail.contains("x-api-key: [REDACTED]"));
    assert!(error_entry.detail.contains("password=[REDACTED]"));
    assert!(error_entry.detail.contains("secret=\"[REDACTED]\""));
    assert!(error_entry.detail.contains("Basic [REDACTED]"));
    assert!(!error_entry.detail.contains("sk-secret"));
    assert!(!error_entry.detail.contains("raw-key"));
    assert!(!error_entry.detail.contains("raw-token"));
    assert!(!error_entry.detail.contains("raw-cookie"));
    assert!(!error_entry.detail.contains("raw-set-cookie"));
    assert!(!error_entry.detail.contains("raw-x-key"));
    assert!(!error_entry.detail.contains("raw-password"));
    assert!(!error_entry.detail.contains("raw-secret"));
    assert!(!error_entry.detail.contains("QWxhZGRpbjpvcGVuIHNlc2FtZQ=="));
    assert!(!error_entry.detail.contains("sk-direct-secret-1234567890"));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn failed_turn_recovery_renders_actionable_normalized_failure_messages() {
    let billing = crate::infra::error::llm_http_status_error(
        "transit",
        403,
        r#"{"error":{"code":"insufficient_quota","message":"insufficient balance"}}"#,
    );
    assert_eq!(
        crate::api::chat::render_error_message(&billing),
        "账户余额或额度不足。充值或切换 Provider 后可重试。"
    );

    let overflow = crate::infra::error::llm_stream_terminal_error(
        "transit",
        "request exceeds the context window",
        Some("context_length_exceeded".to_string()),
    );
    let rendered = crate::api::chat::render_error_message(&overflow);
    assert!(rendered.contains("/compact"));
    assert!(rendered.contains("/restore"));
}

#[test]
fn failed_turn_recovery_truncates_large_error_detail_after_redaction() {
    const ENV_KEY: &str = "TOMCAT_CHAT_FAILED_TURN_DETAIL_TRUNCATION_KEY";

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry giant error"}))
        .unwrap();
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let huge_body = format!(
        "<html><body><p>Host: huge.gateway</p><p>Request-Id: req_big</p><p>Cookie: SID=raw-cookie</p><p>{}</p><p>TAIL-SHOULD-NOT-APPEAR</p></body></html>",
        "A".repeat(9000)
    );
    let err = crate::infra::error::llm_http_status_error("openai-responses", 403, &huge_body);

    let changed = crate::api::chat::run_loop::recover_context_state_after_failed_turn(
        &ctx,
        &ctx.config.context,
        "sys",
        &err,
        &mut state,
    );
    assert!(changed, "large failed turn should still trigger recovery");

    let error_entry = crate::core::session::read_entries_tail(&transcript_path, 8)
        .unwrap()
        .into_iter()
        .find_map(|entry| match entry {
            crate::core::TranscriptEntry::Error(error) => Some(error),
            _ => None,
        })
        .expect("expected structured error entry");
    assert!(error_entry.detail.contains("Cookie: [REDACTED]"));
    assert!(
        error_entry.detail.ends_with("\n...[truncated]"),
        "oversized detail should end with a truncation marker"
    );
    assert!(
        !error_entry.detail.contains("TAIL-SHOULD-NOT-APPEAR"),
        "content beyond the truncation limit must be dropped"
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

/// 用真实抓到的 sunmi 网关 403 页面（关内网复现，节点 PS-CZX-01wky52）回归：
/// `render_error_message` 应从正文里的 URL 解析出网关域名、从 `Request-Id:` 提取
/// 真实请求号，摘要保持“状态码 · 域名 · Request-Id”的干净格式；结构化 Error 记录的
/// `status_code`/`request_id` 正确，`detail` 完整保留原始正文（含节点/URL/请求号）供排查。
#[test]
fn failed_turn_recovery_summarizes_real_sunmi_gateway_403_html() {
    const ENV_KEY: &str = "TOMCAT_CHAT_FAILED_TURN_REAL_403_SUMMARY_KEY";

    // 真实网关返回的 403 页面（原文，未改造）。
    const REAL_403_HTML: &str = r#"<!DOCTYPE html>
<html>
	<head>
		<meta charset="utf-8">
		<meta http-equiv="X-UA-Compatible" content="IE=edge">
		<meta name="viewport" content="width=device-width, initial-scale=1">
		<title>403 Forbidden</title>
		<style type="text/css">body{margin:5% auto 0 auto;padding:0 18px}.P{margin:0 22%}.O{margin-top:20px}.N{margin-top:10px}.M{margin:10px 0 30px 0}.L{margin-bottom:60px}.K{font-size:25px;color:#F90}.J{font-size:14px}.I{font-size:20px}.H{font-size:18px}.G{font-size:16px}.F{width:230px;float:left}.E{margin-top:5px}.D{margin:8px 0 0 -20px}.C{color:#3CF;cursor:pointer}.B{color:#909090;margin-top:15px}.A{line-height:30px}.hide_me{display:none}</style>
	</head>
	<body>
		<div id="p" class="P">
			<div class="K">403</div>
			<div class="O I">Forbidden</div>
			<p class="J A L">Error Times: Fri, 17 Jul 2026 00:03:40 GMT
				<br>
				<span class="F">IP: 112.65.39.1</span>Node information: PS-CZX-01wky52
				<br>URL: https://aigateway.sunmi.com/v1/responses
				<br>Request-Id: 6a59715c_PS-CZX-01wky52_16724-27663
				<br>Check:
				<span class="C G" onclick="s(0)">Details</span></p>
		</div>
	</body>
</html>"#;

    let (_dir, ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    ctx.session_runtime
        .session
        .append_message(json!({"role":"user","content":"retry after intranet 403"}))
        .unwrap();
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let err = crate::infra::error::llm_http_status_error("openai-responses", 403, REAL_403_HTML);

    assert_eq!(
        crate::api::chat::render_error_message(&err),
        "API 错误 403 · aigateway.sunmi.com · Request-Id 6a59715c_PS-CZX-01wky52_16724-27663"
    );

    let changed = crate::api::chat::run_loop::recover_context_state_after_failed_turn(
        &ctx,
        &ctx.config.context,
        "sys",
        &err,
        &mut state,
    );
    assert!(
        changed,
        "真实 403 失败轮应触发上下文恢复并落一条 Error 记录"
    );

    let error_entry = crate::core::session::read_entries_tail(&transcript_path, 8)
        .unwrap()
        .into_iter()
        .find_map(|entry| match entry {
            crate::core::TranscriptEntry::Error(error) => Some(error),
            _ => None,
        })
        .expect("expected structured error entry");
    assert_eq!(error_entry.status_code, Some(403));
    assert_eq!(
        error_entry.request_id.as_deref(),
        Some("6a59715c_PS-CZX-01wky52_16724-27663")
    );
    assert_eq!(
        error_entry.summary,
        "API 错误 403 · aigateway.sunmi.com · Request-Id 6a59715c_PS-CZX-01wky52_16724-27663"
    );
    assert!(error_entry
        .detail
        .contains("Node information: PS-CZX-01wky52"));
    assert!(error_entry
        .detail
        .contains("URL: https://aigateway.sunmi.com/v1/responses"));
    assert!(error_entry
        .detail
        .contains("Request-Id: 6a59715c_PS-CZX-01wky52_16724-27663"));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn turn_end_writes_checkpoint() {
    const ENV_KEY: &str = "TOMCAT_CHAT_TURN_END_CKPT_KEY";

    let (_dir, mut ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let spy_state = Arc::new(Mutex::new(RecordSpyState::default()));
    ctx.scope_services.checkpoint_store = Arc::new(RecordSpyStore {
        transcript_path,
        state: Arc::clone(&spy_state),
    });
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let messages = vec![crate::ChatMessage::assistant("turn end reply")];

    let appended_ids =
        persist_turn_result(&ctx, &mut state, messages, crate::CheckpointKind::TurnEnd).unwrap();

    let guard = spy_state.lock().unwrap();
    assert_eq!(guard.requests.len(), 1, "TurnEnd 应写入一次 checkpoint");
    assert_eq!(guard.observed_leaf_ids.len(), 1);
    assert_eq!(
        guard.requests[0].message_anchor.as_deref(),
        appended_ids.last().map(String::as_str),
        "checkpoint anchor 应指向刚落盘的 assistant 行"
    );
    assert_eq!(
        guard.observed_leaf_ids[0].as_deref(),
        appended_ids.last().map(String::as_str),
        "record() 触发时 transcript 末尾应已是新写入消息"
    );
    assert!(matches!(
        guard.requests[0].kind,
        crate::CheckpointKind::TurnEnd
    ));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn interrupt_writes_checkpoint_after_partial_persist() {
    const ENV_KEY: &str = "TOMCAT_CHAT_INTERRUPT_CKPT_KEY";

    let (_dir, mut ctx, transcript_path) = checkpoint_recording_test_context(ENV_KEY);
    let spy_state = Arc::new(Mutex::new(RecordSpyState::default()));
    ctx.scope_services.checkpoint_store = Arc::new(RecordSpyStore {
        transcript_path,
        state: Arc::clone(&spy_state),
    });
    let mut state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();
    let tool_calls = vec![serde_json::json!({
        "id": "call_1",
        "type": "function",
        "function": { "name": "read", "arguments": r#"{"path":"note.txt"}"# }
    })];
    let messages = vec![
        crate::ChatMessage::assistant_with_tool_calls(Some("partial reply"), tool_calls),
        crate::ChatMessage::tool("call_1", "tool result"),
    ];

    let appended_ids =
        persist_turn_result(&ctx, &mut state, messages, crate::CheckpointKind::Interrupt).unwrap();

    let guard = spy_state.lock().unwrap();
    assert_eq!(guard.requests.len(), 1, "Interrupt 应写入一次 checkpoint");
    assert_eq!(
        guard.requests[0].message_anchor.as_deref(),
        appended_ids.last().map(String::as_str),
        "Interrupt checkpoint anchor 应指向最后一条 partial/tool transcript 行"
    );
    assert_eq!(
        guard.observed_leaf_ids[0].as_deref(),
        appended_ids.last().map(String::as_str),
        "record(Interrupt) 必须发生在 partial transcript 落盘之后"
    );
    assert!(matches!(
        guard.requests[0].kind,
        crate::CheckpointKind::Interrupt
    ));

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
fn user_prompt_for_mode_separates_session_mode_from_plan_lifecycle() {
    use crate::core::plan_runtime::{file_store::PlanFileState, ActivePlan};
    use crate::core::session::AgentMode;

    assert_eq!(
        super::super::prompt::user_prompt_for_mode(AgentMode::Chat, None),
        "u[Chat]> "
    );
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(AgentMode::Plan, None),
        "u[Plan]> "
    );
    let executing = ActivePlan {
        id: "p1".into(),
        path: std::path::PathBuf::from("/tmp/p1.plan.md"),
        state: PlanFileState::Executing,
    };
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(AgentMode::Chat, Some(&executing)),
        "u[Chat·plan:executing]> "
    );
}

#[test]
fn plan_reminders_follow_session_mode_and_active_plan_lifecycle() {
    use crate::api::chat::run_loop::render_plan_runtime_reminder;
    use crate::core::plan_runtime::{file_store::PlanFileState, PlanRuntime};

    let runtime = PlanRuntime::new("plan-reminder-test");
    assert!(
        render_plan_runtime_reminder(&runtime).is_none(),
        "plain Chat must not receive a plan reminder"
    );

    runtime.enter_plan().expect("enter Plan mode");
    let planner = render_plan_runtime_reminder(&runtime).expect("Plan mode reminder");
    assert!(planner.contains("<system_reminder"));
    assert!(
        planner.to_lowercase().contains("plan"),
        "Plan mode must receive planner guidance"
    );

    runtime.exit_plan().expect("return to Chat mode");
    runtime.seed_active_plan_for_test("plan-1".into(), PlanFileState::Executing);
    let executor = render_plan_runtime_reminder(&runtime).expect("executor reminder");
    assert!(executor.contains("plan-1"));
    assert!(
        executor.contains("<system_reminder"),
        "an executing plan must receive executor guidance"
    );

    runtime.seed_active_plan_for_test("plan-1".into(), PlanFileState::Completed);
    assert!(
        render_plan_runtime_reminder(&runtime).is_none(),
        "completion removes executor guidance without a separate drop action"
    );

    runtime.seed_active_plan_for_test("plan-1".into(), PlanFileState::Pending);
    assert!(
        render_plan_runtime_reminder(&runtime).is_none(),
        "parking to pending removes executor guidance"
    );

    runtime.seed_active_plan_for_test("plan-1".into(), PlanFileState::Executing);
    assert!(
        render_plan_runtime_reminder(&runtime)
            .as_deref()
            .is_some_and(|reminder| reminder.contains("plan-1")),
        "resuming execution restores executor guidance"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn validate_capabilities_rejects_pdf_before_appending_user_message() {
    const ENV_KEY: &str = "TOMCAT_VALIDATE_PDF_BEFORE_APPEND_KEY";
    let _api_guard = EnvGuard::set(ENV_KEY, "stub");
    let (_dir, ctx, transcript_path) =
        chat_turn_test_context(&[crate::test_support::TestModelOverride {
            files: false,
            ..crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)
        }]);
    let before = fs::read(&transcript_path).expect("read transcript before");
    let mut context_state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();

    let outcome = run_chat_turn_with_message(
        &ctx,
        Some(pdf_user_message()),
        "sys",
        &mut context_state,
        CancellationToken::new(),
    )
    .await
    .expect("turn outcome");

    assert!(matches!(outcome, AgentRunOutcome::Failed(_)));
    let after = fs::read(&transcript_path).expect("read transcript after");
    assert_eq!(
        before, after,
        "capability reject 不应把这条 user turn 写进 transcript"
    );
    let rendered = String::from_utf8_lossy(&after);
    assert!(!rendered.contains("brief.pdf"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn validate_capabilities_accepts_pdf_when_files_enabled() {
    const ENV_KEY: &str = "TOMCAT_VALIDATE_PDF_ENABLED_KEY";
    let _api_guard = EnvGuard::set(ENV_KEY, "stub");
    let _no_proxy = EnvGuard::set("NO_PROXY", "127.0.0.1,localhost");
    let _no_proxy_lower = EnvGuard::set("no_proxy", "127.0.0.1,localhost");
    let (base_url, hits, handle) = spawn_single_response_server(404, r#"{"error":"not found"}"#);
    let (_dir, ctx, transcript_path) = chat_turn_test_context(&[
        crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)
            .with_base_url(&base_url),
    ]);
    let mut context_state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();

    let outcome = run_chat_turn_with_message(
        &ctx,
        Some(pdf_user_message()),
        "sys",
        &mut context_state,
        CancellationToken::new(),
    )
    .await
    .expect("turn outcome");

    assert!(matches!(outcome, AgentRunOutcome::Failed(_)));
    let rendered = fs::read_to_string(&transcript_path).expect("read transcript after");
    assert!(
        rendered.contains("brief.pdf"),
        "正常路径应保留 user transcript"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "通过校验后应实际发起一次请求"
    );
    handle.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn validate_capabilities_rejects_image_before_appending_user_message() {
    const ENV_KEY: &str = "TOMCAT_VALIDATE_IMAGE_BEFORE_APPEND_KEY";
    let _api_guard = EnvGuard::set(ENV_KEY, "stub");
    let (_dir, ctx, transcript_path) =
        chat_turn_test_context(&[crate::test_support::TestModelOverride {
            vision: false,
            ..crate::test_support::TestModelOverride::gpt54_openai_responses(ENV_KEY)
        }]);
    let before = fs::read(&transcript_path).expect("read transcript before");
    let mut context_state =
        init_context_state(&ctx.session_runtime.session, &ctx.config.context, "sys").unwrap();

    let outcome = run_chat_turn_with_message(
        &ctx,
        Some(image_user_message()),
        "sys",
        &mut context_state,
        CancellationToken::new(),
    )
    .await
    .expect("turn outcome");

    assert!(matches!(outcome, AgentRunOutcome::Failed(_)));
    let after = fs::read(&transcript_path).expect("read transcript after");
    assert_eq!(
        before, after,
        "vision reject 不应把这条 user turn 写进 transcript"
    );
    let rendered = String::from_utf8_lossy(&after);
    assert!(!rendered.contains("image/png"));
}

#[test]
fn agent_prompt_for_mode_uses_agent_prefix_and_hides_plan_id() {
    use crate::core::plan_runtime::{file_store::PlanFileState, ActivePlan};
    use crate::core::session::AgentMode;

    assert_eq!(
        super::super::prompt::agent_prompt_for_mode("main", AgentMode::Chat, None),
        "agent.main> "
    );
    let executing = ActivePlan {
        id: "ship-001".into(),
        path: std::path::PathBuf::from("/tmp/ship-001.plan.md"),
        state: PlanFileState::Executing,
    };
    assert_eq!(
        super::super::prompt::agent_prompt_for_mode("main", AgentMode::Chat, Some(&executing)),
        "agent.main[Chat·plan:executing]> "
    );
}

#[tokio::test]
async fn chat_cleanup_on_session_end_handles_delete_404_idempotently() {
    let (base_url, hits, handle) = spawn_single_response_server(404, r#"{"error":"not found"}"#);
    let old_no_proxy = std::env::var("NO_PROXY").ok();
    let old_no_proxy_lower = std::env::var("no_proxy").ok();
    // SAFETY: 测试作用域内确保本地 mock 地址不走代理，避免 127.0.0.1 请求被外部代理改写。
    unsafe {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    }
    let mut cfg = AppConfig::default();
    let dir = tempfile::tempdir().unwrap();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(
        dir.path(),
        &[
            crate::test_support::TestModelOverride::gpt54_openai_responses(
                "TOMCAT_CHAT_CLEANUP_TEST_KEY",
            )
            .with_base_url(&base_url),
        ],
    );
    // SAFETY: 测试内部临时设置 env，结束后立即清理。
    unsafe { std::env::set_var("TOMCAT_CHAT_CLEANUP_TEST_KEY", "stub") };

    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let entry = ctx
        .session_runtime
        .session
        .get_session(ctx.session_runtime.session.current_session_key())
        .expect("session lookup should succeed")
        .expect("current session should exist");
    let resolved = ctx
        .resolve_call(crate::core::llm::LlmScene::Main, Some(&entry))
        .expect("main model should resolve");
    let runtime = ctx
        .openai_files_runtime_for(&resolved)
        .expect("openai-responses should expose files runtime");
    runtime.enqueue_delete("file-chat-cleanup".to_string(), Some(10), Some(1), "test");
    assert!(runtime.pending_cleanup_count() >= 1);

    cleanup_openai_files_on_session_end(&ctx, "chat_test_end").await;
    assert_eq!(
        runtime.pending_cleanup_count(),
        0,
        "404 删除应按幂等成功清空队列"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "应发起 1 次 DELETE");
    handle.join().unwrap();
    // SAFETY: 清理测试环境变量。
    unsafe {
        std::env::remove_var("TOMCAT_CHAT_CLEANUP_TEST_KEY");
        match old_no_proxy {
            Some(v) => std::env::set_var("NO_PROXY", v),
            None => std::env::remove_var("NO_PROXY"),
        }
        match old_no_proxy_lower {
            Some(v) => std::env::set_var("no_proxy", v),
            None => std::env::remove_var("no_proxy"),
        }
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn cleanup_plugin_sessions_on_session_end_releases_current_session_vm() {
    const API_ENV: &str = "TOMCAT_CHAT_PLUGIN_CLEANUP_TEST_KEY";
    const PLUGIN_ID: &str = "session-cleanup-plugin";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_session_plugin_fixture(workspace.path(), PLUGIN_ID, "session");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    crate::test_support::write_models_override(
        work_dir.path(),
        &[crate::test_support::TestModelOverride::gpt54_openai_responses(API_ENV)],
    );
    cfg.plugin.auto_load = vec![PLUGIN_ID.to_string()];

    let ctx = ChatContext::from_config(cfg).expect("chat context");
    let plugin_manager = ctx
        .global_services
        .plugin_manager
        .as_ref()
        .expect("plugin manager");
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current session id query")
        .expect("current session id");
    let instance_id = format!("{session_id}/{PLUGIN_ID}");

    if !plugin_manager.has_session_vm(&session_id, PLUGIN_ID) {
        plugin_manager
            .start_session_vm(&session_id, PLUGIN_ID)
            .await
            .expect("start session vm");
    }

    assert!(
        plugin_manager.has_session_vm(&session_id, PLUGIN_ID),
        "fixture should have an active session VM before cleanup"
    );
    assert!(
        ctx.scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_some(),
        "session VM should register an event channel before cleanup"
    );

    cleanup_plugin_sessions_on_session_end(&ctx, "suite_test_cleanup").await;
    cleanup_plugin_sessions_on_session_end(&ctx, "suite_test_cleanup_again").await;

    assert!(
        !plugin_manager.has_session_vm(&session_id, PLUGIN_ID),
        "cleanup helper should remove the current session VM"
    );
    assert!(
        ctx.scope_services
            .scope_container
            .dispatcher
            .get_event_sender(&instance_id)
            .is_none(),
        "cleanup helper should remove the session event channel"
    );
}
