use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::io::AsyncWrite;

use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, LlmResolver, LlmScene, ResolvedCall,
    StreamEvent,
};
use crate::{AppConfig, ServeConfig};

use super::registry::SessionSlot;
use super::types::NewSessionParams;
use super::writer::{spawn_writer, WriterConfig, WriterHandle};
use super::{create_session_slot, register_slot_hooks, ServeState, SessionTurnState};
use crate::{
    ensure_work_dir_structure, init_context_state, resolve_sessions_dir, session_key_for_agent,
    AppError, SessionManager, SessionMode,
};

pub const TEST_API_KEY_ENV: &str = "OPENAI_API_KEY";

#[derive(Clone, Default)]
pub struct SharedWriterBuffer(pub Arc<Mutex<Vec<u8>>>);

pub struct VecWriter {
    pub inner: SharedWriterBuffer,
}

impl AsyncWrite for VecWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.inner.0.lock().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub fn spawn_buffered_writer(config: &ServeConfig) -> (WriterHandle, SharedWriterBuffer) {
    let shared = SharedWriterBuffer::default();
    let writer = VecWriter {
        inner: shared.clone(),
    };
    let handle = spawn_writer(Box::pin(writer), WriterConfig::from(config));
    (handle, shared)
}

pub fn read_ndjson_lines(buffer: &SharedWriterBuffer) -> Vec<serde_json::Value> {
    let text = String::from_utf8(buffer.0.lock().clone()).expect("writer buffer is utf8");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("ndjson line"))
        .collect()
}

pub struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => unsafe { std::env::set_var(self.key, prev) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

pub fn write_models_override(work_dir: &Path, base_url: &str) {
    std::fs::write(
        work_dir.join("models.toml"),
        format!(
            r#"
[[models]]
id = "gpt-5.4"
api = "openai"
provider = "openai"
api_key_env = "{TEST_API_KEY_ENV}"
base_url = "{base_url}"

[models.capabilities]
vision = true
files = true
tools = true
reasoning = true
web_search = false
"#
        ),
    )
    .expect("write models override");
}

pub fn serve_test_config(work_dir: &Path, base_url: &str) -> AppConfig {
    write_models_override(work_dir, base_url);
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg.llm.default_model = "gpt-5.4".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    cfg.skills.enabled = false;
    cfg.serve = ServeConfig {
        max_sessions: 8,
        max_buffered_frames: 8,
        delta_coalesce_ms: 10,
        ..ServeConfig::default()
    };
    cfg
}

#[allow(dead_code)]
pub async fn build_initialized_state(
    base_url: &str,
) -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), base_url);
    ensure_work_dir_structure(&cfg).expect("work dir");
    let (writer, buffer) = spawn_buffered_writer(&cfg.serve);
    let state = ServeState::new(cfg, writer);
    let slot = create_session_slot(Arc::clone(&state), NewSessionParams::default(), false)
        .await
        .expect("initial session");
    state
        .registry
        .insert(Arc::clone(&slot))
        .expect("insert initial session");
    register_slot_hooks(&state, &slot);
    state
        .initialized
        .store(true, std::sync::atomic::Ordering::SeqCst);
    (state, buffer, temp, slot)
}

pub struct DeterministicMockLlm {
    streams: Mutex<std::collections::VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl DeterministicMockLlm {
    pub fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[derive(Clone, Default)]
pub struct SharedRequests(pub Arc<Mutex<Vec<ChatRequest>>>);

pub struct RecordingMockLlm {
    streams: Mutex<std::collections::VecDeque<Vec<Result<StreamEvent, AppError>>>>,
    requests: SharedRequests,
}

impl RecordingMockLlm {
    pub fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> (Self, SharedRequests) {
        let requests = SharedRequests::default();
        (
            Self {
                streams: Mutex::new(streams.into()),
                requests: requests.clone(),
            },
            requests,
        )
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
        let events =
            self.streams.lock().pop_front().ok_or_else(|| {
                AppError::Llm("DeterministicMockLlm: no more streams".to_string())
            })?;
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[async_trait]
impl LlmProvider for RecordingMockLlm {
    fn provider_name(&self) -> &str {
        "recording-mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("recording mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.0.lock().push(req);
        let events = self.streams.lock().pop_front().ok_or_else(|| {
            AppError::Llm("RecordingMockLlm: no more streams".to_string())
        })?;
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

pub struct FixedResolver {
    catalog: Arc<crate::core::llm::ModelCatalog>,
    provider: Arc<dyn LlmProvider>,
    default_model: String,
}

impl FixedResolver {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        default_model: impl Into<String>,
        catalog: Arc<crate::core::llm::ModelCatalog>,
    ) -> Self {
        Self {
            catalog,
            provider,
            default_model: default_model.into(),
        }
    }
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        _scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let entry = self
            .catalog
            .lookup_explicit(session_override.unwrap_or(&self.default_model))?;
        Ok(ResolvedCall {
            provider_impl: Arc::clone(&self.provider),
            model: entry.id,
            api: entry.api,
            provider: entry.provider,
            base_url: entry.base_url,
            key_source: "test".to_string(),
            thinking_format: ThinkingFormat::Auto,
            capabilities: entry.capabilities,
        })
    }
}

pub struct PanickingMockLlm;

#[async_trait]
impl LlmProvider for PanickingMockLlm {
    fn provider_name(&self) -> &str {
        "panic-mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        panic!("panic mock chat should not be called")
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        panic!("panic mock stream")
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

async fn build_initialized_state_with_provider(
    temp: tempfile::TempDir,
    cfg: AppConfig,
    provider: Arc<dyn LlmProvider>,
) -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
) {
    ensure_work_dir_structure(&cfg).expect("work dir");
    let (writer, buffer) = spawn_buffered_writer(&cfg.serve);
    let state = ServeState::new(cfg.clone(), writer);

    let cwd_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sessions_dir = resolve_sessions_dir(&cfg).expect("sessions dir");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let session_key = session_key_for_agent(&cfg.agent.id, SessionMode::Code, &cwd_path);
    let session_manager = SessionManager::new_scoped(sessions_dir, session_key);
    let cwd_string = Some(cwd_path.to_string_lossy().to_string());
    let current_entry = session_manager
        .ensure_current_session(cwd_string.clone())
        .expect("current session");
    session_manager.pin_session(&current_entry.session_id);

    let overrides = ChatContextOverrides::default()
        .suppress_cli_output()
        .with_shared_agent_registry(Arc::clone(&state.shared_agent_registry))
        .with_session_cwd_override(cwd_path.clone());
    let mut ctx =
        ChatContext::from_config_with_mode_and_overrides(cfg.clone(), SessionMode::Code, overrides)
            .expect("chat context");
    state
        .shared_event_bus
        .register_session_bus(current_entry.session_id.clone(), ctx.global_services.event_bus.clone());
    let ask_panel = state.ask_question.panel_for_session(
        ctx.global_services.event_bus.clone(),
        &current_entry.session_id,
    );
    ctx.session_runtime
        .plan_runtime
        .attach_ask_question_panel(ask_panel);
    ctx.global_services.llm = Arc::clone(&provider);
    ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(
        provider,
        "gpt-5.4",
        Arc::clone(&ctx.global_services.model_catalog),
    ));

    let context_budget_chars =
        crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let system_text = crate::api::chat::build_system_text(&ctx, context_budget_chars).await;
    let context_state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        &system_text,
    )
    .expect("context state");
    if let Err(err) = ctx
        .session_runtime
        .plan_runtime
        .attach_from_event(context_state.latest_plan_event.clone())
    {
        tracing::warn!(error = %err, "plan_runtime attach_from_event failed during serve slot init");
    }

    let slot = Arc::new(SessionSlot::new(
        current_entry.session_id.clone(),
        Arc::new(ctx),
        SessionMode::Code,
        cwd_string,
        SessionTurnState {
            context_state,
            system_text,
            context_budget_chars,
        },
    ));
    state
        .registry
        .insert(Arc::clone(&slot))
        .expect("insert initial session");
    register_slot_hooks(&state, &slot);
    state
        .initialized
        .store(true, std::sync::atomic::Ordering::SeqCst);
    (state, buffer, temp, slot)
}

pub async fn build_initialized_state_with_streams(
    streams: Vec<Vec<Result<StreamEvent, AppError>>>,
) -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(streams));
    build_initialized_state_with_provider(temp, cfg, provider).await
}

pub async fn build_initialized_state_with_recorded_streams(
    streams: Vec<Vec<Result<StreamEvent, AppError>>>,
) -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
    SharedRequests,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let (provider, requests) = RecordingMockLlm::new(streams);
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let (state, buffer, temp, slot) = build_initialized_state_with_provider(temp, cfg, provider).await;
    (state, buffer, temp, slot, requests)
}

pub async fn build_initialized_state_with_streams_and_max_sessions(
    max_sessions: usize,
    streams: Vec<Vec<Result<StreamEvent, AppError>>>,
) -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.serve.max_sessions = max_sessions;
    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(streams));
    build_initialized_state_with_provider(temp, cfg, provider).await
}

pub async fn build_initialized_state_with_panicking_provider() -> (
    Arc<ServeState>,
    SharedWriterBuffer,
    tempfile::TempDir,
    Arc<SessionSlot>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(PanickingMockLlm);
    build_initialized_state_with_provider(temp, cfg, provider).await
}

#[allow(dead_code)]
pub fn spawn_quick_openai_stream_server(
    reply: &'static str,
) -> (String, std::thread::JoinHandle<()>) {
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
                    if served >= 8 {
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

pub fn install_test_api_key() -> EnvGuard {
    EnvGuard::set(TEST_API_KEY_ENV, "test-key")
}
