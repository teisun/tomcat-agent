//! `AgentLoopConfig::Default` 与 `SubagentType` 编译断言（plan §P0.5 D1/D12）。
//!
//! 这是一个「shape check」测试：当 `AgentLoopConfig` / `SubagentType` schema 变更时，
//! 这里的字段访问会立刻编译失败，提示其他构造点（`chat_loop` / 测试 fixtures）同步更新。

use crate::core::agent_loop::{AgentLoopConfig, SubagentType};
use crate::infra::config::{DEFAULT_AGENT_MAX_ATTEMPTS, DEFAULT_AGENT_RETRY_BASE_DELAY_MS};

#[test]
fn agent_loop_config_default_includes_subagent_fields() {
    let cfg = AgentLoopConfig::default();
    assert!(
        cfg.parent_session_id.is_none(),
        "default parent_session_id should be None for root chat_loop"
    );
    assert_eq!(
        cfg.spawn_depth, 0,
        "default spawn_depth should be 0 (root chat_loop)"
    );
    assert_eq!(
        cfg.subagent_type,
        SubagentType::User,
        "default subagent_type should be User (chat_loop), not Reviewer"
    );
    assert_eq!(
        cfg.max_attempts, DEFAULT_AGENT_MAX_ATTEMPTS,
        "default max_attempts should track llm config defaults"
    );
    assert_eq!(
        cfg.retry_base_delay_ms, DEFAULT_AGENT_RETRY_BASE_DELAY_MS,
        "default retry_base_delay_ms should track llm config defaults"
    );
}

#[test]
fn subagent_type_root_and_as_str_are_correct() {
    assert!(SubagentType::User.is_root());
    assert!(!SubagentType::PlanReviewer.is_root());
    assert!(!SubagentType::CodeReviewer.is_root());
    assert!(!SubagentType::Verifier.is_root());
    assert_eq!(SubagentType::User.as_str(), "user");
    assert_eq!(SubagentType::PlanReviewer.as_str(), "plan_reviewer");
    assert_eq!(SubagentType::CodeReviewer.as_str(), "code_reviewer");
    assert_eq!(SubagentType::Verifier.as_str(), "verifier");
}
