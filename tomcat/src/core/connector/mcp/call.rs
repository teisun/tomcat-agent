//! Per-call ownership. SDK request IDs/results/progress remain SDK-owned.
use std::sync::{
    atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;

use super::failure::{FailureKind, McpFailure};
use rmcp::model::{CancelledNotificationParam, ClientRequest, GetExtensions, ServerResult};
use rmcp::service::PeerRequestOptions;
use rmcp::{Peer, RoleClient, ServiceError};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub(crate) const CLEANUP_GRACE: Duration = Duration::from_secs(5);

/// Local metadata only; never serialized into an MCP request.
#[derive(Clone)]
pub struct McpCallContext {
    pub session_id: Arc<str>,
    pub tool_call_id: Arc<str>,
    pub cancel: CancellationToken,
    pub(crate) owner_tasks: Option<TaskTracker>,
}

impl McpCallContext {
    pub fn new(
        session_id: impl Into<Arc<str>>,
        tool_call_id: impl Into<Arc<str>>,
        parent: &CancellationToken,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            tool_call_id: tool_call_id.into(),
            cancel: parent.child_token(),
            owner_tasks: None,
        }
    }
}

impl Default for McpCallContext {
    fn default() -> Self {
        Self::new("", "", &CancellationToken::new())
    }
}

/// An SDK-local extension carries the cleanup receipt through transport into
/// the actual reqwest response stream. No second request-ID routing table.
#[derive(Default)]
pub(crate) struct CallIo {
    phase: AtomicU8, // 0 not started; 1 owns HTTP I/O; 2 released/sealed
    pub rejected: AtomicBool,
    pub possibly_sent: AtomicBool,
    pub http_status: Arc<AtomicU16>,
    done: CancellationToken,
}

pub(crate) struct IoGuard(Arc<CallIo>);
impl Drop for IoGuard {
    fn drop(&mut self) {
        self.0.phase.store(2, Ordering::SeqCst);
        self.0.done.cancel();
    }
}

impl CallIo {
    pub fn begin(self: &Arc<Self>) -> Option<IoGuard> {
        self.phase
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        self.possibly_sent.store(true, Ordering::SeqCst);
        Some(IoGuard(self.clone()))
    }
    pub fn reject_new_sends(&self) {
        self.rejected.store(true, Ordering::SeqCst);
    }
    pub async fn wait_released(&self) {
        // A cancellation can win before the HTTP worker gets to this request.
        // Seal that state; a late worker cannot start I/O after its owner exits.
        if self
            .phase
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.done.cancel();
        }
        self.done.cancelled().await;
    }
}

#[derive(Debug)]
pub(crate) enum CallFailure {
    Busy,
    QueueTimeout,
    SubmitTimeout,
    Cancelled { possibly_sent: bool },
    Retired { possibly_sent: bool },
    Sdk(ServiceError),
    Http(McpFailure),
}

impl CallFailure {
    pub fn facts(&self) -> Option<McpFailure> {
        match self {
            Self::Http(failure) => Some(failure.clone()),
            Self::Sdk(error) => Some(McpFailure::from_service(error, "tools/call")),
            _ => None,
        }
    }

    fn sdk(error: ServiceError, io: &CallIo) -> Self {
        let status = io.http_status.load(Ordering::SeqCst);
        if status >= 400 {
            let mut failure = McpFailure::from_service(&error, "tools/call");
            failure.http_status = Some(status);
            if failure.kind != FailureKind::SessionExpired
                && failure.kind != FailureKind::Authorization
            {
                failure.kind = match status {
                    401 => FailureKind::Authorization,
                    403 => FailureKind::Forbidden,
                    _ => failure.kind,
                };
            }
            Self::Http(failure)
        } else {
            Self::Sdk(error)
        }
    }

    pub fn session_expired(&self) -> bool {
        self.facts()
            .is_some_and(|failure| failure.kind == FailureKind::SessionExpired)
    }

    pub fn to_app_error(&self, tool: &str) -> crate::infra::error::AppError {
        let detail = match self {
            Self::Busy => "source busy; request not sent".into(),
            Self::QueueTimeout => "admission queue timed out; request not sent".into(),
            Self::SubmitTimeout => "SDK submission timed out; delivery not confirmed".into(),
            Self::Http(failure) => format!("{failure}; execution result unknown (not replayed)"),
            Self::Cancelled { possibly_sent } | Self::Retired { possibly_sent } => {
                let action = if matches!(self, Self::Retired { .. }) {
                    "connection retired"
                } else {
                    "cancelled"
                };
                let outcome = if *possibly_sent {
                    "execution result unknown"
                } else {
                    "request not sent"
                };
                format!("{action}; {outcome}")
            }
            Self::Sdk(ServiceError::Timeout { .. }) => {
                "idle wait timed out; execution result unknown".into()
            }
            Self::Sdk(ServiceError::McpError(error)) => {
                format!("server returned JSON-RPC error code {}", error.code.0)
            }
            Self::Sdk(ServiceError::TransportClosed) => {
                "response ended before a result; execution result unknown".into()
            }
            Self::Sdk(_) if self.session_expired() => {
                "MCP session expired; execution result unknown (not replayed)".into()
            }
            Self::Sdk(_) => "request failed; execution result unknown".into(),
        };
        // Do not interpolate an SDK error chain, URL, headers or response body.
        crate::infra::error::AppError::Tool(format!("MCP tool '{tool}': {detail}"))
    }
}

pub(crate) struct CallRuntime {
    pub slots: Arc<Semaphore>,
    pub admitted: Arc<Semaphore>,
    pub retired: CancellationToken,
    pub tasks: TaskTracker,
    pub timeout: Duration,
}

impl CallRuntime {
    pub fn new(limit: usize, timeout: Duration) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(limit)),
            admitted: Arc::new(Semaphore::new(2 * limit)),
            retired: CancellationToken::new(),
            tasks: TaskTracker::new(),
            timeout,
        }
    }
    pub fn retire(&self) {
        self.retired.cancel();
        self.slots.close();
        self.admitted.close();
        self.tasks.close();
    }

    pub fn spawn(
        &self,
        peer: Peer<RoleClient>,
        mut request: ClientRequest,
        context: McpCallContext,
        deadline: Instant,
        slot: OwnedSemaphorePermit,
        admission: OwnedSemaphorePermit,
    ) -> oneshot::Receiver<Result<ServerResult, CallFailure>> {
        let (result_tx, result_rx) = oneshot::channel();
        let retired = self.retired.clone();
        let timeout = self.timeout;
        let io = Arc::new(CallIo::default());
        request.extensions_mut().insert(io.clone());
        // TaskTracker owns completion tracking, without a task -> owner -> task
        // reference cycle. Dropping the caller only signals its child token.
        let owner_task = context.owner_tasks.as_ref().map(TaskTracker::token);
        self.tasks.spawn(async move {
            let _owner_task = owner_task;
            let mut result_tx = Some(result_tx);
            let result = run_request(
                peer,
                request,
                &context,
                retired,
                CallTiming {
                    admission_deadline: deadline,
                    idle_timeout: timeout,
                },
                io.clone(),
                &mut result_tx,
            )
            .await;
            io.reject_new_sends();
            io.wait_released().await;
            drop(slot);
            drop(admission);
            if let Some(tx) = result_tx {
                let _ = tx.send(result);
            }
        });
        result_rx
    }
}

impl Drop for CallRuntime {
    fn drop(&mut self) {
        self.retire();
    }
}

fn report(tx: &mut Option<oneshot::Sender<Result<ServerResult, CallFailure>>>, error: CallFailure) {
    if let Some(tx) = tx.take() {
        let _ = tx.send(Err(error));
    }
}

struct CallTiming {
    admission_deadline: Instant,
    idle_timeout: Duration,
}

async fn run_request(
    peer: Peer<RoleClient>,
    request: ClientRequest,
    context: &McpCallContext,
    retired: CancellationToken,
    timing: CallTiming,
    io: Arc<CallIo>,
    result_tx: &mut Option<oneshot::Sender<Result<ServerResult, CallFailure>>>,
) -> Result<ServerResult, CallFailure> {
    let options = PeerRequestOptions::with_timeout(timing.idle_timeout).reset_timeout_on_progress();
    debug_assert!(options.max_total_timeout.is_none());
    let submit = peer.send_request_with_option(request, options);
    tokio::pin!(submit);
    let handle = tokio::select! {
        biased;
        _ = context.cancel.cancelled() => {
            io.reject_new_sends();
            report(result_tx, CallFailure::Cancelled { possibly_sent: false });
            submit.await
        }
        _ = retired.cancelled() => {
            io.reject_new_sends();
            report(result_tx, CallFailure::Retired { possibly_sent: false });
            submit.await
        }
        _ = tokio::time::sleep_until(timing.admission_deadline) => {
            io.reject_new_sends();
            report(result_tx, CallFailure::SubmitTimeout);
            submit.await
        }
        result = &mut submit => result,
    }
    .map_err(|error| CallFailure::sdk(error, &io))?;
    let id = handle.id.clone();
    let response = handle.await_response();
    tokio::pin!(response);
    let interruption = tokio::select! {
        biased;
        _ = context.cancel.cancelled() => Some(false),
        _ = retired.cancelled() => Some(true),
        result = &mut response, if result_tx.is_some() => return result.map_err(|error| CallFailure::sdk(error, &io)),
        // Submission's deadline/cancellation won. Do not begin a fresh call.
        _ = std::future::ready(()), if result_tx.is_none() => Some(false),
    };
    if let Some(retired) = interruption {
        io.reject_new_sends();
        let possibly_sent = io.possibly_sent.load(Ordering::SeqCst);
        report(
            result_tx,
            if retired {
                CallFailure::Retired { possibly_sent }
            } else {
                CallFailure::Cancelled { possibly_sent }
            },
        );
        let cancellation = peer.notify_cancelled(CancelledNotificationParam::new(
            Some(id),
            Some("caller stopped".into()),
        ));
        // Keep polling await_response to let rmcp remove its private progress
        // watcher. Do not abort/drop it in the dispatcher's cancellation branch.
        let cleanup = async {
            let (_, result) = tokio::join!(cancellation, &mut response);
            result
        };
        tokio::pin!(cleanup);
        tokio::select! {
            result = &mut cleanup => return result.map_err(|error| CallFailure::sdk(error, &io)),
            _ = tokio::time::sleep(CLEANUP_GRACE) => {
                // Never pretend cleanup succeeded or release a held slot while
                // work remains. This indicates an invariant failure, not proof
                // that a healthy shared connection should be killed.
                tracing::error!(session_id = %context.session_id, tool_call_id = %context.tool_call_id,
                    phase = "cleanup", "MCP request cleanup exceeded grace");
            }
        }
        return cleanup.await.map_err(|error| CallFailure::sdk(error, &io));
    }
    unreachable!("response or interruption selected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn io_receipt_waits_for_actual_drop_and_seals_unstarted_requests() {
        let active = Arc::new(CallIo::default());
        let guard = active.begin().unwrap();
        let released = {
            let active = active.clone();
            tokio::spawn(async move { active.wait_released().await })
        };
        tokio::task::yield_now().await;
        assert!(!released.is_finished());
        drop(guard);
        released.await.unwrap();
        assert!(active.begin().is_none());
        let unstarted = Arc::new(CallIo::default());
        unstarted.wait_released().await;
        assert!(unstarted.begin().is_none());
    }

    #[test]
    fn rpc_errors_keep_the_code_without_disclosing_remote_message_or_data() {
        let failure = CallFailure::Sdk(ServiceError::McpError(rmcp::ErrorData::invalid_params(
            "SENTINEL_PRIVATE_ERROR",
            Some(serde_json::json!({"token":"SENTINEL_PRIVATE_DATA"})),
        )));
        assert!(!failure.session_expired());
        let message = failure.to_app_error("mcp__fixture__echo").to_string();
        assert!(message.contains("-32602"));
        assert!(!message.contains("SENTINEL"));
    }
}
