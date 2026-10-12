//! Agent-authored JavaScript for deferred-tool fan-out and local aggregation.
//!
//! This deliberately reuses `PluginVmInstance`: one QuickJS implementation, one
//! heap/timeout/interrupt policy, no subprocess and no connector IPC daemon.

use std::sync::Arc;

use super::super::{media, ToolExecCtx, ToolExecOutcome};
use crate::core::connector::mcp::manager::{McpCallContext, McpManager};
use crate::core::connector::mcp::CLEANUP_GRACE;
use crate::ext::{HostRequest, HostResponse, PluginVmInstance};
use crate::infra::error::AppError;
use dashmap::DashMap;
use tokio_util::task::TaskTracker;

/// Align with the existing background-bash preview ceiling. This applies only after image
/// blocks have been extracted into `InputImage`; it is never applied to base64 image data.
const MAX_CODE_RESULT_TEXT_BYTES: usize = 64 * 1024;
const TRUNCATION_SUFFIX_RESERVE_BYTES: usize = 192;

#[derive(Clone)]
enum PendingCodeCall {
    Pending,
    Done(HostResponse),
}

struct CodeCallScope {
    context: McpCallContext,
    calls: DashMap<String, PendingCodeCall>,
    tasks: TaskTracker,
}

impl CodeCallScope {
    fn stop(&self) {
        self.context.cancel.cancel();
        self.tasks.close();
    }
}

pub(in crate::core::agent_loop) async fn handle_tool_run_code(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> ToolExecOutcome {
    let code = match args.get("code").and_then(serde_json::Value::as_str) {
        Some(code) if !code.trim().is_empty() => code.to_string(),
        _ => return ToolExecOutcome::err("tool_run_code requires a non-empty string 'code'"),
    };
    let manager = match ctx.connector_registry {
        Some(connectors) => connectors.mcp_manager(),
        None => {
            return ToolExecOutcome::err(
                "tool_run_code is unavailable: no enabled MCP connector is configured",
            )
        }
    };
    let vm_config = match ctx.plugin_engine_config {
        Some(config) => config.clone(),
        None => {
            return ToolExecOutcome::err(
                "tool_run_code is unavailable: plugin JavaScript runtime is not configured",
            )
        }
    };
    let context = McpCallContext::new(ctx.session_id, ctx.tool_call_id, ctx.cancel);
    match run_code(vm_config, manager, code, context).await {
        Ok(result) => {
            let mut outcome =
                media::extract_mcp_tool_result_media(&result, ctx.openai_files_runtime).await;
            outcome.model_text = truncate_code_result_text(&outcome.model_text);
            outcome
        }
        Err(error) => ToolExecOutcome::err(error.to_string()),
    }
}

async fn run_code(
    config: crate::ext::PluginEngineConfig,
    manager: Arc<McpManager>,
    code: String,
    context: McpCallContext,
) -> Result<serde_json::Value, AppError> {
    let scope = Arc::new(CodeCallScope {
        context,
        calls: DashMap::new(),
        tasks: TaskTracker::new(),
    });
    // The dispatcher may drop this entire future. This signal reaches both the
    // already-running blocking VM and only this VM's MCP children.
    let _cancel_on_drop = scope.context.cancel.clone().drop_guard();
    let runtime = tokio::runtime::Handle::current();
    let owner_task = scope.context.owner_tasks.as_ref().map(TaskTracker::token);
    let worker_scope = scope.clone();
    let worker = tokio::task::spawn_blocking(move || {
        run_code_in_plugin_vm(config, manager, runtime, code, worker_scope)
    });
    let (reply, result) = tokio::sync::oneshot::channel();
    // Keep ownership of the blocking JoinHandle even when the caller vanishes.
    // No abort assumption: the VM checks its cancellation in JS and while idle.
    tokio::spawn(async move {
        let _owner_task = owner_task;
        let value = worker
            .await
            .map_err(|error| AppError::QuickJS(format!("agent code VM worker panicked: {error}")))
            .and_then(|value| value);
        scope.stop();
        if tokio::time::timeout(CLEANUP_GRACE, scope.tasks.wait())
            .await
            .is_err()
        {
            tracing::error!(session_id = %scope.context.session_id, tool_call_id = %scope.context.tool_call_id,
                "agent code child cleanup exceeded grace");
            let _ = reply.send(Err(AppError::QuickJS(
                "agent code child cleanup is not confirmed".into(),
            )));
            // Do not release ownership or claim cleanup completed on a timeout.
            scope.tasks.wait().await;
        } else {
            scope.calls.clear();
            let _ = reply.send(value);
        }
    });
    result
        .await
        .map_err(|_| AppError::QuickJS("agent code owner exited without a result".into()))?
}

fn run_code_in_plugin_vm(
    config: crate::ext::PluginEngineConfig,
    manager: Arc<McpManager>,
    runtime: tokio::runtime::Handle,
    code: String,
    scope: Arc<CodeCallScope>,
) -> Result<serde_json::Value, AppError> {
    if scope.context.cancel.is_cancelled() {
        return Err(AppError::QuickJS(
            "agent code execution was cancelled".into(),
        ));
    }
    let mut vm = PluginVmInstance::new(config, "__agent_code__".to_string())?;
    let host_scope = scope.clone();
    vm.register_host_binding(move |request_json| {
        if host_scope.context.cancel.is_cancelled() {
            return serde_json::to_string(&HostResponse::err("agent code execution is closed"))
                .map_err(AppError::Serialize);
        }
        let request: HostRequest = serde_json::from_str(request_json).map_err(|error| {
            AppError::QuickJS(format!("agent code hostcall parse failed: {error}"))
        })?;
        let response = match (request.module.as_str(), request.method.as_str()) {
            ("connector", "callTool") => {
                submit_connector_call(&host_scope, &runtime, &manager, request)
            }
            ("__async", "poll") => poll_connector_call(&host_scope.calls, &request),
            _ => HostResponse::err(format!(
                "agent code may only call connector.callTool, got {}.{}",
                request.module, request.method
            )),
        };
        serde_json::to_string(&response).map_err(AppError::Serialize)
    })?;
    let cancel = scope.context.cancel.clone();
    vm.run_agent_code_with_cancel(&code, cancel, move || !scope.calls.is_empty())
}

fn submit_connector_call(
    scope: &Arc<CodeCallScope>,
    runtime: &tokio::runtime::Handle,
    manager: &Arc<McpManager>,
    request: HostRequest,
) -> HostResponse {
    let Some(call_id) = request.call_id else {
        return HostResponse::err("agent code connector.callTool requires an async call id");
    };
    let name = request
        .params
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned);
    let arguments = request
        .params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let (Some(name), true) = (name, arguments.is_object()) else {
        return HostResponse::err(if arguments.is_object() {
            "callTool requires a non-empty string name"
        } else {
            "callTool arguments must be a JSON object"
        });
    };
    match scope.calls.entry(call_id.clone()) {
        dashmap::mapref::entry::Entry::Occupied(_) => {
            return HostResponse::err(format!("duplicate agent code hostcall id: {call_id}"));
        }
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            entry.insert(PendingCodeCall::Pending);
        }
    }
    let mut context = McpCallContext::new(
        scope.context.session_id.clone(),
        format!("{}:{call_id}", scope.context.tool_call_id),
        &scope.context.cancel,
    );
    context.owner_tasks = Some(scope.tasks.clone());
    let manager = manager.clone();
    let response_call_id = call_id.clone();
    let task_scope = scope.clone();
    scope.tasks.spawn_on(
        async move {
            let response = match manager
                .call_model_tool_with_context(&name, arguments, context)
                .await
            {
                Ok(result) => HostResponse::ok(result),
                Err(error) => HostResponse::err(error.to_string()),
            };
            // Never recreate a removed result cell or publish into a closed VM.
            if !task_scope.context.cancel.is_cancelled() {
                if let Some(mut entry) = task_scope.calls.get_mut(&call_id) {
                    *entry = PendingCodeCall::Done(response);
                }
            }
        },
        runtime,
    );
    HostResponse {
        ok: true,
        data: Some(serde_json::json!({ "pending": true })),
        error: None,
        call_id: Some(response_call_id),
    }
}

fn poll_connector_call(
    calls: &DashMap<String, PendingCodeCall>,
    request: &HostRequest,
) -> HostResponse {
    let Some(call_id) = request
        .params
        .get("callId")
        .and_then(serde_json::Value::as_str)
    else {
        return HostResponse::err("agent code async poll requires callId");
    };
    let Some(entry) = calls.get(call_id) else {
        return HostResponse::err(format!("unknown agent code hostcall id: {call_id}"));
    };
    let state = entry.value().clone();
    drop(entry);
    match state {
        PendingCodeCall::Pending => HostResponse::ok(serde_json::json!({ "ready": false })),
        PendingCodeCall::Done(response) => {
            calls.remove(call_id);
            HostResponse::ok(serde_json::json!({ "ready": true, "response": response }))
        }
    }
}

fn truncate_code_result_text(text: &str) -> String {
    if text.len() <= MAX_CODE_RESULT_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_CODE_RESULT_TEXT_BYTES.saturating_sub(TRUNCATION_SUFFIX_RESERVE_BYTES);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let omitted_bytes = text.len() - end;
    let suffix = format!(
        "\n[Output truncated; omitted {omitted_bytes} bytes. Filter or aggregate the result in code and return a smaller final value.]"
    );
    format!("{}{}", &text[..end], suffix)
}

#[cfg(test)]
#[path = "tests/code_test.rs"]
pub(super) mod tests;
