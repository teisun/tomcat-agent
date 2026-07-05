use super::super::*;
use crate::api::chat::run_loop::cleanup_plugin_sessions_on_session_end;
use crate::api::chat::run_loop::compose_planned_turn_messages;
use crate::core::session::manager::init_context_state;
use crate::SessionEntry;
use crate::{
    AppConfig, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind, CheckpointMeta,
    CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, ListOptions, RestoreOptions,
    RetentionPolicy, SessionManager,
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

// T2-P1-002 PR-PLA：build_tool_definitions 现在需要 &ChatContext 才能按 PlanState 过滤。
// 这三个测试改为直接读 catalog 默认视图（与 build_tool_definitions 在 PlanState::Chat 时等价）。
fn build_tool_definitions_default_view() -> Vec<serde_json::Value> {
    crate::core::tools::contract::catalog::build_function_definitions_for_chat_default()
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
fn build_tool_definitions_default_view_excludes_plan_only_tools() {
    let defs = build_tool_definitions_default_view();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    for plan_tool in ["create_plan", "update_plan", "todos", "ask_question"] {
        assert!(
            !names.contains(&plan_tool.to_string()),
            "CHAT 默认视图不应暴露 plan_only 工具 {plan_tool}, got: {names:?}"
        );
    }
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
        ctx.session_runtime.plan_runtime.ask_question_panel().is_some(),
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
fn user_prompt_for_mode_formats_all_states() {
    use crate::core::plan_runtime::PlanState;

    assert_eq!(
        super::super::prompt::user_prompt_for_mode(&PlanState::Chat),
        "u[Chat]> "
    );
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(&PlanState::Planning),
        "u[Plan:planning]> "
    );
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(&PlanState::Executing {
            plan_id: "p1".into(),
        }),
        "u[Plan:executing]> "
    );
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(&PlanState::Pending {
            plan_id: "p1".into(),
        }),
        "u[Plan:pending]> "
    );
    assert_eq!(
        super::super::prompt::user_prompt_for_mode(&PlanState::Completed {
            plan_id: "p1".into(),
        }),
        "u[Chat]> "
    );
}

#[test]
fn agent_prompt_for_mode_uses_agent_prefix_and_hides_plan_id() {
    use crate::core::plan_runtime::PlanState;

    assert_eq!(
        super::super::prompt::agent_prompt_for_mode("main", &PlanState::Chat),
        "agent.main> "
    );
    assert_eq!(
        super::super::prompt::agent_prompt_for_mode(
            "main",
            &PlanState::Executing {
                plan_id: "ship-001".into(),
            }
        ),
        "agent.main[Plan:executing]> "
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
    let runtime = ctx
        .session_runtime
        .openai_files_runtime
        .as_ref()
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
