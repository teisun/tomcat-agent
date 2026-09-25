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
pub(super) mod tests {
    use super::{run_code, truncate_code_result_text, MAX_CODE_RESULT_TEXT_BYTES};
    use crate::core::connector::mcp::manager::{McpCallContext, McpManager, ServerState};
    use crate::ext::PluginEngineConfig;
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::{sync::CancellationToken, task::TaskTracker};

    pub(in super::super) async fn vm_fixture(
        timeout_ms: u64,
    ) -> (
        tempfile::TempDir,
        AppConfig,
        Arc<McpManager>,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let events = temp.path().join("events.jsonl");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        cfg.connector.mcp.call_timeout_ms = timeout_ms;
        let config_path = get_work_dir(&cfg).unwrap().join("mcp.json");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        std::fs::write(
            config_path,
            serde_json::json!({"mcpServers":{"fake":{
                "command":"node", "args":[fixture,"--events",events]
            }}})
            .to_string(),
        )
        .unwrap();
        let manager = crate::core::connector::ConnectorRegistry::new(&cfg, None)
            .unwrap()
            .mcp_manager();
        manager.connect_server("fake").await.unwrap();
        (temp, cfg, manager, events)
    }

    async fn events(path: &std::path::Path) -> Vec<serde_json::Value> {
        tokio::fs::read_to_string(path)
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|row| serde_json::from_str(row).ok())
            .collect()
    }

    pub(in super::super) async fn seen(path: &std::path::Path, kind: &str, label: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !events(path)
                .await
                .iter()
                .any(|event| event["kind"] == kind && event["label"] == label)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fixture event must arrive");
    }

    fn vm_limits() -> PluginEngineConfig {
        PluginEngineConfig {
            call_timeout_ms: 20,
            interrupt_budget: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn code_vm_fanout_uses_independent_progress_windows_not_vm_wall_time() {
        let (temp, cfg, manager, log) = vm_fixture(300).await;
        let gate = temp.path().join("release");
        let a = serde_json::json!({"label":"A", "gateFile":gate, "delayMs":900, "progressMs":40});
        let b = serde_json::json!({"label":"B", "gateFile":gate, "delayMs":900, "progressMs":40});
        let code = format!("return await Promise.all([callTool('mcp__fake__capture', {a}), callTool('mcp__fake__capture', {b})]);");
        let vm = tokio::spawn(run_code(
            vm_limits(),
            manager.clone(),
            code,
            McpCallContext::default(),
        ));
        seen(&log, "started", "A").await;
        seen(&log, "started", "B").await;
        assert!(!vm.is_finished());
        tokio::fs::write(gate, "release").await.unwrap();
        let value = vm.await.unwrap().unwrap();
        assert_eq!(value[0]["content"][0]["text"], "fixture result: A");
        assert_eq!(value[1]["content"][0]["text"], "fixture result: B");
        assert_eq!(
            events(&log)
                .await
                .iter()
                .filter(|e| e["kind"] == "started")
                .count(),
            2
        );
        assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
        manager.remove_configured_server("fake", &cfg).unwrap();
    }

    #[tokio::test]
    async fn code_vm_polling_and_sibling_progress_cannot_renew_a_wrong_token_call() {
        let (_temp, cfg, manager, log) = vm_fixture(300).await;
        let code = r#"
const settled = await Promise.allSettled([
  callTool('mcp__fake__capture', {label:'A', delayMs:900, progressMs:40}),
  callTool('mcp__fake__status', {label:'B', delayMs:900, progressMs:40, wrongToken:true})
]);
return settled.map(r => r.status === 'fulfilled'
  ? {status:r.status, text:r.value.content[0].text}
  : {status:r.status, error:String(r.reason)});
"#
        .to_string();
        let vm = tokio::spawn(run_code(
            vm_limits(),
            manager.clone(),
            code,
            McpCallContext::default(),
        ));
        seen(&log, "exited", "B").await;
        assert!(!vm.is_finished());
        let value = vm.await.unwrap().unwrap();
        assert_eq!(value[0]["text"], "fixture result: A");
        assert_eq!(value[1]["status"], "rejected");
        assert!(value[1]["error"]
            .as_str()
            .unwrap()
            .contains("idle wait timed out"));
        assert_eq!(
            events(&log)
                .await
                .iter()
                .filter(|e| e["kind"] == "started")
                .count(),
            2
        );
        assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
        manager.remove_configured_server("fake", &cfg).unwrap();
    }

    #[tokio::test]
    async fn equal_host_ids_in_two_vms_remain_isolated_on_stop_and_drop() {
        let (temp, cfg, manager, log) = vm_fixture(300).await;
        for drop_caller in [false, true] {
            for index in 0..10 {
                let label_a = format!("A-{drop_caller}-{index}");
                let label_b = format!("B-{drop_caller}-{index}");
                let gate = temp.path().join(&label_b);
                let args_a = serde_json::json!({"label":label_a, "gateFile":temp.path().join(&label_a), "progressMs":40});
                let args_b = serde_json::json!({"label":label_b, "gateFile":gate, "progressMs":40});
                let cancel = CancellationToken::new();
                let owner = TaskTracker::new();
                let mut context_a = McpCallContext::new("session-A", "same-outer-id", &cancel);
                context_a.owner_tasks = Some(owner.clone());
                let a = tokio::spawn(run_code(vm_limits(), manager.clone(),
                    format!("Date.now = () => 0; return await callTool('mcp__fake__capture', {args_a});"), context_a));
                let b = tokio::spawn(run_code(vm_limits(), manager.clone(),
                    format!("Date.now = () => 0; return await callTool('mcp__fake__capture', {args_b});"),
                    McpCallContext::new("session-B", "same-outer-id", &CancellationToken::new())));
                seen(&log, "started", &label_a).await;
                seen(&log, "started", &label_b).await;
                owner.close();
                if drop_caller {
                    a.abort();
                    let _ = a.await;
                } else {
                    cancel.cancel();
                    assert!(a.await.unwrap().is_err());
                }
                tokio::time::timeout(Duration::from_secs(5), owner.wait())
                    .await
                    .unwrap();
                assert!(!b.is_finished(), "stopping A must not finish B");
                tokio::fs::write(gate, "release").await.unwrap();
                assert_eq!(
                    b.await.unwrap().unwrap()["content"][0]["text"],
                    format!("fixture result: {label_b}")
                );
                seen(&log, "exited", &label_a).await;
                assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
                assert_eq!(manager.statuses()[0].state, ServerState::Ready);
            }
        }
        let started: Vec<_> = events(&log)
            .await
            .into_iter()
            .filter(|e| e["kind"] == "started")
            .collect();
        assert_eq!(started.len(), 40);
        let ids: std::collections::HashSet<_> =
            started.iter().map(|e| e["id"].to_string()).collect();
        assert_eq!(ids.len(), 40);
        manager.remove_configured_server("fake", &cfg).unwrap();
    }

    #[tokio::test]
    async fn returning_throwing_and_busy_vm_drop_reap_unawaited_children() {
        let (temp, cfg, manager, log) = vm_fixture(300).await;
        for tail in [
            "return 7;",
            "throw new Error('test throw');",
            "while (true) {}",
        ] {
            for index in 0..10 {
                let label = format!("child-{tail}-{index}");
                let barrier = format!("barrier-{tail}-{index}");
                let gate = temp.path().join(&barrier);
                let child = serde_json::json!({"label":label,"gateFile":temp.path().join(&label),"progressMs":40});
                let second = serde_json::json!({"label":barrier,"gateFile":gate,"progressMs":40});
                let mut context = McpCallContext::default();
                let owner = TaskTracker::new();
                context.owner_tasks = Some(owner.clone());
                // Give the busy-loop case a long local budget: cancellation,
                // not that budget, must end an already running blocking worker.
                let limits = if tail.starts_with("while") {
                    PluginEngineConfig {
                        call_timeout_ms: 5000,
                        interrupt_budget: 0,
                        ..Default::default()
                    }
                } else {
                    vm_limits()
                };
                let vm = tokio::spawn(run_code(limits, manager.clone(),
                    format!("callTool('mcp__fake__capture', {child}); await callTool('mcp__fake__status', {second}); {tail}"), context));
                seen(&log, "started", &label).await;
                seen(&log, "started", &barrier).await;
                tokio::fs::write(gate, "release").await.unwrap();
                seen(&log, "exited", &barrier).await;
                owner.close();
                if tail.starts_with("while") {
                    tokio::time::sleep(Duration::from_millis(75)).await;
                    vm.abort();
                    let _ = vm.await;
                } else {
                    let result = vm.await.unwrap();
                    if tail.starts_with("return") {
                        assert_eq!(result.unwrap(), 7);
                    } else {
                        assert!(result.unwrap_err().to_string().contains("test throw"));
                    }
                }
                tokio::time::timeout(Duration::from_secs(5), owner.wait())
                    .await
                    .unwrap();
                seen(&log, "exited", &label).await;
                assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
                assert_eq!(manager.statuses()[0].state, ServerState::Ready);
            }
        }
        manager.remove_configured_server("fake", &cfg).unwrap();
    }

    #[test]
    fn truncation_preserves_utf8_and_explains_the_remedy() {
        let text = "图".repeat(MAX_CODE_RESULT_TEXT_BYTES);
        let truncated = truncate_code_result_text(&text);
        assert!(truncated.len() <= MAX_CODE_RESULT_TEXT_BYTES);
        assert!(truncated.contains("Output truncated"));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[tokio::test]
    async fn code_vm_calls_deferred_mcp_without_tool_registry() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let config_path = get_work_dir(&cfg).expect("work dir").join("mcp.json");
        std::fs::create_dir_all(config_path.parent().expect("config directory"))
            .expect("config directory");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        std::fs::write(
            config_path,
            serde_json::json!({
                "mcpServers": {
                    "fake": {
                        "command": "node",
                        "args": [fixture],
                    }
                }
            })
            .to_string(),
        )
        .expect("write MCP config");
        let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct manager");
        manager
            .connect_server("fake")
            .await
            .expect("connect fake MCP");

        let result = run_code(
            PluginEngineConfig::default(),
            manager.clone(),
            r#"return await callTool("mcp__fake__capture", {});"#.to_string(),
            McpCallContext::default(),
        )
        .await
        .expect("call deferred MCP through the code VM");
        assert_eq!(manager.call_debug_counts("fake"), Some((0, 16, 32)));
        assert_eq!(result["content"][0]["text"], "fake capture complete");
    }

    #[tokio::test]
    async fn code_vm_cannot_bypass_untrusted_project_connector() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        let config_path = workspace.join(".agents/mcp.json");
        std::fs::create_dir_all(config_path.parent().expect("project config directory"))
            .expect("project config directory");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp/fake_stdio_server.mjs");
        std::fs::write(
            config_path,
            serde_json::json!({
                "mcpServers": {
                    "project-fake": {
                        "command": "node",
                        "args": [fixture],
                    }
                }
            })
            .to_string(),
        )
        .expect("write untrusted project MCP config");

        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let manager = McpManager::new(&cfg, Some(&workspace)).expect("construct manager");
        manager
            .connect_server("project-fake")
            .await
            .expect("untrusted source records a confirmation requirement");
        assert!(matches!(
            manager.statuses().pop().expect("project status").state,
            ServerState::AwaitingProjectTrust
        ));

        let error = run_code(
            PluginEngineConfig::default(),
            manager,
            r#"return await callTool("mcp__project-fake__capture", {});"#.to_string(),
            McpCallContext::default(),
        )
        .await
        .expect_err("agent code must not bypass project connector confirmation");
        assert!(
            error
                .to_string()
                .contains("unknown or not-ready deferred tool"),
            "unapproved connector must remain unavailable to agent code: {error}"
        );
    }
}
