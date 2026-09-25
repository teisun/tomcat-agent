//! Minimal loopback callback listener used by the desktop OAuth flow.

use std::net::SocketAddr;

use crate::infra::error::AppError;

pub struct OAuthCallbackListener {
    listener: tokio::net::TcpListener,
}

impl OAuthCallbackListener {
    pub async fn bind() -> Result<Self, AppError> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| AppError::Tool(format!("bind OAuth callback listener: {error}")))?;
        Ok(Self { listener })
    }

    pub async fn bind_for_redirect(redirect_uri: &str) -> Result<Self, AppError> {
        let url = reqwest::Url::parse(redirect_uri)
            .map_err(|error| AppError::Config(format!("invalid OAuth callback URL: {error}")))?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .is_some_and(|host| host == "127.0.0.1" || host == "localhost" || host == "::1")
        {
            return Err(AppError::Config(
                "OAuth callback URL must use an HTTP loopback address".to_string(),
            ));
        }
        let port = url.port().ok_or_else(|| {
            AppError::Config("OAuth callback URL must include a port".to_string())
        })?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| AppError::Tool(format!("bind OAuth callback listener: {error}")))?;
        Ok(Self { listener })
    }

    pub fn redirect_uri(&self) -> Result<String, AppError> {
        let address = self
            .listener
            .local_addr()
            .map_err(|error| AppError::Tool(format!("read OAuth callback address: {error}")))?;
        Ok(format!("http://127.0.0.1:{}/callback", address.port()))
    }

    pub async fn wait(self, expected_state: &str) -> Result<String, AppError> {
        let (mut stream, _) =
            tokio::time::timeout(std::time::Duration::from_secs(240), self.listener.accept())
                .await
                .map_err(|_| {
                    AppError::Tool("OAuth callback timed out after 5 minutes".to_string())
                })?
                .map_err(|error| AppError::Tool(format!("accept OAuth callback: {error}")))?;
        let mut request = vec![0_u8; 16 * 1024];
        let length = tokio::io::AsyncReadExt::read(&mut stream, &mut request)
            .await
            .map_err(|error| AppError::Tool(format!("read OAuth callback: {error}")))?;
        let request = String::from_utf8_lossy(&request[..length]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| {
                AppError::Tool("OAuth callback did not contain a GET target".to_string())
            })?;
        let callback_url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
            .map_err(|error| AppError::Tool(format!("parse OAuth callback URL: {error}")))?;
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
            (
                "400 Bad Request",
                "OAuth authorization failed: invalid state",
            )
        } else if code.is_none() {
            (
                "400 Bad Request",
                "OAuth authorization failed: missing code",
            )
        } else {
            (
                "200 OK",
                "Authorization complete. You may close this window.",
            )
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
            .await
            .map_err(|error| AppError::Tool(format!("write OAuth callback response: {error}")))?;
        if !valid_state {
            return Err(AppError::Tool(
                "OAuth callback state did not match".to_string(),
            ));
        }
        code.ok_or_else(|| AppError::Tool("OAuth callback missing authorization code".to_string()))
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
