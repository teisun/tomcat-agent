//! # 子模块焦小测（Phase 4 新增）
//!
//! 这些测试不经过 `AgentLoop::run` 整链路，而是直接调用 `pub(super)` 自由函数 /
//! 辅助函数，断言其内部契约。这样在三层循环骨架被进一步重构时，子模块的契约
//! 不会因外层改造而丢失"焦点"。
//!
//! 当前覆盖：
//!
//! - `error_classifier::handle_overflow_retry`：
//!   * 非 overflow 错误 → `OverflowTrimStats::applied == false`，无事件；
//!   * 缺 `context_state` → `applied == false`，无事件。
//! - `tool_exec::execute_tool`：
//!   * unknown 工具名 → `(msg, true)`；
//!   * `read` 正常路径 → `(content, false)`；
//!   * 旧 `read_file` 名 → 走 unknown 分支（PR-RA：运行时无别名）。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::error_classifier::handle_overflow_retry;
use crate::core::agent_loop::tool_exec::execute_tool;
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, ToolCallInfo};
use crate::core::llm::ChatMessage;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::infra::DefaultEventBus;

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

fn make_agent() -> AgentLoop {
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-submod".to_string(),
        ..Default::default()
    };
    AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new())
}

/// 非 context overflow 错误（429 限流）：handle_overflow_retry 应当跳过 trim，
/// 返回 `applied == false` 的默认 stats。
#[tokio::test]
async fn handle_overflow_retry_skipped_when_not_overflow() {
    let mut agent = make_agent();
    let mut messages = vec![ChatMessage::user("hi")];
    let stats = handle_overflow_retry(&mut agent, &mut messages, 1, "API 错误 429: rate limit");
    assert!(
        !stats.applied,
        "non-overflow error must not trigger L3 trim, stats={:?}",
        stats
    );
    assert_eq!(stats.trim_tokens, 0);
    assert_eq!(stats.trim_turns, 0);
}

/// overflow 错误但 `context_state` 缺失：handle_overflow_retry 仅记录诊断日志，
/// 不触发 trim、不发事件，返回 `applied == false`。
#[tokio::test]
async fn handle_overflow_retry_skipped_when_no_context_state() {
    let mut agent = make_agent();
    let mut messages = vec![ChatMessage::user("hi")];
    let err = r#"API 错误 400: {"error":{"code":"context_length_exceeded"}}"#;
    let stats = handle_overflow_retry(&mut agent, &mut messages, 1, err);
    assert!(
        !stats.applied,
        "overflow without context_state must skip trim, stats={:?}",
        stats
    );
    assert_eq!(messages.len(), 1, "messages must be left untouched");
}

/// unknown 工具名：execute_tool 返回 `is_error == true`，content 含 unknown 提示。
#[tokio::test]
async fn tool_exec_unknown_tool_returns_is_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "x".to_string(),
        name: "no_such_tool".to_string(),
        arguments: "{}".to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error, "unknown tool must report is_error=true");
    assert!(
        msg.contains("no_such_tool") || msg.to_lowercase().contains("unknown"),
        "msg should mention the unknown tool name: {}",
        msg
    );
}

/// read 正常路径：execute_tool 返回 `is_error == false`，content 由 mock 直接产出。
#[tokio::test]
async fn tool_exec_read_returns_content() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "r1".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/abc"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(!is_error, "read success must report is_error=false");
    assert!(
        msg.contains("/tmp/abc"),
        "content should include path from mock: {}",
        msg
    );
}

/// PR-RA：旧 `read_file` 名 → 运行时按未知工具回错（无别名 / 无重定向）。
#[tokio::test]
async fn tool_exec_legacy_read_file_returns_unknown_tool_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "legacy_1".to_string(),
        name: "read_file".to_string(),
        arguments: r#"{"path":"/tmp/legacy"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "legacy 'read_file' must NOT be aliased to 'read'; it should return is_error=true"
    );
    assert!(
        msg.contains("read_file") || msg.to_lowercase().contains("unknown"),
        "msg should mention the unknown tool name: {}",
        msg
    );
}

/// PR-RB §2.6：`read.offset = 0` 触发 horizontal gate，返回结构化错误。
#[tokio::test]
async fn tool_exec_read_offset_zero_returns_bound_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "b1".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/x","offset":0,"limit":10}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(
        msg.contains("offset") && msg.contains(">= 1"),
        "bound error should mention `offset` and `>= 1`, got: {}",
        msg
    );
}

/// PR-RB §2.6：`read.limit = 99999` 越上界，返回结构化错误。
#[tokio::test]
async fn tool_exec_read_limit_over_max_returns_bound_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "b2".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/x","limit":99999}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(
        msg.contains("limit") && msg.contains("[1, 10000]"),
        "bound error should mention `limit` range, got: {}",
        msg
    );
}

// ─── T2-P0-016 PR-I：bash 后台三件套分支 ─────────────────────────────────

/// 未注入 BashTaskRegistry 时，`bash run_in_background=true` 走「未启用」错误，
/// 而**不**误调 PrimitiveExecutor::execute_bash 的同步路径。
#[tokio::test]
async fn tool_exec_bash_background_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "bg1".to_string(),
        name: "bash".to_string(),
        arguments: r#"{"command":"sleep 1","run_in_background":true}"#.to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "未注入 registry 时 background bash 必须 is_error=true"
    );
    assert!(msg.contains("未启用"), "错误文案应提示「未启用」：{}", msg);
}

#[tokio::test]
async fn tool_exec_task_output_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "to1".to_string(),
        name: "task_output".to_string(),
        arguments: r#"{"task_id":"abc"}"#.to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(msg.contains("未启用"));
}

#[tokio::test]
async fn tool_exec_task_list_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "tl1".to_string(),
        name: "task_list".to_string(),
        arguments: "{}".to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(msg.contains("未启用"));
}

/// 起后台 → 拉输出 → stop → list：bash.md §2.4.4 验收的端到端路径，
/// 在 tool_exec 层用真实 BashTaskRegistry 走通。
#[tokio::test]
async fn tool_exec_bash_background_full_lifecycle() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    // 起：background bash 应当立即返回 ticket JSON。
    let start_tc = ToolCallInfo {
        id: "bg-1".to_string(),
        name: "bash".to_string(),
        arguments: r#"{"command":"i=0; while [ $i -lt 50 ]; do echo line-$i; i=$((i+1)); sleep 0.1; done","run_in_background":true}"#.to_string(),
    };
    let (start_msg, start_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    assert!(!start_err, "起后台必须成功：{}", start_msg);
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).expect("ticket 应为合法 JSON");
    let task_id = ticket["taskId"]
        .as_str()
        .expect("ticket 含 taskId")
        .to_string();
    assert!(!task_id.is_empty());

    // 等几行写出来再拉。
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    // 拉：task_output 必须返回非空 content + finished=false。
    let out_tc = ToolCallInfo {
        id: "to-1".to_string(),
        name: "task_output".to_string(),
        arguments: format!(r#"{{"task_id":"{}"}}"#, task_id),
    };
    let (out_msg, out_err, _) = execute_tool(&primitive, &None, &registry_opt, None, &out_tc).await;
    assert!(!out_err, "task_output 必须成功：{}", out_msg);
    let chunk: serde_json::Value = serde_json::from_str(&out_msg).expect("chunk 应为合法 JSON");
    assert_eq!(chunk["finished"], serde_json::Value::Bool(false));
    assert!(
        chunk["content"]
            .as_str()
            .map(|s| s.contains("line-0"))
            .unwrap_or(false),
        "content 应含 line-0：{}",
        out_msg
    );

    // stop：返回成功提示。
    let stop_tc = ToolCallInfo {
        id: "ts-1".to_string(),
        name: "task_stop".to_string(),
        arguments: format!(r#"{{"task_id":"{}"}}"#, task_id),
    };
    let (stop_msg, stop_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &stop_tc).await;
    assert!(!stop_err, "task_stop 必须成功：{}", stop_msg);
    assert!(stop_msg.contains(&task_id));

    // 给 wait 任务 reap 留点时间。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // list：返回 1 条且 status.state == "stopped"。
    let list_tc = ToolCallInfo {
        id: "tl-1".to_string(),
        name: "task_list".to_string(),
        arguments: "{}".to_string(),
    };
    let (list_msg, list_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &list_tc).await;
    assert!(!list_err, "task_list 必须成功：{}", list_msg);
    let infos: serde_json::Value = serde_json::from_str(&list_msg).expect("list 应为合法 JSON");
    let arr = infos.as_array().expect("list 是数组");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["taskId"], serde_json::Value::String(task_id));
    assert_eq!(
        arr[0]["status"]["state"],
        serde_json::Value::String("stopped".to_string())
    );
}
