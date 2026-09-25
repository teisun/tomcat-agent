//! Client-side HTTP/SSE ownership probe. Server socket counts alone cannot
//! demonstrate that an SDK stopped reading and dropped its response body.
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

type Headers = HashMap<HeaderName, HeaderValue>;
type Error = StreamableHttpError<reqwest::Error>;

#[derive(Clone)]
pub struct ProbeClient {
    client: reqwest::Client,
    active: Arc<AtomicUsize>,
}

struct IoGuard(Arc<AtomicUsize>);
impl Drop for IoGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ProbeClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
    fn begin(&self, message: &ClientJsonRpcMessage) -> Option<IoGuard> {
        if serde_json::to_value(message).expect("message")["method"] != "tools/call" {
            return None;
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        Some(IoGuard(self.active.clone()))
    }
    fn observe(
        result: Result<StreamableHttpPostResponse, Error>,
        guard: Option<IoGuard>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        match result? {
            StreamableHttpPostResponse::Sse(stream, session) => {
                let stream = futures_util::stream::unfold(
                    (stream, guard),
                    |(mut stream, guard)| async move {
                        stream.next().await.map(|item| (item, (stream, guard)))
                    },
                )
                .boxed();
                Ok(StreamableHttpPostResponse::Sse(stream, session))
            }
            response => Ok(response),
        }
    }
}

impl StreamableHttpClient for ProbeClient {
    type Error = reqwest::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let guard = self.begin(&message);
        Self::observe(
            self.client
                .post_message(uri, message, session_id, auth_header, headers)
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
        let guard = self.begin(&message);
        Self::observe(
            self.client
                .post_message_with_max_sse_event_size(
                    uri,
                    message,
                    session_id,
                    auth_header,
                    headers,
                    max_size,
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
        self.client
            .delete_session(uri, session_id, auth_header, headers)
            .await
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        headers: Headers,
    ) -> Result<BoxedSseResponse, Error> {
        self.client
            .get_stream(uri, session_id, last_event_id, auth_header, headers)
            .await
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
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                headers,
                max_size,
            )
            .await
    }
}
