use crate::core::llm::http_client::build_http_client;
use crate::infra::LlmConfig;
use serial_test::serial;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
async fn web_search_build_http_client_prefers_proxy_override_over_config_and_env() {
    let env_proxy = ProxyCaptureServer::start("env").await;
    let cfg_proxy = ProxyCaptureServer::start("cfg").await;
    let override_proxy = ProxyCaptureServer::start("override").await;
    let _env = EnvGuard::set_many(&[
        ("HTTP_PROXY", Some(env_proxy.url.as_str())),
        ("HTTPS_PROXY", None),
        ("ALL_PROXY", None),
        // Concurrent tests talk to loopback mock servers; only example.test may reach this proxy.
        ("NO_PROXY", Some("127.0.0.1,localhost")),
        ("no_proxy", Some("127.0.0.1,localhost")),
    ]);
    let cfg = LlmConfig {
        proxy: Some(cfg_proxy.url.clone()),
        ..LlmConfig::default()
    };
    let runtime = cfg.runtime();
    let client = build_http_client(&runtime, Some(&format!("{} ", override_proxy.url)), Some("http://example.test"), None)
        .expect("build http client");
    let response = client
        .get("http://example.test/override")
        .send()
        .await
        .expect("proxy request should succeed");
    let body = response.text().await.expect("read response body");
    assert_eq!(body, "override");
    assert!(override_proxy.saw("GET http://example.test/override HTTP/1.1"));
    assert!(!cfg_proxy.saw("GET http://example.test/override HTTP/1.1"));
    assert!(!env_proxy.saw("GET http://example.test/override HTTP/1.1"));
}

#[tokio::test]
#[serial]
async fn web_search_build_http_client_uses_env_proxy_when_config_is_absent() {
    let env_proxy = ProxyCaptureServer::start("env").await;
    let _env = EnvGuard::set_many(&[
        ("HTTP_PROXY", Some(env_proxy.url.as_str())),
        ("HTTPS_PROXY", None),
        ("ALL_PROXY", None),
        // Concurrent tests talk to loopback mock servers; only example.test may reach this proxy.
        ("NO_PROXY", Some("127.0.0.1,localhost")),
        ("no_proxy", Some("127.0.0.1,localhost")),
    ]);
    let runtime = LlmConfig::default().runtime();
    let client = build_http_client(&runtime, None, Some("http://example.test"), None).expect("build http client");
    let response = client
        .get("http://example.test/from-env")
        .send()
        .await
        .expect("env proxy request should succeed");
    let body = response.text().await.expect("read response body");
    assert_eq!(body, "env");
    assert!(env_proxy.saw("GET http://example.test/from-env HTTP/1.1"));
}

// No NO_PROXY escape hatch: these requests must exercise the client's route decision.
#[tokio::test]
#[serial]
async fn loopback_routes_bypass_discovery_but_preserve_explicit_proxy_and_remote_fallback() {
    for (remote_primary, fallback, explicit_proxy, expected) in [
        (false, None, false, "direct"),
        (false, Some("http://127.0.0.2:1234"), false, "direct"),
        (true, None, false, "proxy"),
        (false, Some("http://fallback.invalid"), false, "proxy"),
        (false, None, true, "proxy"),
    ] {
        let proxy = ProxyCaptureServer::start("proxy").await;
        let model = ProxyCaptureServer::start("direct").await;
        #[cfg(unix)]
        let _lowercase = EnvGuard::set_many(&[
            ("http_proxy", None), ("https_proxy", None), ("all_proxy", None), ("no_proxy", None),
        ]);
        let _env = EnvGuard::set_many(&[
            ("HTTP_PROXY", Some(&proxy.url)), ("HTTPS_PROXY", None),
            ("ALL_PROXY", None), ("NO_PROXY", Some("")),
        ]);
        let cfg = LlmConfig {
            proxy: explicit_proxy.then(|| proxy.url.clone()),
            api_base_fallback: fallback.map(str::to_owned),
            ..LlmConfig::default()
        };
        let target = if remote_primary { "http://model.invalid" } else { &model.url };
        let route = crate::core::llm::registry::ProviderRoute::new(&cfg.runtime(), Some(target)).unwrap();
        let body = route.client.get(format!("{target}/route-check"))
            .timeout(std::time::Duration::from_secs(5)).send().await.unwrap().text().await.unwrap();
        assert_eq!(body, expected, "remote={remote_primary}, fallback={fallback:?}, explicit={explicit_proxy}");
        assert_eq!(proxy.saw("/route-check"), expected == "proxy");
        assert_eq!(model.saw("/route-check"), expected == "direct");
    }
}
