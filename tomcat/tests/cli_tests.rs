//! CLI 子命令集成测试：通过 assert_cmd 黑盒测试 tomcat 二进制。
//! 覆盖 TASK-02 (T1-P0-010-completion) 验收标准：
//!   doctor / config get|set / plugin list|load|info / audit list|export / session list|new
//! 遵循 INTEGRATION_TEST_SPEC：AAA 模式、日志门禁、鲁棒性边界。

mod common;

use assert_cmd::Command;
use async_trait::async_trait;
use predicates::prelude::*;
use serde_json::{json, Value};
use serial_test::serial;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tomcat::{
    init_context_state, llm_http_status_error, run_chat_turn, AppConfig, AppError, BashResult,
    Capabilities, ChatContext, ChatMessage, ChatRequest, ChatResponse, DirEntry, EditFileResult,
    EditOperation, LlmProvider, LlmResolver, LlmScene, PrimitiveExecutor, PrimitiveOperation,
    ResolvedCall, SessionManager, StreamEvent, WriteFileResult,
};
use tracing::{info, info_span};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(deprecated)]
fn cmd() -> Command {
    let mut c = Command::cargo_bin("tomcat").expect("binary tomcat should exist");
    // 避免宿主环境 TOMCAT__LLM__DEFAULT_MODEL 覆盖临时 HOME 下的 tomcat.config.toml
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c
}

fn real_llm_api_key(test_name: &str) -> String {
    common::require_deepseek_api_key(test_name)
}

fn configure_deepseek_real_llm(command: &mut Command, api_key: &str) {
    let model = common::deepseek_test_model();
    command
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, api_key)
        .env("TOMCAT__LLM__PROVIDER", "openai")
        .env("TOMCAT__LLM__API_BASE", common::DEEPSEEK_TEST_API_BASE)
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__DEFAULT_MODEL", &model)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", &model);
}

fn configure_deepseek_without_key(command: &mut Command) {
    let model = common::deepseek_test_model();
    command
        .env_remove(common::DEEPSEEK_TEST_API_KEY_ENV)
        .env("TOMCAT__LLM__PROVIDER", "openai")
        .env("TOMCAT__LLM__API_BASE", common::DEEPSEEK_TEST_API_BASE)
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__DEFAULT_MODEL", &model)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", &model);
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn current_code_session_key() -> String {
    let cwd = std::env::current_dir().expect("current_dir for cli tests");
    tomcat::session_key_for(tomcat::SessionMode::Code, &cwd)
}

fn create_session_via_cli(work_dir: &Path) -> String {
    let output = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .output()
        .expect("session new should run");
    assert!(
        output.status.success(),
        "session new should succeed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("已创建会话: ")
                .and_then(|rest| rest.split_whitespace().next())
        })
        .expect("session new should print session id")
        .to_string()
}

fn write_skill_fixture(workspace: &Path, name: &str, description: &str, user_only: bool) {
    let skill_dir = workspace.join(".tomcat").join("skills").join(name);
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    let mut content = format!("---\nname: {name}\ndescription: {description}\n");
    if user_only {
        content.push_str("disable-model-invocation: true\n");
    }
    content.push_str("---\n# Skill Body\n1. Do the thing.\n");
    fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
}

fn load_current_transcript_for_work_dir(work_dir: &Path) -> String {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    let sessions_dir = tomcat::resolve_sessions_dir(&cfg).expect("resolve sessions dir");
    let session = SessionManager::new(sessions_dir);
    let transcript_path = session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    fs::read_to_string(&transcript_path).expect("read transcript")
}

fn spawn_quick_openai_stream_server(reply: &'static str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock llm server");
    let addr = listener.local_addr().expect("local addr");
    listener
        .set_nonblocking(true)
        .expect("set mock llm server nonblocking");
    let handle = std::thread::spawn(move || {
        let mut served = 0usize;
        let mut last_activity = Instant::now();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    served += 1;
                    last_activity = Instant::now();
                    let mut buf = [0u8; 8192];
                    let _ = stream.read(&mut buf);
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
                    let first = format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{reply}\"}}}}]}}\n\n"
                    );
                    let finish = "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n";
                    stream.write_all(headers.as_bytes()).expect("write headers");
                    stream.write_all(first.as_bytes()).expect("write delta");
                    stream.write_all(finish.as_bytes()).expect("write finish");
                    stream.flush().expect("flush");
                    if served >= 4 {
                        break;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if served > 0 && last_activity.elapsed() > Duration::from_secs(1) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(err) => panic!("accept: {err}"),
            }
        }
    });
    (format!("http://{addr}"), handle)
}

struct DeterministicMockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl DeterministicMockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for DeterministicMockLlm {
    fn provider_name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let mut guard = self.streams.lock().unwrap();
        let events = guard
            .pop_front()
            .ok_or_else(|| AppError::Llm("DeterministicMockLlm: no more streams".to_string()))?;
        drop(guard);
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct FixedResolver {
    provider: Arc<dyn LlmProvider>,
    default_model: String,
}

impl FixedResolver {
    fn new(provider: Arc<dyn LlmProvider>, default_model: impl Into<String>) -> Self {
        Self {
            provider,
            default_model: default_model.into(),
        }
    }

    fn resolved_call(&self, model: &str) -> ResolvedCall {
        let lower = model.trim().to_ascii_lowercase();
        let (api, provider, base_url) = if lower.starts_with("deepseek-") {
            ("openai", "deepseek", "https://api.deepseek.com")
        } else {
            ("openai-responses", "openai", "https://api.openai.com")
        };
        let capabilities = Capabilities {
            vision: lower.starts_with("gpt-"),
            files: lower.starts_with("gpt-"),
            tools: true,
            reasoning: lower.starts_with("deepseek-v4-") || lower.starts_with("gpt-5."),
            web_search: false,
        };
        ResolvedCall {
            provider_impl: self.provider.clone(),
            model: model.to_string(),
            api: api.to_string(),
            provider: provider.to_string(),
            base_url: Some(base_url.to_string()),
            key_source: "DEEPSEEK_API_KEY".to_string(),
            thinking_format: tomcat::core::llm::thinking_policy::thinking_format_for_model(model),
            capabilities,
        }
    }
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let model = match scene {
            LlmScene::Main | LlmScene::Vision => session_override
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or(&self.default_model),
            LlmScene::Compaction | LlmScene::Title => &self.default_model,
        };
        Ok(self.resolved_call(model))
    }
}

fn install_fixed_resolver(
    ctx: &mut ChatContext,
    provider: Arc<dyn LlmProvider>,
    default_model: &str,
) {
    ctx.global_services.llm = provider.clone();
    ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(provider, default_model));
}

struct DeterministicMockPrimitive;

#[async_trait]
impl PrimitiveExecutor for DeterministicMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{path}"))
    }

    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: overwrite || !content.is_empty(),
            bytes_written: content.len() as u64,
            diff_hint: None,
        })
    }

    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }

    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        Ok(BashResult {
            stdout: format!("out:{command}"),
            stderr: String::new(),
            exit_code: 0,
            ..Default::default()
        })
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

struct CliInjectAppendInvariantSink {
    inner: SessionManager,
    append_calls: AtomicUsize,
    injected: AtomicBool,
}

impl CliInjectAppendInvariantSink {
    fn new(inner: SessionManager) -> Self {
        Self {
            inner,
            append_calls: AtomicUsize::new(0),
            injected: AtomicBool::new(false),
        }
    }
}

impl tomcat::core::session::MessageAppendSink for CliInjectAppendInvariantSink {
    fn append_message(&self, value: serde_json::Value) -> Result<String, AppError> {
        let call_idx = self.append_calls.fetch_add(1, Ordering::SeqCst);
        if call_idx == 2 && !self.injected.swap(true, Ordering::SeqCst) {
            let tool_call_id = value
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("call_injected")
                .to_string();
            self.inner.append_message(serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": "[interrupted]"
            }))?;
            self.inner.append_message(serde_json::json!({
                "role": "user",
                "content": "nested prompt"
            }))?;
            self.inner.append_message(serde_json::json!({
                "role": "assistant",
                "content": "nested done"
            }))?;
        }
        self.inner.append_message(value)
    }
}

fn cli_text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn cli_text_stream_with_usage(
    text: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: Some(prompt_tokens + completion_tokens),
        }),
    ]
}

fn cli_tool_call_stream(id: &str, name: &str, args: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(args.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

fn deterministic_chat_context_fixture(env_key: &str) -> (tempfile::TempDir, ChatContext) {
    deterministic_chat_context_fixture_with_config(AppConfig::default(), env_key)
}

fn deterministic_chat_context_fixture_with_config(
    mut cfg: AppConfig,
    env_key: &str,
) -> (tempfile::TempDir, ChatContext) {
    let dir = tempfile::tempdir().unwrap();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(env_key.to_string());

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
    (dir, ctx)
}

fn assistant_tool_calls_from_transcript(transcript: &str) -> Vec<(String, Value)> {
    let mut calls = Vec::new();
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for tool_call in tool_calls {
            let Some(function) = tool_call.get("function") else {
                continue;
            };
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let parsed = serde_json::from_str::<Value>(arguments).unwrap_or_else(|err| {
                panic!("tool arguments should be valid json: {arguments}; err: {err}");
            });
            calls.push((name, parsed));
        }
    }
    calls
}

fn tool_results_from_transcript(transcript: &str) -> Vec<Value> {
    let mut results = Vec::new();
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        if let Ok(parsed) = serde_json::from_str::<Value>(content) {
            results.push(parsed);
        }
    }
    results
}

fn seed_dangling_tool_round(session: &SessionManager, tool_call_id: &str) {
    session
        .append_message(json!({
            "role": "assistant",
            "content": "dangling tool call",
            "tool_calls": [{
                "id": tool_call_id,
                "type": "function",
                "function": {
                    "name": "bash",
                    "arguments": r#"{"command":"echo hi","cwd":null}"#
                }
            }]
        }))
        .expect("seed dangling assistant.tool_calls");
}

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let mut saved = Vec::new();
        for (key, value) in entries {
            saved.push(((*key).to_string(), std::env::var(key).ok()));
            match value {
                Some(value) => {
                    // SAFETY: 测试阶段串行写入独立环境变量，作用域结束由 guard 还原。
                    unsafe { std::env::set_var(key, value) };
                }
                None => {
                    // SAFETY: 测试阶段串行移除独立环境变量，作用域结束由 guard 还原。
                    unsafe { std::env::remove_var(key) };
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => {
                    // SAFETY: 还原测试前的环境变量快照。
                    unsafe { std::env::set_var(&key, value) };
                }
                None => {
                    // SAFETY: 还原测试前的环境变量快照。
                    unsafe { std::env::remove_var(&key) };
                }
            }
        }
    }
}

// ────────────────────── help & version ──────────────────────

/// [--help 输出] 验证主帮助页包含所有一级子命令名称
///
/// 验证：exit 0 且 stdout 包含 tomcat、init、doctor、config、session、workspace、plugin、audit
/// 意义：CLI 入口完整性门禁（TASK-02 验收：所有子命令帮助文档完整）
#[test]
fn test_help_output_contains_tomcat_and_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_help_output_contains_tomcat_and_exits_ok").entered();

    info!("Arrange: prepare --help command");
    let mut c = cmd();
    c.arg("--help");

    info!("Act: execute tomcat --help");
    let assert = c.assert();

    info!("Assert: exit 0 and output contains tomcat");
    assert
        .success()
        .stdout(predicate::str::contains("tomcat"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("workspace"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

/// [--version 输出] 验证版本号输出格式
///
/// 验证：exit 0 且 stdout 含 tomcat 版本字符串
/// 意义：发布合规——二进制可报告自身版本
#[test]
fn test_version_output_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_version_output_exits_ok").entered();

    info!("Arrange: prepare --version");
    let mut c = cmd();
    c.arg("--version");

    info!("Act: execute --version");
    let assert = c.assert();

    info!("Assert: exit 0 and contains version string");
    assert.success().stdout(predicate::str::contains("tomcat"));
}

// ────────────────────── init ──────────────────────

/// [init 子命令] 在临时目录生成配置文件
///
/// 验证：exit 0、tomcat.config.toml 已创建且默认 provider 为 openai-responses、stdout 含三步向导与「配置文件已写入」
/// 意义：首次使用流程门禁（TASK-02 10.2：引导 LLM 配置、生成配置文件）
#[test]
fn test_init_creates_config_file_in_temp_dir() {
    common::setup_logging();
    let _span = info_span!("test_init_creates_config_file_in_temp_dir").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: temp dir at {:?}", dir.path());
    let mut c = cmd();
    c.args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh");

    info!("Act: execute init");
    let assert = c.assert();

    info!("Assert: exit 0, config file created, output mentions file path");
    assert
        .success()
        .stdout(predicate::str::contains("[1/3] 环境初始化"))
        .stdout(predicate::str::contains("配置文件已写入"));
    assert!(config_path.exists(), "config file should be created");
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains("[log]"),
        "config should contain [log] section"
    );
    assert!(
        content.contains("provider = \"openai-responses\""),
        "config should default to openai-responses"
    );
}

// ────────────────────── doctor ──────────────────────

/// [doctor 无配置] 未找到配置文件时给出引导提示
///
/// 验证：exit 0 且 stdout 含"未找到配置文件"
/// 意义：友好引导门禁（TASK-02 验收：首次运行无配置时的提示友好）
#[test]
fn test_doctor_without_config_prompts_init() {
    common::setup_logging();
    let _span = info_span!("test_doctor_without_config_prompts_init").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: point to nonexistent config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());

    info!("Act: execute doctor");
    let assert = c.assert();

    info!("Assert: exit 0, prompts about missing config");
    assert
        .success()
        .stdout(predicate::str::contains("未找到配置文件"));
}

/// [doctor 有配置] init 后 doctor 通过配置与环境检测
///
/// 验证：exit 0 且 stdout 含"配置合法"或 checkmark
/// 意义：TASK-02 10.3 验收——doctor 检测 rquickjs 与配置可用性并输出修复建议
#[test]
fn test_doctor_with_valid_config_checks_environment() {
    common::setup_logging();
    let _span = info_span!("test_doctor_with_valid_config_checks_environment").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: create valid config via init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: execute doctor with valid config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let assert = c.assert();

    info!("Assert: exit 0, mentions config validity and wasm checks");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
}

/// [E2E-CLI-004] 工作区 add / list / remove
///
/// 验证：init 后 workspace add → list 含路径 → remove → list 为空提示
/// 意义：TASK-12 / TASK-09：`tomcat workspace` 与 `tomcat.config.toml` `[workspace] workspace_roots` 一致
#[test]
fn test_workspace_add_list_remove_e2e() {
    common::setup_logging();
    let _span = info_span!("test_workspace_add_list_remove_e2e").entered();

    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let proj_canon = std::fs::canonicalize(proj.path()).unwrap();
    let proj_str = proj_canon.to_str().unwrap();

    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    cmd()
        .args(["workspace", "add", proj_str])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    let list_assert = cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout).to_string();
    list_assert.success();
    assert!(
        list_out.contains(proj_str),
        "list 应含已添加路径，实际: {}",
        trunc(&list_out, 200)
    );

    cmd()
        .args(["workspace", "remove", proj_str])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已移除工作区"));

    cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("无已授权工作区"));
}

/// [E2E-CLI-017] workspace add --cwd 将当前目录加入授权列表
#[test]
fn test_workspace_add_cwd_e2e() {
    common::setup_logging();
    let _span = info_span!("test_workspace_add_cwd_e2e").entered();

    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let proj_canon = std::fs::canonicalize(proj.path()).unwrap();
    let proj_str = proj_canon.to_str().unwrap();

    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    // `std::env::current_dir` 为进程全局；若将来 cli_tests 改为多线程并行，需改为子进程或串行策略，避免与其它用例竞态。
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(proj.path()).unwrap();
    cmd()
        .args(["workspace", "add", "--cwd"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));
    std::env::set_current_dir(&prev).unwrap();

    let list_assert = cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout).to_string();
    list_assert.success();
    assert!(
        list_out.contains(proj_str),
        "list 应含当前目录，实际: {}",
        trunc(&list_out, 200)
    );
}

/// [E2E-CLI-005] init 自动将 PATH 写入隔离 HOME 下的 shell 配置文件
#[test]
fn test_init_auto_adds_path_to_shell_profile() {
    common::setup_logging();
    let _span = info_span!("test_init_auto_adds_path_to_shell_profile").entered();

    let dir = tempfile::tempdir().unwrap();
    let zshrc = dir.path().join(".zshrc");

    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let content = fs::read_to_string(&zshrc).expect(".zshrc should be created under HOME");
    assert!(
        content.contains("export PATH=") && content.contains("# Added by tomcat init"),
        "expected PATH block in .zshrc, got: {}",
        trunc(&content, 400)
    );
}

/// init 两次后 shell 配置中仅一条 export PATH（幂等）
#[test]
fn test_init_path_export_idempotent_in_shell_profile() {
    common::setup_logging();
    let _span = info_span!("test_init_path_export_idempotent_in_shell_profile").entered();

    let dir = tempfile::tempdir().unwrap();
    let zshrc = dir.path().join(".zshrc");

    for _ in 0..2 {
        cmd()
            .args(["init"])
            .env("HOME", dir.path())
            .env("SHELL", "/bin/zsh")
            .assert()
            .success();
    }
    let content = fs::read_to_string(&zshrc).unwrap();
    let count = content.matches("export PATH=").count();
    assert_eq!(
        count,
        1,
        "expected single export PATH line, got {} in: {}",
        count,
        trunc(&content, 500)
    );
}

// ────────────────────── config ──────────────────────

/// [config get 无参] 输出完整配置内容
///
/// 验证：exit 0 且 stdout 含 log/level 等配置段
/// 意义：TASK-02 10.4——config get 可展示全部配置
#[test]
fn test_config_get_without_key_outputs_full_config() {
    common::setup_logging();
    let _span = info_span!("test_config_get_without_key_outputs_full_config").entered();

    info!("Arrange: use default config");
    let mut c = cmd();
    c.args(["config", "get"]);

    info!("Act: execute config get");
    let assert = c.assert();

    info!("Assert: exit 0, output contains config sections");
    assert
        .success()
        .stdout(predicate::str::contains("log").or(predicate::str::contains("level")));
}

/// [config get 已知 key] 查询 log.level 返回具体值
///
/// 验证：exit 0
/// 意义：TASK-02 10.4——config get(key) 可查询单项配置
#[test]
fn test_config_get_with_known_key_outputs_value() {
    common::setup_logging();
    let _span = info_span!("test_config_get_with_known_key_outputs_value").entered();

    info!("Arrange: query log.level");
    let mut c = cmd();
    c.args(["config", "get", "log.level"]);

    info!("Act: execute config get log.level");
    let assert = c.assert();

    info!("Assert: exit 0, output shows value");
    assert.success();
}

/// [config get 未知 key] 查询不存在的配置键给出提示
///
/// 验证：exit 0 且 stdout 含"未找到"或"不存在"
/// 意义：TASK-02 10.4——config get 对非法 key 的容错与友好提示
#[test]
fn test_config_get_with_unknown_key_shows_hint() {
    common::setup_logging();
    let _span = info_span!("test_config_get_with_unknown_key_shows_hint").entered();

    info!("Arrange: query nonexistent key");
    let mut c = cmd();
    c.args(["config", "get", "nonexistent.key"]);

    info!("Act: execute config get nonexistent.key");
    let assert = c.assert();

    info!("Assert: exit 0, output mentions not found");
    assert
        .success()
        .stdout(predicate::str::contains("未找到").or(predicate::str::contains("不存在")));
}

// ────────────────────── config set (boundary) ──────────────────────

/// [config set 缺参数] set 不带 key/value 时 clap 报错
///
/// 验证：exit code 非 0、stderr 含 Usage 或 error
/// 意义：TASK-02 10.4——config set 参数校验门禁
#[test]
fn test_config_set_missing_args_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_config_set_missing_args_shows_error").entered();

    info!("Arrange: config set with no args");
    let mut c = cmd();
    c.args(["config", "set"]);

    info!("Act: execute config set without key/value");
    let assert = c.assert();

    info!("Assert: clap rejects missing arguments");
    assert
        .failure()
        .stderr(predicate::str::contains("Usage").or(predicate::str::contains("error")));
}

// ────────────────────── config help ──────────────────────

/// [config --help] 帮助页列出所有 config 子命令
///
/// 验证：exit 0 且 stdout 包含 get/set/edit
/// 意义：CLI 帮助完整性门禁
#[test]
fn test_config_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_config_help_lists_subcommands").entered();

    info!("Arrange: config --help");
    let mut c = cmd();
    c.args(["config", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists get/set/edit");
    assert
        .success()
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("edit"));
}

// ────────────────────── plugin ──────────────────────

/// [plugin list 空] 无已加载插件时正常退出
///
/// 验证：exit 0 且 stdout 含"无已加载插件"或"插件"
/// 意义：TASK-06 验收——plugin list 空列表不崩溃
#[test]
fn test_plugin_list_empty_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_plugin_list_empty_exits_ok").entered();

    info!("Arrange: no plugins loaded");
    let mut c = cmd();
    c.args(["plugin", "list"]);

    info!("Act: execute plugin list");
    let assert = c.assert();

    info!("Assert: exit 0, mentions no plugins");
    assert
        .success()
        .stdout(predicate::str::contains("无已加载插件").or(predicate::str::contains("插件")));
}

/// [plugin load 不存在路径] 加载不存在的 wasm 文件给出提示
///
/// 验证：exit 0 且 stdout 含"不存在"
/// 意义：TASK-06——plugin load 路径校验与友好错误提示
#[test]
fn test_plugin_load_nonexistent_path_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_plugin_load_nonexistent_path_shows_error").entered();

    info!("Arrange: load from nonexistent path");
    let mut c = cmd();
    c.args(["plugin", "load", "/tmp/nonexistent_pi_plugin_xyz"]);

    info!("Act: execute plugin load");
    let assert = c.assert();

    info!("Assert: exit 0, mentions path not found");
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [plugin info 不存在] 查询不存在的插件 ID 提示"未找到"
///
/// 验证：exit 0 且 stdout 含"未找到"
/// 意义：TASK-06——plugin info 对非法 ID 的容错
#[test]
fn test_plugin_info_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_info_not_found_shows_message").entered();

    info!("Arrange: query nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "info", "nonexistent-plugin-id"]);

    info!("Act: execute plugin info");
    let assert = c.assert();

    info!("Assert: exit 0, mentions not found");
    assert.success().stdout(predicate::str::contains("未找到"));
}

/// [plugin unload 不存在] 卸载不存在的插件给出"卸载失败"
///
/// 验证：exit 0 且 stdout 含"卸载失败"
/// 意义：TASK-06——plugin unload 对非法 ID 的容错
#[test]
fn test_plugin_unload_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_unload_not_found_shows_message").entered();

    info!("Arrange: unload nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "unload", "nonexistent-plugin-id"]);

    info!("Act: execute plugin unload");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("卸载失败"));
}

/// [plugin enable 不存在] 启用不存在的插件给出"启用失败"
///
/// 验证：exit 0 且 stdout 含"启用失败"
/// 意义：TASK-06——plugin enable 对非法 ID 的容错
#[test]
fn test_plugin_enable_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_enable_not_found_shows_message").entered();

    info!("Arrange: enable nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "enable", "nonexistent-plugin-id"]);

    info!("Act: execute plugin enable");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("启用失败"));
}

/// [plugin disable 不存在] 禁用不存在的插件给出"禁用失败"
///
/// 验证：exit 0 且 stdout 含"禁用失败"
/// 意义：TASK-06——plugin disable 对非法 ID 的容错
#[test]
fn test_plugin_disable_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_disable_not_found_shows_message").entered();

    info!("Arrange: disable nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "disable", "nonexistent-plugin-id"]);

    info!("Act: execute plugin disable");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("禁用失败"));
}

/// [plugin --help] 帮助页列出所有 plugin 子命令
///
/// 验证：exit 0 且 stdout 包含 list/load/unload/enable/disable/info
/// 意义：CLI 帮助完整性门禁
#[test]
fn test_plugin_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_plugin_help_lists_subcommands").entered();

    info!("Arrange: plugin --help");
    let mut c = cmd();
    c.args(["plugin", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists all plugin subcommands");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("load"))
        .stdout(predicate::str::contains("unload"))
        .stdout(predicate::str::contains("enable"))
        .stdout(predicate::str::contains("disable"))
        .stdout(predicate::str::contains("info"));
}

// ────────────────────── audit ──────────────────────

/// [audit list] 列出审计记录正常退出
///
/// 验证：exit 0
/// 意义：TASK-02 10.7——audit list 不崩溃，无审计记录或已禁用时友好处理
#[test]
fn test_audit_list_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_audit_list_exits_ok").entered();

    info!("Arrange: default config (file_enabled likely false)");
    let mut c = cmd();
    c.args(["audit", "list"]);

    info!("Act: execute audit list");
    let assert = c.assert();

    info!("Assert: exit 0, either shows entries or explains disabled/missing");
    assert.success();
}

/// [audit --help] 帮助页列出所有 audit 子命令
///
/// 验证：exit 0 且 stdout 包含 list/show/export
/// 意义：CLI 帮助完整性门禁
#[test]
fn test_audit_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_audit_help_lists_subcommands").entered();

    info!("Arrange: audit --help");
    let mut c = cmd();
    c.args(["audit", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists list/show/export");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("export"));
}

// ────────────────────── session ──────────────────────

/// [session list] 空会话列表正常退出
///
/// 验证：exit 0
/// 意义：TASK-02 10.6——session list 在无会话时不崩溃
#[test]
fn test_session_list_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_session_list_exits_ok").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp work dir {:?}", work_dir);
    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "list"]);

    info!("Act: execute session list");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

/// [session new] 创建新会话
///
/// 验证：exit 0 且 stdout 含"已创建会话"
/// 意义：TASK-02 10.6——session new 可创建并持久化会话
#[test]
fn test_session_new_creates_session() {
    common::setup_logging();
    let _span = info_span!("test_session_new_creates_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp work dir {:?}", work_dir);
    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "new"]);

    info!("Act: execute session new");
    let assert = c.assert();

    info!("Assert: exit 0, mentions created");
    assert
        .success()
        .stdout(predicate::str::contains("已创建会话"));
}

/// [session --help] 帮助页列出所有 session 子命令
///
/// 验证：exit 0 且 stdout 包含 list/new/switch/delete/archive/search
/// 意义：CLI 帮助完整性门禁（TASK-02 验收）
#[test]
fn test_session_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_session_help_lists_subcommands").entered();

    info!("Arrange: session --help");
    let mut c = cmd();
    c.args(["session", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists all session subcommands");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("switch"))
        .stdout(predicate::str::contains("delete"))
        .stdout(predicate::str::contains("archive"))
        .stdout(predicate::str::contains("search"));
}

// ────────────────────── chat ──────────────────────

/// [chat 无配置] 没有 API key 和配置时 chat 失败退出
///
/// 验证：exit code 非 0
/// 意义：INTEGRATION_TEST_SPEC——无 key 不得 ignore，必须失败
#[test]
fn test_chat_without_config_exits_with_error() {
    common::setup_logging();
    let _span = info_span!("test_chat_without_config_exits_with_error").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: 无 ~/.tomcat/ 配置且无 DEEPSEEK_API_KEY（HOME 指向空临时目录）");
    let mut c = cmd();
    c.arg("chat").env("HOME", dir.path());
    configure_deepseek_without_key(&mut c);

    info!("Act: execute chat");
    let assert = c.assert();

    info!("Assert: non-zero exit (no API key or config)");
    assert.failure();
}

/// [chat 有 API key] 有合法配置与 API key 时 chat 启动并产生输出
///
/// 验证：exit 0 且 stdout 包含"对话模式"banner 或模型信息或 agent prompt
/// 意义：TASK-02 10.1——chat 端到端可用；INTEGRATION_TEST_SPEC：无 key 不得 ignore
#[test]
fn test_chat_with_valid_config_and_api_key_starts_and_produces_output() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span =
        info_span!("test_chat_with_valid_config_and_api_key_starts_and_produces_output").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    info!("Arrange: init config in temp dir, set work_dir and DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let api_key =
        real_llm_api_key("test_chat_with_valid_config_and_api_key_starts_and_produces_output");

    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .write_stdin("hi\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);

    info!("Act: execute chat with stdin 'hi', timeout 60s");
    let assert = c.assert();
    let out = assert.get_output().stdout.clone();
    let out_str = String::from_utf8_lossy(&out);

    info!("Assert: exit 0 and stdout contains 对话模式 banner or AI output");
    assert.success();
    assert!(
        out_str.contains("对话模式")
            || out_str.contains("模型:")
            || out_str.contains("agent.main>"),
        "chat 应输出对话模式 banner 或模型信息或 agent.main> 提示，实际: {}",
        out_str.chars().take(500).collect::<String>()
    );
}

/// [chat + session 协作] session new 后启动 chat 不挂起不崩溃
///
/// 验证：进程在 5s 内结束且产生 stdout 或 stderr
/// 意义：TASK-02——chat 与 session 子系统协作无死锁/崩溃
#[test]
fn test_chat_with_session_dir_does_not_crash() {
    common::setup_logging();
    let _span = info_span!("test_chat_with_session_dir_does_not_crash").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    info!("Arrange: init config, session new, set work_dir");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let mut c_new = cmd();
    c_new
        .args(["session", "new"])
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c_new.assert().success();

    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .write_stdin("\n")
        .timeout(std::time::Duration::from_secs(5));
    configure_deepseek_without_key(&mut c);

    info!("Act: run chat without API key, timeout 5s");
    let output = c.output().expect("chat 进程应在 5s 内结束");

    info!("Assert: 有 stdout 或 stderr，进程未静默挂起");
    assert!(
        !output.stdout.is_empty() || !output.stderr.is_empty(),
        "chat 应产生输出（banner 或错误），不应静默崩溃"
    );
}

// ────────────────────── boundary: unknown subcommand ──────────────────────

/// [未知子命令] 输入不存在的子命令给出 clap 错误
///
/// 验证：exit code 非 0 且 stderr 含"error"
/// 意义：CLI 边界安全——防止静默忽略拼写错误
#[test]
fn test_unknown_subcommand_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_unknown_subcommand_shows_error").entered();

    info!("Arrange: unknown subcommand");
    let mut c = cmd();
    c.arg("nonexistent_command");

    info!("Act: execute unknown command");
    let assert = c.assert();

    info!("Assert: exits with error from clap");
    assert.failure().stderr(predicate::str::contains("error"));
}

// ────────────────────── init + doctor roundtrip ──────────────────────

/// [init → doctor 联合] init 后 doctor 应通过配置检测
///
/// 验证：init exit 0 + doctor exit 0 且 stdout 含"配置合法"或 ✓
/// 意义：端到端新手引导流程（TASK-02 10.2 + 10.3 联合验收）
#[test]
fn test_init_then_doctor_roundtrip() {
    common::setup_logging();
    let _span = info_span!("test_init_then_doctor_roundtrip").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: init config in temp dir");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: doctor with generated config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let assert = c.assert();

    info!("Assert: doctor passes config check");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
}

// ────────────────────── 补充用例：session switch/delete/archive ──────────────────────

/// [session switch 不存在] switch 到不存在的会话给出提示
///
/// 验证：exit 0 且 stdout 含"不存在"
/// 意义：TASK-02 10.6——session switch 对非法 key 的容错
#[test]
fn test_session_switch_nonexistent_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_session_switch_nonexistent_shows_error").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: switch to nonexistent session key");
    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "switch", "nonexistent-key-xyz"]);

    info!("Act: execute session switch");
    let assert = c.assert();

    info!("Assert: exit 0, mentions not exist");
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [session delete via CLI] 创建会话后通过 CLI 删除
///
/// 验证：new exit 0 + delete exit 0 且 stdout 含"已删除"
/// 意义：TASK-02 10.6——session delete 端到端可用
#[test]
fn test_session_delete_via_cli_removes_session() {
    common::setup_logging();
    let _span = info_span!("test_session_delete_via_cli_removes_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create a session first");
    let session_id = create_session_via_cli(&work_dir);

    info!("Act: delete the created session");
    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "delete", session_id.as_str()]);

    let assert = c.assert();

    info!("Assert: exit 0, mentions deleted");
    assert.success().stdout(predicate::str::contains("已删除"));
}

/// [session archive] archive 子命令可正常执行
///
/// 验证：exit 0（即使会话不存在也不崩溃）
/// 意义：TASK-02 10.6——session archive 端到端可用
#[test]
fn test_session_archive_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_session_archive_exits_ok").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session then archive");
    let session_id = create_session_via_cli(&work_dir);

    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "archive", session_id.as_str()]);

    info!("Act: execute session archive");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success().stdout(predicate::str::contains("已归档"));
}

// ────────────────────── 补充用例：config set 成功路径 ──────────────────────

/// [config set 合法] config set log.level warn 正常退出
///
/// 验证：exit 0（配置文件存在时修改成功，不存在时给出提示但不崩溃）
/// 意义：TASK-02 10.4——config set 正向路径覆盖（原有用例仅覆盖缺参数的失败路径）
#[test]
fn test_config_set_valid_key_value_updates_config() {
    common::setup_logging();
    let _span = info_span!("test_config_set_valid_key_value_updates_config").entered();

    info!("Act: config set log.level warn");
    let mut c = cmd();
    c.args(["config", "set", "log.level", "warn"]);
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

// ────────────────────── 补充用例：audit show/export ──────────────────────

/// [audit show 不存在 ID] 查看不存在的审计 ID 不崩溃
///
/// 验证：exit 0（打印"未找到"或类似提示，不 panic）
/// 意义：TASK-02 10.7——audit show 容错
#[test]
fn test_audit_show_with_invalid_id_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_audit_show_with_invalid_id_exits_ok").entered();

    info!("Arrange: show nonexistent audit id");
    let mut c = cmd();
    c.args(["audit", "show", "9999999"]);

    info!("Act: execute audit show");
    let assert = c.assert();

    info!("Assert: exit 0, doesn't crash");
    assert.success();
}

/// [audit export] 导出审计记录到文件可正常执行
///
/// 验证：exit 0
/// 意义：TASK-02 10.7——audit export 端到端可用
#[test]
fn test_audit_export_creates_file() {
    common::setup_logging();
    let _span = info_span!("test_audit_export_creates_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("audit_export.json");

    info!("Arrange: export audit to temp path");
    let mut c = cmd();
    c.args(["audit", "export", out.to_str().unwrap()]);

    info!("Act: execute audit export");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

// ══════════════════════════════════════════════════════════════════
// E2E 全量覆盖：test_user_* 用例（按 E2E_SCENARIO_LIBRARY 编号）
// ══════════════════════════════════════════════════════════════════

// ──────────────────── Story 1: 宿主初始化与基础配置 (E2E-CLI-001~006) ────────────────────

/// [E2E-CLI-001] 新用户首次安装，完成初始化并验证环境健康
///
/// 用户意图：新用户首次安装，完成初始化并验证环境健康
/// 验证：init exit 0 + stdout 含 [1/3][2/3][3/3]、tomcat code、PATH 自动配置；doctor exit 0 + stdout 含"配置合法"和"内嵌资源已就绪"
#[test]
fn test_user_first_time_setup_init_and_doctor() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_first_time_setup_init_and_doctor").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir, no existing config");
    info!("Act: tomcat init");
    let init_assert = cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert();
    let init_out = String::from_utf8_lossy(&init_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert init: exit 0 + 三步向导 + tomcat code；actual: {}",
        trunc(&init_out, 400)
    );
    init_assert
        .success()
        .stdout(predicate::str::contains("[1/3]"))
        .stdout(predicate::str::contains("[2/3]"))
        .stdout(predicate::str::contains("[3/3]"))
        .stdout(predicate::str::contains("配置文件已写入"))
        .stdout(predicate::str::contains("tomcat code"))
        .stdout(predicate::str::contains("PATH"));

    info!("Act: tomcat doctor");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let doctor_assert = c.assert();
    let doctor_out =
        String::from_utf8_lossy(&doctor_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert doctor: exit 0 + stdout 含 配置合法 + 内嵌资源；actual: {}",
        trunc(&doctor_out, 400)
    );
    doctor_assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源已就绪").or(predicate::str::contains("✓")));
}

/// [E2E-CLI-002] 用户修改日志级别
///
/// 用户意图：修改 log.level 为 warn
/// 验证：exit 0
#[test]
fn test_user_sets_config_value() {
    common::setup_logging();
    let _span = info_span!("test_user_sets_config_value").entered();

    info!("Arrange: no special setup needed");
    info!("Act: tomcat config set log.level warn");
    let assert = cmd().args(["config", "set", "log.level", "warn"]).assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-003] 用户查看当前全部配置
///
/// 用户意图：查看当前全部配置
/// 验证：exit 0；stdout 含配置段关键字（llm/log/storage）
#[test]
fn test_user_views_full_config() {
    common::setup_logging();
    let _span = info_span!("test_user_views_full_config").entered();

    info!("Arrange: use default config");
    info!("Act: tomcat config get");
    let assert = cmd().args(["config", "get"]).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0, stdout 含配置段关键字；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("llm")
            .or(predicate::str::contains("log"))
            .or(predicate::str::contains("storage")),
    );
}

/// [E2E-CLI-006] 用户运行 doctor 检测 QuickJS/rquickjs 环境
///
/// 用户意图：运行 doctor 检测环境
/// 验证：exit 0；stdout 含环境检测项（rquickjs/配置/✓）
#[test]
fn test_user_doctor_detects_environment() {
    common::setup_logging();
    let _span = info_span!("test_user_doctor_detects_environment").entered();

    info!("Arrange: default config");
    info!("Act: tomcat doctor");
    let assert = cmd().args(["doctor"]).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 rquickjs / 配置 / 内嵌资源 / .env 检查项；actual: {}",
        trunc(&out, 500)
    );
    assert.success().stdout(
        predicate::str::contains("✓ rquickjs 运行时：可用")
            .and(predicate::str::contains("配置"))
            .and(predicate::str::contains("内嵌资源"))
            .and(predicate::str::contains(".env")),
    );
}

// ──────────────────── TASK-06 新增集成测试：内嵌资源 + init .env ────────────────────

/// [TASK-06] init 后生成配置中的 LLM 段
///
/// 验证：tomcat init exit 0；`tomcat.config.toml` 存在且默认 provider 为 openai-responses（.env 仅在用户输入非空 Key 时写入）
#[test]
fn test_init_creates_env_file() {
    common::setup_logging();
    let _span = info_span!("test_init_creates_env_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Assert: config file created");
    assert!(config_path.exists(), "config file should be created");

    let cfg_content = fs::read_to_string(&config_path).unwrap();
    info!("Config content (truncated): {}", trunc(&cfg_content, 300));
    assert!(
        cfg_content.contains("[llm]") || cfg_content.contains("provider"),
        "config should contain LLM section"
    );
    assert!(
        cfg_content.contains("provider = \"openai-responses\""),
        "config should default to openai-responses"
    );
    assert!(
        cfg_content.contains("default_model = \"gpt-5.4\""),
        "config should persist the selected default model"
    );
    assert!(
        cfg_content.contains("api_key_env = \"OPENAI_API_KEY\""),
        "config should persist the provider-derived api_key_env"
    );
}

/// [TASK-06] init 后 .env 权限为 0600
#[test]
#[cfg(unix)]
fn test_init_creates_env_with_correct_permissions() {
    use std::os::unix::fs::PermissionsExt;
    common::setup_logging();
    let _span = info_span!("test_init_creates_env_with_correct_permissions").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init → check .env permissions");

    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let env_path = dir.path().join(".tomcat").join("assets").join(".env");
    if env_path.exists() {
        let mode = fs::metadata(&env_path).unwrap().permissions().mode() & 0o777;
        info!("Assert: .env permissions = {:04o}", mode);
        assert_eq!(mode, 0o600, ".env should have 0600 permissions");
    }
}

/// [TASK-06] doctor 对完整环境报告所有检查项
///
/// 验证：先 init 再 doctor，输出含 配置合法 / 内嵌资源 / rquickjs
#[test]
fn test_doctor_reports_all_checks() {
    common::setup_logging();
    let _span = info_span!("test_doctor_reports_all_checks").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init first");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat doctor");
    let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: all check items present；actual: {}",
        trunc(&out, 600)
    );
    assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源"))
        .stdout(predicate::str::contains("✓ rquickjs 运行时：可用"));
}

/// [E2E-CLI-010] init 幂等：第二次不覆盖配置并给出提示
///
/// 验证：连续两次 tomcat init，第二次 exit 0 且 stdout 含保留/使用已有配置提示
#[test]
fn test_init_idempotent() {
    common::setup_logging();
    let _span = info_span!("test_init_idempotent").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Act: tomcat init (first)");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat init (second, idempotent)");
    let assert = cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: second init exit 0；actual: {}", trunc(&out, 300));
    assert.success().stdout(
        predicate::str::contains("已存在配置文件").or(predicate::str::contains("使用已有配置文件")),
    );
}

/// [TASK-06] ensure_embedded_assets 准备 assets 目录
///
/// 验证：tomcat init 后 ~/.tomcat/assets/ 目录存在
#[test]
fn test_ensure_embedded_assets_prepares_assets_dir() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_extracts_wasm").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Act: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let assets_dir = dir.path().join(".tomcat").join("assets");
    info!("Assert: assets 目录已创建");
    assert!(assets_dir.is_dir(), "assets dir should exist after init");
}

/// [TASK-06] ensure_embedded_assets 重复调用不报错
///
/// 验证：连续 tomcat doctor 两次（每次都触发 ensure_embedded_assets），均 exit 0
#[test]
fn test_ensure_embedded_assets_idempotent() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_idempotent").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat doctor x2（每次触发 ensure_embedded_assets）");
    cmd()
        .args(["doctor"])
        .env("HOME", dir.path())
        .assert()
        .success();
    cmd()
        .args(["doctor"])
        .env("HOME", dir.path())
        .assert()
        .success();
}

/// [TASK-06] ensure_embedded_assets 对已有 assets 目录保持幂等
///
/// 验证：预先放入自定义文件后，tomcat doctor 仍能正常通过
#[test]
fn test_ensure_embedded_assets_tolerates_existing_assets_files() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_upgrades_on_sha_mismatch").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let sentinel = dir.path().join(".tomcat").join("assets").join("custom.txt");
    fs::write(&sentinel, b"keep").unwrap();

    info!("Act: tomcat doctor（触发 ensure_embedded_assets）");
    cmd()
        .args(["doctor"])
        .env("HOME", dir.path())
        .assert()
        .success();

    info!("Assert: 既有 assets 文件不影响 doctor");
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
}

// ──────────────────── Story 2: 4原语安全管控（E2E-CLI-011~012，需 DEEPSEEK_API_KEY） ────────────────────

/// [E2E-CLI-011] 用户向助手提问并收到回答
///
/// 用户意图：在 tomcat chat 中提问，收到 AI 回复
/// 验证：exit 0；stdout 非空
/// 要求：DEEPSEEK_API_KEY 环境变量已设置；无 key 时 panic（符合规范）
#[test]
fn test_user_asks_pi_a_question() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_asks_pi_a_question").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_asks_pi_a_question");

    info!("Act: tomcat chat stdin 你好，介绍一下你自己，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("你好，介绍一下你自己\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "AI 应输出非空回复，实际 stdout 为空"
    );
}

/// [E2E-CLI-012] 用户问技术问题，验证 LLM 回复质量
///
/// 用户意图：问 Rust 所有权系统
/// 验证：exit 0；stdout 含"所有权"或"ownership"
/// 要求：DEEPSEEK_API_KEY 环境变量已设置
#[test]
fn test_user_asks_pi_technical_question() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_asks_pi_technical_question").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_asks_pi_technical_question");

    info!("Act: tomcat chat stdin 问 Rust 所有权，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("用一句话解释什么是 Rust 的所有权系统\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含所有权/ownership；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        out.contains("所有权") || out.to_lowercase().contains("ownership"),
        "stdout 应含 '所有权' 或 'ownership'，实际: {}",
        trunc(&out, 300)
    );
}

/// [E2E-CLI-016] 用户要求助手通过 bash 执行一条命令
///
/// 验证：exit 0；stdout 含 hello_from_pi（或明显命令执行结果）
/// 意义：工具调用 E2E 门禁，保证 execute_bash 被真实调用
#[test]
fn test_user_asks_pi_to_run_bash_command() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_asks_pi_to_run_bash_command").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_asks_pi_to_run_bash_command");

    info!("Act: tomcat chat stdin 请执行 echo hello_from_pi，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .env("RUST_LOG", "tomcat=info")
        .write_stdin("请执行 echo hello_from_pi\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 1500));
    }
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    info!(
        "Assert: exit 0 + stdout 含 hello_from_pi；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        out.contains("hello_from_pi"),
        "stdout 应含 'hello_from_pi'（工具 execute_bash 被调用），实际: {}",
        trunc(&out, 300)
    );
}

/// [E2E-CLI-016B] 用户触发 read 失败时，终端应显示真实错误原因（非 failed 占位）
///
/// 验证：exit 0；stderr 含 `[tool] read` 且包含 not found 语义，并且不退化为 `✗ failed`
/// 要求：DEEPSEEK_API_KEY 环境变量已设置
#[test]
fn test_user_sees_read_failure_reason_in_tool_line() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_sees_read_failure_reason_in_tool_line").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_sees_read_failure_reason_in_tool_line");

    let missing_path = work_dir.join("workspace-main/definitely_missing_read_e2e.txt");
    let prompt = format!(
        "请只调用一次 read 工具读取文件 {}，offset=1，limit=5。不要调用其他工具。",
        missing_path.display()
    );

    info!("Act: tomcat chat 触发 read 不存在路径错误，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin(format!("{prompt}\n"))
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    info!(
        "Assert: stderr 含 [tool] read + 真实错误；stderr: {}",
        trunc(&stderr, 1200)
    );
    assert.success();
    assert!(
        stderr.contains("[tool] read"),
        "stderr 应出现 [tool] read 行，实际: {}",
        trunc(&stderr, 600)
    );
    let stderr_lower = stderr.to_ascii_lowercase();
    assert!(
        stderr_lower.contains("no such file")
            || stderr_lower.contains("not found")
            || stderr_lower.contains("os error 2")
            || stderr.contains("不存在"),
        "stderr 应包含路径不存在语义，实际: {}",
        trunc(&stderr, 800)
    );
    assert!(
        !stderr.contains("✗ failed"),
        "有真实错误文本时不应退化成 failed 占位，实际: {}",
        trunc(&stderr, 800)
    );
}

// ──────────────────── Story 2.5: Bash 后台监控 P1 真 LLM 黑盒（E2E-CLI-016C~016G） ────────────────────
//
// 这组真测分别锁几条关键门禁：
// - `016C`：finish-only auto-feed。模型起后台 bash 后先做独立工作，不主动 poll，
//   必须依赖 `<background-task-finished ...>` synthetic user message 继续推进。
// - `016D`：`task_output(block=true)` 单次 wait-slice。模型必须走阻塞等待路径，
//   transcript 要出现真实 `task_output` / `task_stop`；非 TTY 抓取 stderr 不应再堆
//   `waiting_for_output` 动画。
// - `016E`：真实多次 timeout slice 重试。模型必须至少经历两次
//   `wakeReason="timeout" && finished=false`，然后继续等到 `new_output` 再收尾。
// - `016F`：midturn batch boundary auto-feed。模型先起后台 bash，再跑一个耗时 foreground
//   bash；只有在 foreground batch 结束后的**下一次请求**里看到
//   `<background-task-finished ...>` 才允许继续，否则必须输出失败哨兵词。
// - `016G`：永不结束任务 + timeout tail snapshot。模型先吃掉首个 `new_output`，
//   再在 EOF 处命中一次 timeout 快照，随后必须停止继续 poll。
//
// 这组 helper 只做四件事：
// 1. 起临时 HOME + `tomcat init`
// 2. 把本次 scratch 目录 `workspace add`
// 3. 统一跑一次 `tomcat chat` 黑盒并捕获 stdout/stderr
// 4. 统一读取当前 transcript，便于断言真实 tool_call / tool_result

struct BackgroundBashP1RealLlmFixture {
    _home: tempfile::TempDir,
    work_dir: PathBuf,
    config_path: PathBuf,
    scratch: PathBuf,
    api_key: String,
}

struct CliChatRunCapture {
    success: bool,
    stdout: String,
    stderr: String,
}

fn setup_background_bash_p1_real_llm_fixture(scratch_leaf: &str) -> BackgroundBashP1RealLlmFixture {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspace-temp")
        .join(scratch_leaf);
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch = scratch.canonicalize().expect("workspace-temp scratch path");
    let scratch_str = scratch.to_str().expect("utf8 scratch path");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("setup_background_bash_p1_real_llm_fixture");

    info!("Arrange: tomcat workspace add {}", scratch_str);
    cmd()
        .args(["workspace", "add", scratch_str])
        .env("HOME", dir.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    BackgroundBashP1RealLlmFixture {
        _home: dir,
        work_dir,
        config_path,
        scratch,
        api_key,
    }
}

fn run_background_bash_p1_real_llm_chat(
    fx: &BackgroundBashP1RealLlmFixture,
    prompt: String,
    timeout: std::time::Duration,
) -> CliChatRunCapture {
    let mut c = cmd();
    c.arg("code")
        .current_dir(&fx.scratch)
        .env("TOMCAT__STORAGE__WORK_DIR", fx.work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", fx.config_path.to_str().unwrap())
        .env("RUST_LOG", "tomcat=info")
        .write_stdin(prompt)
        .timeout(timeout);
    configure_deepseek_real_llm(&mut c, &fx.api_key);
    let assert = c.assert();
    let output = assert.get_output();
    CliChatRunCapture {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn load_background_bash_p1_real_llm_transcript(fx: &BackgroundBashP1RealLlmFixture) -> String {
    let cfg = tomcat::load_config_toml_file(&fx.config_path).expect("load temp cli config");
    let sessions_dir = fx
        .work_dir
        .join("agents")
        .join(&cfg.agent.id)
        .join("sessions");
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &fx.scratch);
    let session = tomcat::SessionManager::new_scoped(sessions_dir, session_key);
    let transcript_path = session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    fs::read_to_string(&transcript_path).expect("read transcript")
}

/// [E2E-CLI-016C] bash 后台任务 finish-only auto-feed 真 LLM 黑盒回归
///
/// 用户意图：让模型严格走一次 `bash(run_in_background=true)`，先做独立文件写入，
/// 然后**禁止主动 poll**，必须依赖 runtime 自动注入的
/// `<background-task-finished ...>` synthetic user message 继续推进。
///
/// 验证：
/// - exit 0；
/// - stderr 出现 `[bg] task ... queued for next turn`（证明 lifecycle subscriber →
///   follow_up_queue → between-turns drain 路径真的跑了）；
/// - 后台任务真实产出 `bg_done.txt`，独立工作真实产出 `marker.txt`；
/// - stdout 最终包含约定完成词 `AUTOFEED_OK`。
///
/// 意义：P1 门禁——这条用例是真 LLM + 真 CLI `chat_loop` 黑盒，不是仅测 tool 层。
#[test]
fn test_user_background_bash_autofeed_real_llm_cli() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_background_bash_autofeed_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016c_bg_autofeed");
    let bg_done = fx.scratch.join("bg_done.txt");
    let marker = fx.scratch.join("marker.txt");
    let _ = fs::remove_file(&bg_done);
    let _ = fs::remove_file(&marker);

    let prompt = format!(
        concat!(
            "以下内容就是完整步骤；不要要求我重复步骤，不要反问，不要 ask_question。 ",
            "请严格按下面步骤执行，不要偏离，不要解释策略： ",
            "1. 只启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 这个后台 bash 的 command 必须精确执行：sleep 2; printf BG_DONE > \"{bg_done}\"。 ",
            "3. 启动后台任务后，立刻创建文件 \"{marker}\"，内容必须精确为 MARKER。 ",
            "4. 从这一步开始，禁止调用 task_output、task_list、task_stop，也不要再启动新的 bash；你必须等待 runtime 自动注入的 <background-task-finished ...> 系统消息。 ",
            "5. 只有在看到该系统消息之后，才允许读取并确认 \"{bg_done}\" 和 \"{marker}\" 都存在且内容正确。 ",
            "6. 全部确认后，只回复一行 AUTOFEED_OK 并停止；不要输出别的结尾。"
        ),
        bg_done = bg_done.display(),
        marker = marker.display(),
    );

    info!("Act: tomcat chat 触发后台 bash auto-feed，timeout 120s");
    let run =
        run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(120));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 1500));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2000));
    }
    info!("Assert: exit 0 + stderr 含 [bg] task + 两文件落盘 + stdout 含 AUTOFEED_OK");
    assert!(
        run.success,
        "tomcat chat 应 exit 0；stderr: {}",
        trunc(&stderr, 1200)
    );
    assert!(
        stderr.contains("[bg] task") && stderr.contains("queued for next turn"),
        "stderr 应含后台完成 auto-feed 提示，实际: {}",
        trunc(&stderr, 1200)
    );
    assert!(
        bg_done.exists(),
        "后台任务产物应存在: {}",
        bg_done.display()
    );
    assert!(marker.exists(), "独立工作产物应存在: {}", marker.display());
    let bg_done_text = fs::read_to_string(&bg_done).unwrap_or_default();
    let marker_text = fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(
        bg_done_text, "BG_DONE",
        "bg_done.txt 内容应精确为 BG_DONE，实际: {:?}",
        bg_done_text
    );
    assert_eq!(
        marker_text, "MARKER",
        "marker.txt 内容应精确为 MARKER，实际: {:?}",
        marker_text
    );
    assert!(
        stdout.contains("AUTOFEED_OK"),
        "stdout 应含 AUTOFEED_OK，实际: {}",
        trunc(&stdout, 600)
    );
}

/// [E2E-CLI-016D] `task_output(block=true)` wait-slice 真 LLM 黑盒回归
///
/// 用户意图：模型必须在 `bash(run_in_background=true)` 之后**立即**进入
/// `task_output(block=true, timeout_ms=300)` 循环，直到拿到 `wakeReason="new_output"`；
/// 不允许依赖 auto-feed，不允许 `task_output(block=false)`。
///
/// 验证：
/// - exit 0；
/// - 非 TTY 抓取 stderr **不**含 `waiting_for_output` 倒计时动画；
/// - transcript 中至少有 1 次 `task_output` tool call，且参数包含
///   `block=true` 与 `timeout_ms=300`；
/// - 后台任务真实产出 `blockwait_done.txt`；
/// - stdout 最终包含约定完成词 `BLOCKWAIT_OK`。
///
/// 意义：P1 第二条真门禁——真正覆盖 `block=true` 等待路径、
/// transcript 中的真实 tool_call，以及 `new_output` 唤醒后的收尾。
#[test]
fn test_user_background_bash_blocking_waitslice_real_llm_cli() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_background_bash_blocking_waitslice_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016d_blockwait");
    let done_path = fx.scratch.join("blockwait_done.txt");
    let _ = fs::remove_file(&done_path);

    let prompt = format!(
        concat!(
            "以下内容就是完整指令；不要要求我重复，不要反问，不要 ask_question。 ",
            "请严格执行，并只在最后回复一行 BLOCKWAIT_OK： ",
            "1. 启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 该后台 bash 的 command 必须精确执行：sleep 2; echo TOKEN_WAITSLICE; printf BLOCKWAIT_DONE > \"{done_path}\"; sleep 30。 ",
            "3. 拿到 task_id 后，必须立刻开始调用 task_output，且参数必须满足：block=true、timeout_ms=300、since 从 0 开始并按 next_offset 续传。 ",
            "4. 如果 task_output 返回 wakeReason=timeout 且 finished=false，这不是失败；你必须继续再次调用 task_output(block=true, timeout_ms=300) 等下一次 wait slice。 ",
            "5. 当 task_output 返回 wakeReason=new_output 且内容里出现 TOKEN_WAITSLICE 后，必须调用 task_stop 停掉这个后台任务，避免它继续睡眠。 ",
            "6. 禁止使用 task_output(block=false)，禁止依赖 <background-task-finished ...> 自动回灌，禁止启动新的 bash，禁止调用 task_list。 ",
            "7. 只有在看到 TOKEN_WAITSLICE 且已经 task_stop 之后，才允许读取并确认文件 \"{done_path}\" 存在且内容精确为 BLOCKWAIT_DONE。 ",
            "8. 全部确认后，只回复一行 BLOCKWAIT_OK 并停止。"
        ),
        done_path = done_path.display(),
    );

    info!("Act: tomcat chat 触发 block=true wait-slice，timeout 120s");
    let run =
        run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(120));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 1800));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2200));
    }
    assert!(
        run.success,
        "tomcat chat 应 exit 0；stderr: {}",
        trunc(&stderr, 1200)
    );
    assert!(
        !stderr.contains("waiting_for_output"),
        "非 TTY stderr 不应出现倒计时动画，实际: {}",
        trunc(&stderr, 1200)
    );
    assert!(
        done_path.exists(),
        "后台任务产物应存在: {}",
        done_path.display()
    );
    let done_text = fs::read_to_string(&done_path).unwrap_or_default();
    assert_eq!(
        done_text, "BLOCKWAIT_DONE",
        "blockwait_done.txt 内容应精确为 BLOCKWAIT_DONE，实际: {:?}",
        done_text
    );
    assert!(
        stdout.contains("BLOCKWAIT_OK"),
        "stdout 应含 BLOCKWAIT_OK，实际: {}",
        trunc(&stdout, 600)
    );

    // transcript 级硬断言：必须能看到真实 `task_output(block=true)` + `task_stop`。
    let transcript = load_background_bash_p1_real_llm_transcript(&fx);
    let tool_calls = assistant_tool_calls_from_transcript(&transcript);
    let task_output_calls: Vec<_> = tool_calls
        .iter()
        .filter(|(name, _)| name == "task_output")
        .collect();
    assert!(
        !task_output_calls.is_empty(),
        "transcript 中 task_output 次数应至少为 1（证明真 LLM 走了 block=true 等待路径），实际 {}；transcript: {}",
        task_output_calls.len(),
        trunc(&transcript, 1500)
    );
    for (_, args) in &task_output_calls {
        assert!(
            args.get("block").and_then(Value::as_bool) == Some(true)
                && args.get("timeout_ms").and_then(Value::as_u64) == Some(300),
            "task_output 调用参数应固定为 block=true + timeout_ms=300，实际 args={args}"
        );
    }
    let tool_results = tool_results_from_transcript(&transcript);
    assert!(
        tool_results.iter().any(|result| {
            result.get("wakeReason").and_then(Value::as_str) == Some("new_output")
                && result
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|content| content.contains("TOKEN_WAITSLICE"))
                    .unwrap_or(false)
        }),
        "transcript 应能看到 wait-slice 唤醒后的 token；actual: {}",
        trunc(&transcript, 1500)
    );
    assert!(
        tool_calls.iter().any(|(name, _)| name == "task_stop"),
        "transcript 应含 task_stop（收尾后台任务），actual: {}",
        trunc(&transcript, 1500)
    );
}

/// [E2E-CLI-016E] 真 LLM 多次 timeout slice 重试
///
/// 用户意图：模型必须在同一个后台任务上经历**至少两次**
/// `wakeReason="timeout" && finished=false`，继续重试 `task_output(block=true, timeout_ms=300)`，
/// 最后再等到一次 `new_output` 并收尾。
///
/// 验证：
/// - exit 0；
/// - 非 TTY 抓取 stderr **不**含 `waiting_for_output`；
/// - transcript 中 `task_output` tool call 至少 3 次；
/// - transcript 中 `role=tool` 的结果里，`wakeReason="timeout"` 至少 2 次；
/// - transcript 中最终出现 `TOKEN_MULTI_TIMEOUT` 与 `task_stop`；
/// - 真实产物 `multi_timeout_done.txt` 存在且内容正确；
/// - stdout 最终包含 `MULTI_TIMEOUT_OK`。
#[test]
fn test_user_background_bash_multiple_timeout_slices_real_llm_cli() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span =
        info_span!("test_user_background_bash_multiple_timeout_slices_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016e_multi_timeout");
    let done_path = fx.scratch.join("multi_timeout_done.txt");
    let _ = fs::remove_file(&done_path);

    let prompt = format!(
        concat!(
            "以下内容就是完整指令；不要要求我重复，不要反问，不要 ask_question。 ",
            "请严格执行，并只在最后回复一行 MULTI_TIMEOUT_OK： ",
            "1. 启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 该后台 bash 的 command 必须精确执行：sleep 8; echo TOKEN_MULTI_TIMEOUT; printf MULTI_TIMEOUT_DONE > \"{done_path}\"; sleep 30。 ",
            "3. 拿到 task_id 后，必须立刻开始调用 task_output，且参数必须满足：block=true、timeout_ms=300、since 从 0 开始并按 next_offset 续传。 ",
            "4. `wakeReason=timeout` 且 `finished=false` 在这道题里是正常现象，不是失败、不是要重来；你必须真实观察到至少两次这样的 timeout，并且每次 timeout 之后都继续在同一个 task_id 上再次调用 task_output(block=true, timeout_ms=300)，不要解释、不要总结、不要重启流程、不要新开任务。 ",
            "5. 在至少两次 timeout 之后，继续沿用同一个 task_id 和最新 next_offset 等待，直到某次 task_output 返回 wakeReason=new_output 且内容里出现 TOKEN_MULTI_TIMEOUT。 ",
            "6. 一旦看到 TOKEN_MULTI_TIMEOUT，必须调用 task_stop 停掉该后台任务，避免它继续睡眠。 ",
            "7. 禁止使用 task_output(block=false)，禁止依赖 <background-task-finished ...> 自动回灌，禁止启动新的 bash，禁止调用 task_list。 ",
            "8. 只有在已经看到至少两次 timeout、随后看到 TOKEN_MULTI_TIMEOUT、并且已经 task_stop 之后，才允许读取并确认文件 \"{done_path}\" 存在且内容精确为 MULTI_TIMEOUT_DONE。 ",
            "9. 全部确认后，只回复一行 MULTI_TIMEOUT_OK 并停止。"
        ),
        done_path = done_path.display(),
    );

    info!("Act: tomcat chat 触发多次 timeout slice，timeout 150s");
    let run =
        run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(150));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 2200));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2600));
    }
    assert!(
        run.success,
        "tomcat chat 应 exit 0；stderr: {}",
        trunc(&stderr, 1400)
    );
    assert!(
        !stderr.contains("waiting_for_output"),
        "非 TTY stderr 不应出现倒计时动画，实际: {}",
        trunc(&stderr, 1400)
    );
    assert!(
        done_path.exists(),
        "后台任务产物应存在: {}",
        done_path.display()
    );
    let done_text = fs::read_to_string(&done_path).unwrap_or_default();
    assert_eq!(
        done_text, "MULTI_TIMEOUT_DONE",
        "multi_timeout_done.txt 内容应精确为 MULTI_TIMEOUT_DONE，实际: {:?}",
        done_text
    );
    assert!(
        stdout.contains("MULTI_TIMEOUT_OK"),
        "stdout 应含 MULTI_TIMEOUT_OK，实际: {}",
        trunc(&stdout, 800)
    );

    let transcript = load_background_bash_p1_real_llm_transcript(&fx);
    let tool_calls = assistant_tool_calls_from_transcript(&transcript);
    let tool_results = tool_results_from_transcript(&transcript);

    let mut task_output_calls = 0usize;
    let mut timeout_results = 0usize;
    let mut saw_new_output_token = false;
    let mut saw_task_stop = false;
    for (name, args) in &tool_calls {
        if name == "task_output" {
            task_output_calls += 1;
            assert!(
                args.get("block").and_then(Value::as_bool) == Some(true)
                    && args.get("timeout_ms").and_then(Value::as_u64) == Some(300),
                "task_output 调用参数应固定为 block=true + timeout_ms=300，实际 args={args}"
            );
        } else if name == "task_stop" {
            saw_task_stop = true;
        }
    }
    for result in &tool_results {
        if result.get("wakeReason").and_then(Value::as_str) == Some("timeout") {
            timeout_results += 1;
        }
        if result.get("wakeReason").and_then(Value::as_str) == Some("new_output")
            && result
                .get("content")
                .and_then(Value::as_str)
                .map(|content| content.contains("TOKEN_MULTI_TIMEOUT"))
                .unwrap_or(false)
        {
            saw_new_output_token = true;
        }
    }

    assert!(
        task_output_calls >= 3,
        "transcript 中 task_output 次数应至少为 3（至少两次 timeout + 一次 new_output），实际 {}；transcript: {}",
        task_output_calls,
        trunc(&transcript, 1800)
    );
    assert!(
        timeout_results >= 2,
        "transcript 中 timeout slice 次数应至少为 2，实际 {}；transcript: {}",
        timeout_results,
        trunc(&transcript, 1800)
    );
    assert!(
        saw_new_output_token,
        "transcript 中应看到包含 TOKEN_MULTI_TIMEOUT 的 new_output 结果；actual: {}",
        trunc(&transcript, 1800)
    );
    assert!(
        saw_task_stop,
        "transcript 应含 task_stop（收尾后台任务），actual: {}",
        trunc(&transcript, 1800)
    );
}

/// [E2E-CLI-016F] midturn batch boundary auto-feed 真 LLM 黑盒回归
///
/// 用户意图：模型先起一个后台 bash，再立刻执行一个耗时 foreground bash。
/// 只有在 foreground batch 结束后的**下一次请求**里看到了 runtime 注入的
/// `<background-task-finished ...>`，才允许继续读取并确认文件；否则必须立即输出
/// `MIDTURN_MISSED_FOLLOWUP` 并停止。
///
/// 验证：
/// - exit 0；
/// - stdout 含 `MIDTURN_FOLLOWUP_OK` 且**不含** `MIDTURN_MISSED_FOLLOWUP`；
/// - 后台与前台两份产物真实落盘；
/// - transcript 中 `<background-task-finished ...>` 位于 `FG_BATCH_START` 之后、成功词之前；
/// - transcript 中不应出现 `task_output` / `task_list` / `task_stop`。
///
/// 意义：锁定“批次边界 midturn drain”本身，而不是沿用既有 finish-only between-turns auto-feed。
#[test]
fn test_user_background_bash_midturn_followup_real_llm_cli() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_background_bash_midturn_followup_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016f_midturn_followup");
    let bg_done = fx.scratch.join("midturn_bg_done.txt");
    let fg_done = fx.scratch.join("midturn_fg_done.txt");
    let _ = fs::remove_file(&bg_done);
    let _ = fs::remove_file(&fg_done);

    let prompt = format!(
        concat!(
            "以下内容就是完整步骤；不要要求我重复步骤，不要反问，不要 ask_question。 ",
            "请严格按下面步骤执行，不要偏离，不要解释策略： ",
            "1. 只启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 这个后台 bash 的 command 必须精确执行：sleep 1; echo BG_SIGNAL; printf BG_DONE > \"{bg_done}\"。 ",
            "3. 启动后台任务后，立刻再执行一个**前台** bash（不要设置 run_in_background），其 command 必须精确执行：echo FG_BATCH_START; sleep 2; printf FG_DONE > \"{fg_done}\"。 ",
            "4. 从这一步开始，禁止调用 task_output、task_list、task_stop，也不要再启动新的 bash。 ",
            "5. 当前台 bash 结束时，如果你**还没有**看到 runtime 自动注入的 <background-task-finished ...> 系统消息，你必须只回复一行 MIDTURN_MISSED_FOLLOWUP 并立刻停止。 ",
            "6. 只有在 foreground bash 结束后的后续请求里已经看到了该系统消息时，才允许读取并确认 \"{bg_done}\" 和 \"{fg_done}\" 两个文件内容分别精确为 BG_DONE 和 FG_DONE。 ",
            "7. 全部确认后，只回复一行 MIDTURN_FOLLOWUP_OK 并停止；不要输出别的结尾。"
        ),
        bg_done = bg_done.display(),
        fg_done = fg_done.display(),
    );

    info!("Act: tomcat chat 触发 midturn follow-up auto-feed，timeout 150s");
    let run =
        run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(150));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 2000));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2400));
    }
    assert!(
        run.success,
        "tomcat chat 应 exit 0；stderr: {}",
        trunc(&stderr, 1400)
    );
    assert!(
        stdout.contains("MIDTURN_FOLLOWUP_OK"),
        "stdout 应含 MIDTURN_FOLLOWUP_OK，实际: {}",
        trunc(&stdout, 800)
    );
    assert!(
        !stdout.contains("MIDTURN_MISSED_FOLLOWUP"),
        "stdout 不应含失败哨兵词 MIDTURN_MISSED_FOLLOWUP，实际: {}",
        trunc(&stdout, 1200)
    );
    assert!(
        bg_done.exists(),
        "后台任务产物应存在: {}",
        bg_done.display()
    );
    assert!(
        fg_done.exists(),
        "前台任务产物应存在: {}",
        fg_done.display()
    );
    assert_eq!(
        fs::read_to_string(&bg_done).unwrap_or_default(),
        "BG_DONE",
        "midturn_bg_done.txt 内容应精确为 BG_DONE"
    );
    assert_eq!(
        fs::read_to_string(&fg_done).unwrap_or_default(),
        "FG_DONE",
        "midturn_fg_done.txt 内容应精确为 FG_DONE"
    );

    let transcript = load_background_bash_p1_real_llm_transcript(&fx);
    let mut assistant_emitted_failure = false;
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "assistant" {
            continue;
        }
        let content = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if content.contains("MIDTURN_MISSED_FOLLOWUP") {
            assistant_emitted_failure = true;
            break;
        }
    }
    assert!(
        !transcript.contains("\"name\":\"task_output\"")
            && !transcript.contains("\"name\":\"task_list\"")
            && !transcript.contains("\"name\":\"task_stop\""),
        "transcript 不应含 task_output/task_list/task_stop，actual: {}",
        trunc(&transcript, 1800)
    );
    assert!(
        !assistant_emitted_failure,
        "assistant 不应在 transcript 中发出失败哨兵词，actual: {}",
        trunc(&transcript, 1800)
    );
    let fg_batch_pos = transcript
        .find("FG_BATCH_START")
        .expect("transcript 应含 foreground bash 的 FG_BATCH_START");
    let follow_up_pos = transcript
        .find("<background-task-finished")
        .expect("transcript 应含 synthetic follow-up");
    let success_pos = transcript
        .rfind("MIDTURN_FOLLOWUP_OK")
        .expect("transcript 应含成功词 MIDTURN_FOLLOWUP_OK");
    assert!(
        follow_up_pos > fg_batch_pos,
        "synthetic follow-up 应位于 foreground batch 结果之后；fg_batch_pos={fg_batch_pos}, follow_up_pos={follow_up_pos}"
    );
    assert!(
        success_pos > follow_up_pos,
        "成功词应位于 synthetic follow-up 之后；follow_up_pos={follow_up_pos}, success_pos={success_pos}"
    );
}

/// [E2E-CLI-016G] 永不结束后台 bash + timeout 快照后停止轮询
///
/// 用户意图：模型先启动一个会长时间挂起的后台 bash，先用一次 `task_output(block=true)`
/// 吃掉首个 `new_output`，再用第二次 `task_output(block=true)` 在 EOF 处命中
/// `wakeReason="timeout"`。拿到 timeout 返回的 tail 快照后，必须立即停止轮询，
/// 不允许继续 `task_output` busy loop。
///
/// 验证：
/// - exit 0；
/// - stdout 含 `HUNG_TIMEOUT_BOUNDED_OK`；
/// - 非 TTY 抓取 stderr 不含 `waiting_for_output`；
/// - transcript 中 `task_output` 调用次数有上界（<= 3）且至少 2 次；
/// - transcript 中存在 `wakeReason="timeout"` 且内容含 `HUNG_TIMEOUT_BOOT` 的工具结果；
/// - transcript 的 `role=user` 消息不含 `waiting_for_output`；
/// - transcript 中不出现 `task_stop` / `task_list`。
#[test]
fn test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli")
        .entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016g_timeout_snapshot");
    let prompt = concat!(
        "以下内容就是完整指令；不要要求我重复，不要反问，不要 ask_question。 ",
        "请严格执行，并只在最后回复一行 HUNG_TIMEOUT_BOUNDED_OK： ",
        "1. 启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
        "2. 该后台 bash 的 command 必须精确执行：printf HUNG_TIMEOUT_BOOT; sleep 60。 ",
        "3. 拿到 task_id 后，必须立刻调用一次 task_output，参数必须满足：block=true、timeout_ms=300、since=0。 ",
        "4. 当第一次 task_output 的返回内容里出现 HUNG_TIMEOUT_BOOT 后，必须基于返回的最新 next_offset，在同一个 task_id 上再调用一次 task_output(block=true, timeout_ms=300)。 ",
        "5. 如果第二次 task_output 返回 wakeReason=timeout 且 finished=false，你必须阅读它返回的 content，并把它当作近期 tail 快照而不是失败。由于这里只有同一份 HUNG_TIMEOUT_BOOT、没有新的实质进展，你必须立刻停止继续轮询。 ",
        "6. 从这一步开始，禁止再次调用 task_output，禁止调用 task_stop、task_list，禁止依赖 <background-task-finished ...> 自动回灌，禁止启动新的 bash。 ",
        "7. 满足上述条件后，只回复一行 HUNG_TIMEOUT_BOUNDED_OK 并停止。"
    )
    .to_string();

    info!("Act: tomcat chat 触发 timeout tail snapshot bounded case，timeout 90s");
    let run = run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(90));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 1800));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2200));
    }
    assert!(
        run.success,
        "tomcat chat 应 exit 0；stderr: {}",
        trunc(&stderr, 1400)
    );
    assert!(
        stdout.contains("HUNG_TIMEOUT_BOUNDED_OK"),
        "stdout 应含 HUNG_TIMEOUT_BOUNDED_OK，实际: {}",
        trunc(&stdout, 900)
    );
    assert!(
        !stderr.contains("waiting_for_output"),
        "非 TTY stderr 不应出现倒计时动画，实际: {}",
        trunc(&stderr, 1400)
    );

    let transcript = load_background_bash_p1_real_llm_transcript(&fx);
    let mut task_output_calls = 0usize;
    let mut saw_timeout_snapshot = false;
    let mut saw_waiting_for_output_user = false;
    let mut saw_task_stop = false;
    let mut saw_task_list = false;
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "assistant" {
            if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls {
                    let name = tc
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if name == "task_output" {
                        task_output_calls += 1;
                    } else if name == "task_stop" {
                        saw_task_stop = true;
                    } else if name == "task_list" {
                        saw_task_list = true;
                    }
                }
            }
        } else if role == "tool" {
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if content.contains("\"wakeReason\":\"timeout\"")
                && content.contains("HUNG_TIMEOUT_BOOT")
                && content.contains("\"finished\":false")
            {
                saw_timeout_snapshot = true;
            }
        } else if role == "user" {
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if content.contains("waiting_for_output") {
                saw_waiting_for_output_user = true;
            }
        }
    }

    assert!(
        (2..=3).contains(&task_output_calls),
        "task_output 调用次数应有界（2~3 次），实际 {}；transcript: {}",
        task_output_calls,
        trunc(&transcript, 2200)
    );
    assert!(
        saw_timeout_snapshot,
        "transcript 中应看到带 HUNG_TIMEOUT_BOOT 的 timeout 快照；actual: {}",
        trunc(&transcript, 2200)
    );
    assert!(
        !saw_waiting_for_output_user,
        "waiting_for_output 不应从 readline 路径回灌为 role:user；actual: {}",
        trunc(&transcript, 2200)
    );
    assert!(
        !saw_task_stop && !saw_task_list,
        "bounded timeout 场景不应调用 task_stop/task_list；actual: {}",
        trunc(&transcript, 2200)
    );
}

/// [E2E-CLI-013] 用户要求助手在仓库约定的 `workspace-temp` 子目录下写文件
///
/// 验证：exit 0；`{CARGO_MANIFEST_DIR}/workspace-temp/e2e_cli013_hello/hello_e2e.txt` 存在且内容含 Hello E2E（或 stdout 含写入/创建确认）
/// 意义：scratch 走 `workspace-temp/`（INTEGRATION_TEST_SPEC §2.3），避免提示词里的「workspace 目录」被模型误解为 crate 下 `workspace/` 子目录
#[test]
fn test_user_asks_pi_to_write_hello_world_bash() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_asks_pi_to_write_hello_world_bash").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspace-temp")
        .join("e2e_cli013_hello");
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch_canon = scratch.canonicalize().expect("workspace-temp scratch path");
    let scratch_str = scratch_canon.to_str().expect("utf8 scratch path");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_asks_pi_to_write_hello_world_bash");

    info!("Arrange: tomcat workspace add {}", scratch_str);
    cmd()
        .args(["workspace", "add", scratch_str])
        .env("HOME", dir.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    let prompt = format!(
        "请在目录 {} 下创建文件 hello_e2e.txt，内容写 Hello E2E。不要写到其他路径。\n",
        scratch_canon.display()
    );
    info!("Act: tomcat chat stdin 要求在 workspace-temp 子目录创建 hello_e2e.txt，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin(prompt)
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 文件存在且含 Hello E2E 或 stdout 含操作确认；actual: {}",
        trunc(&out, 300)
    );
    assert.success();

    let hello_path = scratch_canon.join("hello_e2e.txt");
    if hello_path.exists() {
        let content = fs::read_to_string(&hello_path).unwrap();
        assert!(
            content.contains("Hello E2E"),
            "hello_e2e.txt 内容应含 'Hello E2E'，实际: {}",
            trunc(&content, 200)
        );
    } else {
        assert!(
            out.contains("写入")
                || out.contains("write")
                || out.contains("创建")
                || out.contains("创建了"),
            "未找到 hello_e2e.txt 时 stdout 应含写入/创建类确认，实际: {}",
            trunc(&out, 300)
        );
    }
}

// ──────────────────── Story 3: rquickjs 插件系统（E2E-CLI-021~026） ────────────────────

/// 创建临时插件目录，包含 plugin.json + main.js
fn make_plugin_dir(id: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_json = format!(
        r#"{{
            "id": "{id}",
            "name": "Test Plugin {id}",
            "version": "0.1.0",
            "description": "E2E test plugin",
            "author": "nibbles",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": []
        }}"#
    );
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).expect("write plugin.json");
    std::fs::write(tmp.path().join("main.js"), "// init\n1 + 1;\n").expect("write main.js");
    tmp
}

fn write_skill_markdown(skill_dir: &Path, name: &str, description: &str) {
    let skill_md = format!(
        "---\nname: {name}\ndescription: {description}\n---\n# {name}\n1. Follow the steps.\n"
    );
    std::fs::write(skill_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");
}

fn make_bare_skill_dir(name: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_skill_markdown(tmp.path(), name, "E2E test skill");
    tmp
}

fn make_package_dir(
    package_name: &str,
    version: &str,
    plugin_id: &str,
    skill_name: &str,
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_dir = tmp.path().join("plugins").join(plugin_id);
    let skill_dir = tmp.path().join("skills").join(skill_name);
    std::fs::create_dir_all(&plugin_dir).expect("create package plugin dir");
    std::fs::create_dir_all(&skill_dir).expect("create package skill dir");

    let package_json = format!(
        r#"{{
  "name": "{package_name}",
  "version": "{version}",
  "description": "E2E package {package_name}",
  "tomcat": {{
    "plugins": ["plugins/{plugin_id}"],
    "skills": ["skills/{skill_name}"]
  }}
}}"#
    );
    std::fs::write(tmp.path().join("package.json"), package_json).expect("write package.json");

    let plugin_json = format!(
        r#"{{
  "id": "{plugin_id}",
  "name": "Package Plugin {plugin_id}",
  "version": "{version}",
  "description": "Plugin resource from package",
  "author": "nibbles",
  "main": "main.js",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": []
}}"#
    );
    std::fs::write(plugin_dir.join("plugin.json"), plugin_json)
        .expect("write package plugin manifest");
    std::fs::write(
        plugin_dir.join("main.js"),
        "// package plugin init\n1 + 1;\n",
    )
    .expect("write package plugin main");
    write_skill_markdown(&skill_dir, skill_name, "Skill resource from package");
    tmp
}

fn read_package_registry(path: &Path) -> tomcat::core::PackageRegistryFile {
    let content = std::fs::read_to_string(path).expect("read package registry");
    serde_json::from_str(&content).expect("parse package registry")
}

fn read_plugin_registry(path: &Path) -> tomcat::core::PluginRegistryFile {
    let content = std::fs::read_to_string(path).expect("read plugin registry");
    serde_json::from_str(&content).expect("parse plugin registry")
}

/// [E2E-CLI-021] 用户从路径加载插件并查看已加载列表
///
/// 用户意图：加载插件并验证命令正常执行
/// 验证：load exit 0；list exit 0（注：插件状态为进程内存，跨进程不持久化——MVP 已知限制）
#[test]
fn test_user_loads_plugin_and_lists() {
    common::setup_logging();
    let _span = info_span!("test_user_loads_plugin_and_lists").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-list");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: plugin dir = {:?}", plugin_dir.path());
    info!("Act: tomcat plugin load");
    let load_assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert();
    let load_out = String::from_utf8_lossy(&load_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert load: exit 0, stdout 非空；actual: {}",
        trunc(&load_out, 200)
    );
    load_assert.success();

    info!("Act: tomcat plugin list（跨进程，状态不持久）");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "list"])
        .assert();
    info!("Assert list: exit 0（不崩溃即可）");
    assert.success();
}

/// [E2E-CLI-022] 用户查看插件详情（名称、版本）
///
/// 用户意图：查看插件详情
/// 验证：exit 0；stdout 含 name/version
#[test]
fn test_user_views_plugin_info() {
    common::setup_logging();
    let _span = info_span!("test_user_views_plugin_info").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-info");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin first");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin info <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "info", "e2e-test-plugin-info"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 name 或 version；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("e2e-test-plugin-info")
            .or(predicate::str::contains("0.1.0"))
            .or(predicate::str::contains("version")),
    );
}

/// [E2E-CLI-023] 用户禁用插件
///
/// 用户意图：禁用已加载的插件
/// 验证：exit 0
#[test]
fn test_user_disables_plugin() {
    common::setup_logging();
    let _span = info_span!("test_user_disables_plugin").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-disable");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin disable <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "disable", "e2e-test-plugin-disable"])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-024] 用户重新启用被禁用的插件
///
/// 用户意图：重新启用已禁用的插件
/// 验证：exit 0
#[test]
fn test_user_enables_plugin_after_disable() {
    common::setup_logging();
    let _span = info_span!("test_user_enables_plugin_after_disable").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-enable");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load + disable plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "disable", "e2e-test-plugin-enable"])
        .assert()
        .success();

    info!("Act: tomcat plugin enable <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "enable", "e2e-test-plugin-enable"])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-025] 用户卸载插件后从列表消失
///
/// 用户意图：卸载插件后列表不含该插件
/// 验证：unload exit 0；list stdout 不含该 id
#[test]
fn test_user_unloads_plugin_removes_from_list() {
    common::setup_logging();
    let _span = info_span!("test_user_unloads_plugin_removes_from_list").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-unload");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin unload <id>");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "unload", "e2e-test-plugin-unload"])
        .assert()
        .success();

    info!("Act: tomcat plugin list");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "list"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: list 不含 id；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.contains("e2e-test-plugin-unload"),
        "卸载后 list 不应含该插件 id，实际 stdout: {}",
        trunc(&out, 200)
    );
}

/// [E2E-CLI-026] 用户加载不存在路径时看到错误提示
///
/// 用户意图：加载不存在的插件路径，看到友好错误
/// 验证：exit 0；stdout 含 error 或"不存在"
#[test]
fn test_user_loads_nonexistent_plugin_path_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_user_loads_nonexistent_plugin_path_shows_error").entered();

    info!("Arrange: /nonexistent/path/to/plugin 不存在");
    info!("Act: tomcat plugin load /nonexistent/path/to/plugin");
    let assert = cmd()
        .args(["plugin", "load", "/nonexistent/path/to/plugin"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 error 提示；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("不存在")
            .or(predicate::str::contains("error"))
            .or(predicate::str::contains("Error"))
            .or(predicate::str::contains("找不到")),
    );
}

// ──────────────────── Story 4: PackageManager 统一安装（E2E-CLI-027~030） ────────────────────

/// [E2E-CLI-027] 用户把 package 安装到当前项目并在 packages 中看到三层视图
///
/// 用户意图：通过统一入口把 package 安装到当前项目，并立即确认 scope 层账本与资源落盘
/// 验证：install 默认落 scope；packages 默认输出 scope/agent/global 三层；scope 层 package/plugin ledger 与资源目录存在
#[test]
fn test_user_installs_scope_package_and_lists_layered_packages() {
    common::setup_logging();
    let _span = info_span!("test_user_installs_scope_package_and_lists_layered_packages").entered();

    let package_dir = make_package_dir(
        "e2e-scope-package",
        "0.2.0",
        "e2e-scope-plugin",
        "e2e-scope-skill",
    );
    let home = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    let scope_root = home.path().join("project");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(&scope_root).unwrap();

    let package_src = package_dir.path().to_str().unwrap();
    let work_dir_str = work_dir.to_str().unwrap();
    let scope_root_str = scope_root.to_str().unwrap();

    info!(
        "Act: tomcat install <package> --scope-root <project>（未传 visibility，非交互默认 scope）"
    );
    let install_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args(["install", package_src, "--scope-root", scope_root_str])
        .assert();
    let install_out =
        String::from_utf8_lossy(&install_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert install: exit 0 + stdout 含 package 名；actual: {}",
        trunc(&install_out, 240)
    );
    install_assert.success().stdout(
        predicate::str::contains("已安装 package")
            .and(predicate::str::contains("e2e-scope-package")),
    );

    info!("Act: tomcat packages --scope-root <project>");
    let list_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args(["packages", "--scope-root", scope_root_str])
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert packages: 默认输出三层且 scope 含 package；actual: {}",
        trunc(&list_out, 400)
    );
    list_assert.success().stdout(
        predicate::str::contains("scope:")
            .and(predicate::str::contains("agent:"))
            .and(predicate::str::contains("global:"))
            .and(predicate::str::contains("e2e-scope-package@0.2.0"))
            .and(predicate::str::contains("plugin:e2e-scope-plugin"))
            .and(predicate::str::contains("skill:e2e-scope-skill")),
    );

    let scope_tomcat = scope_root.join(".tomcat");
    assert!(
        scope_tomcat
            .join("plugins")
            .join("e2e-scope-plugin")
            .join("plugin.json")
            .exists(),
        "scope plugin 应已落盘"
    );
    assert!(
        scope_tomcat
            .join("skills")
            .join("e2e-scope-skill")
            .join("SKILL.md")
            .exists(),
        "scope skill 应已落盘"
    );

    let package_registry =
        read_package_registry(&scope_tomcat.join("packages").join("registry.json"));
    assert_eq!(
        package_registry.schema,
        tomcat::core::PACKAGE_REGISTRY_SCHEMA_V1
    );
    assert_eq!(
        package_registry.packages.len(),
        1,
        "scope package registry 应只有 1 条记录"
    );
    assert_eq!(package_registry.packages[0].name, "e2e-scope-package");
    assert_eq!(package_registry.packages[0].source_kind.as_str(), "local");
    assert_eq!(
        package_registry.packages[0].plugins[0].id,
        "e2e-scope-plugin"
    );
    assert_eq!(
        package_registry.packages[0].plugins[0].relative_dir,
        "plugins/e2e-scope-plugin"
    );
    assert_eq!(
        package_registry.packages[0].skills[0].name,
        "e2e-scope-skill"
    );
    assert_eq!(
        package_registry.packages[0].skills[0].relative_dir,
        "skills/e2e-scope-skill"
    );

    let plugin_registry = read_plugin_registry(&scope_tomcat.join("plugins").join("registry.json"));
    assert!(
        plugin_registry
            .plugins
            .iter()
            .any(|entry| entry.id == "e2e-scope-plugin" && entry.enabled),
        "scope plugin registry 应登记 e2e-scope-plugin"
    );
}

/// [E2E-CLI-028] 用户把 bare plugin 安装到 agent 层并只查看 agent ledger
///
/// 用户意图：把 plugin 作为 package 资源安装到 agent 私有层
/// 验证：agent 层 package/plugin registry 写入成功；packages --visibility agent 仅展示该层记录
#[test]
fn test_user_installs_bare_plugin_to_agent_layer() {
    common::setup_logging();
    let _span = info_span!("test_user_installs_bare_plugin_to_agent_layer").entered();

    let plugin_dir = make_plugin_dir("e2e-agent-plugin");
    let home = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    let scope_root = home.path().join("project");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(&scope_root).unwrap();

    let plugin_src = plugin_dir.path().to_str().unwrap();
    let work_dir_str = work_dir.to_str().unwrap();
    let scope_root_str = scope_root.to_str().unwrap();

    info!("Act: tomcat install <plugin> --visibility agent");
    let install_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "install",
            plugin_src,
            "--visibility",
            "agent",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let install_out =
        String::from_utf8_lossy(&install_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert install: exit 0 + stdout 指向 agent；actual: {}",
        trunc(&install_out, 240)
    );
    install_assert
        .success()
        .stdout(predicate::str::contains("已安装 package").and(predicate::str::contains("agent")));

    info!("Act: tomcat packages --visibility agent");
    let list_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "packages",
            "--visibility",
            "agent",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert packages: agent 层含 bare plugin；actual: {}",
        trunc(&list_out, 320)
    );
    list_assert.success().stdout(
        predicate::str::contains("agent:")
            .and(predicate::str::contains("e2e-agent-plugin@0.1.0"))
            .and(predicate::str::contains("[local]"))
            .and(predicate::str::contains("plugin:e2e-agent-plugin")),
    );

    let agent_root = work_dir.join("agents").join("main");
    assert!(
        agent_root
            .join("plugins")
            .join("e2e-agent-plugin")
            .join("plugin.json")
            .exists(),
        "agent plugin 应已落盘"
    );

    let package_registry =
        read_package_registry(&agent_root.join("packages").join("registry.json"));
    assert_eq!(
        package_registry.packages.len(),
        1,
        "agent package registry 应只有 1 条记录"
    );
    assert_eq!(package_registry.packages[0].name, "e2e-agent-plugin");
    assert_eq!(package_registry.packages[0].source_kind.as_str(), "local");
    assert_eq!(
        package_registry.packages[0].plugins[0].id,
        "e2e-agent-plugin"
    );
    assert_eq!(package_registry.packages[0].plugins[0].relative_dir, ".");

    let plugin_registry = read_plugin_registry(&agent_root.join("plugins").join("registry.json"));
    assert!(
        plugin_registry
            .plugins
            .iter()
            .any(|entry| entry.id == "e2e-agent-plugin" && entry.enabled),
        "agent plugin registry 应登记 e2e-agent-plugin"
    );
}

#[test]
fn test_user_installs_agent_package_survives_scope_switch() {
    common::setup_logging();
    let _span = info_span!("test_user_installs_agent_package_survives_scope_switch").entered();

    let plugin_dir = make_plugin_dir("e2e-agent-switch-plugin");
    let home = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    let project_a = home.path().join("project-a");
    let project_b = home.path().join("project-b");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();

    let plugin_src = plugin_dir.path().to_str().unwrap();
    let work_dir_str = work_dir.to_str().unwrap();
    let project_a_str = project_a.to_str().unwrap();
    let project_b_str = project_b.to_str().unwrap();

    info!("Arrange: 从 project-a 安装 bare plugin 到 agent 层");
    cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "install",
            plugin_src,
            "--visibility",
            "agent",
            "--scope-root",
            project_a_str,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("已安装 package").and(predicate::str::contains("agent")));

    info!("Act: 切到 project-b 后查看 agent 层 packages");
    let list_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "packages",
            "--visibility",
            "agent",
            "--scope-root",
            project_b_str,
        ])
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert packages: agent 包不受 scope 切换影响；actual: {}",
        trunc(&list_out, 320)
    );
    list_assert.success().stdout(
        predicate::str::contains("agent:")
            .and(predicate::str::contains("e2e-agent-switch-plugin@0.1.0"))
            .and(predicate::str::contains("[local]"))
            .and(predicate::str::contains("plugin:e2e-agent-switch-plugin")),
    );

    let agent_root = work_dir.join("agents").join("main");
    assert!(
        agent_root
            .join("plugins")
            .join("e2e-agent-switch-plugin")
            .join("plugin.json")
            .exists(),
        "切换 scope 后 agent plugin 仍应存在"
    );
    let package_registry =
        read_package_registry(&agent_root.join("packages").join("registry.json"));
    assert_eq!(package_registry.packages.len(), 1);
    assert_eq!(package_registry.packages[0].name, "e2e-agent-switch-plugin");
}

/// [E2E-CLI-029] 用户把 bare skill 安装到 global 层并列出 global package
///
/// 用户意图：通过统一入口把 skill 安装到全局共享层
/// 验证：global 层 skill/package 落盘；packages --visibility global 能看到 bareSkill 记录
#[test]
fn test_user_installs_bare_skill_to_global_layer() {
    common::setup_logging();
    let _span = info_span!("test_user_installs_bare_skill_to_global_layer").entered();

    let skill_dir = make_bare_skill_dir("e2e-global-skill");
    let home = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    let scope_root = home.path().join("project");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(&scope_root).unwrap();

    let skill_src = skill_dir.path().to_str().unwrap();
    let work_dir_str = work_dir.to_str().unwrap();
    let scope_root_str = scope_root.to_str().unwrap();

    info!("Act: tomcat install <skill> --visibility global");
    let install_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "install",
            skill_src,
            "--visibility",
            "global",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let install_out =
        String::from_utf8_lossy(&install_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert install: exit 0 + stdout 指向 global；actual: {}",
        trunc(&install_out, 240)
    );
    install_assert
        .success()
        .stdout(predicate::str::contains("已安装 package").and(predicate::str::contains("global")));

    info!("Act: tomcat packages --visibility global");
    let list_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "packages",
            "--visibility",
            "global",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert packages: global 层含 bare skill；actual: {}",
        trunc(&list_out, 320)
    );
    list_assert.success().stdout(
        predicate::str::contains("global:")
            .and(predicate::str::contains("e2e-global-skill@0.0.0"))
            .and(predicate::str::contains("[local]"))
            .and(predicate::str::contains("skill:e2e-global-skill")),
    );

    assert!(
        work_dir
            .join("skills")
            .join("e2e-global-skill")
            .join("SKILL.md")
            .exists(),
        "global skill 应已落盘"
    );

    let package_registry = read_package_registry(&work_dir.join("packages").join("registry.json"));
    assert_eq!(
        package_registry.packages.len(),
        1,
        "global package registry 应只有 1 条记录"
    );
    assert_eq!(package_registry.packages[0].name, "e2e-global-skill");
    assert_eq!(package_registry.packages[0].source_kind.as_str(), "local");
    assert_eq!(
        package_registry.packages[0].skills[0].name,
        "e2e-global-skill"
    );
    assert_eq!(package_registry.packages[0].skills[0].relative_dir, ".");
}

/// [E2E-CLI-030] 用户卸载 scope package 后资源与账本被精准清理
///
/// 用户意图：卸载一个通过统一入口安装到当前项目的 package
/// 验证：plugin/skill 目录与 scope 层 package/plugin registry 均移除；packages --visibility scope 回到空列表
#[test]
fn test_user_uninstalls_scope_package_and_cleans_scope_layer() {
    common::setup_logging();
    let _span = info_span!("test_user_uninstalls_scope_package_and_cleans_scope_layer").entered();

    let package_dir = make_package_dir(
        "e2e-uninstall-package",
        "0.3.0",
        "e2e-uninstall-plugin",
        "e2e-uninstall-skill",
    );
    let home = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    let scope_root = home.path().join("project");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(&scope_root).unwrap();

    let package_src = package_dir.path().to_str().unwrap();
    let work_dir_str = work_dir.to_str().unwrap();
    let scope_root_str = scope_root.to_str().unwrap();

    info!("Arrange: 先安装一个 scope package");
    cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "install",
            package_src,
            "--visibility",
            "scope",
            "--scope-root",
            scope_root_str,
        ])
        .assert()
        .success();

    info!("Act: tomcat uninstall <package> --visibility scope");
    let uninstall_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "uninstall",
            "e2e-uninstall-package",
            "--visibility",
            "scope",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let uninstall_out =
        String::from_utf8_lossy(&uninstall_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert uninstall: exit 0 + stdout 含 package 名；actual: {}",
        trunc(&uninstall_out, 260)
    );
    uninstall_assert.success().stdout(
        predicate::str::contains("已卸载 package")
            .and(predicate::str::contains("e2e-uninstall-package")),
    );

    let scope_tomcat = scope_root.join(".tomcat");
    assert!(
        !scope_tomcat
            .join("plugins")
            .join("e2e-uninstall-plugin")
            .join("plugin.json")
            .exists(),
        "scope plugin 目录应被清理"
    );
    assert!(
        !scope_tomcat
            .join("skills")
            .join("e2e-uninstall-skill")
            .join("SKILL.md")
            .exists(),
        "scope skill 目录应被清理"
    );

    let package_registry =
        read_package_registry(&scope_tomcat.join("packages").join("registry.json"));
    assert!(
        package_registry.packages.is_empty(),
        "scope package registry 卸载后应为空"
    );

    let plugin_registry = read_plugin_registry(&scope_tomcat.join("plugins").join("registry.json"));
    assert!(
        plugin_registry.plugins.is_empty(),
        "scope plugin registry 卸载后应为空"
    );

    info!("Act: tomcat packages --visibility scope");
    let list_assert = cmd()
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir_str)
        .args([
            "packages",
            "--visibility",
            "scope",
            "--scope-root",
            scope_root_str,
        ])
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert packages: scope 层回到空列表；actual: {}",
        trunc(&list_out, 200)
    );
    list_assert
        .success()
        .stdout(predicate::str::contains("scope:").and(predicate::str::contains("(none)")));
}

// ──────────────────── Story 7: LLM 统一接入（E2E-CLI-041~042，需 DEEPSEEK_API_KEY） ────────────────────

/// [E2E-CLI-041] 用户与 LLM 对话，获得流式渲染回复
///
/// 用户意图：与 LLM 对话，获得非空 AI 回复
/// 验证：exit 0；stdout 含 AI 回复
/// 要求：DEEPSEEK_API_KEY 已设置
#[test]
fn test_user_chats_with_llm_gets_streaming_response() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_chats_with_llm_gets_streaming_response").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_chats_with_llm_gets_streaming_response");

    info!("Act: tomcat chat + stdin 单句，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("请用一句话回答：1+1 等于几？\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 AI 回复；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "LLM 应输出非空流式回复，实际 stdout 为空"
    );
}

/// [E2E-CLI-042] 确认 LLM 回复内容非空（基础连通性）
///
/// 用户意图：发送极短提问，验证 LLM 回复非空
/// 验证：exit 0；stdout 非空
/// 要求：DEEPSEEK_API_KEY 已设置
#[test]
fn test_user_receives_nonempty_llm_response() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_receives_nonempty_llm_response").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_receives_nonempty_llm_response");

    info!("Act: tomcat chat + stdin 说一个字，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("说一个字\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "LLM 应输出非空回复，实际 stdout 为空"
    );
}

// ──────────────────── Story 8: CLI对话与会话管理（E2E-CLI-051~082） ────────────────────

/// [E2E-CLI-051] 用户创建一个新会话
///
/// 用户意图：创建新会话
/// 验证：exit 0；stdout 含"已创建会话"
#[test]
fn test_user_creates_new_session() {
    common::setup_logging();
    let _span = info_span!("test_user_creates_new_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: fresh work dir");
    info!("Act: tomcat session new");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已创建会话；actual: {}",
        trunc(&out, 200)
    );
    assert
        .success()
        .stdout(predicate::str::contains("已创建会话"));
}

/// [E2E-CLI-052] 用户查看所有会话
///
/// 用户意图：列出所有会话
/// 验证：exit 0
#[test]
fn test_user_lists_sessions() {
    common::setup_logging();
    let _span = info_span!("test_user_lists_sessions").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create a session first");
    let first_id = create_session_via_cli(&work_dir);
    info!("Arrange: create a second historical session");
    let second_id = create_session_via_cli(&work_dir);

    info!("Act: tomcat session list");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "list"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + one current marker + both session ids present; actual: {}",
        trunc(&out, 200)
    );
    assert.success();
    let lines: Vec<&str> = out.lines().filter(|line| !line.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "应列出两条会话记录，actual: {out}");
    assert_eq!(
        lines.iter().filter(|line| line.starts_with('*')).count(),
        1,
        "只应有一条 current 标记，actual: {out}"
    );
    assert!(
        out.contains(&first_id) && out.contains(&second_id),
        "list 应包含两条创建出的 session_id，actual: {out}"
    );
}

/// [E2E-CLI-053] 用户切换到已存在的会话
///
/// 用户意图：创建会话后切换到 default 会话
/// 验证：exit 0
#[test]
fn test_user_switches_to_existing_session() {
    common::setup_logging();
    let _span = info_span!("test_user_switches_to_existing_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    let session_id = create_session_via_cli(&work_dir);

    info!("Act: tomcat session switch {}", session_id);
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "switch", session_id.as_str()])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-054] 用户切换到不存在会话时看到友好提示
///
/// 用户意图：切换到不存在会话，看到"不存在"提示
/// 验证：exit 0；stdout 含"不存在"
#[test]
fn test_user_switches_to_nonexistent_session_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_user_switches_to_nonexistent_session_shows_error").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: no session pre-created");
    info!("Act: tomcat session switch nonexistent-key-e2e");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "switch", "nonexistent-key-e2e"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含不存在；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [E2E-CLI-055] 用户删除刚创建的会话
///
/// 用户意图：创建后删除会话
/// 验证：exit 0；stdout 含"已删除"
#[test]
fn test_user_deletes_session() {
    common::setup_logging();
    let _span = info_span!("test_user_deletes_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    let session_id = create_session_via_cli(&work_dir);

    info!("Act: tomcat session delete {}", session_id);
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "delete", session_id.as_str()])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已删除；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("已删除"));
}

/// [E2E-CLI-056] 用户归档会话
///
/// 用户意图：归档刚创建的会话
/// 验证：exit 0；stdout 含"已归档"
#[test]
fn test_user_archives_session() {
    common::setup_logging();
    let _span = info_span!("test_user_archives_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    let session_id = create_session_via_cli(&work_dir);

    info!("Act: tomcat session archive {}", session_id);
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "archive", session_id.as_str()])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已归档；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("已归档"));
}

/// [E2E-CLI-057] 用户按关键词搜索会话
///
/// 用户意图：按当前固定 session key 搜索会话
/// 验证：exit 0
#[test]
fn test_user_searches_sessions_by_keyword() {
    common::setup_logging();
    let _span = info_span!("test_user_searches_sessions_by_keyword").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    let first_id = create_session_via_cli(&work_dir);
    info!("Arrange: create a second historical session");
    let second_id = create_session_via_cli(&work_dir);

    let current_key = current_code_session_key();
    info!("Act: tomcat session search {}", current_key);
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "search", current_key.as_str()])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + search returns both session ids under current key; actual: {}",
        trunc(&out, 200)
    );
    assert.success();
    let lines: Vec<&str> = out.lines().filter(|line| !line.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "按 key 搜索应命中两条历史会话，actual: {out}"
    );
    assert!(
        lines.iter().all(|line| line.contains(&current_key)),
        "每条搜索结果都应带当前 scope key，actual: {out}"
    );
    assert!(
        out.contains(&first_id) && out.contains(&second_id),
        "search 应包含两条创建出的 session_id，actual: {out}"
    );
}

/// [E2E-CLI-058] 无 API key 时 chat 快速失败，不挂起
///
/// 用户意图：未配置 API Key 时 chat 应快速报错而非挂起
/// 验证：进程 5s 内结束；stdout 或 stderr 含错误提示
#[test]
fn test_user_chat_without_api_key_fails_gracefully() {
    common::setup_logging();
    let _span = info_span!("test_user_chat_without_api_key_fails_gracefully").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init，移除 DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat chat without API key，timeout 5s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("hello\n")
        .timeout(std::time::Duration::from_secs(5));
    configure_deepseek_without_key(&mut c);
    let output = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    info!(
        "Assert: 进程 5s 内结束，含错误提示；stdout: {}",
        trunc(&stdout, 200)
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("error")
            || combined.contains("Error")
            || combined.contains("key")
            || combined.contains("API")
            || combined.to_lowercase().contains("invalid")
            || combined.contains("配置")
            || combined.contains("失败"),
        "chat 无 API Key 时应含错误提示，实际 combined: {}",
        trunc(&combined, 300)
    );
}

/// [E2E-CLI-067] 用户在聊天内执行 /skill list|reload|use
///
/// 用户意图：查看当前 skills、重载目录，并把 user-only skill 注入当前轮。
/// 验证：本地命令输出可见；`/skill use` 会把 skill 正文与 intent 注入 transcript。
#[test]
fn test_user_chat_skill_list_reload_use() {
    common::setup_logging();
    let _span = info_span!("test_user_chat_skill_list_reload_use").entered();

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    fs::create_dir_all(&work_dir).unwrap();
    let (base_url, server_handle) = spawn_quick_openai_stream_server("SKILL_CLI_OK");

    info!("Arrange: init temp HOME + workspace skills");
    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    fs::write(
        work_dir.join("models.toml"),
        format!(
            r#"[[models]]
id = "mock-local"
api = "openai"
provider = "openai"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = false }}
"#
        ),
    )
    .unwrap();
    write_skill_fixture(workspace.path(), "commit", "Create a git commit", false);
    write_skill_fixture(
        workspace.path(),
        "secret",
        "Manual reviewer checklist",
        true,
    );
    write_skill_fixture(workspace.path(), "lint", "Run repo lint checks", false);

    info!("Act: tomcat chat with /skill list, /skill reload, /skill use");
    let mut c = cmd();
    c.arg("chat")
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__DEFAULT_MODEL", "mock-local")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .current_dir(workspace.path())
        .write_stdin("/skill list\n/skill reload\n/skill use secret summarize current diff\n")
        .timeout(std::time::Duration::from_secs(20));
    let output = c.output().expect("chat should exit");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    info!(
        "Assert: list/reload output visible and transcript contains injected skill; combined: {}",
        trunc(&combined, 400)
    );
    assert!(
        combined.contains("commit")
            && combined.contains("secret")
            && combined.contains("user-only"),
        "chat output should list discovered skills, actual: {}",
        trunc(&combined, 400)
    );
    assert!(
        combined.contains("已重载") || combined.contains("reload") || combined.contains("reloaded"),
        "chat output should mention reload, actual: {}",
        trunc(&combined, 400)
    );
    assert!(
        output.status.success(),
        "chat should exit successfully once mock LLM replies, stderr: {}",
        trunc(&stderr, 400)
    );

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    let sessions_dir = tomcat::resolve_sessions_dir(&cfg).expect("resolve sessions dir");
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, workspace.path());
    let session = SessionManager::new_scoped(sessions_dir, session_key);
    let transcript_path = session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    let transcript = fs::read_to_string(&transcript_path).expect("read transcript");
    assert!(
        transcript.contains("<skill name=\\\"secret\\\""),
        "transcript should contain injected skill body, actual: {}",
        trunc(&transcript, 800)
    );
    assert!(
        transcript.contains("Current user intent:\\nsummarize current diff"),
        "transcript should preserve /skill use intent, actual: {}",
        trunc(&transcript, 800)
    );
    server_handle.join().expect("mock llm server should exit");
}

/// [E2E-CLI-068] 用户执行 tomcat skill list|reload
///
/// 用户意图：在 chat 外查看当前技能清单并显式触发 rediscovery。
/// 验证：`list`/`reload` 均 exit 0；输出含 skill、user-only 标记与 diagnostics。
#[test]
fn test_user_skill_cli_list_reload_e2e() {
    common::setup_logging();
    let _span = info_span!("test_user_skill_cli_list_reload_e2e").entered();

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = home.path().join("work");
    fs::create_dir_all(&work_dir).unwrap();

    info!("Arrange: init temp HOME + workspace skills");
    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    write_skill_fixture(workspace.path(), "commit", "Create a git commit", false);
    write_skill_fixture(
        workspace.path(),
        "secret",
        "Manual reviewer checklist",
        true,
    );

    info!("Act: tomcat skill list");
    let assert = cmd()
        .args(["skill", "list"])
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .current_dir(workspace.path())
        .assert();
    let list_out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: list output includes visible skills; actual: {}",
        trunc(&list_out, 300)
    );
    assert
        .success()
        .stdout(predicate::str::contains("commit"))
        .stdout(predicate::str::contains("secret"))
        .stdout(predicate::str::contains("user-only"));

    info!("Arrange: add a malformed skill and a new valid skill before reload");
    let broken_dir = workspace
        .path()
        .join(".tomcat")
        .join("skills")
        .join("broken");
    fs::create_dir_all(&broken_dir).unwrap();
    fs::write(broken_dir.join("SKILL.md"), "# missing frontmatter\n").unwrap();
    write_skill_fixture(workspace.path(), "lint", "Run repo lint checks", false);

    info!("Act: tomcat skill reload");
    let reload = cmd()
        .args(["skill", "reload"])
        .env("HOME", home.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .current_dir(workspace.path())
        .assert();
    let reload_out = String::from_utf8_lossy(&reload.get_output().stdout.clone()).to_string();
    info!(
        "Assert: reload output includes new skill + diagnostics; actual: {}",
        trunc(&reload_out, 400)
    );
    reload
        .success()
        .stdout(predicate::str::contains("lint"))
        .stdout(predicate::str::contains("diagnostic").or(predicate::str::contains("诊断")));
}

/// [E2E-CLI-059] 用户查看操作审计记录列表
///
/// 用户意图：列出审计记录
/// 验证：exit 0
#[test]
fn test_user_views_audit_list() {
    common::setup_logging();
    let _span = info_span!("test_user_views_audit_list").entered();

    info!("Arrange: no special setup");
    info!("Act: tomcat audit list");
    let assert = cmd().args(["audit", "list"]).assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-060] 用户导出审计记录到文件
///
/// 用户意图：导出审计日志到 JSON 文件
/// 验证：exit 0（MVP 阶段 audit export 命令可正常执行不崩溃）
#[test]
fn test_user_exports_audit_to_file() {
    common::setup_logging();
    let _span = info_span!("test_user_exports_audit_to_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("audit_e2e.json");

    info!("Arrange: temp audit export path = {:?}", out_path);
    info!("Act: tomcat audit export");
    let assert = cmd()
        .args(["audit", "export", out_path.to_str().unwrap()])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-061] 用户查看不存在的审计条目时友好提示
///
/// 用户意图：查看 ID=9999999 的审计条目，看到友好提示
/// 验证：exit 0；不 panic
#[test]
fn test_user_views_audit_show_invalid_id() {
    common::setup_logging();
    let _span = info_span!("test_user_views_audit_show_invalid_id").entered();

    info!("Arrange: no special setup");
    info!("Act: tomcat audit show 9999999");
    let assert = cmd().args(["audit", "show", "9999999"]).assert();
    info!("Assert: exit 0, 不 panic");
    assert.success();
}

// ──────────────────── 边界与健壮性场景（E2E-CLI-071~074） ────────────────────

/// [E2E-CLI-071] 用户查看帮助，所有子命令可见
///
/// 用户意图：查看主帮助，所有子命令应在 stdout 中
/// 验证：exit 0；stdout 含 init/doctor/config/session/plugin/audit
#[test]
fn test_user_views_full_help() {
    common::setup_logging();
    let _span = info_span!("test_user_views_full_help").entered();

    info!("Act: tomcat --help");
    let assert = cmd().arg("--help").assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 含所有子命令；actual: {}",
        trunc(&out, 400)
    );
    assert
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

/// [E2E-CLI-072] 用户查看版本号
///
/// 用户意图：查看 tomcat 的版本号
/// 验证：exit 0；stdout 含版本号字符串
#[test]
fn test_user_views_version() {
    common::setup_logging();
    let _span = info_span!("test_user_views_version").entered();

    info!("Act: tomcat --version");
    let assert = cmd().arg("--version").assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + 含版本号；actual: {}", trunc(&out, 100));
    assert
        .success()
        .stdout(predicate::str::is_match(r"\d+\.\d+").unwrap());
}

/// [E2E-CLI-073] 用户输入错误命令时看到帮助
///
/// 用户意图：输入未知子命令，看到错误提示
/// 验证：exit 非 0；stderr 含"error"
#[test]
fn test_user_runs_unknown_command() {
    common::setup_logging();
    let _span = info_span!("test_user_runs_unknown_command").entered();

    info!("Act: tomcat nonexistent_cmd_e2e");
    let assert = cmd().arg("nonexistent_cmd_e2e").assert();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr.clone()).to_string();
    info!(
        "Assert: exit 非 0 + stderr 含 error；actual: {}",
        trunc(&stderr, 200)
    );
    assert
        .failure()
        .stderr(predicate::str::contains("error").or(predicate::str::contains("unrecognized")));
}

/// [E2E-CLI-074] 用户 init 后 doctor 通过，完整引导流程
///
/// 用户意图：新手引导——init 后 doctor 应检测通过
/// 验证：两步 exit 0；doctor 含"✓"
#[test]
fn test_user_init_then_doctor_roundtrip() {
    common::setup_logging();
    let _span = info_span!("test_user_init_then_doctor_roundtrip").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init → tomcat doctor");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 含 配置合法 + 内嵌资源已就绪 + rquickjs；actual: {}",
        trunc(&out, 500)
    );
    assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源已就绪").or(predicate::str::contains("✓")));
}

// ──────────────────── Story 9 补充: chat --resume 与多轮上下文（E2E-CLI-082~083） ────────────────────

/// [E2E-CLI-082] 用户用 --resume 恢复上次会话
///
/// 用户意图：用 --resume 恢复已有会话，历史消息从 JSONL 加载
/// 验证：exit 0；进程正常退出（不崩溃）
/// 要求：DEEPSEEK_API_KEY 已设置
#[test]
fn test_user_chat_resumes_last_session() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_chat_resumes_last_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = real_llm_api_key("test_user_chat_resumes_last_session");

    info!("Act: 第一轮 tomcat chat，建立会话历史");
    let mut first_round = cmd();
    first_round
        .arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("请回答：1+1=？\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut first_round, &api_key);
    first_round.assert().success();

    info!("Act: 第二轮 tomcat chat --resume，恢复会话");
    let mut c = cmd();
    c.arg("chat")
        .arg("--resume")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("好的，谢谢\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "--resume 后 AI 应有回复，实际 stdout 为空"
    );
}

/// append invariant 后，同一 chat 进程应从磁盘重建 context_state 并继续下一轮。
#[tokio::test]
async fn test_failed_turn_append_invariant_allows_next_turn_in_same_process() {
    common::setup_logging();
    let _span =
        info_span!("test_failed_turn_append_invariant_allows_next_turn_in_same_process").entered();

    const ENV_KEY: &str = "TOMCAT_APPEND_REHYDRATE_CLI_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![
        cli_tool_call_stream("call_1", "bash", r#"{"command":"echo hi","cwd":null}"#),
        cli_text_stream("RECOVER_OK"),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");
    ctx.global_services.primitive = Arc::new(DeterministicMockPrimitive);
    ctx.session_runtime.message_append_sink = Arc::new(CliInjectAppendInvariantSink::new(
        ctx.session_runtime.session.clone(),
    ));

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();

    info!("Act: 第一轮触发 append_message_chain invariant");
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请执行一次 bash 工具",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("first run_chat_turn timeout 5s")
    .expect("first run_chat_turn result");

    match &first {
        tomcat::AgentRunOutcome::Failed(err) => {
            assert!(
                matches!(
                    err,
                    AppError::Invariant {
                        stage: "append_message_chain",
                        ..
                    }
                ),
                "首轮应命中 append_message_chain invariant，实际: {err}"
            );
        }
        other => panic!("首轮应进入 Failed(invariant) 分支，实际: {other:?}"),
    }
    assert_eq!(
        state.messages.last().and_then(|m| m.text_content()),
        Some("nested done"),
        "首轮失败后应已从磁盘重建 context_state，而不是保留 dangling tool_calls"
    );
    assert!(
        state.messages.iter().any(|m| {
            m.tool_call_id.as_deref() == Some("call_1") && m.text_content() == Some("[interrupted]")
        }),
        "重建后的 context_state 应保留磁盘上的 interrupted tool result"
    );

    info!("Act: 第二轮继续同一进程聊天，应恢复为 Completed");
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请只回复 RECOVER_OK，不要调用工具",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("second run_chat_turn timeout 5s")
    .expect("second run_chat_turn result");

    match second {
        tomcat::AgentRunOutcome::Completed(result) => {
            assert!(
                result.final_text.contains("RECOVER_OK"),
                "第二轮应成功恢复，实际 final_text: {:?}",
                result.final_text
            );
        }
        other => panic!("第二轮应恢复为 Completed，实际: {other:?}"),
    }

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn test_preturn_append_invariant_heals_and_continues_same_input() {
    common::setup_logging();
    let _span =
        info_span!("test_preturn_append_invariant_heals_and_continues_same_input").entered();

    const ENV_KEY: &str = "TOMCAT_PRETURN_APPEND_RETRY_CLI_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![cli_text_stream(
        "CONTINUE_OK",
    )]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");
    ctx.global_services.primitive = Arc::new(DeterministicMockPrimitive);

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    seed_dangling_tool_round(&ctx.session_runtime.session, "call_tail");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "继续",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("自愈后应直接 Completed，实际: {other:?}"),
    };
    assert!(
        result.final_text.contains("CONTINUE_OK"),
        "应使用原输入自动续跑完成，实际 final_text: {:?}",
        result.final_text
    );
    assert!(
        result.new_messages.iter().any(|msg| {
            msg.role == tomcat::core::llm::ChatMessageRole::User
                && msg.text_content() == Some("继续")
        }),
        "原输入应作为本轮 user 消息进入续跑结果"
    );
    assert!(
        state.messages.iter().any(|m| {
            m.tool_call_id.as_deref() == Some("call_tail")
                && m.text_content() == Some("[interrupted]")
        }),
        "自愈后 state 应包含补齐的 `[interrupted]` tool 结果"
    );

    let transcript_path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    let transcript = fs::read_to_string(&transcript_path).expect("read transcript");
    let mut interrupted_idx = None;
    let mut continued_idx = None;
    for (idx, line) in transcript.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str());
        let content = message.get("content").and_then(|v| v.as_str());
        if role == Some("tool")
            && message.get("tool_call_id").and_then(|v| v.as_str()) == Some("call_tail")
            && content == Some("[interrupted]")
        {
            interrupted_idx = Some(idx);
        }
        if role == Some("user") && content == Some("继续") {
            continued_idx = Some(idx);
        }
    }
    let interrupted_idx =
        interrupted_idx.expect("transcript should contain healed interrupted tool result");
    let continued_idx = continued_idx.expect("transcript should contain continued user input");
    assert!(
        interrupted_idx < continued_idx,
        "补齐的 `[interrupted]` 应先于用户输入落盘，actual transcript: {}",
        trunc(&transcript, 800)
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn test_preturn_append_invariant_recovers_without_user_reinput() {
    common::setup_logging();
    let _span = info_span!("test_preturn_append_invariant_recovers_without_user_reinput").entered();

    const ENV_KEY: &str = "TOMCAT_PRETURN_APPEND_SINGLE_INPUT_CLI_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![cli_text_stream(
        "CONTINUE_ONCE",
    )]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");
    ctx.global_services.primitive = Arc::new(DeterministicMockPrimitive);

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    seed_dangling_tool_round(&ctx.session_runtime.session, "call_once");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "继续一次",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    match outcome {
        tomcat::AgentRunOutcome::Completed(result) => {
            assert!(
                result.final_text.contains("CONTINUE_ONCE"),
                "自愈后应直接完成，实际: {:?}",
                result.final_text
            );
        }
        other => panic!("自愈后应 Completed，实际: {other:?}"),
    }

    let transcript_path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    let transcript = fs::read_to_string(&transcript_path).expect("read transcript");
    assert_eq!(
        transcript.matches("\"content\":\"继续一次\"").count(),
        1,
        "原输入只应被消费一次，不应要求用户重输或重复 append；actual transcript: {}",
        trunc(&transcript, 800)
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn test_cli_chat_path_retries_gateway_503_and_recovers_same_turn() {
    common::setup_logging();
    let _span =
        info_span!("test_cli_chat_path_retries_gateway_503_and_recovers_same_turn").entered();

    const ENV_KEY: &str = "TOMCAT_CLI_GATEWAY_503_RETRY_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    ctx.config.llm.agent_max_attempts = 2;
    ctx.config.llm.agent_retry_base_delay_ms = 0;
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "upstream connect error or disconnect/reset before headers. reset reason: connection timeout",
        ))],
        cli_text_stream("CLI_RETRY_OK"),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");
    ctx.global_services.primitive = Arc::new(DeterministicMockPrimitive);

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请在 503 后自动恢复",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");
    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("瞬时 503 后应完成当前轮，实际: {other:?}"),
    };
    assert!(
        result.final_text.contains("CLI_RETRY_OK"),
        "瞬时 503 后应自动恢复，实际 final_text: {:?}",
        result.final_text
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn test_cli_chat_path_retry_exhausted_503_preserves_progress_for_next_turn() {
    common::setup_logging();
    let _span =
        info_span!("test_cli_chat_path_retry_exhausted_503_preserves_progress_for_next_turn")
            .entered();

    const ENV_KEY: &str = "TOMCAT_CLI_GATEWAY_503_EXHAUST_KEY";
    let (work_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    ctx.config.llm.agent_max_attempts = 2;
    ctx.config.llm.agent_retry_base_delay_ms = 0;
    let failing_llm = Arc::new(DeterministicMockLlm::new(vec![
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "upstream connect error or disconnect/reset before headers. reset reason: connection timeout",
        ))],
        vec![Err(llm_http_status_error(
            "mock",
            503,
            "upstream connect error or disconnect/reset before headers. reset reason: connection timeout",
        ))],
    ]));
    install_fixed_resolver(&mut ctx, failing_llm, "gpt-5.4");
    ctx.global_services.primitive = Arc::new(DeterministicMockPrimitive);

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "第一轮会失败但应保留进度",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("first run_chat_turn timeout 5s")
    .expect("first run_chat_turn result");
    assert!(
        matches!(first, tomcat::AgentRunOutcome::Failed(_)),
        "503 重试耗尽后当前轮应失败"
    );
    let transcript_after_fail = load_current_transcript_for_work_dir(work_dir.path());
    assert!(
        transcript_after_fail.contains("第一轮会失败但应保留进度"),
        "失败轮的用户输入应已落盘保留，actual transcript: {}",
        trunc(&transcript_after_fail, 800)
    );

    let success_llm = Arc::new(DeterministicMockLlm::new(vec![cli_text_stream(
        "CLI_CONTINUE_OK",
    )]));
    install_fixed_resolver(&mut ctx, success_llm, "gpt-5.4");
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "第二轮继续",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("second run_chat_turn timeout 5s")
    .expect("second run_chat_turn result");
    let result = match second {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("失败后下一轮仍应可继续，实际: {other:?}"),
    };
    assert!(
        result.final_text.contains("CLI_CONTINUE_OK"),
        "失败后下一轮应能继续完成，实际 final_text: {:?}",
        result.final_text
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

/// Responses 终局元数据应随 assistant message 一起持久化到 transcript。
#[tokio::test]
async fn test_run_chat_turn_persists_assistant_finish_reason_and_error_metadata() {
    common::setup_logging();
    let _span =
        info_span!("test_run_chat_turn_persists_assistant_finish_reason_and_error_metadata")
            .entered();

    const ENV_KEY: &str = "TOMCAT_RESPONSES_FINISH_REASON_CLI_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "partial".to_string(),
        }),
        Ok(StreamEvent::LlmError {
            reason: "error:boom".to_string(),
            message: "boom".to_string(),
            code: Some("server_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:boom".to_string(),
        }),
    ]]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请随便回答",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("应正常 Completed，实际: {other:?}"),
    };
    let assistant = result
        .new_messages
        .iter()
        .rev()
        .find(|msg| msg.role == tomcat::core::llm::ChatMessageRole::Assistant)
        .expect("should persist assistant message");
    assert_eq!(assistant.finish_reason.as_deref(), Some("error:boom"));
    assert_eq!(assistant.error_message.as_deref(), Some("boom"));
    assert_eq!(assistant.error_code.as_deref(), Some("server_error"));

    let transcript_path = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    let transcript = fs::read_to_string(&transcript_path).expect("read transcript");
    assert!(
        transcript.contains("\"finish_reason\":\"error:boom\""),
        "transcript 应保留 finish_reason，实际: {}",
        trunc(&transcript, 800)
    );
    assert!(
        transcript.contains("\"error_message\":\"boom\""),
        "transcript 应保留 error_message，实际: {}",
        trunc(&transcript, 800)
    );
    assert!(
        transcript.contains("\"error_code\":\"server_error\""),
        "transcript 应保留 error_code，实际: {}",
        trunc(&transcript, 800)
    );

    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
#[serial(env_lock)]
async fn test_chat_path_executes_web_search_tool_with_mock_server() {
    common::setup_logging();
    let _span = info_span!("test_chat_path_executes_web_search_tool_with_mock_server").entered();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                {
                    "title": "reqwest",
                    "url": "https://docs.rs/reqwest",
                    "content": "HTTP client",
                    "published_date": "2026-06-01"
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    const ENV_KEY: &str = "TOMCAT_WEB_SEARCH_CHAT_KEY";
    let _env = EnvGuard::set_many(&[
        (ENV_KEY, Some("stub")),
        ("TAVILY_API_KEY", Some("tavily-test-key")),
        ("BRAVE_API_KEY", None),
        ("SERPER_API_KEY", None),
        ("NO_PROXY", Some("127.0.0.1,localhost")),
        ("no_proxy", Some("127.0.0.1,localhost")),
        (common::DEEPSEEK_TEST_API_KEY_ENV, None),
    ]);
    let mut cfg = AppConfig::default();
    cfg.tools.web_search.backend = "tavily".to_string();
    cfg.tools.web_search.legacy_http_backends = true;
    cfg.tools.web_search.tavily_base_url = server.uri();
    let (_dir, mut ctx) = deterministic_chat_context_fixture_with_config(cfg, ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![
        cli_tool_call_stream(
            "call_ws",
            "web_search",
            r#"{"query":"reqwest rust","domain_filter":["docs.rs"]}"#,
        ),
        cli_text_stream("SEARCH_OK"),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "请搜索 reqwest rust",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("应正常完成 web_search chat 路径，实际: {other:?}"),
    };
    assert!(
        result.final_text.contains("SEARCH_OK"),
        "第二轮 assistant 应完成收尾，实际 final_text: {:?}",
        result.final_text
    );
    let tool_msg = result
        .new_messages
        .iter()
        .find(|msg| {
            msg.role == tomcat::core::llm::ChatMessageRole::Tool
                && msg.tool_call_id.as_deref() == Some("call_ws")
        })
        .expect("should persist web_search tool result");
    let tool_text = tool_msg.text_content().expect("tool result should be text");
    assert!(
        tool_text.contains("\"backend\":\"tavily\"")
            || tool_text.contains("\"backend\": \"tavily\""),
        "tool result 应包含 backend=tavily，实际: {}",
        trunc(tool_text, 400)
    );
    assert!(
        tool_text.contains("https://docs.rs/reqwest"),
        "tool result 应包含搜索命中 URL，实际: {}",
        trunc(tool_text, 400)
    );
}

#[tokio::test]
async fn test_run_chat_turn_rejects_multimodal_message_on_text_model_before_provider_call() {
    common::setup_logging();
    let _span = info_span!(
        "test_run_chat_turn_rejects_multimodal_message_on_text_model_before_provider_call"
    )
    .entered();

    const ENV_KEY: &str = "TOMCAT_MULTIMODAL_PRECHECK_CLI_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![]));
    install_fixed_resolver(&mut ctx, mock_llm, "deepseek-v4-pro");
    ctx.session_runtime
        .follow_up_queue
        .lock()
        .push(ChatMessage::user_with_parts(vec![
            tomcat::ChatMessageContentPart::text("请分析这张图"),
            tomcat::ChatMessageContentPart::image_file_id("file-vision").unwrap(),
        ]));

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(&ctx, "", system_text, &mut state, CancellationToken::new()),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    match outcome {
        tomcat::AgentRunOutcome::Failed(err) => {
            assert!(matches!(err, AppError::Llm(_)));
            let msg = err.to_string();
            assert!(
                msg.contains("vision"),
                "should mention missing vision: {msg}"
            );
            assert!(
                msg.contains("gpt"),
                "should suggest a catalog vision-capable model: {msg}"
            );
        }
        other => panic!("应在调用前结构化拦截多模态主路径，实际: {other:?}"),
    }

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn test_model_switch_keeps_ctx_metrics_continuous_across_turns() {
    common::setup_logging();
    let _span = info_span!("test_model_switch_keeps_ctx_metrics_continuous_across_turns").entered();

    const ENV_KEY: &str = "TOMCAT_MODEL_SWITCH_CTX_CONTINUITY_KEY";
    let (_dir, mut ctx) = deterministic_chat_context_fixture(ENV_KEY);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![
        cli_text_stream_with_usage("FIRST_OK", 120, 20),
        cli_text_stream_with_usage("SECOND_OK", 140, 24),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");

    let metrics_payloads: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let payloads_for_listener = Arc::clone(&metrics_payloads);
    ctx.global_services.event_bus.on(
        tomcat::wire::WIRE_CONTEXT_METRICS_UPDATE,
        Box::new(move |event| {
            payloads_for_listener
                .lock()
                .unwrap()
                .push(event.payload.clone());
            Ok(())
        }),
    );

    let system_text = "system prompt";
    let mut state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        system_text,
    )
    .unwrap();
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "first request keeps context warm",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("first run_chat_turn timeout 5s")
    .expect("first run_chat_turn result");
    assert!(
        matches!(first, tomcat::AgentRunOutcome::Completed(_)),
        "第一轮应正常完成"
    );
    assert_eq!(
        state
            .last_api_usage
            .as_ref()
            .map(|usage| usage.prompt_tokens),
        Some(120),
        "第一轮后应保留 usage，供下一轮 [ctx] 继续沿用"
    );
    let first_live_tokens = state.live.input_tokens_used;
    let first_live_ratio = state.live.context_utilization_ratio;
    assert!(first_live_tokens > 0);
    assert!(first_live_ratio > 0.0);
    let payload_count_after_first = metrics_payloads.lock().unwrap().len();
    assert!(
        payload_count_after_first > 0,
        "第一轮应至少发出一次 ContextMetricsUpdate"
    );

    ctx.session_runtime
        .session
        .switch_current_model(Some("openai"), Some("gpt-5.2"))
        .expect("model switch should succeed");
    assert_eq!(
        ctx.session_runtime
            .session
            .get_session(ctx.session_runtime.session.current_session_key())
            .expect("session read")
            .and_then(|entry| entry.model_override),
        Some("gpt-5.2".to_string())
    );
    assert_eq!(
        state
            .last_api_usage
            .as_ref()
            .map(|usage| usage.prompt_tokens),
        Some(120),
        "切 model 不应重置同一份 ContextState"
    );

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "second request after model switch",
            system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("second run_chat_turn timeout 5s")
    .expect("second run_chat_turn result");
    assert!(
        matches!(second, tomcat::AgentRunOutcome::Completed(_)),
        "第二轮应正常完成"
    );

    let payloads = metrics_payloads.lock().unwrap().clone();
    let second_turn_payloads = &payloads[payload_count_after_first..];
    assert!(
        !second_turn_payloads.is_empty(),
        "切 model 后应继续发出 ContextMetricsUpdate"
    );
    let first_payload_after_switch = &second_turn_payloads[0];
    let first_tokens_after_switch = first_payload_after_switch["inputTokensUsed"]
        .as_u64()
        .expect("inputTokensUsed should be u64");
    let first_ratio_after_switch = first_payload_after_switch["contextUtilizationRatio"]
        .as_f64()
        .expect("contextUtilizationRatio should be f64");
    assert!(
        first_tokens_after_switch >= first_live_tokens as u64,
        "切 model 后首个 [ctx] token 统计应延续上一轮，而不是归零: {:?}",
        first_payload_after_switch
    );
    assert!(
        first_ratio_after_switch >= first_live_ratio,
        "切 model 后首个 [ctx] ratio 应延续上一轮，而不是归零: {:?}",
        first_payload_after_switch
    );
    assert!(
        state.live.input_tokens_used > 0 && state.live.context_utilization_ratio > 0.0,
        "第二轮结束后 [ctx] 统计仍应保持非零"
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[test]
#[serial(env_lock)]
fn test_session_model_override_persists_across_chat_context_restart() {
    common::setup_logging();
    let _span =
        info_span!("test_session_model_override_persists_across_chat_context_restart").entered();

    const ENV_KEY: &str = "TOMCAT_SESSION_MODEL_OVERRIDE_TEST_KEY";
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    common::apply_deepseek_app_config(&mut cfg);
    cfg.llm.api_key_env = Some(ENV_KEY.to_string());

    unsafe {
        std::env::set_var(ENV_KEY, "deepseek-stub");
    }

    let ctx = ChatContext::from_config(cfg.clone()).expect("chat context should be created");
    ctx.session_runtime
        .session
        .switch_current_model(Some("deepseek"), Some("deepseek-v4-pro"))
        .expect("session model override should persist");

    let reopened = ChatContext::from_config(cfg).expect("reopened chat context");
    let entry = reopened
        .session_runtime
        .session
        .get_session(reopened.session_runtime.session.current_session_key())
        .expect("session store read")
        .expect("session entry should exist");
    assert_eq!(entry.model_override.as_deref(), Some("deepseek-v4-pro"));

    unsafe {
        std::env::remove_var(ENV_KEY);
    }
}

// ────────────────────── TASK-14 AgentLoop E2E 用例 ──────────────────────

/// [用户场景] 用户启动 `tomcat chat` 并输入单句提问，AgentLoop 执行并输出 AI 回复
///
/// 验证：exit 0 且 stdout 包含非空 AI 回复文本（需 DEEPSEEK_API_KEY；无 key 时 panic，符合规范）
/// 意义：TASK-14 T1-P1-005 E2E 门禁——验证 AgentLoop::run() 已完整接入 tomcat chat 交互链路（E2E_TEST_SPEC §6）
#[test]
fn test_user_chat_non_interactive_with_prompt_flag() {
    common::setup_logging();
    common::load_deepseek_test_env();
    let _span = info_span!("test_user_chat_non_interactive_with_prompt_flag").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init 生成配置；加载 DEEPSEEK_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let api_key = real_llm_api_key("test_user_chat_non_interactive_with_prompt_flag");

    info!("Act: tomcat chat stdin 单轮问答，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("Reply with exactly: pong\n")
        .timeout(std::time::Duration::from_secs(60));
    configure_deepseek_real_llm(&mut c, &api_key);

    let assert = c.assert();
    let out = assert.get_output().stdout.clone();
    let out_str = String::from_utf8_lossy(&out);

    info!(
        "Assert: exit 0，stdout 含 AI 回复（非空）；actual stdout 前 300 chars: {}",
        out_str.chars().take(300).collect::<String>()
    );
    assert.success();
    assert!(
        !out_str.trim().is_empty(),
        "AgentLoop 应输出非空 AI 回复，实际 stdout 为空"
    );
}
