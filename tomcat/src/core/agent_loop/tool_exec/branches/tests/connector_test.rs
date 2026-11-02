use super::super::code::tests::{seen, vm_fixture, vm_limits};
use crate::core::agent_loop::tool_exec::{
    execute_tool_full_with_policy_and_connectors, ToolExecOutcome,
};
use crate::core::agent_loop::types::{SubagentType, ToolCallInfo};
use crate::core::connector::ConnectorRegistry;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

async fn entry(
    registry: Arc<ConnectorRegistry>,
    session: &'static str,
    cancel: CancellationToken,
    call: ToolCallInfo,
) -> ToolExecOutcome {
    let primitive = crate::core::agent_loop::tests::mock_primitive();
    let vm_config = vm_limits();
    let tools =
        crate::core::connector::mcp::manager::McpToolView::new(registry.mcp_manager(), None);
    execute_tool_full_with_policy_and_connectors(
        &primitive,
        session,
        &None,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&tools),
        Some(&vm_config),
        SubagentType::User,
        false,
        &cancel,
        &call,
        None,
        None,
        None,
    )
    .await
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCallInfo {
    let args = if name == "tool_call" {
        serde_json::json!({"name":"mcp__fake__capture", "arguments":arguments})
    } else {
        serde_json::json!({"code":format!("return await callTool('mcp__fake__capture', {arguments});")})
    };
    ToolCallInfo {
        id: "same-outer-id".into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}

#[tokio::test]
async fn real_tool_entries_preserve_progress_and_session_stop_isolation() {
    for name in ["tool_call", "tool_run_code"] {
        for stop in [false, true] {
            let (temp, cfg, manager, log) = vm_fixture(300).await;
            let registry = ConnectorRegistry::new(&cfg, None).unwrap();
            assert!(Arc::ptr_eq(&manager, &registry.mcp_manager()));
            let cancel = CancellationToken::new();
            let gate_a = temp.path().join("A");
            let gate_b = temp.path().join("B");
            let args_a =
                serde_json::json!({"label":"A", "gateFile":gate_a, "delayMs":900, "progressMs":40});
            let args_b =
                serde_json::json!({"label":"B", "gateFile":gate_b, "delayMs":900, "progressMs":40});
            let a = tokio::spawn(entry(
                registry.clone(),
                "session-A",
                cancel.clone(),
                call(name, args_a),
            ));
            let b = tokio::spawn(entry(
                registry,
                "session-B",
                CancellationToken::new(),
                call(name, args_b),
            ));
            seen(&log, "started", "A").await;
            seen(&log, "started", "B").await;
            assert!(!a.is_finished() && !b.is_finished());
            if stop {
                cancel.cancel();
            } else {
                tokio::fs::write(gate_a, "release").await.unwrap();
            }
            let result_a = a.await.unwrap();
            assert_eq!(result_a.is_error, stop, "{}", result_a.model_text);
            if stop {
                assert!(
                    result_a.model_text.contains("cancelled"),
                    "{}",
                    result_a.model_text
                );
            } else {
                assert!(result_a.model_text.contains("fixture result: A"));
            }
            assert!(
                !b.is_finished(),
                "{name}, stop={stop}: B ended before release: {}",
                b.await.unwrap().model_text
            );
            tokio::fs::write(gate_b, "release").await.unwrap();
            let result_b = b.await.unwrap();
            assert!(!result_b.is_error, "{}", result_b.model_text);
            assert!(result_b.model_text.contains("fixture result: B"));
            tokio::time::timeout(Duration::from_secs(5), async {
                while manager.call_debug_counts("fake") != Some((0, 16, 32)) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            seen(&log, "exited", "A").await;
            manager.remove_configured_server("fake", &cfg).unwrap();
        }
    }
}
