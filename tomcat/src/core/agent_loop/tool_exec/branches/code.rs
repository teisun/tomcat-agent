//! Agent-authored JavaScript for deferred-tool fan-out and local aggregation.
//!
//! This deliberately reuses `PluginVmInstance`: one QuickJS implementation, one
//! heap/timeout/interrupt policy, no subprocess and no connector IPC daemon.

use std::sync::Arc;

use super::super::{media, ToolExecCtx, ToolExecOutcome};
use crate::core::connector::mcp::manager::McpManager;
use crate::ext::{HostRequest, HostResponse, PluginVmInstance};
use crate::infra::error::AppError;
use dashmap::DashMap;

/// Align with the existing background-bash preview ceiling. This applies only after image
/// blocks have been extracted into `InputImage`; it is never applied to base64 image data.
const MAX_CODE_RESULT_TEXT_BYTES: usize = 64 * 1024;
const TRUNCATION_SUFFIX_RESERVE_BYTES: usize = 192;

#[derive(Clone)]
enum PendingCodeCall {
    Pending,
    Done(HostResponse),
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
    let runtime = tokio::runtime::Handle::current();

    let result = tokio::task::spawn_blocking(move || {
        run_code_in_plugin_vm(vm_config, manager, runtime, code)
    })
    .await
    .map_err(|error| AppError::QuickJS(format!("agent code VM worker panicked: {error}")));
    match result {
        Ok(Ok(result)) => {
            let mut outcome =
                media::extract_mcp_tool_result_media(&result, ctx.openai_files_runtime).await;
            outcome.model_text = truncate_code_result_text(&outcome.model_text);
            outcome
        }
        Ok(Err(error)) | Err(error) => ToolExecOutcome::err(error.to_string()),
    }
}

fn run_code_in_plugin_vm(
    config: crate::ext::PluginEngineConfig,
    manager: Arc<McpManager>,
    runtime: tokio::runtime::Handle,
    code: String,
) -> Result<serde_json::Value, AppError> {
    let mut vm = PluginVmInstance::new(config, "__agent_code__".to_string())?;
    let calls = Arc::new(DashMap::<String, PendingCodeCall>::new());
    vm.register_host_binding(move |request_json| {
        let request: HostRequest = serde_json::from_str(request_json).map_err(|error| {
            AppError::QuickJS(format!("agent code hostcall parse failed: {error}"))
        })?;
        let response = match (request.module.as_str(), request.method.as_str()) {
            ("connector", "callTool") => submit_connector_call(&calls, &runtime, &manager, request),
            ("__async", "poll") => poll_connector_call(&calls, &request),
            _ => HostResponse::err(format!(
                "agent code may only call connector.callTool, got {}.{}",
                request.module, request.method
            )),
        };
        serde_json::to_string(&response).map_err(AppError::Serialize)
    })?;
    vm.run_agent_code(&code)
}

fn submit_connector_call(
    calls: &Arc<DashMap<String, PendingCodeCall>>,
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
    if calls
        .insert(call_id.clone(), PendingCodeCall::Pending)
        .is_some()
    {
        return HostResponse::err(format!("duplicate agent code hostcall id: {call_id}"));
    }
    let calls = calls.clone();
    let manager = manager.clone();
    let response_call_id = call_id.clone();
    runtime.spawn(async move {
        let response = match manager.call_model_tool(&name, arguments).await {
            Ok(result) => HostResponse::ok(result),
            Err(error) => HostResponse::err(error.to_string()),
        };
        calls.insert(call_id, PendingCodeCall::Done(response));
    });
    HostResponse {
        ok: true,
        data: Some(serde_json::json!({ "pending": true })),
        error: None,
        call_id: Some(response_call_id),
    }
}

fn poll_connector_call(
    calls: &Arc<DashMap<String, PendingCodeCall>>,
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
mod tests {
    use super::{run_code_in_plugin_vm, truncate_code_result_text, MAX_CODE_RESULT_TEXT_BYTES};
    use crate::core::connector::mcp::manager::{McpManager, ServerState};
    use crate::ext::PluginEngineConfig;
    use crate::infra::config::get_work_dir;
    use crate::AppConfig;

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
        let manager = McpManager::new(&cfg, &workspace).expect("construct manager");
        manager
            .connect_server("fake")
            .await
            .expect("connect fake MCP");

        let runtime = tokio::runtime::Handle::current();
        let result = tokio::task::spawn_blocking(move || {
            run_code_in_plugin_vm(
                PluginEngineConfig::default(),
                manager,
                runtime,
                r#"
const response = await callTool("mcp__fake__capture", {});
return response;
"#
                .to_string(),
            )
        })
        .await
        .expect("code VM worker should not panic")
        .expect("call deferred MCP through the code VM");
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
        let manager = McpManager::new(&cfg, &workspace).expect("construct manager");
        manager
            .connect_server("project-fake")
            .await
            .expect("untrusted source records a confirmation requirement");
        assert!(matches!(
            manager.statuses().pop().expect("project status").state,
            ServerState::NeedsConfirmation
        ));

        let runtime = tokio::runtime::Handle::current();
        let error = tokio::task::spawn_blocking(move || {
            run_code_in_plugin_vm(
                PluginEngineConfig::default(),
                manager,
                runtime,
                r#"return await callTool("mcp__project-fake__capture", {});"#.to_string(),
            )
        })
        .await
        .expect("code VM worker should not panic")
        .expect_err("agent code must not bypass project connector confirmation");
        assert!(
            error
                .to_string()
                .contains("unknown or not-ready deferred tool"),
            "unapproved connector must remain unavailable to agent code: {error}"
        );
    }
}
