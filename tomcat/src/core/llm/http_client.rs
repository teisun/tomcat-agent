use std::time::Duration;

use crate::infra::config::LlmRuntimeConfig;
use crate::infra::error::AppError;
use crate::infra::http_client::{
    build_outbound_client, OutboundClientErrorKind, OutboundClientOptions,
};

pub(crate) fn build_http_client(
    cfg: &LlmRuntimeConfig,
    proxy_override: Option<&str>,
) -> Result<reqwest::Client, AppError> {
    let mut options = OutboundClientOptions::new(proxy_override.or(cfg.proxy.as_deref()));
    if cfg.http_read_timeout_sec > 0 {
        options.read_timeout = Some(Duration::from_secs(cfg.http_read_timeout_sec));
    }
    build_outbound_client(
        options,
        OutboundClientErrorKind::Llm,
        "创建 HTTP 客户端失败",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use crate::infra::LlmConfig;
    use serial_test::serial;
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
            ("NO_PROXY", None),
            ("no_proxy", None),
        ]);

        let cfg = LlmConfig {
            proxy: Some(cfg_proxy.url.clone()),
            ..LlmConfig::default()
        };
        let runtime = cfg.runtime();
        let client = build_http_client(&runtime, Some(&format!("{} ", override_proxy.url)))
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
            ("NO_PROXY", None),
            ("no_proxy", None),
        ]);

        let runtime = LlmConfig::default().runtime();
        let client = build_http_client(&runtime, None).expect("build http client");
        let response = client
            .get("http://example.test/from-env")
            .send()
            .await
            .expect("env proxy request should succeed");
        let body = response.text().await.expect("read response body");

        assert_eq!(body, "env");
        assert!(env_proxy.saw("GET http://example.test/from-env HTTP/1.1"));
    }
}
