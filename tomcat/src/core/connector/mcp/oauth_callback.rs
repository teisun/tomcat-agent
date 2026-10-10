//! Minimal loopback callback listener used by the desktop OAuth flow.

use std::net::SocketAddr;

use crate::infra::error::AppError;
use crate::infra::i18n::tr;

pub struct OAuthCallbackListener {
    listener: tokio::net::TcpListener,
}

impl OAuthCallbackListener {
    pub async fn bind() -> Result<Self, AppError> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| {
                AppError::Tool(tr("oauth.bindFailed", &[("detail", &error.to_string())]))
            })?;
        Ok(Self { listener })
    }

    pub async fn bind_for_redirect(redirect_uri: &str) -> Result<Self, AppError> {
        let url = reqwest::Url::parse(redirect_uri).map_err(|error| {
            AppError::Config(tr("oauth.invalidUrl", &[("detail", &error.to_string())]))
        })?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .is_some_and(|host| host == "127.0.0.1" || host == "localhost" || host == "::1")
        {
            return Err(AppError::Config(tr("oauth.loopbackRequired", &[])));
        }
        let port = url
            .port()
            .ok_or_else(|| AppError::Config(tr("oauth.portRequired", &[])))?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| {
                AppError::Tool(tr("oauth.bindFailed", &[("detail", &error.to_string())]))
            })?;
        Ok(Self { listener })
    }

    pub fn redirect_uri(&self) -> Result<String, AppError> {
        let address = self.listener.local_addr().map_err(|error| {
            AppError::Tool(tr("oauth.addressFailed", &[("detail", &error.to_string())]))
        })?;
        Ok(format!("http://127.0.0.1:{}/callback", address.port()))
    }

    pub async fn wait(self, expected_state: &str) -> Result<String, AppError> {
        let (mut stream, _) =
            tokio::time::timeout(std::time::Duration::from_secs(240), self.listener.accept())
                .await
                .map_err(|_| AppError::Tool(tr("oauth.timeout", &[])))?
                .map_err(|error| {
                    AppError::Tool(tr("oauth.acceptFailed", &[("detail", &error.to_string())]))
                })?;
        let mut request = vec![0_u8; 16 * 1024];
        let length = tokio::io::AsyncReadExt::read(&mut stream, &mut request)
            .await
            .map_err(|error| {
                AppError::Tool(tr("oauth.readFailed", &[("detail", &error.to_string())]))
            })?;
        let request = String::from_utf8_lossy(&request[..length]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| AppError::Tool(tr("oauth.getRequired", &[])))?;
        let callback_url =
            reqwest::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|error| {
                AppError::Tool(tr("oauth.parseFailed", &[("detail", &error.to_string())]))
            })?;
        let state = callback_url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned());
        let code = callback_url
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned());
        let valid_state = state.as_deref() == Some(expected_state);
        let (status, body) = if !valid_state {
            ("400 Bad Request", tr("oauth.statePage", &[]))
        } else if code.is_none() {
            ("400 Bad Request", tr("oauth.codePage", &[]))
        } else {
            ("200 OK", tr("oauth.successPage", &[]))
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
            .await
            .map_err(|error| {
                AppError::Tool(tr("oauth.writeFailed", &[("detail", &error.to_string())]))
            })?;
        if !valid_state {
            return Err(AppError::Tool(tr("oauth.stateMismatch", &[])));
        }
        code.ok_or_else(|| AppError::Tool(tr("oauth.codeMissing", &[])))
    }
}

#[allow(dead_code)]
fn _socket_address_is_loopback(address: SocketAddr) -> bool {
    address.ip().is_loopback()
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    use super::OAuthCallbackListener;

    #[tokio::test]
    async fn binds_a_loopback_callback_with_dynamic_port() {
        let listener = OAuthCallbackListener::bind().await.expect("listener");
        let redirect = listener.redirect_uri().expect("redirect");
        assert!(redirect.starts_with("http://127.0.0.1:"));
        assert!(redirect.ends_with("/callback"));
    }

    #[tokio::test]
    async fn dropping_a_scoped_callback_releases_its_listener() {
        let listener = OAuthCallbackListener::bind().await.unwrap();
        let address = listener.listener.local_addr().unwrap();
        let mut waiting = Box::pin(listener.wait("state"));
        tokio::select! {
            biased;
            _ = &mut waiting => panic!("callback should still be waiting"),
            _ = tokio::task::yield_now() => {},
        }
        drop(waiting);
        let _rebound = tokio::net::TcpListener::bind(address)
            .await
            .expect("listener must not remain in a detached task");
    }

    #[tokio::test]
    async fn callback_pages_keep_status_and_utf8_byte_length_without_echoing_code() {
        use crate::infra::i18n::{tr_in, Locale};
        use tokio::io::AsyncReadExt;
        for (query, status, key) in [
            (
                "code=private-code&state=expected",
                "200 OK",
                "oauth.successPage",
            ),
            ("state=expected", "400 Bad Request", "oauth.codePage"),
            (
                "code=private-code&state=other",
                "400 Bad Request",
                "oauth.statePage",
            ),
        ] {
            let listener = OAuthCallbackListener::bind().await.unwrap();
            let address = listener.listener.local_addr().unwrap();
            let task = tokio::spawn(async move { listener.wait("expected").await });
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            let result = task.await.unwrap();
            assert_eq!(result.is_ok(), status == "200 OK");
            let (headers, content) = response.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with(&format!("HTTP/1.1 {status}")));
            assert!(headers.contains(&format!("Content-Length: {}", content.len())));
            assert!(content.contains(&tr_in(Locale::En, key, &[])), "{content}");
            assert!(!content.contains("private-code"));
        }
    }

    #[tokio::test]
    async fn rejects_invalid_state() {
        let listener = OAuthCallbackListener::bind().await.expect("listener");
        let address = listener.listener.local_addr().expect("address");
        let task = tokio::spawn(async move { listener.wait("expected-state").await });
        let mut stream = TcpStream::connect(address)
            .await
            .expect("callback connection");
        stream
            .write_all(b"GET /callback?code=code&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("callback request");
        assert!(task.await.expect("callback task").is_err());
    }
}
