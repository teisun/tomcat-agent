//! Delegate protocol parsing/routing to rmcp, retaining per-request I/O
//! receipts and structured HTTP facts before the SDK flattens an error body.
use std::collections::HashMap;
use std::future::Future;
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc,
};

use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, GetExtensions};
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

use super::call::{CallIo, IoGuard};
use super::failure::{FailureKind, McpFailure};
type Headers = HashMap<HeaderName, HeaderValue>;
type Error = StreamableHttpError<McpFailure>;

tokio::task_local! { static HTTP_STATUS: Arc<AtomicU16>; }

/// The classifier only observes headers and ALWAYS returns Success (no retry).
/// This retains reqwest::retry::never's no-replay policy; it is not a retry hook
/// for tools. The response body, URL and headers are never formatted or stored.
pub(crate) fn no_replay_policy() -> reqwest::retry::Builder {
    reqwest::retry::never().classify_fn(|exchange| {
        if let Some(status) = exchange.status() {
            let _ = HTTP_STATUS.try_with(|slot| slot.store(status.as_u16(), Ordering::SeqCst));
        }
        exchange.success()
    })
}

fn sanitized(error: StreamableHttpError<reqwest::Error>, status: Option<u16>) -> Error {
    // These variants control SDK protocol/stream behavior and carry no secrets.
    match error {
        StreamableHttpError::SessionExpired => return StreamableHttpError::SessionExpired,
        StreamableHttpError::ServerDoesNotSupportSse => {
            return StreamableHttpError::ServerDoesNotSupportSse
        }
        StreamableHttpError::ServerDoesNotSupportDeleteSession => {
            return StreamableHttpError::ServerDoesNotSupportDeleteSession
        }
        StreamableHttpError::UnexpectedEndOfStream => {
            return StreamableHttpError::UnexpectedEndOfStream
        }
        StreamableHttpError::TransportChannelClosed => {
            return StreamableHttpError::TransportChannelClosed
        }
        StreamableHttpError::ControlRequestTimeout => {
            return StreamableHttpError::ControlRequestTimeout
        }
        StreamableHttpError::SessionRecoveryTimeout => {
            return StreamableHttpError::SessionRecoveryTimeout
        }
        _ => {}
    }
    let mut failure = McpFailure::new(FailureKind::Protocol, "http");
    failure.http_status = status;
    match error {
        StreamableHttpError::AuthRequired(auth) => {
            failure.kind = FailureKind::Authorization;
            failure.http_status = Some(401);
            failure.challenge = Some(auth.www_authenticate_header);
        }
        StreamableHttpError::InsufficientScope(auth) => {
            failure.kind = FailureKind::Authorization;
            failure.http_status = Some(403);
            failure.challenge = Some(auth.www_authenticate_header);
        }
        StreamableHttpError::Client(error) => {
            failure.http_status = failure
                .http_status
                .or_else(|| error.status().map(|s| s.as_u16()));
            failure.kind = if error.is_timeout() {
                FailureKind::Timeout
            } else if error.is_connect() || error.is_body() || error.is_request() {
                FailureKind::Transient
            } else {
                FailureKind::Protocol
            };
        }
        StreamableHttpError::Io(_) => failure.kind = FailureKind::Transient,
        StreamableHttpError::ReservedHeaderConflict(_) => failure.kind = FailureKind::Configuration,
        _ => {}
    }
    if failure.kind != FailureKind::Authorization {
        failure.kind = match failure.http_status {
            Some(401) => FailureKind::Authorization,
            Some(403) => FailureKind::Forbidden,
            Some(500..=599) => FailureKind::Transient,
            _ => failure.kind,
        };
    }
    StreamableHttpError::Client(failure)
}

async fn observed<T>(
    status: Arc<AtomicU16>,
    request: impl Future<Output = Result<T, StreamableHttpError<reqwest::Error>>>,
) -> Result<T, Error> {
    let result = HTTP_STATUS.scope(status.clone(), request).await;
    let code = status.load(Ordering::SeqCst);
    result.map_err(|error| sanitized(error, (code != 0).then_some(code)))
}

#[derive(Clone)]
pub(crate) struct ScopedHttpClient {
    client: reqwest::Client,
    lifetime: tokio_util::sync::CancellationToken,
}

impl ScopedHttpClient {
    pub(crate) fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            lifetime: Default::default(),
        }
    }
    pub(crate) fn with_lifetime(mut self, lifetime: tokio_util::sync::CancellationToken) -> Self {
        self.lifetime = lifetime;
        self
    }

    async fn request<T>(
        &self,
        status: Arc<AtomicU16>,
        request: impl Future<Output = Result<T, StreamableHttpError<reqwest::Error>>>,
    ) -> Result<T, Error> {
        // rmcp can be inside its startup POST before its normal worker select
        // loop starts. Source retirement must cancel that I/O too, not only a
        // RunningService which may not exist yet. Ordinary call cancellation
        // never cancels this source token.
        tokio::select! {
            biased;
            _ = self.lifetime.cancelled() => Err(StreamableHttpError::Client(McpFailure::new(FailureKind::Cancelled, "source I/O retired"))),
            result = observed(status, request) => result,
        }
    }

    fn stream(&self, stream: BoxedSseResponse, guard: Option<IoGuard>) -> BoxedSseResponse {
        futures_util::stream::unfold(
            (stream, guard, self.lifetime.clone()),
            |(mut stream, guard, lifetime)| async move {
                tokio::select! {
                    biased;
                    _ = lifetime.cancelled() => None,
                    item = stream.next() => item.map(|item| (item, (stream, guard, lifetime))),
                }
            },
        )
        .boxed()
    }

    fn begin(message: &ClientJsonRpcMessage) -> Result<(Option<IoGuard>, Arc<AtomicU16>), Error> {
        let scope = match message {
            ClientJsonRpcMessage::Request(request) => {
                request.request.extensions().get::<Arc<CallIo>>()
            }
            _ => None,
        };
        let Some(scope) = scope else {
            return Ok((None, Arc::default()));
        };
        let guard = scope.begin().ok_or_else(|| {
            StreamableHttpError::Client(McpFailure::new(FailureKind::Cancelled, "submit"))
        })?;
        Ok((Some(guard), scope.http_status.clone()))
    }

    fn observe(
        &self,
        result: Result<StreamableHttpPostResponse, Error>,
        guard: Option<IoGuard>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        match result? {
            StreamableHttpPostResponse::Sse(stream, session) => Ok(
                StreamableHttpPostResponse::Sse(self.stream(stream, guard), session),
            ),
            StreamableHttpPostResponse::Accepted if guard.is_some() => {
                Err(StreamableHttpError::Client(McpFailure::new(
                    FailureKind::Protocol,
                    "empty tool acknowledgement",
                )))
            }
            response => Ok(response),
        }
    }
}

impl StreamableHttpClient for ScopedHttpClient {
    type Error = McpFailure;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let (guard, status) = Self::begin(&message)?;
        self.observe(
            self.request(
                status,
                self.client
                    .post_message(uri, message, session_id, auth_header, headers),
            )
            .await,
            guard,
        )
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        headers: Headers,
        max_size: usize,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let (guard, status) = Self::begin(&message)?;
        self.observe(
            self.request(
                status,
                self.client.post_message_with_max_sse_event_size(
                    uri,
                    message,
                    session_id,
                    auth_header,
                    headers,
                    max_size,
                ),
            )
            .await,
            guard,
        )
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        headers: Headers,
    ) -> Result<(), Error> {
        // DELETE is teardown, not a long-running tool call. An unresponsive
        // endpoint cannot keep the SDK's service task alive indefinitely.
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            observed(
                Arc::default(),
                self.client
                    .delete_session(uri, session_id, auth_header, headers),
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(StreamableHttpError::Client(McpFailure::new(
                FailureKind::Timeout,
                "delete session",
            )))
        })
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        headers: Headers,
    ) -> Result<BoxedSseResponse, Error> {
        self.request(
            Arc::default(),
            self.client
                .get_stream(uri, session_id, last_event_id, auth_header, headers),
        )
        .await
        .map(|stream| self.stream(stream, None))
    }
    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        headers: Headers,
        max_size: usize,
    ) -> Result<BoxedSseResponse, Error> {
        self.request(
            Arc::default(),
            self.client.get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                headers,
                max_size,
            ),
        )
        .await
        .map(|stream| self.stream(stream, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_facts_are_typed_and_never_render_remote_secrets() {
        for (status, kind) in [
            (401, FailureKind::Authorization),
            (403, FailureKind::Forbidden),
            (404, FailureKind::Protocol),
            (503, FailureKind::Transient),
        ] {
            let error = sanitized(
                StreamableHttpError::UnexpectedServerResponse(
                    "SECRET_BODY https://secret.invalid/?token=SECRET_URL".into(),
                ),
                Some(status),
            );
            let failure = McpFailure::from_error(&error, "initialize");
            assert_eq!(failure.kind, kind);
            assert_eq!(failure.http_status, Some(status));
            assert!(!format!("{error:?} {failure}").contains("SECRET"));
        }
        let expired = sanitized(StreamableHttpError::SessionExpired, Some(404));
        assert_eq!(
            McpFailure::from_error(&expired, "call").kind,
            FailureKind::SessionExpired
        );
        let auth = sanitized(
            StreamableHttpError::AuthRequired(
                rmcp::transport::streamable_http_client::AuthRequiredError::new(
                    "Bearer SECRET_CHALLENGE".into(),
                ),
            ),
            Some(401),
        );
        let facts = McpFailure::from_error(&auth, "call");
        assert!(facts.challenge.unwrap().contains("SECRET_CHALLENGE"));
        assert!(!format!("{auth:?}").contains("SECRET"));
    }
}
