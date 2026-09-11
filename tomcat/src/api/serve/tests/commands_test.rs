use super::*;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use crate::core::llm::multimodal::{
    UNSUPPORTED_FILE_INPUT_PLACEHOLDER, UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER,
};
use crate::core::llm::{
    Capabilities, ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatRequest,
    ChatResponse, ChatResponseChoice, ContextRefKind, FileSource, ImageSource, LlmProvider,
    MessageKind, ModelEntryInput, StreamEvent,
};
use crate::{
    init_context_state, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind,
    CheckpointMeta, CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, ListOptions,
    RestoreOptions,
};

// ── 附件测试脚手架 ────────────────────────────────────────────────────
//
// 协议上只有 ingest_attachment 携带字节，因此测试也必须先把字节交给后端换回哈希，
// 再用哈希去发送 —— 这跟真实客户端走的是同一条路。

/// 1x1 PNG，最小的合法位图。
fn test_png_bytes() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0x60,
        0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

fn test_pdf_bytes() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n".to_vec()
}

/// `/compact` calls the non-streaming compaction scene, unlike normal agent turns.
/// Keep this explicit so the serve command E2E catches accidental routing to chat_stream.
struct CompactOnlyProvider {
    streams: parking_lot::Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl CompactOnlyProvider {
    fn with_streams(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: parking_lot::Mutex::new(streams.into()),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for CompactOnlyProvider {
    fn provider_name(&self) -> &str {
        "compact_only"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        Ok(ChatResponse {
            id: Some("compact-summary".to_string()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("Compacted conversation summary."),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let events = self.streams.lock().pop_front().ok_or_else(|| {
            AppError::Llm("unexpected chat_stream after manual compact".to_string())
        })?;
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// A scripted stream whose final Usage can be held back so tests can inspect
/// serve state while a second request is still in flight.
struct ChannelMockLlm {
    streams: parking_lot::Mutex<VecDeque<mpsc::UnboundedReceiver<Result<StreamEvent, AppError>>>>,
}

impl ChannelMockLlm {
    fn new(streams: Vec<mpsc::UnboundedReceiver<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: parking_lot::Mutex::new(streams.into()),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for ChannelMockLlm {
    fn provider_name(&self) -> &str {
        "channel-mock"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("channel mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let receiver = self
            .streams
            .lock()
            .pop_front()
            .ok_or_else(|| AppError::Llm("ChannelMockLlm: no more streams".to_string()))?;
        Ok(Box::new(
            tokio_stream::wrappers::UnboundedReceiverStream::new(receiver),
        ))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// 把字节落进这个会话的 blob store，返回它的 sha —— 等价于走一遍 ingest_attachment。
fn ingest_test_bytes(slot: &Arc<crate::api::serve::registry::SessionSlot>, bytes: &[u8]) -> String {
    let sha = slot
        .ctx
        .session_runtime
        .session
        .attachment_store()
        .put(bytes)
        .expect("store attachment bytes");
    slot.ctx
        .session_runtime
        .session
        .attachment_store()
        .mark_pending(&slot.session_id, &sha)
        .expect("lease attachment bytes");
    sha
}

/// 一个不存在于任何 blob store 里的、格式合法的 sha —— 用来验证伪造哈希被拒。
fn forged_sha() -> String {
    "f".repeat(64)
}

/// 在临时 store 上装配一条消息。
///
/// `build_user_message` 只依赖 blob store 而不依赖整个 session slot，
/// 所以这类纯装配逻辑的测试不需要拉起一个会话。
fn build_message_for_test(
    text: &str,
    params: &ServeMessageParams,
) -> Result<crate::core::llm::ChatMessage, String> {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store = crate::core::session::attachments::AttachmentBlobStore::new(tmp.path());
    crate::api::serve::commands::build_user_message(
        &store,
        text.to_string(),
        params,
        crate::api::serve::commands::AttachmentBytes::Archival,
    )
}

struct CurrentDirGuard {
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

async fn wait_for_line(
    buffer: &crate::api::serve::test_support::SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

fn count_event(lines: &[serde_json::Value], event_type: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some(event_type))
        .count()
}

fn latest_user_request_parts(request: &ChatRequest) -> &[ChatMessageContentPart] {
    let user_message = latest_persisted_user_request(request);
    let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
        panic!(
            "expected multimodal user parts, got {:?}",
            user_message.content
        );
    };
    parts
}

fn latest_persisted_user_request(request: &ChatRequest) -> &ChatMessage {
    request
        .messages
        .iter()
        .rev()
        .find(|message| {
            matches!(message.role, crate::core::llm::ChatMessageRole::User)
                && message.kind != crate::core::llm::MessageKind::EphemeralTail
        })
        .expect("persisted user message")
}

fn request_has_input_file(request: &ChatRequest) -> bool {
    latest_user_request_parts(request)
        .iter()
        .any(|part| matches!(part, ChatMessageContentPart::InputFile { .. }))
}

fn request_has_file_placeholder(request: &ChatRequest) -> bool {
    latest_user_request_parts(request).iter().any(|part| {
        matches!(
            part,
            ChatMessageContentPart::InputText { text }
                if text.contains(UNSUPPORTED_FILE_INPUT_PLACEHOLDER)
        )
    })
}

fn first_event_index(lines: &[serde_json::Value], event_type: &str) -> Option<usize> {
    lines
        .iter()
        .position(|line| line.get("type").and_then(serde_json::Value::as_str) == Some(event_type))
}

fn append_history_message(
    slot: &Arc<crate::api::serve::SessionSlot>,
    role: &str,
    content: &str,
) -> String {
    slot.ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": role,
                "content": content,
            }),
        )
        .expect("append history message")
}

fn payload_message_ids(response: &serde_json::Value) -> Vec<String> {
    response["payload"]["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn decode_cursor(cursor: &str) -> serde_json::Value {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cursor.as_bytes())
        .expect("decode cursor");
    serde_json::from_slice(&bytes).expect("parse cursor json")
}

fn session_message_entries(
    slot: &Arc<crate::api::serve::SessionSlot>,
) -> Vec<crate::core::session::transcript::MessageEntry> {
    slot.ctx
        .session_runtime
        .session
        .get_entries_for_session(&slot.session_id, 256)
        .expect("read session entries")
        .into_iter()
        .filter_map(|entry| match entry {
            crate::core::session::transcript::TranscriptEntry::Message(message) => Some(message),
            _ => None,
        })
        .collect()
}

fn count_message_entries_with_id(
    slot: &Arc<crate::api::serve::SessionSlot>,
    message_id: &str,
) -> usize {
    session_message_entries(slot)
        .into_iter()
        .filter(|entry| entry.id.as_deref() == Some(message_id))
        .count()
}

fn latest_user_entry(
    slot: &Arc<crate::api::serve::SessionSlot>,
) -> crate::core::session::transcript::MessageEntry {
    session_message_entries(slot)
        .into_iter()
        .rev()
        .find(|entry| {
            entry
                .message
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("user")
        })
        .expect("latest user entry")
}

fn unsupported_file_input_stream(message: &str) -> Vec<Result<StreamEvent, crate::AppError>> {
    vec![
        Ok(StreamEvent::LlmError {
            reason: "error:invalid_request_error".to_string(),
            message: message.to_string(),
            code: Some("invalid_request_error".to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error:invalid_request_error".to_string(),
        }),
    ]
}

fn content_filter_stream() -> Vec<Result<StreamEvent, crate::AppError>> {
    vec![
        Ok(StreamEvent::LlmError {
            reason: "error".to_string(),
            message: "content_filter".to_string(),
            code: None,
        }),
        Ok(StreamEvent::FinishReason {
            reason: "error".to_string(),
        }),
    ]
}

fn ok_text_stream(text: &str) -> Vec<Result<StreamEvent, crate::AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

async fn prompt_with_pdf_attachment(
    state: &Arc<ServeState>,
    slot: &Arc<crate::api::serve::registry::SessionSlot>,
    id: &str,
    text: &str,
) {
    handle_command(
        Arc::clone(state),
        ServeCommand::Prompt {
            id: Some(id.to_string()),
            session_id: Some(slot.session_id.clone()),
            text: text.to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("notes.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some(ingest_test_bytes(slot, &test_pdf_bytes())),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
}

#[derive(Debug, Clone)]
struct ScriptedHttpResponse {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl ScriptedHttpResponse {
    fn sse(lines: &[&str]) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            body: lines.join(""),
        }
    }

    fn html(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.to_string(),
        }
    }
}

struct RecordingHttpServer {
    base_url: String,
    requests: Arc<parking_lot::Mutex<Vec<String>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RecordingHttpServer {
    async fn start(initial_responses: Vec<ScriptedHttpResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");
        let requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let responses = Arc::new(parking_lot::Mutex::new(VecDeque::from(initial_responses)));
        let requests_clone = Arc::clone(&requests);
        let responses_clone = Arc::clone(&responses);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { continue; };
                        let request_text = read_full_http_request(&mut socket).await;
                        requests_clone.lock().push(request_text);
                        let response = responses_clone
                            .lock()
                            .pop_front()
                            .unwrap_or_else(|| ScriptedHttpResponse::html(500, "{\"error\":\"unplanned request\"}"));
                        let reason = match response.status {
                            200 => "OK",
                            400 => "Bad Request",
                            401 => "Unauthorized",
                            403 => "Forbidden",
                            429 => "Too Many Requests",
                            500 => "Internal Server Error",
                            _ => "Unknown",
                        };
                        let raw = format!(
                            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response.status,
                            reason,
                            response.content_type,
                            response.body.len(),
                            response.body
                        );
                        let _ = socket.write_all(raw.as_bytes()).await;
                        let _ = socket.shutdown().await;
                    }
                }
            }
        });
        Self {
            base_url,
            requests,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(join_handle),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().clone()
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }
}

async fn read_full_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut header_end = None;
    let mut content_len = 0usize;
    loop {
        let mut chunk = [0u8; 4096];
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if header_end.is_none() {
            if let Some(pos) = find_header_end(&buf) {
                header_end = Some(pos);
                content_len = parse_content_length(&buf[..pos]);
            }
        }
        if let Some(pos) = header_end {
            let body_start = pos + 4;
            if buf.len() >= body_start + content_len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header_bytes: &[u8]) -> usize {
    let header = String::from_utf8_lossy(header_bytes);
    for line in header.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse::<usize>().unwrap_or(0);
        }
    }
    0
}

struct FixedCheckpointStore {
    checkpoints: Vec<CheckpointMeta>,
    restore_report: CheckpointRestoreReport,
}

impl CheckpointStore for FixedCheckpointStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        Ok(self
            .checkpoints
            .first()
            .map(|meta| meta.id.clone())
            .unwrap_or_else(CheckpointId::null))
    }

    fn list(
        &self,
        session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(self
            .checkpoints
            .iter()
            .filter(|meta| meta.session_id == session_id)
            .cloned()
            .collect())
    }

    fn show(&self, id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(self.checkpoints.iter().find(|meta| &meta.id == id).cloned())
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Ok(CheckpointDiff::default())
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Ok(self.restore_report.clone())
    }

    fn prune(&self, _policy: crate::RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

struct SlowCheckpointStore {
    calls: Arc<AtomicUsize>,
    sleep: Duration,
}

impl CheckpointStore for SlowCheckpointStore {
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
        Ok(CheckpointDiff::default())
    }

    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Ok(CheckpointRestoreReport::default())
    }

    fn prune(&self, _policy: crate::RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

fn install_checkpoint_store(
    state: &Arc<ServeState>,
    session_id: &str,
    store: Arc<dyn CheckpointStore>,
) -> Arc<crate::api::serve::SessionSlot> {
    let slot = state
        .registry
        .get(session_id)
        .expect("lookup slot for checkpoint store override");
    // Test-only override: the fixture keeps the same Arc<ChatContext> in multiple places,
    // so we swap the checkpoint store through the shared pointer instead of rebuilding the slot.
    unsafe {
        let ctx_ptr = Arc::as_ptr(&slot.ctx) as *mut ChatContext;
        (*ctx_ptr).scope_services.checkpoint_store = store;
    }
    slot
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_command_routes_by_session_id() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    let first = state.registry.active_session_id().unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("n1".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();
    let sessions = state.registry.list();
    assert_eq!(sessions.len(), 2);
    let second = sessions
        .iter()
        .find(|session| session.session_id != first)
        .unwrap()
        .session_id
        .clone();

    handle_command(
        Arc::clone(&state),
        ServeCommand::SwitchSession {
            id: Some("sw1".to_string()),
            session_id: second.clone(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        state.registry.active_session_id().as_deref(),
        Some(second.as_str())
    );
    let first_slot = state.registry.get(&first).expect("first session slot");
    let second_slot = state.registry.get(&second).expect("second session slot");
    assert!(Arc::ptr_eq(
        &first_slot.ctx.agent_registry,
        &second_slot.ctx.agent_registry
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn detached_new_session_does_not_change_live_or_durable_current() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let active_before = state.registry.active_session_id().unwrap();
    let durable_before = slot
        .ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap();
    let disk_before = slot.ctx.session_runtime.session.list_session_ids().unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("detached-1".to_string()),
            params: NewSessionParams {
                detached: true,
                ..NewSessionParams::default()
            },
        },
    )
    .await
    .unwrap();

    assert_eq!(state.registry.len(), 1, "detached session is disk-only");
    assert_eq!(state.registry.active_session_id(), Some(active_before));
    assert_eq!(
        slot.ctx
            .session_runtime
            .session
            .current_session_id()
            .unwrap(),
        durable_before,
    );
    let disk_after = slot.ctx.session_runtime.session.list_session_ids().unwrap();
    assert_eq!(disk_after.len(), disk_before.len() + 1);
}

#[tokio::test]
#[serial(env_lock)]
async fn detached_target_retain_and_discard_preserves_source_and_reclaims_target_leases() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let source_id = slot.session_id.clone();
    let active_before = state.registry.active_session_id();
    let current_before = slot
        .ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap();
    let store = slot.ctx.session_runtime.session.attachment_store();
    let blob_sha = store.put(b"draft original").unwrap();
    let provider_sha = store.put(b"draft provider rendition").unwrap();
    store.mark_pending(&source_id, &blob_sha).unwrap();
    store.mark_pending(&source_id, &provider_sha).unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("detached-create".to_string()),
            params: NewSessionParams {
                detached: true,
                ..NewSessionParams::default()
            },
        },
    )
    .await
    .unwrap();
    let create_response = wait_for_line(&buffer, |line| line["id"] == "detached-create")
        .await
        .into_iter()
        .find(|line| line["id"] == "detached-create")
        .expect("detached response");
    let target_id = create_response["sessionId"].as_str().unwrap().to_string();

    handle_command(
        Arc::clone(&state),
        ServeCommand::RetainAttachmentLeases {
            id: Some("retain-target".to_string()),
            session_id: target_id.clone(),
            params: RetainAttachmentLeasesParams {
                attachments: vec![RetainAttachmentLeaseRef {
                    blob_sha: blob_sha.clone(),
                    provider_sha: Some(provider_sha.clone()),
                }],
            },
        },
    )
    .await
    .unwrap();
    let retained = store.list_pending(&target_id).unwrap();
    assert_eq!(retained.len(), 2, "blob + provider rendition both retained");
    assert_eq!(state.registry.active_session_id(), active_before);
    assert_eq!(
        slot.ctx
            .session_runtime
            .session
            .current_session_id()
            .unwrap(),
        current_before
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::DiscardDetachedSession {
            id: Some("discard-target".to_string()),
            session_id: target_id.clone(),
        },
    )
    .await
    .unwrap();
    assert!(slot
        .ctx
        .session_runtime
        .session
        .get_session_by_id(&target_id)
        .unwrap()
        .is_none());
    assert!(store.list_pending(&target_id).unwrap().is_empty());
    assert_eq!(state.registry.active_session_id(), active_before);
    assert_eq!(store.list_pending(&source_id).unwrap().len(), 2);
    assert!(store.exists(&blob_sha) && store.exists(&provider_sha));

    handle_command(
        state,
        ServeCommand::DiscardDetachedSession {
            id: Some("discard-target-again".to_string()),
            session_id: target_id,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_same_session_second_prompt_is_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = state.registry.active_session_id().unwrap();
    let slot = state.registry.get(&session_id).unwrap();
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("p2".to_string()),
            session_id: Some(session_id.clone()),
            text: "second".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("p2")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("p2"))
        .unwrap();
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("busy")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_unknown_session_returns_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("unknown-1".to_string()),
            session_id: Some("missing-session".to_string()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("unknown-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("unknown_session")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_emits_assistant_message_id_on_stream_and_turn_end() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello stable id".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-stable-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let message_start_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_start"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_start assistantMessageId");
    let message_update_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_update"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_update assistantMessageId");
    let message_end_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("message_end"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("message_end assistantMessageId");
    let turn_end_id = lines
        .iter()
        .find(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("turn_end"))
        .and_then(|line| line.get("assistantMessageId"))
        .and_then(serde_json::Value::as_str)
        .expect("turn_end assistantMessageId");

    assert_eq!(message_update_id, message_start_id);
    assert_eq!(message_end_id, message_start_id);
    assert_eq!(turn_end_id, message_start_id);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_new_session_rejects_when_registry_is_full() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_streams_and_max_sessions(1, vec![]).await;
    let before_ids = slot
        .ctx
        .session_runtime
        .session
        .list_session_ids()
        .expect("list session ids before rejection");
    let before_current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session before rejection")
        .expect("current session should exist");

    handle_command(
        Arc::clone(&state),
        ServeCommand::NewSession {
            id: Some("full-1".to_string()),
            params: NewSessionParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("full-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("full-1"))
        .unwrap();
    assert_eq!(
        response.get("error").and_then(serde_json::Value::as_str),
        Some("too_many_sessions")
    );
    let after_ids = slot
        .ctx
        .session_runtime
        .session
        .list_session_ids()
        .expect("list session ids after rejection");
    let after_current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session after rejection")
        .expect("current session should still exist");
    assert_eq!(state.registry.len(), 1, "registry should remain unchanged");
    assert_eq!(
        after_ids, before_ids,
        "rejected new_session must not create transcript files"
    );
    assert_eq!(
        after_current.session_id, before_current.session_id,
        "rejected new_session must not repoint current session"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_drives_agent_run() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
        Ok(StreamEvent::Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(2),
            reasoning_tokens: None,
            text_tokens: None,
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    let session_id = state.registry.active_session_id().unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("p1".to_string()),
            session_id: Some(session_id.clone()),
            text: "say hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("p1")
                && line.get("success").and_then(serde_json::Value::as_bool) == Some(true)
        }),
        "expected prompt acceptance response, got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_start")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(session_id.as_str())
        }),
        "expected agent_start, got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(session_id.as_str())
        }),
        "expected agent_end, got {lines:?}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-usage".to_string()),
            session_id: Some(session_id),
        },
    )
    .await
    .unwrap();
    let state_lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-usage")
    })
    .await;
    let state_response = state_lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-usage")
        })
        .expect("get_state response after usage");
    assert!(
        state_response["payload"]["contextUtilizationRatio"]
            .as_f64()
            .is_some_and(|ratio| ratio > 0.0),
        "get_state must replay the nonzero provider measurement: {state_response:?}"
    );
    assert!(
        slot.last_context_ratio.lock().is_some(),
        "matching metrics event must populate the slot cache"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_second_prompt_keeps_waterline_while_usage_is_pending() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let (first_tx, first_rx) = mpsc::unbounded_channel();
    for event in [
        StreamEvent::ContentDelta {
            delta: "first".to_string(),
        },
        StreamEvent::FinishReason {
            reason: "stop".to_string(),
        },
        StreamEvent::Usage {
            prompt_tokens: 120,
            completion_tokens: 10,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(130),
            reasoning_tokens: None,
            text_tokens: None,
        },
    ] {
        first_tx.send(Ok(event)).expect("queue first stream event");
    }
    drop(first_tx);

    let (second_tx, second_rx) = mpsc::unbounded_channel();
    for event in [
        StreamEvent::ContentDelta {
            delta: "second".to_string(),
        },
        StreamEvent::FinishReason {
            reason: "stop".to_string(),
        },
    ] {
        second_tx
            .send(Ok(event))
            .expect("queue second stream prefix");
    }
    let provider: Arc<dyn LlmProvider> = Arc::new(ChannelMockLlm::new(vec![first_rx, second_rx]));
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;
    let session_id = slot.session_id.clone();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("waterline-first".to_string()),
            session_id: Some(session_id.clone()),
            text: "first".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .expect("first prompt");
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("sessionId").and_then(serde_json::Value::as_str)
                == Some(session_id.as_str())
    })
    .await;
    assert!(
        slot.last_context_ratio.lock().is_some(),
        "the first provider Usage must establish a displayable waterline"
    );
    buffer.0.lock().clear();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("waterline-second".to_string()),
            session_id: Some(session_id.clone()),
            text: "second".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .expect("second prompt");
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str)
            == Some(wire::WIRE_CONTEXT_METRICS_UPDATE)
            && line
                .get("providerUsageMeasured")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
    })
    .await;
    assert!(
        slot.last_context_ratio.lock().is_some(),
        "the second request's pre-Usage estimate must refresh, never blank, the waterline"
    );

    second_tx
        .send(Ok(StreamEvent::Usage {
            prompt_tokens: 150,
            completion_tokens: 12,
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(162),
            reasoning_tokens: None,
            text_tokens: None,
        }))
        .expect("release second Usage");
    drop(second_tx);
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("sessionId").and_then(serde_json::Value::as_str)
                == Some(session_id.as_str())
    })
    .await;
    assert!(
        slot.last_context_ratio.lock().is_some(),
        "the second provider Usage must continue to refresh the established waterline"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_resume_rejects_a_dangling_ask_question_before_hydration_repairs_it() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_streams(vec![ok_text_stream("must not run")]).await;
    slot.ctx
        .session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "需要一个选择",
            "tool_calls": [{
                "id": "restart-ask-1",
                "type": "function",
                "function": {
                    "name": "ask_question",
                    "arguments": "{\"questions\":[{\"id\":\"q1\",\"prompt\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"recommended\":true},{\"id\":\"no\",\"label\":\"No\",\"recommended\":false}]}]}"
                }
            }]
        }))
        .expect("seed dangling ask_question");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Resume {
            id: Some("resume-dangling-ask".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("resume-dangling-ask")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("resume-dangling-ask")
        })
        .expect("structured Resume rejection");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(response["error"].as_str(), Some("nothing_to_resume"));
}

#[tokio::test]
#[serial(env_lock)]
async fn register_slot_hooks_auto_rearms_pending_ask_question_on_session_attach() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "attach-resumed".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]]));
    ensure_work_dir_structure(&cfg).expect("work dir");
    let (writer, buffer) = spawn_buffered_writer(&cfg.serve);
    let shared_model_prefs = build_shared_model_prefs(&cfg).expect("shared model preferences");
    let state = ServeState::new(cfg.clone(), writer, shared_model_prefs).expect("serve state");
    let cwd_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sessions_dir = crate::resolve_sessions_dir(&cfg).expect("sessions dir");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let session_key =
        crate::session_key_for_agent(&cfg.agent.id, crate::SessionMode::Code, &cwd_path);
    let session_manager = crate::SessionManager::new_scoped(sessions_dir, session_key);
    let cwd_string = Some(cwd_path.to_string_lossy().to_string());
    let current_entry = session_manager
        .ensure_current_session(cwd_string.clone())
        .expect("current session");
    session_manager.pin_session(&current_entry.session_id);
    let overrides = crate::api::chat::ChatContextOverrides::default()
        .suppress_cli_output()
        .with_shared_agent_registry(Arc::clone(&state.shared_agent_registry))
        .with_shared_model_prefs(Arc::clone(&state.shared_model_prefs))
        .with_session_cwd_override(cwd_path.clone());
    let mut ctx = crate::api::chat::ChatContext::from_config_with_mode_and_overrides(
        cfg.clone(),
        crate::SessionMode::Code,
        overrides,
    )
    .expect("chat context");
    state.shared_event_bus.register_session_bus(
        current_entry.session_id.clone(),
        ctx.global_services.event_bus.clone(),
    );
    let ask_panel = state.ask_question.panel_for_session(
        ctx.global_services.event_bus.clone(),
        &current_entry.session_id,
    );
    ctx.session_runtime
        .plan_runtime
        .attach_ask_question_panel(ask_panel);
    ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(
        provider,
        "gpt-5.4",
        ctx.global_services.model_catalog.snapshot(),
    ));
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "需要一个选择",
            "tool_calls": [{
                "id": "auto-ask-1",
                "type": "function",
                "function": {
                    "name": "ask_question",
                    "arguments": "{\"questions\":[{\"id\":\"q1\",\"prompt\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"recommended\":true},{\"id\":\"no\",\"label\":\"No\",\"recommended\":false}]}]}"
                }
            }]
        }))
        .expect("seed pending ask_question before hook registration");
    let context_budget_chars =
        crate::infra::config::compute_context_budget_chars(&ctx.config.context);
    let prompt_snapshot = crate::api::chat::build_prompt_snapshot(&ctx, context_budget_chars).await;
    let context_state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        prompt_snapshot.system_text(),
    )
    .expect("context state");
    let slot = Arc::new(crate::api::serve::registry::SessionSlot::new(
        current_entry.session_id.clone(),
        Arc::new(ctx),
        crate::SessionMode::Code,
        cwd_string,
        crate::api::serve::registry::SessionTurnState {
            context_state,
            prompt_snapshot,
            context_budget_chars,
        },
    ));
    state
        .registry
        .insert(Arc::clone(&slot))
        .expect("insert initial session");
    register_slot_hooks(&state, &slot);
    handle_command(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "initialize-before-rearm".to_string(),
            subtype: "initialize".to_string(),
            session_id: Some(slot.session_id.clone()),
            payload: serde_json::Value::Null,
        },
    )
    .await
    .expect("initialize handshake");

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
            && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
    })
    .await;
    let request = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
                && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
        })
        .expect("slot attach should rearm pending ask_question");
    assert_eq!(
        request["payload"]["toolCallId"].as_str(),
        Some("auto-ask-1")
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ControlResponse {
            request_id: request["requestId"]
                .as_str()
                .expect("control request id")
                .to_string(),
            session_id: Some(slot.session_id.clone()),
            payload: serde_json::json!({
                "requestId": request["payload"]["requestId"],
                "result": {
                    "outcome": "answered",
                    "cancelled": false,
                    "answers": [{
                        "questionId": "q1",
                        "optionIds": ["yes"],
                        "pickedRecommended": true
                    }]
                }
            }),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").is_none_or(serde_json::Value::is_null)
        }),
        "re-armed attach flow should finish the recovered turn: {lines:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_skips_pending_ask_question_before_persisting_new_input() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "new prompt handled".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    slot.ctx
        .session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "需要一个选择",
            "tool_calls": [{
                "id": "skip-ask-1",
                "type": "function",
                "function": {
                    "name": "ask_question",
                    "arguments": "{\"questions\":[{\"id\":\"q1\",\"prompt\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"recommended\":true},{\"id\":\"no\",\"label\":\"No\",\"recommended\":false}]}]}"
                }
            }]
        }))
        .expect("seed dangling ask_question");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("new-prompt-after-question".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "跳过旧问题，继续做别的".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").is_none_or(serde_json::Value::is_null)
        }),
        "a new prompt must not be blocked by the old pending question: {lines:?}"
    );

    let entries = session_message_entries(&slot);
    let skip = entries
        .iter()
        .find(|message| {
            message
                .message
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("tool")
                && message
                    .message
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    == Some("skip-ask-1")
        })
        .expect("old ask_question should be recorded as skipped before prompt append");
    let result: serde_json::Value = serde_json::from_str(
        skip.message["content"]
            .as_str()
            .expect("tool result content"),
    )
    .expect("skip result JSON");
    assert_eq!(result["outcome"], "skipped");
    assert_eq!(result["cancelled"], true);

    let skipped_index = entries
        .iter()
        .position(|message| message.id == skip.id)
        .expect("skip entry index");
    let prompt_index = entries
        .iter()
        .position(|message| {
            message
                .message
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("user")
                && message
                    .message
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    == Some("跳过旧问题，继续做别的")
        })
        .expect("new prompt should be persisted");
    assert!(
        skipped_index < prompt_index,
        "skipped tool result must precede the new user message"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_resume_replaces_legacy_synthetic_ask_question_result() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "已根据选择继续".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    slot.ctx
        .session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": "需要一个选择",
            "tool_calls": [{
                "id": "legacy-ask-1",
                "type": "function",
                "function": {
                    "name": "ask_question",
                    "arguments": "{\"questions\":[{\"id\":\"q1\",\"prompt\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"recommended\":true},{\"id\":\"no\",\"label\":\"No\",\"recommended\":false}]}]}"
                }
            }]
        }))
        .expect("seed ask_question declaration");
    slot.ctx
        .session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "tool",
            "tool_call_id": "legacy-ask-1",
            "content": "{\"answers\":[],\"cancelled\":true,\"outcome\":\"host_disconnected\"}"
        }))
        .expect("seed legacy synthetic ask_question result");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Resume {
            id: Some("resume-legacy-ask".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
            && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
    })
    .await;
    let request = lines
        .iter()
        .find(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
                && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
        })
        .expect("legacy result should be retried through the host");
    assert_eq!(
        request["payload"]["toolCallId"].as_str(),
        Some("legacy-ask-1")
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ControlResponse {
            request_id: request["requestId"]
                .as_str()
                .expect("control request id")
                .to_string(),
            session_id: Some(slot.session_id.clone()),
            payload: serde_json::json!({
                "requestId": request["payload"]["requestId"],
                "result": {
                    "outcome": "answered",
                    "cancelled": false,
                    "answers": [{
                        "questionId": "q1",
                        "optionIds": ["no"],
                        "pickedRecommended": false
                    }]
                }
            }),
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    let results = session_message_entries(&slot)
        .into_iter()
        .filter(|message| {
            message
                .message
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("tool")
                && message
                    .message
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    == Some("legacy-ask-1")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results.len(),
        2,
        "legacy placeholder should be superseded and followed by one real result"
    );
    assert_eq!(
        results
            .iter()
            .filter(|message| {
                message
                    .message
                    .get("superseded")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
            })
            .count(),
        1,
        "there must be exactly one active result after recovery"
    );
    let result: serde_json::Value = serde_json::from_str(
        results
            .iter()
            .find(|message| {
                message
                    .message
                    .get("superseded")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
            })
            .expect("active recovered result")
            .message["content"]
            .as_str()
            .expect("tool result content"),
    )
    .expect("ask_question result JSON");
    assert_eq!(result["outcome"], "answered");
    assert_eq!(result["answers"][0]["option_ids"][0], "no");
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_emits_agent_idle_after_agent_end_and_marks_slot_idle() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    let checkpoint_calls = Arc::new(AtomicUsize::new(0));
    install_checkpoint_store(
        &state,
        &slot.session_id,
        Arc::new(SlowCheckpointStore {
            calls: Arc::clone(&checkpoint_calls),
            sleep: Duration::from_millis(500),
        }),
    );

    let started = Instant::now();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-idle".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "say hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_line(&buffer, |line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_idle")
        }),
    )
    .await
    .expect("agent_idle must not wait for a slow TurnEnd checkpoint");
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "TurnEnd checkpoint must not delay agent_idle"
    );
    assert_eq!(
        count_event(&lines, "agent_end"),
        1,
        "expected one agent_end: {lines:?}"
    );
    assert_eq!(
        count_event(&lines, "agent_idle"),
        1,
        "expected one agent_idle: {lines:?}"
    );
    assert!(
        first_event_index(&lines, "agent_end") < first_event_index(&lines, "agent_idle"),
        "agent_idle must arrive after agent_end: {lines:?}"
    );
    assert!(
        !slot.is_busy(),
        "observing agent_idle implies slot should already be idle"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-idle".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-idle")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-idle"))
        .expect("state-after-idle response");
    assert_eq!(response["payload"]["busy"].as_bool(), Some(false));

    tokio::time::timeout(Duration::from_secs(2), async {
        while checkpoint_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("TurnEnd checkpoint should still be recorded in the background");
}

#[tokio::test(flavor = "current_thread")]
#[serial(env_lock)]
async fn serve_prompt_with_precancelled_turn_emits_agent_idle_once() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "should never finish normally".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-precancel".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "interrupt immediately".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    slot.ctx.session_runtime.cancel_token.lock().cancel();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_idle")
    })
    .await;
    assert_eq!(
        count_event(&lines, "agent_idle"),
        1,
        "expected one agent_idle: {lines:?}"
    );
    let interrupted_end = lines.iter().find(|line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str) == Some("interrupted")
    });
    assert!(
        interrupted_end.is_some(),
        "precancelled turn should terminate as interrupted: {lines:?}"
    );
    assert!(
        !slot.is_busy(),
        "precancelled interrupted turn should leave the slot idle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(env_lock)]
async fn serve_interrupt_emits_idle_without_waiting_for_slow_checkpoint() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let (stream_tx, stream_rx) = mpsc::unbounded_channel();
    let provider: Arc<dyn LlmProvider> = Arc::new(ChannelMockLlm::new(vec![stream_rx]));
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;
    let checkpoint_calls = Arc::new(AtomicUsize::new(0));
    install_checkpoint_store(
        &state,
        &slot.session_id,
        Arc::new(SlowCheckpointStore {
            calls: Arc::clone(&checkpoint_calls),
            sleep: Duration::from_millis(500),
        }),
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-interrupt-background-checkpoint".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "start then interrupt".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    stream_tx
        .send(Ok(StreamEvent::ContentDelta {
            delta: "partial reply".to_string(),
        }))
        .expect("active stream receiver");
    tokio::time::timeout(
        Duration::from_secs(1),
        wait_for_line(&buffer, |line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("message_update")
        }),
    )
    .await
    .expect("stream delta should reach the event writer");

    let started = Instant::now();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-background-checkpoint".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    let lines = tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_line(&buffer, |line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_idle")
        }),
    )
    .await
    .expect("agent_idle must not wait for a slow Interrupt checkpoint");

    assert!(
        started.elapsed() < Duration::from_millis(300),
        "Interrupt checkpoint must not delay agent_idle"
    );
    assert_eq!(count_event(&lines, "agent_interrupted"), 1);
    assert_eq!(count_event(&lines, "agent_idle"), 1);
    assert!(!slot.is_busy());

    tokio::time::timeout(Duration::from_secs(2), async {
        while checkpoint_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Interrupt checkpoint should still be recorded in the background");
}

#[tokio::test(flavor = "current_thread")]
#[serial(env_lock)]
async fn serve_prompt_installs_fresh_cancel_token_before_spawned_turn_runs() {
    let _api_key = install_test_api_key();
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![vec![]]).await;
    let previous = slot.ctx.session_runtime.cancel_token.lock().clone();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("replace-token".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let current = slot.ctx.session_runtime.cancel_token.lock().clone();
    previous.cancel();
    assert!(
        !current.is_cancelled(),
        "accepted prompt should replace the prior cancel token before the spawned turn observes it"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_image_attachment_builds_multimodal_message() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "vision ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("img-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "describe this".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: None,
                    mime_type: None,
                    blob_sha: None,
                    provider_sha: None,
                    file_id: Some("file-vision".to_string()),
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    assert_eq!(captured.len(), 1, "expected exactly one LLM request");
    let user_message = latest_persisted_user_request(&captured[0]);
    let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
        panic!(
            "expected multimodal parts user message, got {:?}",
            user_message.content
        );
    };
    assert_eq!(parts.len(), 2, "expected text + image parts");
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "describe this"
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputImage {
            source: ImageSource::Uploaded(ref uploaded),
            ..
        } if uploaded.file_id == "file-vision"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_follow_up_with_attachment_queues_multimodal_message_when_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::FollowUp {
            id: Some("fu-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "look at this too".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: None,
                    mime_type: None,
                    blob_sha: None,
                    provider_sha: None,
                    file_id: Some("file-follow-up".to_string()),
                }],
                user_message_id: Some("follow-up-fixed-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("fu-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("fu-1"))
        .expect("queued follow_up response");
    assert_eq!(response["payload"]["queued"].as_bool(), Some(true));

    let queue = slot.ctx.session_runtime.follow_up_queue.lock();
    assert_eq!(queue.len(), 1, "expected one queued follow_up");
    assert_eq!(queue[0].msg_id.as_deref(), Some("follow-up-fixed-id"));
    let Some(ChatMessageContent::Parts(parts)) = &queue[0].content else {
        panic!(
            "expected queued multimodal follow_up, got {:?}",
            queue[0].content
        );
    };
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputImage {
            source: ImageSource::Uploaded(ref uploaded),
            ..
        } if uploaded.file_id == "file-follow-up"
    ));
    drop(queue);
    assert_eq!(
        count_message_entries_with_id(&slot, "follow-up-fixed-id"),
        1
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_invalid_attachment_returns_error() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("bad-attachment".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "bad".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: None,
                    mime_type: None,
                    blob_sha: Some(ingest_test_bytes(&slot, &test_png_bytes())),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-attachment")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-attachment"))
        .expect("invalid attachment response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("invalid_attachment: image attachment requires mimeType")
    );
    assert!(
        requests.0.lock().is_empty(),
        "invalid attachment should not reach LLM"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_file_attachment_without_filename_returns_error() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("bad-file-attachment".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "bad file".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: None,
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some(ingest_test_bytes(&slot, &test_pdf_bytes())),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-file-attachment")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("bad-file-attachment")
        })
        .expect("invalid file attachment response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("invalid_attachment: file attachment requires filename")
    );
    assert!(
        requests.0.lock().is_empty(),
        "invalid file attachment should not reach LLM"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_non_pdf_file_attachment_returns_error() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("bad-file-type".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "bad file type".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("notes.md".to_string()),
                    mime_type: Some("text/markdown".to_string()),
                    blob_sha: Some(forged_sha()),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-file-type")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-file-type"))
        .expect("non-pdf file attachment response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some(
            "invalid_attachment: file attachments only support application/pdf; use kind=image for images (got text/markdown)"
        )
    );
    assert!(
        requests.0.lock().is_empty(),
        "non-pdf file attachment should not reach LLM"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_inline_file_attachment_builds_multimodal_message() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("file-inline-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "summarize file".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("notes.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some(ingest_test_bytes(&slot, &test_pdf_bytes())),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let user_message = latest_persisted_user_request(&captured[0]);
    let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
        panic!(
            "expected multimodal parts user message, got {:?}",
            user_message.content
        );
    };
    assert_eq!(parts.len(), 2, "expected text + file parts");
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "summarize file"
    ));
    let expected_pdf = base64::engine::general_purpose::STANDARD.encode(test_pdf_bytes());
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputFile {
            source: FileSource::Inline(ref inline),
        } if inline.filename == "notes.pdf"
            && inline.mime_type == "application/pdf"
            && inline.data == expected_pdf
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_retries_stream_terminal_refusal_without_rendering_llm_error() {
    let _api_key = install_test_api_key();
    let refusal = unsupported_file_input_stream(
        "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.",
    );
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![refusal, ok_text_stream("retry ok")])
            .await;

    prompt_with_pdf_attachment(&state, &slot, "retry-once-ok", "summarize file").await;

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_none()
    })
    .await;

    assert_eq!(count_event(&lines, "auto_retry_start"), 1);
    assert_eq!(count_event(&lines, "llm_error"), 0);
    let recorded = requests.0.lock().clone();
    assert_eq!(
        recorded.len(),
        2,
        "transient refusal should be retried once"
    );
    assert!(
        recorded.iter().all(request_has_input_file),
        "the raw retry path must keep input_file unchanged on both attempts",
    );
    let transcript = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .expect("read transcript");
    assert!(
        transcript.contains("retry ok"),
        "assistant success should reach transcript"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_degrades_after_second_refusal_and_succeeds() {
    let _api_key = install_test_api_key();
    let refusal_message = "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.";
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![
            unsupported_file_input_stream(refusal_message),
            unsupported_file_input_stream(refusal_message),
            ok_text_stream("degraded retry ok"),
        ])
        .await;

    prompt_with_pdf_attachment(&state, &slot, "retry-degrade-ok", "summarize file").await;

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_none()
    })
    .await;

    assert_eq!(count_event(&lines, "auto_retry_start"), 2);
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("llm_notice")
                && line.get("message").and_then(serde_json::Value::as_str)
                    == Some("本轮附件未被当前端点接受，已按纯文本发送")
        }),
        "degrade retry should surface a notice: {lines:?}"
    );
    // Provider-call ladder is pinned in `run_basic_test`; this serve test keeps its scope
    // to the real stdout/session stack: the user should see the notice and still land on a
    // successful assistant turn.
    assert_eq!(count_event(&lines, "llm_error"), 0);
    let recorded = requests.0.lock().clone();
    assert_eq!(
        recorded.len(),
        3,
        "second refusal should produce original + raw retry + degraded retry",
    );
    assert!(
        recorded[..2].iter().all(request_has_input_file),
        "the first two attempts must keep input_file before degradation",
    );
    assert!(
        !request_has_input_file(&recorded[2]),
        "the degraded retry must strip input_file from the final request",
    );
    assert!(
        request_has_file_placeholder(&recorded[2]),
        "the degraded retry must carry the file placeholder text",
    );
    let transcript = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .expect("read transcript");
    assert!(
        transcript.contains("degraded retry ok"),
        "successful degraded retry should still persist assistant output",
    );
    assert!(
        !requests.0.lock().is_empty(),
        "integration stack should issue provider requests",
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_exhausted_stream_terminal_refusal_surfaces_one_final_error() {
    let _api_key = install_test_api_key();
    let refusal_message = "[OneOfParam] [input[0].content[1]] [invalid_enum_value] Invalid value: 'input_file'. Supported values are: 'input_text'.";
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![
            unsupported_file_input_stream(refusal_message),
            unsupported_file_input_stream(refusal_message),
            unsupported_file_input_stream(refusal_message),
            unsupported_file_input_stream(refusal_message),
        ])
        .await;

    prompt_with_pdf_attachment(&state, &slot, "retry-exhausted", "summarize file").await;

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some()
    })
    .await;

    assert_eq!(count_event(&lines, "llm_error"), 0);
    let final_errors = lines
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
        })
        .count();
    assert_eq!(final_errors, 1, "terminal refusal should render once");
    let recorded = requests.0.lock().clone();
    assert_eq!(
        recorded.len(),
        4,
        "four consecutive refusals should consume the full retry budget",
    );
    assert!(
        request_has_file_placeholder(&recorded[2]) && request_has_file_placeholder(&recorded[3]),
        "degraded attempts must stay degraded until the retry budget is exhausted",
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_content_filter_stream_refusal_does_not_retry() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![content_filter_stream()]).await;

    prompt_with_pdf_attachment(&state, &slot, "content-filter-terminal", "summarize file").await;

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|message| message.contains("content_filter"))
    })
    .await;

    assert_eq!(count_event(&lines, "auto_retry_start"), 0);
    assert_eq!(
        requests.0.lock().len(),
        1,
        "content_filter must remain fatal"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_uses_requested_user_message_id_for_transcript_and_context() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("fixed-user-id-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "plain text".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("user-fixed-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let user_message = latest_persisted_user_request(&captured[0]);
    assert_eq!(user_message.msg_id.as_deref(), Some("user-fixed-id"));
    drop(captured);
    assert_eq!(count_message_entries_with_id(&slot, "user-fixed-id"), 1);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_without_attachments_falls_back_to_user_text() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("plain-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "plain text".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let user_message = latest_persisted_user_request(&captured[0]);
    assert!(matches!(
        &user_message.content,
        Some(ChatMessageContent::Text(text)) if text == "plain text"
    ));
    assert!(user_message
        .msg_id
        .as_deref()
        .is_some_and(|message_id| !message_id.is_empty()));
}

#[test]
fn serve_message_params_segments_make_payload_non_empty() {
    let params = ServeMessageParams {
        segments: vec![ServeContentSegment::Reference {
            reference: ServeContextReference {
                kind: ServeContextRefKind::File,
                path: "src/lib.rs".to_string(),
                label: "lib.rs".to_string(),
                line_start: None,
                line_end: None,
                text: None,
            },
        }],
        ..ServeMessageParams::default()
    };
    assert!(!params.is_empty());
}

#[test]
fn build_user_message_preserves_segment_order_and_appends_attachments() {
    let params = ServeMessageParams {
        segments: vec![
            ServeContentSegment::Text {
                text: "before ".to_string(),
            },
            ServeContentSegment::Reference {
                reference: ServeContextReference {
                    kind: ServeContextRefKind::Selection,
                    path: "src/lib.rs".to_string(),
                    label: "lib.rs:10-12".to_string(),
                    line_start: Some(10),
                    line_end: Some(12),
                    text: Some("fn hello() {}".to_string()),
                },
            },
            ServeContentSegment::Text {
                text: " after".to_string(),
            },
        ],
        attachments: vec![ServeAttachment {
            kind: ServeAttachmentKind::Image,
            filename: None,
            mime_type: None,
            blob_sha: None,
            provider_sha: None,
            file_id: Some("image-file-id".to_string()),
        }],
        ..ServeMessageParams::default()
    };

    let message = build_message_for_test("fallback text", &params).expect("build message");
    let parts = match message.content {
        Some(ChatMessageContent::Parts(parts)) => parts,
        other => panic!("expected multipart content, got {other:?}"),
    };

    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "before "
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputReference { reference }
            if reference.ref_kind == ContextRefKind::Selection
                && reference.path == "src/lib.rs"
                && reference.label == "lib.rs:10-12"
                && reference.line_start == Some(10)
                && reference.line_end == Some(12)
                && reference.text.as_deref() == Some("fn hello() {}")
    ));
    assert!(matches!(
        &parts[2],
        ChatMessageContentPart::InputText { text } if text == " after"
    ));
    assert!(matches!(
        &parts[3],
        ChatMessageContentPart::InputImage {
            source: ImageSource::Uploaded(uploaded),
            ..
        } if uploaded.file_id == "image-file-id"
    ));
}

#[test]
fn build_user_message_accepts_reference_only_segments() {
    let params = ServeMessageParams {
        segments: vec![ServeContentSegment::Reference {
            reference: ServeContextReference {
                kind: ServeContextRefKind::File,
                path: "README.md".to_string(),
                label: "README.md".to_string(),
                line_start: None,
                line_end: None,
                text: None,
            },
        }],
        ..ServeMessageParams::default()
    };

    let message = build_message_for_test("", &params).expect("build message");
    let parts = match message.content {
        Some(ChatMessageContent::Parts(parts)) => parts,
        other => panic!("expected multipart content, got {other:?}"),
    };
    assert_eq!(parts.len(), 1);
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputReference { reference }
            if reference.ref_kind == ContextRefKind::File
                && reference.path == "README.md"
                && reference.label == "README.md"
    ));
}

#[test]
fn build_user_message_accepts_non_pdf_file_references_without_attachment_validation() {
    let params = ServeMessageParams {
        segments: vec![ServeContentSegment::Reference {
            reference: ServeContextReference {
                kind: ServeContextRefKind::File,
                path: "src/app.ts".to_string(),
                label: "app.ts".to_string(),
                line_start: None,
                line_end: None,
                text: None,
            },
        }],
        ..ServeMessageParams::default()
    };

    let message = build_message_for_test("", &params)
        .expect("non-PDF file references should stay on the context-reference path");
    let parts = match message.content {
        Some(ChatMessageContent::Parts(parts)) => parts,
        other => panic!("expected multipart content, got {other:?}"),
    };

    assert_eq!(parts.len(), 1);
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputReference { reference }
            if reference.ref_kind == ContextRefKind::File
                && reference.path == "src/app.ts"
                && reference.label == "app.ts"
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_blank_user_message_id_falls_back_to_generated_entry_id() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("blank-user-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "blank id".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("   ".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let entry = latest_user_entry(&slot);
    assert_ne!(entry.id.as_deref(), Some("   "));
    assert!(entry
        .id
        .as_deref()
        .is_some_and(|message_id| !message_id.trim().is_empty()));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_duplicate_user_message_id_falls_back_to_generated_entry_id() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, _buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    slot.ctx
        .session_runtime
        .session
        .append_message_with_id(
            serde_json::json!({
                "role": "user",
                "content": "existing",
            }),
            "dup-user-id",
        )
        .expect("seed duplicate id");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("duplicate-user-id".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "new text".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("dup-user-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let entry = latest_user_entry(&slot);
    assert_ne!(entry.id.as_deref(), Some("dup-user-id"));
    assert_eq!(count_message_entries_with_id(&slot, "dup-user-id"), 1);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_steer_ignores_attachments() {
    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Steer {
            id: Some("steer-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "just steer".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: None,
                    mime_type: None,
                    blob_sha: None,
                    provider_sha: None,
                    file_id: Some("ignored-file".to_string()),
                }],
                user_message_id: Some("steer-fixed-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let steering_message = captured[0]
        .messages
        .iter()
        .rev()
        .find(|message| message.kind == MessageKind::Steering)
        .expect("steering message");
    assert!(matches!(
        &steering_message.content,
        Some(ChatMessageContent::Text(text)) if text == "just steer"
    ));
    assert_eq!(steering_message.msg_id.as_deref(), Some("steer-fixed-id"));
    drop(captured);
    assert_eq!(count_message_entries_with_id(&slot, "steer-fixed-id"), 1);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_busy_steer_queues_and_persists_requested_user_message_id() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::Steer {
            id: Some("steer-busy".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "redirect".to_string(),
            params: ServeMessageParams {
                user_message_id: Some("steer-busy-fixed-id".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("steer-busy")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("steer-busy"))
        .expect("queued steer response");
    assert_eq!(response["payload"]["queued"].as_bool(), Some(true));

    let queue = slot.ctx.session_runtime.steering_queue.lock();
    assert_eq!(queue.len(), 1, "expected one queued steering message");
    assert_eq!(queue[0].msg_id.as_deref(), Some("steer-busy-fixed-id"));
    drop(queue);
    assert_eq!(
        count_message_entries_with_id(&slot, "steer-busy-fixed-id"),
        1
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_uptoseq_is_null_placeholder() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-1"))
        .expect("get_messages response");
    assert!(response["payload"].get("upToSeq").is_some());
    assert!(response["payload"]["upToSeq"].is_null());
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_returns_cursor_metadata_and_continuous_pages() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "first");
    append_history_message(&slot, "assistant", "second");
    append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-page-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-1"))
        .expect("first get_messages response");
    let first_page_ids = payload_message_ids(first);
    assert_eq!(first_page_ids.len(), 2);
    assert_eq!(first["payload"]["hasMore"].as_bool(), Some(true));
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor");
    let decoded = decode_cursor(next_cursor);
    assert_eq!(
        decoded["boundaryId"].as_str(),
        Some(first_page_ids[0].as_str())
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-page-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor.to_string()),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-page-2"))
        .expect("second get_messages response");
    assert_eq!(payload_message_ids(second).len(), 2);
    assert_eq!(second["payload"]["hasMore"].as_bool(), Some(false));
    assert!(second["payload"]["nextCursor"].is_null());
    let all_ids = [payload_message_ids(second), first_page_ids].concat();
    assert_eq!(all_ids.len(), 4);
    assert_eq!(
        all_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn transcript_migration_preserves_get_messages_image_and_display_semantics() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let image_bytes = b"migration fixture image";
    let image_b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
    crate::core::session::transcript::append_entry(
        &transcript,
        &crate::core::session::transcript::TranscriptEntry::Message(
            crate::core::session::transcript::MessageEntry {
                id: Some("legacy-user-image".to_string()),
                parent_id: None,
                timestamp: "2020-01-01T00:00:00.000Z".to_string(),
                message: serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "input_image",
                        "mime_type": "image/png",
                        "image_b64": image_b64
                    }]
                }),
            },
        ),
    )
    .unwrap();
    crate::core::session::transcript::append_entry(
        &transcript,
        &crate::core::session::transcript::TranscriptEntry::Message(
            crate::core::session::transcript::MessageEntry {
                id: Some("legacy-tool-display".to_string()),
                parent_id: None,
                timestamp: "2020-01-01T00:00:01.000Z".to_string(),
                message: serde_json::json!({
                    "role": "tool",
                    "tool_call_id": "legacy-tool-call",
                    "content": "edited",
                    "tool_display": {
                        "kind": "file",
                        "file": "src/lib.rs",
                        "added": 1,
                        "removed": 0,
                        "diff": [{"tag": "add", "newLine": 1, "text": "new"}]
                    }
                }),
            },
        ),
    )
    .unwrap();

    let sessions_dir = transcript.parent().unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/migrate_transcripts_v2.py");
    let migration = StdCommand::new("python3")
        .env("TOMCAT_MIGRATION_TEST_ALLOW_RUNNING_SERVE", "1")
        .arg(script)
        .arg("--sessions-dir")
        .arg(sessions_dir)
        .output()
        .expect("run transcript migration script");
    assert!(
        migration.status.success(),
        "migration failed: {}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let rewritten = std::fs::read_to_string(&transcript).unwrap();
    assert!(!rewritten.contains("image_b64"));
    assert!(!rewritten.contains("tool_display"));

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-migrated".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                attachment_mode: AttachmentMode::Reference,
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-migrated")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-migrated"))
        .expect("migrated get_messages response");
    let messages = response["payload"]["messages"]
        .as_array()
        .expect("messages");
    let image = messages
        .iter()
        .find_map(|entry| {
            entry["message"]["content"]
                .as_array()
                .and_then(|content| content.first())
        })
        .expect("migrated image message");
    assert_eq!(image["type"], "input_image_ref");
    let sha = image["blob_sha"].as_str().expect("migrated image sha");
    assert_eq!(
        std::fs::read(sessions_dir.join("attachments/blobs").join(sha)).unwrap(),
        image_bytes
    );
    let tool = messages
        .iter()
        .find(|entry| entry["message"]["tool_call_id"] == "legacy-tool-call")
        .expect("migrated tool result");
    assert_eq!(tool["message"]["content"], "edited");
    assert_eq!(tool["message"]["tool_display"]["kind"], "file");
    assert_eq!(tool["message"]["tool_display"]["expired"], true);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_keeps_assistant_and_tool_result_atomic_by_tool_call_id() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let seed_id = append_history_message(&slot, "user", "seed");
    let assistant_id = slot
        .ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "ask-call-1",
                    "type": "function",
                    "function": {
                        "name": "ask_question",
                        "arguments": "{\"questions\":[{\"id\":\"q1\"}]}"
                    }
                }]
            }),
        )
        .expect("append assistant ask_question call");
    let tool_id = slot
        .ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "ask-call-1",
                "content": "{\"answers\":[],\"cancelled\":true,\"outcome\":\"skipped\"}",
                "tool_display": {
                    "kind": "file",
                    "file": "src/lib.rs",
                    "added": 1,
                    "removed": 0,
                    "diff": [{"tag": "add", "newLine": 1, "text": "new"}]
                }
            }),
        )
        .expect("append ask_question result");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-atomic-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(1),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-atomic-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-atomic-1"))
        .expect("atomic first page");
    assert_eq!(
        payload_message_ids(first),
        vec![assistant_id.clone(), tool_id]
    );
    let messages = first["payload"]["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["message"]["tool_calls"][0]["id"], "ask-call-1");
    assert_eq!(messages[1]["message"]["tool_call_id"], "ask-call-1");
    assert_eq!(messages[1]["message"]["tool_display"]["kind"], "file");
    assert_eq!(
        messages[1]["message"]["tool_display"]["diff"][0]["text"],
        "new"
    );
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("cursor before atomic companion")
        .to_string();
    assert_eq!(decode_cursor(&next_cursor)["boundaryId"], assistant_id);

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-atomic-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor),
                limit: Some(1),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-atomic-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-atomic-2"))
        .expect("atomic second page");
    assert_eq!(payload_message_ids(second), vec![seed_id]);
    assert_eq!(second["payload"]["hasMore"], false);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_relocates_stale_cursor_by_boundary_id() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let first_id = append_history_message(&slot, "user", "first");
    let second_id = append_history_message(&slot, "assistant", "second");
    let third_id = append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-relocate-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-1"))
        .expect("first get_messages response");
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();

    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    crate::core::session::transcript::insert_entry_after_message_id(
        &transcript_path,
        &second_id,
        &crate::core::session::transcript::TranscriptEntry::Custom(
            crate::core::session::transcript::CustomEntry {
                id: Some("inserted-before-third".to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:02.500Z".to_string(),
                extra: serde_json::json!({
                    "event": "history.inserted"
                }),
            },
        ),
    )
    .expect("insert before cursor boundary");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-relocate-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-relocate-2"))
        .expect("second get_messages response");
    let ids = payload_message_ids(second);
    assert_eq!(ids, vec![second_id, "inserted-before-third".to_string()]);
    assert!(!ids.contains(&first_id));
    assert!(!ids.contains(&third_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_uses_best_effort_when_boundary_id_disappears() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "first");
    append_history_message(&slot, "assistant", "second");
    let third_id = append_history_message(&slot, "user", "third");
    append_history_message(&slot, "assistant", "fourth");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-best-effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-1")
    })
    .await;
    let first = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-1"))
        .expect("first get_messages response");
    let next_cursor = first["payload"]["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();

    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let lines = std::fs::read_to_string(&transcript_path)
        .expect("read transcript")
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let rewritten = lines
        .into_iter()
        .map(|line| {
            if line.contains(&format!("\"id\":\"{third_id}\"")) {
                serde_json::to_string(&crate::core::session::transcript::TranscriptEntry::Custom(
                    crate::core::session::transcript::CustomEntry {
                        id: Some("replacement-entry".to_string()),
                        parent_id: None,
                        timestamp: "2025-01-01T00:00:03.000Z".to_string(),
                        extra: serde_json::json!({
                            "event": "history.rewritten"
                        }),
                    },
                ))
                .expect("serialize replacement")
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&transcript_path, rewritten).expect("rewrite transcript");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-best-effort-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                cursor: Some(next_cursor),
                limit: Some(2),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-2")
    })
    .await;
    let second = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-best-effort-2"))
        .expect("second get_messages response");
    assert_eq!(
        second.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let ids = payload_message_ids(second);
    assert!(!ids.is_empty());
    assert!(!ids.contains(&third_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_messages_returns_boundary_entries_without_truncation() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    append_history_message(&slot, "user", "before boundary");
    let transcript_path = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    crate::core::session::transcript::append_entry(
        &transcript_path,
        &crate::core::session::transcript::TranscriptEntry::BranchSummary(
            crate::core::session::transcript::BranchSummaryEntry {
                id: Some("boundary-1".to_string()),
                parent_id: None,
                timestamp: "2025-01-01T00:00:02.000Z".to_string(),
                summary: Some("Earlier turns were summarized".to_string()),
                covered_start_id: None,
                covered_end_id: None,
                covered_count: Some(4),
                is_boundary: Some(true),
                preheat_compaction_id: None,
                estimated_covered_tokens_before: None,
                estimated_summary_tokens: None,
                estimated_tokens_saved: None,
                error: None,
                attempts: None,
            },
        ),
    )
    .expect("append boundary entry");
    append_history_message(&slot, "assistant", "after boundary");

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetMessages {
            id: Some("gm-boundary".to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                limit: Some(4),
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("gm-boundary")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("gm-boundary"))
        .expect("get_messages response");
    let entries = response["payload"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1]["type"].as_str(), Some("branch_summary"));
    assert_eq!(entries[1]["id"].as_str(), Some("boundary-1"));
    assert_eq!(
        entries[2]["message"]["content"].as_str(),
        Some("after boundary")
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_panic_isolation_emits_agent_end_error() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_panicking_provider().await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("panic-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "panic".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").and_then(serde_json::Value::as_str)
                == Some("serve session task panicked")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("serve session task panicked")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(slot.session_id.as_str())
        }),
        "expected panic-isolated agent_end, got {lines:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_panic_isolation_emits_agent_idle_once() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_panicking_provider().await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("panic-idle".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "panic".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_idle")
    })
    .await;
    assert_eq!(
        count_event(&lines, "agent_end"),
        1,
        "expected one panic agent_end: {lines:?}"
    );
    assert_eq!(
        count_event(&lines, "agent_idle"),
        1,
        "expected one panic agent_idle: {lines:?}"
    );
    assert!(
        first_event_index(&lines, "agent_end") < first_event_index(&lines, "agent_idle"),
        "panic path should emit agent_idle after agent_end: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("serve session task panicked")
        }),
        "panic path should still surface the panic agent_end: {lines:?}"
    );
    assert!(
        !slot.is_busy(),
        "panic path should restore the slot to idle"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_invalid_model_override_emits_agent_idle_once() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "recovered".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    slot.ctx
        .session_runtime
        .session
        .switch_current_model(None, Some("totally-missing-model"))
        .expect("seed stale model override");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("invalid-override-idle".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_idle")
    })
    .await;
    assert_eq!(
        count_event(&lines, "agent_end"),
        1,
        "expected one failed agent_end: {lines:?}"
    );
    assert_eq!(
        count_event(&lines, "agent_idle"),
        1,
        "expected one failed agent_idle: {lines:?}"
    );
    assert!(
        first_event_index(&lines, "agent_end") < first_event_index(&lines, "agent_idle"),
        "failed pre-loop resolve should still emit agent_idle after agent_end: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|message| message.contains("totally-missing-model"))
        }),
        "failed path should mention the invalid override in agent_end: {lines:?}"
    );
    assert!(
        !slot.is_busy(),
        "failed path should restore the slot to idle"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_stale_invalid_model_override_emits_single_agent_end_and_recovers() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "recovered".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    slot.ctx
        .session_runtime
        .session
        .switch_current_model(None, Some("totally-missing-model"))
        .expect("seed stale model override");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("invalid-override-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some()
    })
    .await;
    let error_ends = lines
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        error_ends.len(),
        1,
        "invalid stale model override should emit exactly one terminal error event: {lines:?}"
    );
    assert!(
        error_ends[0]
            .get("error")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.contains("totally-missing-model")),
        "expected stale model error to mention the invalid override: {lines:?}"
    );
    assert!(
        slot.turn_state.lock().is_some(),
        "turn_state should be restored after pre-loop resolve failure"
    );
    for _ in 0..50 {
        if !slot.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    slot.ctx
        .session_runtime
        .session
        .switch_current_model(None, Some("gpt-5.4"))
        .expect("restore valid model");
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("invalid-override-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "recover".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let after_recovery_prompt = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("invalid-override-2")
    })
    .await;
    let recovery_response = after_recovery_prompt
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("invalid-override-2")
        })
        .expect("recovery prompt response");
    assert_eq!(
        recovery_response["success"].as_bool(),
        Some(true),
        "recovery prompt should still be accepted: {after_recovery_prompt:?}"
    );
    let recovered = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    let all_agent_ends = recovered
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
        .count();
    assert_eq!(
        all_agent_ends, 2,
        "expected one failed + one recovered terminal event: {recovered:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_retry_copies_failed_prompt_forward_without_duplicating_model_input() {
    let _api_key = install_test_api_key();
    let recovered_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "recovered".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![
            vec![Err(crate::llm_http_status_error(
                "mock",
                503,
                "temporary upstream outage",
            ))],
            vec![Err(crate::llm_http_status_error(
                "mock",
                503,
                "temporary upstream outage",
            ))],
            vec![Err(crate::llm_http_status_error(
                "mock",
                503,
                "temporary upstream outage",
            ))],
            vec![Err(crate::llm_http_status_error(
                "mock",
                503,
                "temporary upstream outage",
            ))],
            recovered_stream,
        ])
        .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("resume-failed-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "retry this exact prompt".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|message| message.contains("temporary upstream outage"))
    })
    .await;

    let failed_user_id = session_message_entries(&slot)
        .into_iter()
        .find(|entry| {
            entry
                .message
                .get("role")
                .and_then(serde_json::Value::as_str)
                == Some("user")
                && entry
                    .message
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    == Some("retry this exact prompt")
        })
        .and_then(|entry| entry.id)
        .expect("failed user row has a durable transcript id");
    let transcript_before_retry = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .expect("read auto-retry transcript");
    assert!(
        transcript_before_retry
            .matches("\"event\":\"auto_retry_start\"")
            .count()
            >= 1,
        "the exhausted automatic-retry path must retain its custom diagnostics"
    );
    handle_command(
        Arc::clone(&state),
        ServeCommand::Retry {
            id: Some("resume-failed-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            message_id: failed_user_id.clone(),
        },
    )
    .await
    .unwrap();
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").is_none_or(serde_json::Value::is_null)
    })
    .await;

    let recorded = requests.0.lock().clone();
    assert_eq!(
        recorded.len(),
        5,
        "four automatic attempts and one copy-forward retry request"
    );
    let resumed_prompt_occurrences = recorded[4]
        .messages
        .iter()
        .filter_map(|message| message.text_content())
        .filter(|text| *text == "retry this exact prompt")
        .count();
    assert_eq!(
        resumed_prompt_occurrences, 1,
        "Retry must activate exactly one copied user message in the provider request"
    );
    let rows = session_message_entries(&slot);
    assert_eq!(
        rows.iter()
            .filter(|entry| {
                entry
                    .message
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    == Some("retry this exact prompt")
            })
            .count(),
        2,
        "the archived failed row and its new live copy are both auditable"
    );
    let archived = rows
        .iter()
        .find(|entry| entry.id.as_deref() == Some(failed_user_id.as_str()))
        .expect("original failed row remains in transcript");
    assert_eq!(archived.message["superseded"], true);
    assert_eq!(archived.message["turn_failed"], true);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_resume_rejects_without_a_complete_tool_result_tail() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_text_stream("must not run")]).await;
    slot.ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({"role": "user", "content": "retry me"}),
        )
        .expect("seed incomplete non-tool tail");
    let transcript = slot
        .ctx
        .session_runtime
        .session
        .transcript_path(&slot.session_id);
    let before = std::fs::read(&transcript).expect("read transcript before rejected Resume");

    handle_command(
        Arc::clone(&state),
        ServeCommand::Resume {
            id: Some("resume-no-tool-tail".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("resume-no-tool-tail")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("resume-no-tool-tail")
        })
        .expect("structured Resume rejection");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(response["error"].as_str(), Some("nothing_to_resume"));
    assert!(
        requests.0.lock().is_empty(),
        "failed validation must not start an LLM request"
    );
    assert_eq!(
        std::fs::read(transcript).expect("read transcript after rejected Resume"),
        before,
        "failed validation must not mutate the transcript"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_resume_rejects_when_a_failed_user_hides_an_old_tool_tail() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_text_stream("must not run")]).await;
    let session = &slot.ctx.session_runtime.session;
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "user",
                "content": "the older tool turn",
            }),
        )
        .unwrap();
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "old-read",
                    "type": "function",
                    "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"},
                }],
            }),
        )
        .unwrap();
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "old-read",
                "content": "# old result",
            }),
        )
        .unwrap();
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "user",
                "content": "the failed retry target",
                "superseded": true,
                "turn_failed": true,
            }),
        )
        .unwrap();
    session
        .append_error_entry(crate::core::session::transcript::ErrorEntry {
            id: Some("error-old-tool-tail".to_string()),
            parent_id: None,
            timestamp: "2026-08-01T00:00:00.000Z".to_string(),
            phase: None,
            provider: None,
            model: None,
            api_family: None,
            status_code: Some(503),
            request_id: None,
            failure_kind: None,
            failure_domain: None,
            summary: "failed retry target".to_string(),
            detail: "failed retry target".to_string(),
        })
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let before = std::fs::read(&transcript).unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Resume {
            id: Some("resume-old-tool-tail".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("resume-old-tool-tail")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("resume-old-tool-tail")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("nothing_to_resume")
        }),
        "an old tool tail must not authorize Resume for the superseded newer user"
    );
    assert!(requests.0.lock().is_empty());
    assert_eq!(std::fs::read(transcript).unwrap(), before);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_retry_rejects_a_stale_anchor_without_mutating_the_transcript() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_text_stream("must not run")]).await;
    let session = &slot.ctx.session_runtime.session;
    let stale_id = session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({"role": "user", "content": "old failed request"}),
        )
        .unwrap();
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({"role": "user", "content": "newer live request"}),
        )
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let before = std::fs::read(&transcript).unwrap();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Retry {
            id: Some("retry-stale-anchor".to_string()),
            session_id: Some(slot.session_id.clone()),
            message_id: stale_id,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("retry-stale-anchor")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("retry-stale-anchor")
                && line.get("error").and_then(serde_json::Value::as_str)
                    == Some("retry_target_stale")
        }),
        "the service must report a stale Retry anchor rather than falling back to Resume"
    );
    assert!(requests.0.lock().is_empty());
    assert_eq!(std::fs::read(transcript).unwrap(), before);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_resume_after_truncated_tool_turn_continues_from_tool_tail() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) = build_initialized_state_with_recorded_streams(
        vec![ok_text_stream("resumed from placeholder")],
    )
    .await;
    let session = &slot.ctx.session_runtime.session;
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "user",
                "content": "finish the truncated tool turn",
            }),
        )
        .unwrap();
    session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "truncated-read",
                    "type": "function",
                    "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"},
                }],
            }),
        )
        .unwrap();
    session
        .append_error_entry(crate::core::session::transcript::ErrorEntry {
            id: Some("error-truncated-read".to_string()),
            parent_id: None,
            timestamp: "2026-08-01T00:00:00.000Z".to_string(),
            phase: None,
            provider: None,
            model: None,
            api_family: None,
            status_code: None,
            request_id: None,
            failure_kind: None,
            failure_domain: None,
            summary: "达到 max_tokens，回答可能未完成".to_string(),
            detail: "the prior tool turn stopped at max_tokens".to_string(),
        })
        .unwrap();
    let system_text = slot
        .turn_state
        .lock()
        .as_ref()
        .expect("session turn state")
        .prompt_snapshot
        .system_text()
        .to_string();
    let healed_context = init_context_state(
        &slot.ctx.session_runtime.session,
        &slot.ctx.config.context,
        &system_text,
    )
    .expect("failure recovery hydration must persist a placeholder after the error row");
    if let Some(turn_state) = slot.turn_state.lock().as_mut() {
        turn_state.context_state = healed_context;
    }
    let healed_entries = session.get_entries(16).unwrap();
    assert!(
        matches!(
            healed_entries.last(),
            Some(crate::core::session::transcript::TranscriptEntry::Message(entry))
                if entry.message.get("role").and_then(serde_json::Value::as_str) == Some("tool")
                    && entry.message.get("tool_call_id").and_then(serde_json::Value::as_str)
                        == Some("truncated-read")
                    && entry.message.get("content").and_then(serde_json::Value::as_str)
                        == Some(crate::core::session::UNKNOWN_RESTART_TOOL_RESULT_TEXT)
        ),
        "hydration must append the healed result after the durable error annotation"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Resume {
            id: Some("resume-post-error-placeholder".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("error").is_none_or(serde_json::Value::is_null)
    })
    .await;

    let recorded = requests.0.lock().clone();
    assert_eq!(recorded.len(), 1);
    let tail = recorded[0]
        .messages
        .iter()
        .rev()
        .find(|message| message.kind != crate::core::llm::MessageKind::EphemeralTail)
        .expect("Resume provider persisted request tail");
    assert_eq!(tail.role, crate::core::llm::ChatMessageRole::Tool);
    assert_eq!(
        tail.text_content(),
        Some(crate::core::session::UNKNOWN_RESTART_TOOL_RESULT_TEXT),
        "the post-error healed placeholder is the legal continuation tail"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_failed_turn_retry_does_not_replay_superseded_user_tail() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    crate::test_support::write_models_override(
        temp.path(),
        &[
            crate::test_support::TestModelOverride {
                id: "deepseek-v4-pro",
                model_name: None,
                api: "openai",
                provider: "deepseek",
                api_key_env: TEST_API_KEY_ENV,
                base_url: "http://127.0.0.1:1",
                thinking_format: None,
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
                web_search: false,
            },
            crate::test_support::TestModelOverride::gpt54_openai_responses(TEST_API_KEY_ENV)
                .with_base_url("http://127.0.0.1:1"),
        ],
    );
    cfg.llm.default_model = "deepseek-v4-pro".to_string();
    cfg.context.compaction_model = "deepseek-v4-pro".to_string();

    let first_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "first ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let fail_stream = vec![Err(crate::AppError::Llm("sunmi 403".to_string()))];
    let third_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "retry ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) = RecordingMockLlm::new(vec![first_stream, fail_stream, third_stream]);
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("stuck-history-ok".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "history prompt".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_none()
    })
    .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("switch-to-responses".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("stuck-fail".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "will fail".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let failed_lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|message| message.contains("sunmi 403"))
    })
    .await;
    assert!(
        failed_lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|message| message.contains("sunmi 403"))
        }),
        "expected failed turn terminal event: {failed_lines:?}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("stuck-retry".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "retry after reconnect".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let after_retry = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    let agent_ends = after_retry
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
        .collect::<Vec<_>>();
    assert_eq!(
        agent_ends.len(),
        3,
        "expected three terminal events: {after_retry:?}"
    );
    assert!(
        agent_ends
            .last()
            .and_then(|line| line.get("error"))
            .map(|value| value.is_null())
            .unwrap_or(true),
        "retry turn should recover successfully: {after_retry:?}"
    );

    let recorded = requests.0.lock().clone();
    assert_eq!(recorded.len(), 3, "expected three provider requests");
    let third_texts: Vec<String> = recorded[2]
        .messages
        .iter()
        .filter_map(|message| message.text_content().map(str::to_string))
        .collect();
    assert!(
        third_texts.iter().any(|text| text == "history prompt"),
        "successful history before the failure should still be replayed: {third_texts:?}"
    );
    assert!(
        third_texts
            .iter()
            .any(|text| text == "retry after reconnect"),
        "latest retry prompt should be present: {third_texts:?}"
    );
    assert!(
        !third_texts.iter().any(|text| text == "will fail"),
        "failed-turn user tail must be superseded before the retry request: {third_texts:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_failed_turn_retry_keeps_previous_response_id_hint() {
    let _api_key = install_test_api_key();
    let server = RecordingHttpServer::start(vec![
        ScriptedHttpResponse::html(
            403,
            "<html><body><h1>403 Forbidden</h1><p>Host: PS-SHA-01JfN78</p><p>Request-Id: req_turn2</p></body></html>",
        ),
        ScriptedHttpResponse::sse(&[
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m2\",\"content_index\":0,\"delta\":\"retry ok\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_turn3\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ]),
    ])
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), &server.base_url);
    cfg.llm.default_model = "gpt-5.4".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    cfg.llm.title_model = None;
    cfg.llm.reasoning_continuity.enabled = true;
    cfg.llm.openai_responses.use_previous_response_id = true;
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "{env}"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write responses override");
    let (state, buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;
    let credential_fingerprint = {
        let digest = Sha256::digest(b"test-key");
        format!("{digest:x}")[..16].to_string()
    };

    append_history_message(&slot, "user", "seed user");
    let seeded_assistant = crate::core::llm::ChatMessage::assistant("seed assistant")
        .with_reasoning_state(
            Some("safe summary".to_string()),
            Some(crate::core::llm::ReasoningContinuation {
                source_provider: "openai".to_string(),
                source_api: "responses".to_string(),
                source_model: "gpt-5".to_string(),
                format: crate::core::llm::ReasoningFormat::OpenaiResponsesReasoningItems,
                opaque_payload: serde_json::json!([{
                    "id": "rs_1",
                    "type": "reasoning",
                    "encrypted_content": "enc_turn1",
                    "summary": [{"type": "summary_text", "text": "safe summary"}]
                }]),
                fallback_text: Some("safe summary".to_string()),
                provider_refs: Some(crate::core::llm::ProviderRefs {
                    openai_response_id: Some("resp_turn1".to_string()),
                    replay_profile_id: Some(
                        crate::core::llm::ProviderCompatProfile::openai_responses_routed(
                            "gpt-5",
                            "openai",
                            &server.base_url,
                            &credential_fingerprint,
                        )
                        .profile_id,
                    ),
                }),
            }),
            Some(crate::core::llm::ContinuityMetadata {
                had_tool_call: false,
                replay_requirement: crate::core::llm::ReplayRequirement::SameProfileOptional,
            }),
        );
    slot.ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &slot.session_id,
            serde_json::to_value(&seeded_assistant).expect("serialize seeded assistant"),
        )
        .expect("append seeded assistant");
    let system_text = {
        let guard = slot.turn_state.lock();
        guard
            .as_ref()
            .expect("session turn state")
            .prompt_snapshot
            .system_text()
            .to_string()
    };
    let seeded_state = init_context_state(
        &slot.ctx.session_runtime.session,
        &slot.ctx.config.context,
        &system_text,
    )
    .expect("rehydrate seeded context");
    if let Some(turn_state) = slot.turn_state.lock().as_mut() {
        turn_state.context_state = seeded_state;
    }

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("responses-turn-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "second".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|message| message.contains("403"))
    })
    .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("responses-turn-3".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "third".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let lines = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    assert!(
        lines
            .iter()
            .filter(|line| {
                line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            })
            .count()
            >= 2,
        "expected failed turn + retry success terminal events: {lines:?}"
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2, "expected two upstream requests");
    assert!(
        requests[0].contains("\"previous_response_id\":\"resp_turn1\""),
        "failed turn should still attempt previous_response_id fast path first: {}",
        requests[0]
    );
    assert!(
        requests[1].contains("\"previous_response_id\":\"resp_turn1\""),
        "改动A 已回退：瞬时失败后同会话重发应仍携带 previous_response_id 续写线索: {}",
        requests[1]
    );

    server.shutdown().await;
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_with_attachment_history_then_deepseek_degrades_history_and_succeeds() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "{env}"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}

[[models]]
id = "deepseek-v4-pro"
api = "openai"
provider = "deepseek"
api_key_env = "{env}"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV
        ),
    )
    .expect("write dual-model override");
    let first_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "vision ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let second_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "pdf ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let third_stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "history ok".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) =
        RecordingMockLlm::new(vec![first_stream, second_stream, third_stream]);
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "describe image".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: None,
                    mime_type: None,
                    blob_sha: None,
                    provider_sha: None,
                    file_id: Some("file-vision".to_string()),
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let after_first = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_none()
    })
    .await;
    assert_eq!(
        after_first
            .iter()
            .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
            .count(),
        1
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-2".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "summarize pdf".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("notes.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some(ingest_test_bytes(&slot, &test_pdf_bytes())),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let after_second_history = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                        && line
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .is_none()
                })
                .count()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    assert_eq!(
        after_second_history
            .iter()
            .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
            .count(),
        2
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("set-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("attachment-history-3".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "follow up".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let after_second = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if lines
                .iter()
                .filter(|line| {
                    line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                })
                .count()
                >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };
    let agent_end_total = after_second
        .iter()
        .filter(|line| line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end"))
        .count();
    let agent_end_error_total = after_second
        .iter()
        .filter(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
        })
        .count();
    assert_eq!(
        agent_end_total, 3,
        "expected three successful agent_end events: {after_second:?}"
    );
    assert_eq!(
        agent_end_error_total, 0,
        "history downgrade should avoid capability mismatch errors: {after_second:?}"
    );
    let recorded = requests.0.lock().clone();
    assert_eq!(recorded.len(), 3, "expected three provider requests");
    let third_messages = &recorded[2].messages;
    assert!(
        third_messages.iter().any(|message| matches!(
            &message.content,
            Some(ChatMessageContent::Parts(parts))
                if parts.iter().any(|part| matches!(
                    part,
                    ChatMessageContentPart::InputText { text }
                        if text.contains(UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER)
                ))
        )),
        "deepseek follow-up should contain image omission placeholder in historical user message: {third_messages:?}"
    );
    assert!(
        third_messages.iter().any(|message| matches!(
            &message.content,
            Some(ChatMessageContent::Parts(parts))
                if parts.iter().any(|part| matches!(
                    part,
                    ChatMessageContentPart::InputText { text }
                        if text.contains(UNSUPPORTED_FILE_INPUT_PLACEHOLDER)
                ))
        )),
        "deepseek follow-up should contain file omission placeholder in historical user message: {third_messages:?}"
    );
    assert!(
        third_messages.iter().all(|message| match &message.content {
            Some(ChatMessageContent::Parts(parts)) => parts.iter().all(|part| {
                !matches!(
                    part,
                    ChatMessageContentPart::InputImage { .. }
                        | ChatMessageContentPart::InputFile { .. }
                )
            }),
            _ => true,
        }),
        "deepseek follow-up should not carry raw multimodal parts after downgrade: {third_messages:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_model_rejects_invalid_id_without_mutating_session_override() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("bad-model".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek".to_string(),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-model")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-model"))
        .expect("invalid model response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|message| message.contains("deepseek")),
        "expected invalid model error to mention requested id: {response:?}"
    );

    let current = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .expect("read current session")
        .expect("current session entry");
    assert_eq!(
        current.model_override, None,
        "invalid set_model must not persist a bad model_override"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("models-after-bad-set".to_string()),
        },
    )
    .await
    .unwrap();
    let after = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("models-after-bad-set")
    })
    .await;
    let list_models = after
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("models-after-bad-set")
        })
        .expect("list_models response after invalid set_model");
    assert_eq!(list_models["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_roundtrips_in_get_state() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-effort-1".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-effort-1")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-effort-1"))
        .expect("get_state response");
    let payload = response["payload"].clone();
    assert_eq!(response["success"].as_bool(), Some(true), "{response:?}");
    assert_eq!(payload["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(payload["thinkingLevel"].as_str(), Some("xhigh"));
}

#[tokio::test]
#[serial(env_lock)]
async fn upsert_model_response_includes_non_fatal_warnings() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::UpsertModel {
            id: Some("upsert-warning".to_string()),
            model: ModelEntryInput {
                id: "relay-openai".to_string(),
                model_name: Some("gpt-5.4".to_string()),
                api: "openai-responses".to_string(),
                provider: "relay".to_string(),
                api_key_env: Some("SERVE_TEST_GATEWAY_API_KEY".to_string()),
                base_url: Some("https://api.example.test/v1".to_string()),
                capabilities: Capabilities {
                    tools: true,
                    reasoning: true,
                    ..Capabilities::default()
                },
                context_window: Some(200_000),
                context_window_options: None,
                max_output_tokens: None,
                description: None,
                supported_reasoning_levels: None,
                thinking_format: Some("anthropic".to_string()),
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("upsert-warning")
    })
    .await;
    let upsert = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("upsert-warning"))
        .expect("upsert_model warning response");
    assert_eq!(upsert["success"].as_bool(), Some(true));
    assert_eq!(
        upsert["payload"]["model"]["id"].as_str(),
        Some("relay-openai")
    );
    let warnings = upsert["payload"]["warnings"]
        .as_array()
        .expect("warnings array");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|msg| msg.contains("openai-responses") && msg.contains("anthropic")),
        "unexpected warnings payload: {:?}",
        warnings
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_model_admin_roundtrip_updates_key_presence() {
    let _api_key = install_test_api_key();
    let (state, buffer, temp, _slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::UpsertModel {
            id: Some("upsert-model".to_string()),
            model: ModelEntryInput {
                id: "claude-opus-gateway".to_string(),
                model_name: Some("claude-opus-4-6".to_string()),
                api: "anthropic-messages".to_string(),
                provider: "anthropic".to_string(),
                api_key_env: Some("SERVE_TEST_GATEWAY_API_KEY".to_string()),
                base_url: Some("https://api.example.test/v1".to_string()),
                capabilities: Capabilities {
                    tools: true,
                    reasoning: true,
                    ..Capabilities::default()
                },
                context_window: Some(200_000),
                context_window_options: None,
                max_output_tokens: Some(128_000),
                description: None,
                supported_reasoning_levels: None,
                thinking_format: Some("anthropic".to_string()),
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("upsert-model")
    })
    .await;
    let upsert = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("upsert-model"))
        .expect("upsert_model response");
    assert_eq!(upsert["success"].as_bool(), Some(true));

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("list-models-before-key".to_string()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-models-before-key")
    })
    .await;
    let listed = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-models-before-key")
        })
        .expect("list_models before key");
    let before_key = listed["payload"]["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|entry| entry["id"].as_str() == Some("claude-opus-gateway"))
        .expect("custom model in list");
    assert_eq!(before_key["source"].as_str(), Some("user"));
    assert_eq!(before_key["keyPresent"].as_bool(), Some(false));

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetProviderKey {
            id: Some("set-provider-key".to_string()),
            env_name: "SERVE_TEST_GATEWAY_API_KEY".to_string(),
            value: "relay-secret".to_string(),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("set-provider-key")
    })
    .await;
    let set_key = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("set-provider-key"))
        .expect("set_provider_key response");
    assert_eq!(set_key["success"].as_bool(), Some(true));
    assert_eq!(
        set_key["payload"]["envName"].as_str(),
        Some("SERVE_TEST_GATEWAY_API_KEY")
    );
    assert_eq!(set_key["payload"]["keyPresent"].as_bool(), Some(true));
    assert!(
        !set_key.to_string().contains("relay-secret"),
        "set_provider_key response must not leak plaintext secrets: {set_key}"
    );
    let env_path = temp.path().join("assets").join(".env");
    let env_text = std::fs::read_to_string(&env_path).unwrap();
    assert!(env_text.contains("SERVE_TEST_GATEWAY_API_KEY=relay-secret"));
    std::fs::write(
        &env_path,
        "SERVE_TEST_GATEWAY_API_KEY=relay-secret\nFCODEX_OPENAI_API_KEY=external-secret\n",
    )
    .expect("externally add key slot");

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListProviderKeys {
            id: Some("list-provider-keys".to_string()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-provider-keys")
    })
    .await;
    let key_list = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-provider-keys")
        })
        .expect("list_provider_keys response");
    assert_eq!(key_list["success"].as_bool(), Some(true));
    let provider_key = key_list["payload"]["keys"]
        .as_array()
        .expect("provider keys array")
        .iter()
        .find(|entry| entry["envName"].as_str() == Some("SERVE_TEST_GATEWAY_API_KEY"))
        .expect("provider key entry");
    assert_eq!(provider_key["keyPresent"].as_bool(), Some(true));
    assert_eq!(provider_key["provider"].as_str(), Some(""));
    assert!(key_list["payload"]["keys"]
        .as_array()
        .expect("provider keys array")
        .iter()
        .any(|entry| entry["envName"].as_str() == Some("FCODEX_OPENAI_API_KEY")));
    assert!(
        !key_list.to_string().contains("relay-secret")
            && !key_list.to_string().contains("external-secret"),
        "list_provider_keys must not leak plaintext secrets: {key_list}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("list-models-after-key".to_string()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-models-after-key")
    })
    .await;
    let listed = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-models-after-key")
        })
        .expect("list_models after key");
    let after_key = listed["payload"]["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|entry| entry["id"].as_str() == Some("claude-opus-gateway"))
        .expect("custom model in list after key");
    assert_eq!(after_key["keyPresent"].as_bool(), Some(true));
    assert!(
        !listed.to_string().contains("relay-secret"),
        "list_models must not leak plaintext secrets: {listed}"
    );

    std::fs::write(&env_path, "SERVE_TEST_GATEWAY_API_KEY=relay-secret\n")
        .expect("externally remove key slot");
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListProviderKeys {
            id: Some("list-provider-keys-after-delete".to_string()),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str)
            == Some("list-provider-keys-after-delete")
    })
    .await;
    let after_delete = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str)
                == Some("list-provider-keys-after-delete")
        })
        .expect("list_provider_keys after external delete");
    assert!(!after_delete["payload"]["keys"]
        .as_array()
        .expect("provider keys after delete")
        .iter()
        .any(|entry| entry["envName"].as_str() == Some("FCODEX_OPENAI_API_KEY")));

    handle_command(
        Arc::clone(&state),
        ServeCommand::RemoveModel {
            id: Some("remove-model".to_string()),
            model_id: "claude-opus-gateway".to_string(),
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("remove-model")
    })
    .await;
    let removed = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("remove-model"))
        .expect("remove_model response");
    assert_eq!(removed["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_tracks_per_model_thinking_level_after_switching_models() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    for command in [
        ServeCommand::SetThinkingLevel {
            id: Some("effort-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "low".to_string(),
        },
        ServeCommand::SetThinkingLevel {
            id: Some("effort-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
            level: "max".to_string(),
        },
        ServeCommand::SetModel {
            id: Some("switch-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "deepseek-v4-pro".to_string(),
        },
        ServeCommand::GetState {
            id: Some("state-deepseek".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
        ServeCommand::SetModel {
            id: Some("switch-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
        },
        ServeCommand::GetState {
            id: Some("state-gpt".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    ] {
        handle_command(Arc::clone(&state), command).await.unwrap();
    }

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-gpt")
    })
    .await;
    let deepseek = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-deepseek"))
        .expect("deepseek get_state");
    let gpt = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-gpt"))
        .expect("gpt get_state");

    assert_eq!(
        deepseek["payload"]["model"].as_str(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(deepseek["payload"]["thinkingLevel"].as_str(), Some("max"));
    assert_eq!(gpt["payload"]["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(gpt["payload"]["thinkingLevel"].as_str(), Some("low"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_model_reestimates_an_already_displayed_waterline() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    *slot.last_context_ratio.lock() = Some(0.5);
    let current_entry = slot
        .ctx
        .session_runtime
        .session
        .current_session_entry()
        .unwrap();
    let current_model = slot.ctx.effective_model(current_entry.as_ref());
    let target_model = if current_model == "deepseek-v4-pro" {
        "gpt-5.4"
    } else {
        "deepseek-v4-pro"
    };

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetModel {
            id: Some("switch-waterline-model".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: target_model.to_string(),
        },
    )
    .await
    .expect("switch model");

    let expected_ratio = slot
        .turn_state
        .lock()
        .as_ref()
        .expect("idle slot")
        .context_state
        .usage_ratio();
    let displayed_ratio = (*slot.last_context_ratio.lock())
        .expect("model switch must refresh a provider-primed waterline");
    assert!(
        (displayed_ratio - expected_ratio).abs() < f64::EPSILON,
        "model switch must apply the new input budget before publishing its estimate: displayed={displayed_ratio}, expected={expected_ratio}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-waterline-model-switch".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .expect("get state");
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str)
            == Some("state-after-waterline-model-switch")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str)
                == Some("state-after-waterline-model-switch")
        })
        .expect("get_state response");
    assert!(
        response["payload"]["contextUtilizationRatio"]
            .as_f64()
            .is_some_and(|ratio| (ratio - expected_ratio).abs() < f64::EPSILON),
        "get_state must replay the model-budget estimate: {response:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_reports_interrupted_alongside_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);
    slot.ctx.session_runtime.cancel_token.lock().cancel();

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-interrupted".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-interrupted")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-interrupted")
        })
        .expect("get_state response");
    let payload = response["payload"].clone();

    assert_eq!(payload["busy"].as_bool(), Some(true));
    assert_eq!(payload["interrupted"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_list_sessions_live_reports_interrupted_flag() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);
    slot.ctx.session_runtime.cancel_token.lock().cancel();

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListSessions {
            id: Some("list-live-interrupted".to_string()),
            scope: Some(ListSessionsScope::Live),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-live-interrupted")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-live-interrupted")
        })
        .expect("list_sessions response");
    let sessions = response["payload"]["sessions"]
        .as_array()
        .expect("sessions array");
    let current = sessions
        .iter()
        .find(|entry| entry["sessionId"].as_str() == Some(slot.session_id.as_str()))
        .expect("current session summary");

    assert_eq!(current["busy"].as_bool(), Some(true));
    assert_eq!(current["interrupted"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_interrupt_rearms_root_token_before_next_turn_can_spawn_subagents() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "after interrupt".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![stream]).await;

    crate::api::serve::control::handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::Interrupt {
            id: Some("interrupt-rearm".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let blocked = slot
        .ctx
        .agent_registry
        .spawn_subagent_internal(
            &slot.session_id,
            crate::core::agent_loop::SubagentType::PlanReviewer,
            |ctx| async move {
                crate::core::agent_registry::SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: crate::core::agent_registry::SubagentOutcomeLabel::Completed,
                    error_message: None,
                }
            },
        )
        .await
        .expect_err("interrupt 后、rearm 前应拒绝派生子 Agent");
    assert!(
        matches!(
            blocked,
            crate::core::agent_registry::SpawnError::ParentAborted(_)
        ),
        "实际错误 = {blocked:?}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-after-interrupt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello again".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
    for _ in 0..50 {
        if !slot.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !slot.is_busy(),
        "第二回合结束后 session 应回到 idle，才能验证 rearm 结果"
    );
    assert!(
        !slot.ctx.session_runtime.cancel_token.lock().is_cancelled(),
        "start_turn 应已安装新的 turn token"
    );

    let outcome = slot
        .ctx
        .agent_registry
        .spawn_subagent_internal(
            &slot.session_id,
            crate::core::agent_loop::SubagentType::PlanReviewer,
            |ctx| async move {
                crate::core::agent_registry::SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: crate::core::agent_registry::SubagentOutcomeLabel::Completed,
                    error_message: None,
                }
            },
        )
        .await
        .expect("start_turn rearm 后应可再次派生子 Agent");
    assert_eq!(
        outcome.outcome_label,
        crate::core::agent_registry::SubagentOutcomeLabel::Completed
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_plan_mode_exit_returns_plan_mode_to_chat_without_changing_plan_file() {
    use crate::core::plan_runtime::file_store::{
        read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };

    let _api_key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;

    slot.ctx
        .session_runtime
        .plan_runtime
        .enter_plan()
        .expect("enter plan");
    let plan_path = temp.path().join("exit-pending.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "plan-exit".into(),
            goal: "leave plan mode safely".into(),
            state: PlanFileState::Pending,
            session_key: None,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".into(),
                content: "ship it".into(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            }],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Plan\n- pending".into(),
    };
    write_plan(
        &plan_path,
        &plan,
        slot.ctx.session_runtime.plan_runtime.lock_timeout_ms(),
    )
    .expect("write temp plan");
    slot.ctx
        .session_runtime
        .plan_runtime
        .bind_plan_file_for_test(plan_path.clone());

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetPlanMode {
            id: Some("exit-executing".to_string()),
            session_id: Some(slot.session_id.clone()),
            action: SetPlanModeAction::Exit,
            plan_id: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("exit-executing")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("exit-executing"))
        .expect("exit response");
    assert_eq!(
        response.get("success").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        response["payload"]["agentMode"].as_str(),
        Some("chat"),
        "退出后会话应回到 chat"
    );

    let disk_plan = read_plan(&plan_path).expect("read plan");
    assert_eq!(
        disk_plan.frontmatter.state,
        PlanFileState::Pending,
        "退出会话模式不得改写计划文件生命周期"
    );
    assert_eq!(
        slot.ctx.session_runtime.plan_runtime.mode(),
        crate::core::session::AgentMode::Chat,
        "runtime 也应回到 Chat"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_build_persists_kickoff_message_before_responding() {
    use crate::core::plan_runtime::file_store::{
        write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };

    let _api_key = install_test_api_key();
    let stream = vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })];
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![stream]).await;
    let plan_path = temp.path().join("build-persist.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "build-persist".into(),
            goal: "persist the build kickoff".into(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".into(),
                content: "run the build".into(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            }],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Plan\n- build".into(),
    };
    write_plan(
        &plan_path,
        &plan,
        slot.ctx.session_runtime.plan_runtime.lock_timeout_ms(),
    )
    .expect("write external plan");

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetPlanMode {
            id: Some("build-persist".to_string()),
            session_id: Some(slot.session_id.clone()),
            action: SetPlanModeAction::Build,
            plan_id: Some(plan_path.to_string_lossy().to_string()),
        },
    )
    .await
    .expect("build command");

    let persisted = session_message_entries(&slot)
        .into_iter()
        .find(|entry| {
            entry.message["kind"].as_str() == Some("plan_build")
                && entry.message["role"].as_str() == Some("user")
        })
        .expect("kickoff message must be persisted before the response is observable");
    assert!(
        persisted
            .message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|content| content.contains("start building")),
        "persisted message = {:?}",
        persisted.message
    );

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("build-persist")
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("build-persist")
                && line.get("success").and_then(serde_json::Value::as_bool) == Some(true)
        }),
        "build response missing: {lines:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_contains_plan_and_session_todos() {
    use crate::core::plan_runtime::file_store::{TodoItem, TodoStatus};

    tracing::info!(target: "test", phase = "arrange", test = "serve_get_state_contains_plan_and_session_todos");
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    // 注入一条 session scratchpad todo，使 get_state 的 sessionTodos 非空。
    slot.ctx
        .session_runtime
        .plan_runtime
        .replace_session_todos(vec![TodoItem {
            id: "st-1".to_string(),
            content: "wire session todos".to_string(),
            status: TodoStatus::InProgress,
            evidence: Vec::new(),
            kind: Default::default(),
        }]);

    tracing::info!(target: "test", phase = "act", test = "serve_get_state_contains_plan_and_session_todos");
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-todos".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-todos")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-todos"))
        .expect("get_state response");
    let payload = response["payload"].clone();

    tracing::info!(target: "test", phase = "assert", test = "serve_get_state_contains_plan_and_session_todos");
    assert_eq!(response["success"].as_bool(), Some(true));
    assert!(
        payload.get("activePlan").is_some(),
        "get_state payload must include activePlan"
    );
    assert!(
        payload.get("contextUtilizationRatio").is_some(),
        "get_state payload must include contextUtilizationRatio"
    );
    assert!(
        payload["activePlan"].is_null(),
        "no active plan => activePlan null"
    );
    assert!(
        payload["contextUtilizationRatio"].is_null(),
        "fresh session without persisted metrics => contextUtilizationRatio null"
    );
    // planTodos 字段必须存在且为数组（当前无 active plan → 空数组）。
    let plan_todos = payload["planTodos"]
        .as_array()
        .expect("get_state payload must include planTodos array");
    assert!(plan_todos.is_empty(), "no active plan => planTodos empty");
    // sessionTodos 必须回显注入的 in_progress 项。
    let session_todos = payload["sessionTodos"]
        .as_array()
        .expect("get_state payload must include sessionTodos array");
    assert_eq!(session_todos.len(), 1);
    assert_eq!(session_todos[0]["id"].as_str(), Some("st-1"));
    assert_eq!(
        session_todos[0]["content"].as_str(),
        Some("wire session todos")
    );
    assert_eq!(session_todos[0]["status"].as_str(), Some("in_progress"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_get_state_ignores_persisted_context_ratio() {
    use crate::core::plan_runtime::file_store::{
        write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };

    let _api_key = install_test_api_key();
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session_mgr = &slot.ctx.session_runtime.session;
    session_mgr
        .update_session(session_mgr.current_session_key(), |entry| {
            entry.context_utilization_ratio = Some(0.9);
        })
        .unwrap();
    slot.ctx
        .session_runtime
        .plan_runtime
        .enter_plan()
        .expect("enter planning");
    let plan_path = temp.path().join("active.plan.md");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: "plan-1".into(),
            goal: "Restore active plan".into(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".into(),
                content: "restore".into(),
                status: TodoStatus::Pending,
                evidence: Vec::new(),
                kind: Default::default(),
            }],
            green_build_pass: false,
            green_build_evidence: Vec::new(),
            code_review_pass: false,
            code_review_pass_at_ms: None,
            code_review_residual_findings: Vec::new(),
            completion_gate_cycles: 0,
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Plan\n- restore".into(),
    };
    write_plan(
        &plan_path,
        &plan,
        slot.ctx.session_runtime.plan_runtime.lock_timeout_ms(),
    )
    .expect("write temp plan");
    slot.ctx
        .session_runtime
        .plan_runtime
        .refresh_active_plan_after_write(plan_path.clone(), &plan);

    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-active-plan".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-active-plan")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-active-plan")
        })
        .expect("get_state response");
    let payload = response["payload"].clone();

    assert_eq!(
        payload["activePlan"]["path"].as_str(),
        Some(plan_path.to_string_lossy().as_ref())
    );
    assert!(
        payload["contextUtilizationRatio"].is_null(),
        "a fresh slot must not replay a stale persisted context ratio"
    );
    assert_eq!(payload["agentMode"].as_str(), Some("plan"));
    assert_eq!(payload["activePlan"]["id"].as_str(), Some("plan-1"));
    assert_eq!(payload["activePlan"]["state"].as_str(), Some("planning"));
    let plan_todos = payload["planTodos"]
        .as_array()
        .expect("get_state payload must include planTodos array");
    assert_eq!(plan_todos.len(), 1);
    assert_eq!(plan_todos[0]["id"].as_str(), Some("todo-1"));
    assert_eq!(plan_todos[0]["status"].as_str(), Some("pending"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_invalid_value_returns_error_without_breaking_loop() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("bad-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "turbo".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-bad-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-bad-effort")
    })
    .await;
    let error = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-effort"))
        .expect("bad effort response");
    let state_after = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-bad-effort")
        })
        .expect("follow-up get_state response");

    assert_eq!(error["success"].as_bool(), Some(false));
    assert_eq!(error["error"].as_str(), Some("invalid_thinking_level"));
    assert_eq!(state_after["success"].as_bool(), Some(true));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_context_window_persists_and_lists_selected_tier() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::UpsertModel {
            id: Some("tiered-upsert".to_string()),
            model: ModelEntryInput {
                id: "tiered-model".to_string(),
                model_name: Some("gpt-5.4".to_string()),
                api: "openai-responses".to_string(),
                provider: "openai".to_string(),
                api_key_env: Some(TEST_API_KEY_ENV.to_string()),
                base_url: Some("https://api.example.test/v1".to_string()),
                capabilities: Capabilities::default(),
                context_window: Some(400_000),
                context_window_options: Some(vec![1_000_000, 400_000]),
                max_output_tokens: None,
                description: Some("测试 Context 档位".to_string()),
                supported_reasoning_levels: None,
                thinking_format: Some("openai".to_string()),
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("tiered-upsert")
    })
    .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetContextWindow {
            id: Some("set-context-tier".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "tiered-model".to_string(),
            context_window: 1_000_000,
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("list-context-tier".to_string()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-context-tier")
    })
    .await;
    let set_response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("set-context-tier"))
        .expect("set context response");
    assert_eq!(set_response["success"].as_bool(), Some(true));
    assert_eq!(
        set_response["payload"]["contextWindow"].as_u64(),
        Some(1_000_000)
    );

    let listed = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-context-tier")
        })
        .expect("list models response");
    let model = listed["payload"]["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["id"] == "tiered-model"))
        .expect("tiered model in list");
    assert_eq!(
        model["contextWindowOptions"],
        serde_json::json!([400000, 1000000])
    );
    assert_eq!(model["selectedContextWindow"].as_u64(), Some(1_000_000));
    assert_eq!(model["description"].as_str(), Some("测试 Context 档位"));

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetContextWindow {
            id: Some("bad-context-tier".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "tiered-model".to_string(),
            context_window: 123_456,
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("bad-context-tier")
    })
    .await;
    let error = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("bad-context-tier"))
        .expect("invalid context response");
    assert_eq!(error["success"].as_bool(), Some(false));
    assert!(error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("invalid_context_window"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_for_relay_persists_under_catalog_id_and_lists_selection() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let relay_id = "relay/gpt-sol";
    let wire_model_name = "gpt-sol";

    handle_command(
        Arc::clone(&state),
        ServeCommand::UpsertModel {
            id: Some("relay-effort-upsert".to_string()),
            model: ModelEntryInput {
                id: relay_id.to_string(),
                model_name: Some(wire_model_name.to_string()),
                api: "openai-responses".to_string(),
                provider: "relay".to_string(),
                api_key_env: Some(TEST_API_KEY_ENV.to_string()),
                base_url: Some("https://relay.example.test/v1".to_string()),
                capabilities: Capabilities::default(),
                context_window: Some(400_000),
                context_window_options: None,
                max_output_tokens: None,
                description: None,
                supported_reasoning_levels: Some(vec!["high".to_string(), "xhigh".to_string()]),
                thinking_format: Some("openai".to_string()),
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("relay-effort-upsert")
    })
    .await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("set-relay-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: relay_id.to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::ListModels {
            id: Some("list-relay-effort".to_string()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-relay-effort")
    })
    .await;
    let set_response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("set-relay-effort"))
        .expect("set relay effort response");
    assert_eq!(set_response["success"].as_bool(), Some(true));

    let listed = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("list-relay-effort")
        })
        .expect("list relay models response");
    let relay = listed["payload"]["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["id"] == relay_id))
        .expect("relay model in list");
    assert_eq!(relay["modelName"].as_str(), Some(wire_model_name));
    assert_eq!(relay["selectedReasoningLevel"].as_str(), Some("xhigh"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_context_window_applies_resolved_limits_to_current_session() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.llm.default_model = "tiered-model".to_string();
    cfg.context.compaction_model = "tiered-model".to_string();
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "tiered-model"
model_name = "tiered-model"
api = "openai-responses"
provider = "openai"
api_key_env = "{env}"
base_url = "http://127.0.0.1:1"
context_window = 400000
context_window_options = [400000, 1000000]
max_output_tokens = 64000
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
        ),
    )
    .expect("write tiered model");

    let (state, _buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;
    assert_eq!(
        slot.turn_state
            .lock()
            .as_ref()
            .expect("session context")
            .context_state
            .context_budget_tokens,
        336_000,
        "the baseline model tier should initialize the session budget"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetContextWindow {
            id: Some("apply-context-tier".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: " tiered-model ".to_string(),
            context_window: 1_000_000,
        },
    )
    .await
    .expect("set context tier");

    let turn_state = slot.turn_state.lock();
    let session_context = turn_state.as_ref().expect("session context");
    assert_eq!(session_context.context_state.context_budget_tokens, 936_000);
    assert_eq!(session_context.context_budget_chars, 3_744_000);
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_set_thinking_level_persists_across_restart() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(vec![]));
    let (state, _buffer, temp, slot) =
        build_initialized_state_with_provider(temp, cfg.clone(), provider).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("persist-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();

    drop(slot);
    drop(state);

    let provider: Arc<dyn LlmProvider> = Arc::new(DeterministicMockLlm::new(vec![]));
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-restart".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-restart")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-restart")
        })
        .expect("get_state after restart");
    assert_eq!(response["payload"]["thinkingLevel"].as_str(), Some("xhigh"));
}

#[test]
fn build_shared_model_prefs_uses_global_store_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    cfg.llm.thinking.level = "medium".to_string();
    ensure_work_dir_structure(&cfg).expect("work dir");

    let store = build_shared_model_prefs(&cfg).expect("shared model preferences");
    let global_path = crate::resolve_model_thinking_path(&cfg).expect("global model thinking path");

    assert_eq!(
        store.reasoning_for("gpt-5.4"),
        crate::core::llm::ThinkingLevel::Medium
    );
    assert!(
        global_path.exists(),
        "global model thinking store should be created"
    );

    let persisted =
        std::fs::read_to_string(&global_path).expect("read global model thinking store");
    let parsed: serde_json::Value =
        serde_json::from_str(&persisted).expect("parse global model thinking store");
    assert_eq!(
        parsed["models"].as_object().map(|models| models.len()),
        Some(0)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_passes_per_model_thinking_level_to_main_loop_request() {
    let _api_key = install_test_api_key();
    let stream = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "hello".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![stream]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("prompt-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-with-effort".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "say hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    assert_eq!(
        captured.len(),
        1,
        "expected exactly one recorded LLM request"
    );
    assert_eq!(
        captured[0].thinking_level,
        Some(crate::core::llm::ThinkingLevel::Xhigh)
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_upsert_model_reloads_thinking_format_and_sends_xhigh_without_restart() {
    let _api_key = install_test_api_key();
    let server = RecordingHttpServer::start(vec![
        ScriptedHttpResponse::sse(&[
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"before\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_before\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ]),
        ScriptedHttpResponse::sse(&[
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m2\",\"content_index\":0,\"delta\":\"after\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_after\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ]),
    ])
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), &server.base_url);
    cfg.llm.default_model = "gpt-5.4".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "gpt-5.4"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "openai"
api_key_env = "{env}"
base_url = "{base_url}"
thinking_format = "anthropic"
supported_reasoning_levels = ["low", "medium", "high", "xhigh"]
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write responses override");

    let (state, buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("format-prompt-before".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "before".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _after_first = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if count_event(&lines, "agent_end") >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };

    handle_command(
        Arc::clone(&state),
        ServeCommand::UpsertModel {
            id: Some("upsert-format-openai".to_string()),
            model: ModelEntryInput {
                id: "gpt-5.4".to_string(),
                model_name: Some("gpt-5.4".to_string()),
                api: "openai-responses".to_string(),
                provider: "openai".to_string(),
                api_key_env: Some(TEST_API_KEY_ENV.to_string()),
                base_url: Some(server.base_url.clone()),
                capabilities: Capabilities {
                    vision: true,
                    files: true,
                    tools: true,
                    reasoning: true,
                    web_search: false,
                },
                context_window: Some(400_000),
                context_window_options: None,
                max_output_tokens: None,
                description: None,
                supported_reasoning_levels: Some(vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                    "xhigh".to_string(),
                ]),
                thinking_format: Some("openai".to_string()),
            },
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("set-xhigh-after-upsert".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "gpt-5.4".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("format-prompt-after".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "after".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _after_second = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if count_event(&lines, "agent_end") >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };

    let requests = server.requests();
    let before_request = requests
        .iter()
        .find(|request| {
            request.contains("\"stream\":true")
                && request.contains("\"text\":\"before\"")
                && !request.contains("\"text\":\"after\",\"type\":\"input_text\"")
        })
        .expect("expected a streamed request for the first prompt");
    assert!(
        !before_request.contains("\"effort\":\""),
        "anthropic relay format on responses wire should omit reasoning effort: {}",
        before_request
    );
    let after_request = requests
        .iter()
        .find(|request| {
            request.contains("\"stream\":true")
                && request.contains("\"text\":\"after\",\"type\":\"input_text\"")
                && request.contains("\"effort\":\"xhigh\"")
        })
        .expect("expected a streamed request for the second prompt with xhigh effort");
    assert!(
        after_request.contains("\"effort\":\"xhigh\""),
        "upserted openai format should hot-reload and send xhigh without restart: {}",
        after_request
    );

    server.shutdown().await;
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_glm_max_emits_reasoning_effort_max() {
    let _api_key = install_test_api_key();
    let server = RecordingHttpServer::start(vec![ScriptedHttpResponse::sse(&[
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
    ])])
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), &server.base_url);
    cfg.llm.default_model = "glm-5.2".to_string();
    cfg.context.compaction_model = "glm-5.2".to_string();
    cfg.llm.title_model = None;
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "glm-5.2"
model_name = "glm-5.2"
api = "openai"
provider = "zhipu"
api_key_env = "{env}"
base_url = "{base_url}"
thinking_format = "zai"
supported_reasoning_levels = ["high", "max"]
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write glm override");

    let (state, buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;
    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("set-glm-max".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "glm-5.2".to_string(),
            level: "max".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-glm-max".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "glm".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _lines = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if count_event(&lines, "agent_end") >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };

    let requests = server.requests();
    let glm_request = requests
        .iter()
        .find(|request| {
            request.contains("\"stream\":true") && request.contains("\"reasoning_effort\":\"max\"")
        })
        .expect("expected a streamed glm request with max effort");
    assert!(
        glm_request.contains("\"reasoning_effort\":\"max\""),
        "glm request should send max verbatim: {}",
        glm_request
    );

    server.shutdown().await;
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_uses_catalog_id_for_reasoning_lookup_when_model_name_differs() {
    let _api_key = install_test_api_key();
    let server = RecordingHttpServer::start(vec![ScriptedHttpResponse::sse(&[
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"ok\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_relay\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
    ])])
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), &server.base_url);
    cfg.llm.default_model = "relay/gpt-sol".to_string();
    cfg.context.compaction_model = "relay/gpt-sol".to_string();
    cfg.llm.title_model = None;
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "relay/gpt-sol"
model_name = "gpt-sol"
api = "openai-responses"
provider = "relay"
api_key_env = "{env}"
base_url = "{base_url}"
thinking_format = "openai"
supported_reasoning_levels = ["low", "medium", "high", "xhigh"]
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write relay override");

    let (state, buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;
    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("set-relay-xhigh".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "relay/gpt-sol".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-relay".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-relay".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "relay".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let state_lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-relay")
    })
    .await;
    let state_response = state_lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("state-relay"))
        .expect("relay get_state response");
    assert_eq!(
        state_response["payload"]["model"].as_str(),
        Some("relay/gpt-sol")
    );
    assert_eq!(
        state_response["payload"]["thinkingLevel"].as_str(),
        Some("xhigh")
    );

    let _turn_lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let requests = server.requests();
    let relay_request = requests
        .iter()
        .find(|request| {
            request.contains("\"stream\":true")
                && request.contains("\"model\":\"gpt-sol\"")
                && request.contains("\"effort\":\"xhigh\"")
        })
        .expect("expected a streamed relay request with xhigh effort");
    assert!(
        relay_request.contains("\"effort\":\"xhigh\""),
        "relay request should send xhigh from the catalog id selection: {}",
        relay_request
    );

    server.shutdown().await;
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_prompt_claude_adaptive_uses_output_config_effort() {
    let _api_key = install_test_api_key();
    let server = RecordingHttpServer::start(vec![ScriptedHttpResponse::sse(&[
        "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n",
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":12,\"output_tokens\":34}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ])])
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cfg = serve_test_config(temp.path(), &server.base_url);
    cfg.llm.default_model = "claude-opus-4-8".to_string();
    cfg.context.compaction_model = "claude-opus-4-8".to_string();
    cfg.llm.title_model = None;
    std::fs::write(
        temp.path().join("models.toml"),
        format!(
            r#"
[[models]]
id = "claude-opus-4-8"
model_name = "claude-opus-4-8"
api = "anthropic-messages"
provider = "anthropic"
api_key_env = "{env}"
base_url = "{base_url}"
thinking_format = "anthropic-adaptive"
supported_reasoning_levels = ["low", "medium", "high", "xhigh", "max"]
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}
"#,
            env = TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write claude override");

    let (state, buffer, _temp, slot) =
        crate::api::serve::test_support::build_initialized_state_with_config(temp, cfg).await;
    handle_command(
        Arc::clone(&state),
        ServeCommand::SetThinkingLevel {
            id: Some("set-claude-xhigh".to_string()),
            session_id: Some(slot.session_id.clone()),
            model: "claude-opus-4-8".to_string(),
            level: "xhigh".to_string(),
        },
    )
    .await
    .unwrap();
    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("prompt-claude-adaptive".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "claude".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();

    let _lines = {
        let mut lines = read_ndjson_lines(&buffer);
        for _ in 0..50 {
            lines = read_ndjson_lines(&buffer);
            if count_event(&lines, "agent_end") >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        lines
    };

    let requests = server.requests();
    let claude_request = requests
        .iter()
        .find(|request| {
            request.contains("\"stream\":true")
                && request.contains("\"output_config\":{\"effort\":\"xhigh\"}")
        })
        .expect("expected a streamed claude request with adaptive effort");
    assert!(
        claude_request.contains("\"thinking\":{\"type\":\"adaptive\"}"),
        "claude adaptive request should enable adaptive thinking: {}",
        claude_request
    );
    assert!(
        claude_request.contains("\"output_config\":{\"effort\":\"xhigh\"}"),
        "claude adaptive request should encode effort in output_config: {}",
        claude_request
    );

    server.shutdown().await;
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_list_checkpoints_returns_changed_files() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, initial_slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = initial_slot.session_id.clone();
    let anchor = append_history_message(&initial_slot, "assistant", "anchor reply");
    drop(initial_slot);

    let slot = install_checkpoint_store(
        &state,
        &session_id,
        Arc::new(FixedCheckpointStore {
            checkpoints: vec![CheckpointMeta {
                id: CheckpointId::new("ck_list"),
                session_id: session_id.clone(),
                turn_id: "turn-list-checkpoints".to_string(),
                kind: CheckpointKind::TurnEnd,
                git_commit: Some("deadbeef".to_string()),
                message_anchor: Some(anchor.clone()),
                created_at: "2026-07-12T12:00:00Z".to_string(),
                notes: Some(serde_json::json!({
                    "changedPaths": ["src/app.ts"]
                })),
            }],
            restore_report: CheckpointRestoreReport::default(),
        }),
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::ListCheckpoints {
            id: Some("list-checkpoints".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("list-checkpoints")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("list-checkpoints"))
        .expect("list checkpoints response");

    assert_eq!(response["success"].as_bool(), Some(true));
    assert_eq!(
        response["payload"]["sessionId"].as_str(),
        Some(slot.session_id.as_str())
    );
    let checkpoints = response["payload"]["checkpoints"]
        .as_array()
        .expect("checkpoints array");
    let listed = checkpoints
        .iter()
        .find(|entry| {
            entry["messageAnchor"].as_str() == Some(anchor.as_str())
                && entry["kind"].as_str() == Some("turn_end")
                && entry["changedFiles"] == serde_json::json!(["src/app.ts"])
        })
        .expect("checkpoint payload");
    assert_eq!(listed["kind"].as_str(), Some("turn_end"));
    assert_eq!(listed["messageAnchor"].as_str(), Some(anchor.as_str()));
    assert_eq!(listed["changedFiles"], serde_json::json!(["src/app.ts"]));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_restore_checkpoint_transcript_only_reports_payload_and_supersedes_messages() {
    let _api_key = install_test_api_key();
    let (state, buffer, temp, initial_slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = initial_slot.session_id.clone();
    let workspace_file = temp.path().join("notes.txt");
    std::fs::write(&workspace_file, "keep current file").expect("write workspace file");

    append_history_message(&initial_slot, "user", "before checkpoint");
    let anchor = append_history_message(&initial_slot, "assistant", "anchor reply");
    let superseded_user_id = append_history_message(&initial_slot, "user", "after checkpoint");
    let superseded_assistant_id = append_history_message(&initial_slot, "assistant", "after reply");
    drop(initial_slot);

    let checkpoint_id = CheckpointId::new("ck_restore");
    let slot = install_checkpoint_store(
        &state,
        &session_id,
        Arc::new(FixedCheckpointStore {
            checkpoints: vec![CheckpointMeta {
                id: checkpoint_id.clone(),
                session_id: session_id.clone(),
                turn_id: "turn-restore-checkpoint".to_string(),
                kind: CheckpointKind::TurnEnd,
                git_commit: Some("cafebabe".to_string()),
                message_anchor: Some(anchor.clone()),
                created_at: "2026-07-12T12:05:00Z".to_string(),
                notes: Some(serde_json::json!({
                    "changedPaths": ["notes.txt"]
                })),
            }],
            restore_report: CheckpointRestoreReport::default(),
        }),
    );
    let checkpoint_id_string = checkpoint_id.to_string();

    handle_command(
        Arc::clone(&state),
        ServeCommand::RestoreCheckpoint {
            id: Some("restore-checkpoint".to_string()),
            session_id: Some(slot.session_id.clone()),
            checkpoint_id: checkpoint_id.to_string(),
            revert_files: false,
            dry_run: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint")
        })
        .expect("restore response");

    assert_eq!(response["success"].as_bool(), Some(true));
    assert_eq!(
        response["payload"]["checkpointId"].as_str(),
        Some(checkpoint_id_string.as_str())
    );
    assert_eq!(response["payload"]["revertFiles"].as_bool(), Some(false));
    assert_eq!(
        response["payload"]["transcriptTruncated"].as_bool(),
        Some(true)
    );
    assert_eq!(
        response["payload"]["changedPaths"],
        serde_json::json!(["notes.txt"])
    );
    assert_eq!(
        std::fs::read_to_string(&workspace_file).unwrap(),
        "keep current file",
        "transcript-only restore should not modify workspace files"
    );

    let messages = session_message_entries(&slot);
    let superseded_ids = messages
        .iter()
        .filter(|entry| {
            entry
                .message
                .get("superseded")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        })
        .filter_map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    assert!(superseded_ids.contains(&superseded_user_id));
    assert!(superseded_ids.contains(&superseded_assistant_id));
    let turn_state = slot.turn_state.lock();
    let runtime_messages = &turn_state
        .as_ref()
        .expect("idle slot keeps turn state")
        .context_state
        .messages;
    assert!(
        runtime_messages.iter().all(|message| {
            message.text_content() != Some("after checkpoint")
                && message.text_content() != Some("after reply")
        }),
        "restore must also replace the in-memory context, not only supersede transcript rows"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_restore_rearms_a_question_unpaired_by_the_restore_boundary() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, initial_slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = initial_slot.session_id.clone();
    append_history_message(&initial_slot, "user", "restore this pending question");
    let anchor = initial_slot
        .ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session_id,
            serde_json::json!({
                "role": "assistant",
                "content": "需要选择",
                "tool_calls": [{
                    "id": "restore-ask-1",
                    "type": "function",
                    "function": {
                        "name": "ask_question",
                        "arguments": "{\"questions\":[{\"id\":\"q1\",\"prompt\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\",\"recommended\":true},{\"id\":\"no\",\"label\":\"No\",\"recommended\":false}]}]}"
                    }
                }]
            }),
        )
        .expect("append checkpoint anchor with ask_question");
    initial_slot
        .ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session_id,
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "restore-ask-1",
                "content": "{\"outcome\":\"host_disconnected\",\"cancelled\":true,\"answers\":[]}"
            }),
        )
        .expect("append result that restore will supersede");
    drop(initial_slot);

    let checkpoint_id = CheckpointId::new("ck_restore_pending_question");
    let slot = install_checkpoint_store(
        &state,
        &session_id,
        Arc::new(FixedCheckpointStore {
            checkpoints: vec![CheckpointMeta {
                id: checkpoint_id.clone(),
                session_id: session_id.clone(),
                turn_id: "turn-restore-pending-question".to_string(),
                kind: CheckpointKind::TurnEnd,
                git_commit: None,
                message_anchor: Some(anchor),
                created_at: "2026-07-31T12:00:00Z".to_string(),
                notes: None,
            }],
            restore_report: CheckpointRestoreReport::default(),
        }),
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::RestoreCheckpoint {
            id: Some("restore-pending-question".to_string()),
            session_id: Some(session_id.clone()),
            checkpoint_id: checkpoint_id.to_string(),
            revert_files: false,
            dry_run: None,
        },
    )
    .await
    .expect("restore checkpoint");

    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
            && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
            && line
                .get("payload")
                .and_then(|payload| payload.get("toolCallId"))
                == Some(&serde_json::Value::String("restore-ask-1".to_string()))
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("control_request")
                && line.get("subtype").and_then(serde_json::Value::as_str) == Some("ask_question")
                && line
                    .get("payload")
                    .and_then(|payload| payload.get("toolCallId"))
                    == Some(&serde_json::Value::String("restore-ask-1".to_string()))
        }),
        "restore must rehydrate and rearm the question whose prior result it superseded: {lines:?}"
    );
    assert!(
        session_message_entries(&slot).iter().any(|entry| {
            entry.message["role"] == "tool"
                && entry.message["tool_call_id"] == "restore-ask-1"
                && entry.message["content"]
                    == crate::core::session::manager::PENDING_TOOL_RESULT_TEXT
        }),
        "restore must leave the transcript protocol-complete with a [pending] result"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_compact_persists_boundary_and_rehydrates_runtime_context() {
    let _api_key = install_test_api_key();
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let provider: Arc<dyn LlmProvider> = Arc::new(CompactOnlyProvider::with_streams(vec![vec![
        Ok(StreamEvent::ContentDelta {
            delta: "the compacted session can continue".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]]));
    let (state, buffer, _temp, slot) =
        build_initialized_state_with_provider(temp, cfg, provider).await;
    let large_history = format!("historical detail {}", "x".repeat(32_000));
    append_history_message(&slot, "user", &large_history);
    append_history_message(&slot, "assistant", &large_history);
    *slot.last_context_ratio.lock() = Some(0.42);

    handle_command(
        Arc::clone(&state),
        ServeCommand::Compact {
            id: Some("compact-session".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .expect("compact command");

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("compact-session")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("compact-session"))
        .expect("compact response");
    assert_eq!(response["success"].as_bool(), Some(true));
    let before_ratio = response["payload"]["beforeUsageRatio"]
        .as_f64()
        .expect("compact response must include beforeUsageRatio");
    let after_ratio = response["payload"]["afterUsageRatio"]
        .as_f64()
        .expect("compact response must include afterUsageRatio");
    assert!(
        after_ratio < before_ratio,
        "manual compact must return a strictly smaller context ratio: {response:?}"
    );
    assert!(
        response["payload"]["coveredMessageCount"]
            .as_u64()
            .unwrap_or_default()
            >= 2,
        "manual compact must cover transcript messages: {response:?}"
    );
    assert!(
        slot.ctx
            .session_runtime
            .session
            .get_entries(16)
            .unwrap()
            .iter()
            .any(|entry| matches!(entry, crate::core::session::TranscriptEntry::BranchSummary(summary) if summary.is_boundary == Some(true))),
        "compact must persist a durable boundary"
    );
    let uses_compacted_context = {
        let turn_state = slot.turn_state.lock();
        turn_state
            .as_ref()
            .expect("idle slot")
            .context_state
            .messages
            .iter()
            .any(|message| message.kind == crate::core::llm::MessageKind::CompactionSummary)
    };
    assert!(
        uses_compacted_context,
        "serve must use the compacted context immediately, without a restart"
    );
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetState {
            id: Some("state-after-compact".to_string()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .expect("get state after compact");
    let state_lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-compact")
    })
    .await;
    let state_response = state_lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("state-after-compact")
        })
        .expect("get_state response after compact");
    let displayed_ratio = state_response["payload"]["contextUtilizationRatio"]
        .as_f64()
        .expect("compaction must refresh an already displayed waterline with its estimate");
    let rehydrated_ratio = slot
        .turn_state
        .lock()
        .as_ref()
        .expect("idle slot")
        .context_state
        .usage_ratio();
    assert!(
        (displayed_ratio - rehydrated_ratio).abs() < f64::EPSILON,
        "get_state must replay the rehydrated context estimate: displayed={displayed_ratio}, rehydrated={rehydrated_ratio}"
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("compact-follow-up".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "continue after compaction".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .expect("the compacted session accepts a following prompt");
    let lines = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
            && line.get("sessionId").and_then(serde_json::Value::as_str)
                == Some(slot.session_id.as_str())
    })
    .await;
    assert!(
        lines.iter().any(|line| {
            line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
                && line.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(slot.session_id.as_str())
        }),
        "the next prompt must complete after manual compaction: {lines:?}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_restore_checkpoint_reverts_files_and_reports_payload() {
    if !git_available() {
        return;
    }

    let _api_key = install_test_api_key();
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session_id = slot.session_id.clone();
    let workspace_file = workspace.path().join("notes.txt");
    std::fs::write(&workspace_file, "before checkpoint").expect("write checkpoint file");

    append_history_message(&slot, "user", "before checkpoint");
    let anchor = append_history_message(&slot, "assistant", "anchor reply");
    let checkpoint_id = slot
        .ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_id.clone(),
            turn_id: "turn-restore-checkpoint".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some(anchor.clone()),
            notes: Some(serde_json::json!({
                "changedPaths": ["notes.txt"]
            })),
        })
        .expect("checkpoint");
    let checkpoint_id_string = checkpoint_id.to_string();

    std::fs::write(&workspace_file, "after checkpoint").expect("mutate workspace file");
    let superseded_user_id = append_history_message(&slot, "user", "after checkpoint");
    let superseded_assistant_id = append_history_message(&slot, "assistant", "after reply");

    handle_command(
        Arc::clone(&state),
        ServeCommand::RestoreCheckpoint {
            id: Some("restore-checkpoint-revert".to_string()),
            session_id: Some(slot.session_id.clone()),
            checkpoint_id: checkpoint_id.to_string(),
            revert_files: true,
            dry_run: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-revert")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-revert")
        })
        .expect("restore response");

    assert_eq!(response["success"].as_bool(), Some(true));
    assert_eq!(
        response["payload"]["checkpointId"].as_str(),
        Some(checkpoint_id_string.as_str())
    );
    assert_eq!(response["payload"]["revertFiles"].as_bool(), Some(true));
    assert_eq!(
        response["payload"]["transcriptTruncated"].as_bool(),
        Some(true)
    );
    assert_eq!(
        response["payload"]["changedPaths"],
        serde_json::json!(["notes.txt"])
    );
    assert_eq!(
        response["payload"]["restoredPaths"],
        serde_json::json!(["notes.txt"])
    );
    assert_eq!(
        std::fs::read_to_string(&workspace_file).unwrap(),
        "before checkpoint",
        "revert restore should roll workspace files back to the checkpoint snapshot"
    );

    let messages = session_message_entries(&slot);
    let superseded_ids = messages
        .iter()
        .filter(|entry| {
            entry
                .message
                .get("superseded")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        })
        .filter_map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    assert!(superseded_ids.contains(&superseded_user_id));
    assert!(superseded_ids.contains(&superseded_assistant_id));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_restore_checkpoint_returns_busy_when_session_is_busy() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    slot.busy.store(true, Ordering::SeqCst);

    handle_command(
        Arc::clone(&state),
        ServeCommand::RestoreCheckpoint {
            id: Some("restore-checkpoint-busy".to_string()),
            session_id: Some(slot.session_id.clone()),
            checkpoint_id: "ck_busy".to_string(),
            revert_files: true,
            dry_run: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-busy")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-busy")
        })
        .expect("busy restore response");

    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(response["error"].as_str(), Some("busy"));
}

#[tokio::test]
#[serial(env_lock)]
async fn serve_restore_checkpoint_rejects_foreign_session_checkpoint() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let checkpoint_id = CheckpointId::new("ck_foreign_session");

    install_checkpoint_store(
        &state,
        &slot.session_id,
        Arc::new(FixedCheckpointStore {
            checkpoints: vec![CheckpointMeta {
                id: checkpoint_id.clone(),
                session_id: "other-session".to_string(),
                turn_id: "turn-foreign".to_string(),
                kind: CheckpointKind::TurnEnd,
                git_commit: Some("cafebabe".to_string()),
                message_anchor: Some("assistant-anchor".to_string()),
                created_at: "2026-07-12T12:10:00Z".to_string(),
                notes: Some(serde_json::json!({
                    "changedPaths": ["notes.txt"]
                })),
            }],
            restore_report: CheckpointRestoreReport::default(),
        }),
    );

    handle_command(
        Arc::clone(&state),
        ServeCommand::RestoreCheckpoint {
            id: Some("restore-checkpoint-foreign".to_string()),
            session_id: Some(slot.session_id.clone()),
            checkpoint_id: checkpoint_id.to_string(),
            revert_files: true,
            dry_run: None,
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-foreign")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| {
            line.get("id").and_then(serde_json::Value::as_str) == Some("restore-checkpoint-foreign")
        })
        .expect("foreign restore response");

    assert_eq!(response["success"].as_bool(), Some(false));
    assert_eq!(
        response["error"].as_str(),
        Some("checkpoint 不属于当前会话，不能跨会话 restore")
    );
}
