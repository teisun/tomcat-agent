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
#[path = "tests/oauth_callback_test.rs"]
mod tests;
