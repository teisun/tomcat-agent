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
//!   * `read_file` 正常路径 → `(content, false)`。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::error_classifier::handle_overflow_retry;
use crate::core::agent_loop::tool_exec::execute_tool;
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, ToolCallInfo};
use crate::core::llm::ChatMessage;
use crate::core::primitives::PrimitiveExecutor;
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
    let (msg, is_error) = execute_tool(&primitive, &tc).await;
    assert!(is_error, "unknown tool must report is_error=true");
    assert!(
        msg.contains("no_such_tool") || msg.to_lowercase().contains("unknown"),
        "msg should mention the unknown tool name: {}",
        msg
    );
}

/// read_file 正常路径：execute_tool 返回 `is_error == false`，content 由 mock 直接产出。
#[tokio::test]
async fn tool_exec_read_file_returns_content() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "r1".to_string(),
        name: "read_file".to_string(),
        arguments: r#"{"path":"/tmp/abc"}"#.to_string(),
    };
    let (msg, is_error) = execute_tool(&primitive, &tc).await;
    assert!(!is_error, "read_file success must report is_error=false");
    assert!(
        msg.contains("/tmp/abc"),
        "content should include path from mock: {}",
        msg
    );
}
