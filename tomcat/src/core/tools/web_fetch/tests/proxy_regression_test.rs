use std::sync::{Arc, Mutex};

use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::test_config;

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set_many(entries: &[(&str, Option<&str>)]) -> Self {
        let mut saved = Vec::new();
        for (key, value) in entries {
            saved.push(((*key).to_string(), std::env::var(key).ok()));
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(&key, value) },
                None => unsafe { std::env::remove_var(&key) },
            }
        }
    }
}

struct ProxyCaptureServer {
    url: String,
    request: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyCaptureServer {
    async fn start(response_body: &'static str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind proxy server");
        let addr = listener.local_addr().expect("proxy addr");
        let request = Arc::new(Mutex::new(None));
        let request_capture = request.clone();
        let body = response_body.to_string();
        let task = tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let request_bytes = read_http_request(&mut socket).await;
            *request_capture.lock().expect("lock request") =
                Some(String::from_utf8_lossy(&request_bytes).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        Self {
            url: format!("http://{}", addr),
            request,
            task,
        }
    }

    fn saw(&self, needle: &str) -> bool {
        self.request
            .lock()
            .expect("lock request")
            .as_ref()
            .is_some_and(|request| request.contains(needle))
    }
}

impl Drop for ProxyCaptureServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 1024];
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    buf
}

#[tokio::test]
#[serial]
async fn web_search_build_web_fetch_http_client_prefers_trimmed_llm_proxy_over_env() {
    let env_proxy = ProxyCaptureServer::start("env").await;
    let cfg_proxy = ProxyCaptureServer::start("cfg").await;
    let _env = EnvGuard::set_many(&[
        ("HTTP_PROXY", Some(env_proxy.url.as_str())),
        ("HTTPS_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let mut cfg = test_config();
    cfg.llm.proxy = Some(format!("{} ", cfg_proxy.url));
    let client = super::super::build_web_fetch_http_client(&cfg, &cfg.tools.web_fetch)
        .expect("build web_fetch client");
    let response = client
        .get("http://example.test/web-fetch-cfg")
        .send()
        .await
        .expect("cfg proxy request should succeed");
    let body = response.text().await.expect("read response body");

    assert_eq!(body, "cfg");
    assert!(cfg_proxy.saw("GET http://example.test/web-fetch-cfg HTTP/1.1"));
    assert!(!env_proxy.saw("GET http://example.test/web-fetch-cfg HTTP/1.1"));
}

#[tokio::test]
#[serial]
async fn web_search_build_web_fetch_http_client_uses_env_proxy_when_llm_proxy_missing() {
    let env_proxy = ProxyCaptureServer::start("env").await;
    let _env = EnvGuard::set_many(&[
        ("HTTP_PROXY", Some(env_proxy.url.as_str())),
        ("HTTPS_PROXY", None),
        ("ALL_PROXY", None),
        ("NO_PROXY", None),
        ("no_proxy", None),
    ]);

    let cfg = test_config();
    let client = super::super::build_web_fetch_http_client(&cfg, &cfg.tools.web_fetch)
        .expect("build web_fetch client");
    let response = client
        .get("http://example.test/web-fetch-env")
        .send()
        .await
        .expect("env proxy request should succeed");
    let body = response.text().await.expect("read response body");

    assert_eq!(body, "env");
    assert!(env_proxy.saw("GET http://example.test/web-fetch-env HTTP/1.1"));
}
