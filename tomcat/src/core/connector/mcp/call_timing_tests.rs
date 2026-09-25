//! Exact virtual-clock checks for Tomcat's production per-call runner. HTTP
//! and stdio integration cases separately exercise the same runner over I/O.
use super::call::{CallFailure, CallRuntime, McpCallContext};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest,
    ProgressNotificationParam, ProgressToken, Request, ServerCapabilities, ServerConfig,
    ServerResult,
};
use rmcp::{ClientHandler, Peer, RoleClient, RoleServer, ServiceExt};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{advance, Instant};
use tokio_util::sync::CancellationToken;

struct Started {
    peer: Peer<RoleServer>,
    token: ProgressToken,
    finish: CancellationToken,
}
struct TimingServer {
    started: mpsc::UnboundedSender<Started>,
}
impl rmcp::handler::server::ServerHandler for TimingServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let token = context
            .meta
            .get_progress_token()
            .ok_or_else(|| rmcp::ErrorData::invalid_params("missing token", None))?;
        let finish = CancellationToken::new();
        self.started
            .send(Started {
                peer: context.peer,
                token,
                finish: finish.clone(),
            })
            .unwrap();
        finish.cancelled().await;
        Ok(CallToolResult::success(Vec::new()).into())
    }
}
struct Observer(mpsc::UnboundedSender<()>);
impl ClientHandler for Observer {
    async fn on_progress(
        &self,
        _: ProgressNotificationParam,
        _: rmcp::service::NotificationContext<RoleClient>,
    ) {
        let _ = self.0.send(());
    }
}
struct Pair {
    client: rmcp::service::RunningService<RoleClient, Observer>,
    server: tokio::task::JoinHandle<()>,
    stop: CancellationToken,
    started: mpsc::UnboundedReceiver<Started>,
    progress: mpsc::UnboundedReceiver<()>,
}
impl Pair {
    async fn new() -> Self {
        let (started, starts) = mpsc::unbounded_channel();
        let (observed, progress) = mpsc::unbounded_channel();
        let (server_io, client_io) = tokio::io::duplex(4096);
        let stop = CancellationToken::new();
        let ct = stop.clone();
        let server = tokio::spawn(async move {
            let service = TimingServer { started }
                .serve_with_ct(server_io, ct)
                .await
                .unwrap();
            service.waiting().await.unwrap();
        });
        let client = Observer(observed).serve(client_io).await.unwrap();
        Self {
            client,
            server,
            stop,
            started: starts,
            progress,
        }
    }
    async fn start(
        &mut self,
        runtime: &CallRuntime,
        session: &str,
    ) -> (
        oneshot::Receiver<Result<ServerResult, CallFailure>>,
        Started,
    ) {
        let result = runtime.spawn(
            self.client.peer().clone(),
            ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new("hold"))),
            McpCallContext::new(session, "same-outer-call-id", &CancellationToken::new()),
            Instant::now() + runtime.timeout,
            runtime.slots.clone().try_acquire_owned().unwrap(),
            runtime.admitted.clone().try_acquire_owned().unwrap(),
        );
        (result, self.started.recv().await.unwrap())
    }
    async fn progress(&mut self, request: &Started) {
        request
            .peer
            .notify_progress(
                ProgressNotificationParam::new(request.token.clone(), 100.0).with_total(100.0),
            )
            .await
            .unwrap();
        self.progress.recv().await.unwrap(); // Internal SDK routing precedes the handler.
    }
    async fn stop(mut self) {
        self.stop.cancel();
        self.client.close().await.unwrap();
        self.server.await.unwrap();
    }
}
fn empty<T>(receiver: &mut oneshot::Receiver<T>) {
    assert!(matches!(
        receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
}
fn timed_out(result: Result<ServerResult, CallFailure>) {
    assert!(
        matches!(
            result,
            Err(CallFailure::Sdk(rmcp::ServiceError::Timeout { .. }))
        ),
        "{result:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn default_120_progress_at_110_expires_at_230_and_not_for_a_sibling_or_old_token() {
    let settings = crate::infra::config::McpRuntimeConfig::default();
    assert_eq!(settings.call_timeout_ms, 120_000);
    let runtime = CallRuntime::new(
        settings.max_concurrent_calls,
        Duration::from_millis(settings.call_timeout_ms),
    );
    let mut pair = Pair::new().await;
    let start = Instant::now();
    let (mut a, request_a) = pair.start(&runtime, "A").await;
    let (b, request_b) = pair.start(&runtime, "B").await;
    assert_ne!(request_a.token, request_b.token);
    advance(Duration::from_secs(110)).await;
    pair.progress(&request_a).await;
    advance(Duration::from_secs(10)).await;
    timed_out(b.await.unwrap());
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    empty(&mut a);
    advance(Duration::from_secs(109)).await;
    empty(&mut a);
    advance(Duration::from_secs(1)).await;
    timed_out(a.await.unwrap()); // 100% progress is not a final result.
    assert_eq!(start.elapsed(), Duration::from_secs(230));
    let (c, request_c) = pair.start(&runtime, "C").await;
    advance(Duration::from_secs(110)).await;
    pair.progress(&request_a).await; // Late token from the completed wait.
    advance(Duration::from_secs(10)).await;
    timed_out(c.await.unwrap());
    assert_eq!(start.elapsed(), Duration::from_secs(350));
    assert_eq!(runtime.slots.available_permits(), 16);
    request_a.finish.cancel();
    request_b.finish.cancel();
    request_c.finish.cancel();
    runtime.retire();
    runtime.tasks.wait().await;
    pair.stop().await;
}

#[tokio::test(start_paused = true)]
async fn progress_has_no_total_cap_and_explicit_idle_window_is_preserved() {
    for seconds in [120, 7] {
        let runtime = CallRuntime::new(16, Duration::from_secs(seconds));
        let mut pair = Pair::new().await;
        let (mut result, request) = pair.start(&runtime, "A").await;
        let started = Instant::now();
        for _ in 0..4 {
            advance(Duration::from_secs(seconds - 1)).await;
            pair.progress(&request).await;
            empty(&mut result);
        }
        assert!(started.elapsed() > runtime.timeout * 3);
        request.finish.cancel();
        result.await.unwrap().unwrap();
        runtime.retire();
        runtime.tasks.wait().await;
        pair.stop().await;
    }
}
