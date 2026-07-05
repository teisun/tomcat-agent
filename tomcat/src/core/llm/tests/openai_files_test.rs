use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::core::llm::openai_files::{
    upload_decision_by_size, FilePurpose, OpenAiFilesClient, OpenAiFilesRuntime, UploadDecision,
};
use crate::core::llm::types::ChatMessageContentPart;
use crate::infra::error::{llm_error, llm_http_status, llm_http_status_error, LlmErrorStage};

#[derive(Debug, Clone)]
struct ScriptedResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    delay_ms: u64,
}

impl ScriptedResponse {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.to_string(),
            delay_ms: 0,
        }
    }

    fn with_delay_ms(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }
}

struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockServer {
    async fn start(initial_responses: Vec<ScriptedResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responses = Arc::new(Mutex::new(VecDeque::from(initial_responses)));
        let requests_clone = Arc::clone(&requests);
        let responses_clone = Arc::clone(&responses);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { continue; };
                        let request_bytes = read_full_http_request(&mut socket).await;
                        requests_clone.lock().unwrap().push(request_bytes);
                        let resp = responses_clone
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or_else(|| ScriptedResponse::json(500, r#"{"error":"unplanned request"}"#));
                        if resp.delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(resp.delay_ms)).await;
                        }
                        let mut headers = String::new();
                        for (k, v) in &resp.headers {
                            headers.push_str(k);
                            headers.push_str(": ");
                            headers.push_str(v);
                            headers.push_str("\r\n");
                        }
                        headers.push_str("Connection: close\r\n");
                        let reason = status_reason(resp.status);
                        let body_bytes = resp.body.as_bytes();
                        let raw = format!(
                            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n{}\r\n",
                            resp.status,
                            reason,
                            body_bytes.len(),
                            headers
                        );
                        let _ = socket.write_all(raw.as_bytes()).await;
                        let _ = socket.write_all(body_bytes).await;
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

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn request_texts(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|raw| String::from_utf8_lossy(raw).to_string())
            .collect()
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

fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

async fn read_full_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
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
    buf
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
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

fn test_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build no-proxy reqwest client for local mock server")
}

#[test]
fn upload_decision_matches_arch_thresholds() {
    assert_eq!(
        upload_decision_by_size(1024),
        UploadDecision::InlinePreferred
    );
    assert_eq!(
        upload_decision_by_size(1024 * 1024),
        UploadDecision::UploadPreferred
    );
    assert_eq!(
        upload_decision_by_size(9 * 1024 * 1024),
        UploadDecision::UploadPreferred
    );
    assert_eq!(
        upload_decision_by_size(10 * 1024 * 1024),
        UploadDecision::UploadRequired
    );
}

#[test]
fn registry_path_sanitizes_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = OpenAiFilesRuntime::registry_path_for_session(dir.path(), "agent:main/main");
    let fname = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    assert!(fname.contains("agent_main_main"));
}

#[test]
fn is_retriable_parity_with_responses() {
    assert!(OpenAiFilesClient::is_retriable(&llm_http_status_error(
        "openai-files",
        429,
        "rate limit",
    )));
    assert!(OpenAiFilesClient::is_retriable(&llm_error(
        "openai-files",
        LlmErrorStage::Connect,
        "connection timeout",
    )));
    assert!(!OpenAiFilesClient::is_retriable(&llm_http_status_error(
        "openai-files",
        400,
        "bad request",
    )));
}

#[test]
fn classify_http_error_preserves_status_and_guidance() {
    let client = OpenAiFilesClient::new_for_test(
        test_http_client(),
        "https://api.openai.com".to_string(),
        "stub".to_string(),
        0,
        86_400,
    );
    for (status, body, needle) in [
        (
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid_api_key"}"#,
            "API Key 无效",
        ),
        (
            reqwest::StatusCode::FORBIDDEN,
            r#"{"error":"organization_restricted"}"#,
            "Project/组织未启用 Files",
        ),
        (
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"purpose is invalid"}"#,
            "purpose 不被接受",
        ),
        (
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"error":"file_too_large"}"#,
            "文件超过 OpenAI 上限",
        ),
    ] {
        let err = client.classify_http_error(status, body, "upload");
        assert_eq!(llm_http_status(&err), Some(status.as_u16()));
        assert!(
            err.to_string().contains(needle),
            "status={} 文案应包含 `{}`，实际: {}",
            status,
            needle,
            err
        );
        assert!(
            !OpenAiFilesClient::is_retriable(&err),
            "status={} 不应被视为可重试",
            status
        );
    }
}

#[test]
fn registry_persist_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("openai-files-test.json");
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            "https://api.openai.com".to_string(),
            "stub".to_string(),
            0,
            86_400,
        ),
        registry.clone(),
    );
    runtime.enqueue_delete("file-abc".to_string(), Some(12), Some(34), "manual");
    assert!(runtime.pending_cleanup_count() >= 1);
    drop(runtime);

    let runtime2 = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            "https://api.openai.com".to_string(),
            "stub".to_string(),
            0,
            86_400,
        ),
        registry.clone(),
    );
    assert!(runtime2.pending_cleanup_count() >= 1);
}

#[tokio::test]
async fn upload_retries_on_500_then_succeeds() {
    let server = MockServer::start(vec![
        ScriptedResponse::json(500, r#"{"error":"oops"}"#),
        ScriptedResponse::json(
            200,
            r#"{"id":"file-retry","filename":"a.png","bytes":3,"created_at":1700000000,"purpose":"vision"}"#,
        ),
    ])
    .await;
    let client = OpenAiFilesClient::new_for_test(
        test_http_client(),
        server.base_url.clone(),
        "stub".to_string(),
        1,
        86_400,
    );
    let meta = client
        .upload(FilePurpose::Vision, "a.png", "image/png", &[1, 2, 3])
        .await
        .expect("retry then success");
    assert_eq!(meta.id, "file-retry");
    assert_eq!(server.request_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn delete_404_is_ok() {
    let server = MockServer::start(vec![ScriptedResponse::json(
        404,
        r#"{"error":"not found"}"#,
    )])
    .await;
    let client = OpenAiFilesClient::new_for_test(
        test_http_client(),
        server.base_url.clone(),
        "stub".to_string(),
        0,
        86_400,
    );
    client
        .delete("file-missing")
        .await
        .expect("404 should be idempotent ok");
    assert_eq!(server.request_count(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn upload_expires_after_default_vs_zero_semantics() {
    let server_a = MockServer::start(vec![ScriptedResponse::json(
        200,
        r#"{"id":"file-a","filename":"a.pdf","bytes":3,"created_at":1700000000,"purpose":"user_data"}"#,
    )])
    .await;
    let client_a = OpenAiFilesClient::new_for_test(
        test_http_client(),
        server_a.base_url.clone(),
        "stub".to_string(),
        0,
        86_400,
    );
    client_a
        .upload(
            FilePurpose::UserData,
            "a.pdf",
            "application/pdf",
            &[1, 2, 3],
        )
        .await
        .unwrap();
    let req_a = server_a.request_texts().remove(0);
    assert!(req_a.contains("expires_after[anchor]"));
    assert!(req_a.contains("expires_after[seconds]"));
    assert!(req_a.contains("86400"));
    server_a.shutdown().await;

    let server_b = MockServer::start(vec![ScriptedResponse::json(
        200,
        r#"{"id":"file-b","filename":"b.pdf","bytes":3,"created_at":1700000000,"purpose":"user_data"}"#,
    )])
    .await;
    let client_b = OpenAiFilesClient::new_for_test(
        test_http_client(),
        server_b.base_url.clone(),
        "stub".to_string(),
        0,
        0,
    );
    client_b
        .upload(
            FilePurpose::UserData,
            "b.pdf",
            "application/pdf",
            &[4, 5, 6],
        )
        .await
        .unwrap();
    let req_b = server_b.request_texts().remove(0);
    assert!(
        !req_b.contains("expires_after["),
        "expires_after=0 应不写任何 expires_after 字段，实际请求:\n{}",
        req_b
    );
    server_b.shutdown().await;
}

#[tokio::test]
async fn cache_hit_only_one_post() {
    let server = MockServer::start(vec![ScriptedResponse::json(
        200,
        r#"{"id":"file-cache-1","filename":"x.png","bytes":3,"created_at":1700000000,"purpose":"vision"}"#,
    )])
    .await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [1u8, 2, 3]).unwrap();
    let registry_dir = tempfile::tempdir().unwrap();
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            server.base_url.clone(),
            "stub".to_string(),
            0,
            86_400,
        ),
        registry_dir.path().join("registry.json"),
    );
    let a = runtime
        .resolve_or_upload_path(tmp.path(), "image/png", "x.png", FilePurpose::Vision)
        .await
        .unwrap();
    let b = runtime
        .resolve_or_upload_path(tmp.path(), "image/png", "x.png", FilePurpose::Vision)
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(
        server.request_count(),
        1,
        "cache hit should skip second POST"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn cache_path_changed_uploads_new_id_and_enqueues_delete() {
    let server = MockServer::start(vec![
        ScriptedResponse::json(
            200,
            r#"{"id":"file-old","filename":"x.png","bytes":3,"created_at":1700000000,"purpose":"vision"}"#,
        ),
        ScriptedResponse::json(
            200,
            r#"{"id":"file-new","filename":"x.png","bytes":3,"created_at":1700000001,"purpose":"vision"}"#,
        ),
    ])
    .await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [1u8, 1, 1]).unwrap();
    let registry_dir = tempfile::tempdir().unwrap();
    let registry_path = registry_dir.path().join("registry.json");
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            server.base_url.clone(),
            "stub".to_string(),
            0,
            86_400,
        ),
        registry_path.clone(),
    );
    let old = runtime
        .resolve_or_upload_path(tmp.path(), "image/png", "x.png", FilePurpose::Vision)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    std::fs::write(tmp.path(), [2u8, 2, 2]).unwrap();
    let new_meta = runtime
        .resolve_or_upload_path(tmp.path(), "image/png", "x.png", FilePurpose::Vision)
        .await
        .unwrap();
    assert_ne!(old.id, new_meta.id);
    assert_eq!(server.request_count(), 2);
    let persisted = std::fs::read_to_string(registry_path).unwrap();
    assert!(
        persisted.contains("cache_evicted"),
        "路径内容变更后应把旧 file_id 入删除队列，registry={}",
        persisted
    );
    server.shutdown().await;
}

#[tokio::test]
async fn cache_evicts_expired_entry_then_reuploads() {
    let server = MockServer::start(vec![
        ScriptedResponse::json(
            200,
            r#"{"id":"file-expired","filename":"x.pdf","bytes":3,"created_at":1700000000,"purpose":"user_data","expires_at":1}"#,
        ),
        ScriptedResponse::json(
            200,
            r#"{"id":"file-fresh","filename":"x.pdf","bytes":3,"created_at":1700000001,"purpose":"user_data","expires_at":4102444800}"#,
        ),
    ])
    .await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [5u8, 5, 5]).unwrap();
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            server.base_url.clone(),
            "stub".to_string(),
            0,
            86_400,
        ),
        tempfile::tempdir().unwrap().path().join("registry.json"),
    );
    let first = runtime
        .resolve_or_upload_path(
            tmp.path(),
            "application/pdf",
            "x.pdf",
            FilePurpose::UserData,
        )
        .await
        .unwrap();
    let second = runtime
        .resolve_or_upload_path(
            tmp.path(),
            "application/pdf",
            "x.pdf",
            FilePurpose::UserData,
        )
        .await
        .unwrap();
    assert_ne!(
        first.id, second.id,
        "expired cache entry should trigger reupload"
    );
    assert_eq!(server.request_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn concurrent_same_hash_single_flight() {
    let server = MockServer::start(vec![ScriptedResponse::json(
        200,
        r#"{"id":"file-single-flight","filename":"x.png","bytes":3,"created_at":1700000000,"purpose":"vision"}"#,
    )
    .with_delay_ms(80)])
    .await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), [9u8, 9, 9]).unwrap();
    let runtime = Arc::new(OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            server.base_url.clone(),
            "stub".to_string(),
            0,
            86_400,
        ),
        tempfile::tempdir().unwrap().path().join("registry.json"),
    ));

    let r1 = {
        let runtime = Arc::clone(&runtime);
        let path = tmp.path().to_path_buf();
        tokio::spawn(async move {
            runtime
                .resolve_or_upload_path(&path, "image/png", "x.png", FilePurpose::Vision)
                .await
                .unwrap()
                .id
        })
    };
    let r2 = {
        let runtime = Arc::clone(&runtime);
        let path = tmp.path().to_path_buf();
        tokio::spawn(async move {
            runtime
                .resolve_or_upload_path(&path, "image/png", "x.png", FilePurpose::Vision)
                .await
                .unwrap()
                .id
        })
    };
    let (id1, id2) = tokio::join!(r1, r2);
    assert_eq!(id1.unwrap(), "file-single-flight");
    assert_eq!(id2.unwrap(), "file-single-flight");
    assert_eq!(
        server.request_count(),
        1,
        "single-flight should collapse concurrent uploads"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn cleanup_empty_registry_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            "https://api.openai.com".to_string(),
            "stub".to_string(),
            0,
            86_400,
        ),
        dir.path().join("none.json"),
    );
    let summary = runtime.cleanup_registered_files("session_end").await;
    assert_eq!(summary.total, 0);
    assert_eq!(summary.deleted, 0);
    assert_eq!(summary.failed, 0);
    assert!(!Path::new(&dir.path().join("none.json")).exists());
}

#[tokio::test]
async fn tui_two_phase_attachment_order_interface() {
    let server = MockServer::start(vec![ScriptedResponse::json(
        200,
        r#"{"id":"file-tui-phase","filename":"phase.pdf","bytes":5,"created_at":1700000000,"purpose":"user_data"}"#,
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phase.pdf");
    std::fs::write(&path, b"%PDF-1.7\nx").unwrap();
    let runtime = OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            test_http_client(),
            server.base_url.clone(),
            "stub".to_string(),
            0,
            86_400,
        ),
        dir.path().join("registry.json"),
    );

    // 阶段 A：仅上传（不组 ChatRequest）。
    let uploaded = runtime
        .resolve_or_upload_path(&path, "application/pdf", "phase.pdf", FilePurpose::UserData)
        .await
        .unwrap();
    assert_eq!(uploaded.id, "file-tui-phase");

    // 阶段 B：文本 + 阶段 A 累积的 file_id 组装 user parts。
    let parts = [
        ChatMessageContentPart::text("请总结附件"),
        ChatMessageContentPart::file_file_id(uploaded.id, Some("phase.pdf".to_string())).unwrap(),
    ];
    assert!(matches!(
        &parts[0],
        ChatMessageContentPart::InputText { text } if text == "请总结附件"
    ));
    assert!(matches!(
        &parts[1],
        ChatMessageContentPart::InputFile {
            source: crate::core::llm::FileSource::Uploaded(ref uploaded),
        } if uploaded.file_id == "file-tui-phase"
    ));
    server.shutdown().await;
}
