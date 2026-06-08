//! # 共享测试 fixture
//!
//! 当前仅含 `load_dotenv`：从 crate 根目录加载 `.env`，便于本地有 `OPENAI_API_KEY`
//! 时跑真实 API 用例。所有依赖 `.env` 的测试都先调用此函数，避免重复样板。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// 从 crate 根目录加载 `.env`，便于本地有 key 时跑测试（与 `tests/common::load_openai_test_env` 对齐）。
pub(crate) fn load_dotenv() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(&path);
    let _ = dotenvy::dotenv();
}

#[derive(Debug, Clone)]
pub(crate) struct ScriptedHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay_ms: u64,
    pub declared_content_length: Option<usize>,
}

impl ScriptedHttpResponse {
    pub(crate) fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.to_string(),
            delay_ms: 0,
            declared_content_length: None,
        }
    }

    pub(crate) fn with_declared_content_length(mut self, declared_content_length: usize) -> Self {
        self.declared_content_length = Some(declared_content_length);
        self
    }
}

pub(crate) struct MockHttpServer {
    pub(crate) base_url: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockHttpServer {
    pub(crate) async fn start(initial_responses: Vec<ScriptedHttpResponse>) -> Self {
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
                            .unwrap_or_else(|| ScriptedHttpResponse::json(500, r#"{"error":"unplanned request"}"#));
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
                        let declared_content_length =
                            resp.declared_content_length.unwrap_or(body_bytes.len());
                        let raw = format!(
                            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n{}\r\n",
                            resp.status,
                            reason,
                            declared_content_length,
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

    pub(crate) fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
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

fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}
