//! AgentRegistry 单元测试（§9.3D P3 部分）。
//!
//! 测试覆盖：
//! - `agent_registry_register_unregister_balanced`
//! - `spawn_subagent_internal_is_only_child_construction_point`（grep 锚点 + handle 计数）
//! - `cascade_abort_propagates_to_descendants`
//! - `max_spawn_depth_enforced`
//! - `max_concurrent_agents_enforced`
//! - `max_children_per_agent_enforced`
//! - `subagent_panic_does_not_kill_parent`
//! - `subagent_completes_unregisters_and_decrements_active`
//! - `parent_aborted_blocks_new_spawn`
//! - `cascade_abort_skips_unknown_session`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::core::agent_loop::SubagentType;

fn fresh_registry() -> Arc<AgentRegistry> {
    AgentRegistry::new()
}

fn small_registry() -> Arc<AgentRegistry> {
    AgentRegistry::with_config(AgentRegistryConfig {
        max_spawn_depth: 1,
        max_concurrent_agents: 3,
        max_children_per_agent: 2,
    })
}

#[test]
fn agent_registry_register_unregister_balanced() {
    let reg = fresh_registry();
    {
        let _g = reg.register_root_for_test("s1").unwrap();
        assert_eq!(reg.active_count(), 1);
    }
    assert_eq!(reg.active_count(), 0, "Drop 应自动 unregister");
}

#[test]
fn register_duplicate_session_id_rejected() {
    let reg = fresh_registry();
    let _g = reg.register_root_for_test("s1").unwrap();
    let err = reg.register_root_for_test("s1").unwrap_err();
    matches!(err, RegisterError::DuplicateSessionId(_));
}

#[tokio::test]
async fn spawn_subagent_internal_is_only_child_construction_point() {
    // grep 仓库 `AgentLoop::new` 的真正唯一性由 P4 集成测断言；
    // 这里只断言 `spawn_subagent_internal` 在 register + unregister 计数上是收支平衡的。
    let reg = fresh_registry();
    let _g = reg.register_root_for_test("root").unwrap();
    assert_eq!(reg.active_count(), 1);
    let outcome = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap();
    assert!(outcome.child_session_id.contains("child"));
    assert_eq!(outcome.outcome_label, SubagentOutcomeLabel::Completed);
    // 子结束后 active_count 复原
    assert_eq!(reg.active_count(), 1);
}

#[tokio::test]
async fn cascade_abort_propagates_to_descendants() {
    let reg = fresh_registry();
    let _g = reg.register_root_for_test("root").unwrap();

    // 派生一个子并让它「等」父 abort
    let reg_clone = Arc::clone(&reg);
    let join = tokio::spawn(async move {
        reg_clone
            .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
                // 等到 abort_signal 翻起
                while !ctx.abort_signal.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                SubagentOutcome {
                    child_session_id: ctx.child_session_id,
                    subagent_type: ctx.subagent_type,
                    outcome_label: SubagentOutcomeLabel::Interrupted,
                    error_message: Some("cascade abort".into()),
                }
            })
            .await
    });

    // 等到子已 register（async sleep；tokio::test 默认 single-thread）
    let mut waited = Duration::from_secs(2);
    while reg.active_count() != 2 && waited > Duration::ZERO {
        tokio::time::sleep(Duration::from_millis(5)).await;
        waited = waited.saturating_sub(Duration::from_millis(5));
    }
    assert_eq!(reg.active_count(), 2, "子应已注册");
    reg.cascade_abort("root");

    let outcome = tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("timeout waiting for child")
        .unwrap()
        .unwrap();
    assert_eq!(outcome.outcome_label, SubagentOutcomeLabel::Interrupted);
    assert_eq!(reg.active_count(), 1, "子结束后只剩 root");
}

#[tokio::test]
async fn max_spawn_depth_enforced() {
    let reg = small_registry(); // max_spawn_depth = 1
    let _g = reg.register_root_for_test("root").unwrap();
    // depth 0 → 1：允许
    let outcome = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
            // 在子内部再 spawn 一层 → depth 2 应拒
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap();
    assert_eq!(outcome.outcome_label, SubagentOutcomeLabel::Completed);

    // 直接尝试以子 session_id 作为父 spawn —— 但子已 unregister，故先重建一个 depth=1 的 handle
    let depth1_handle = Arc::new(AgentHandle {
        session_id: "deep-1".to_string(),
        subagent_type: SubagentType::Reviewer,
        spawn_depth: 1,
        parent_session_id: Some("root".into()),
        abort_signal: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        children: Mutex::new(Vec::new()),
    });
    reg.register(depth1_handle).unwrap();
    let err = reg
        .spawn_subagent_internal("deep-1", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::DepthExceeded { .. });
    reg.unregister("deep-1");
}

#[tokio::test]
async fn max_concurrent_agents_enforced() {
    let reg = small_registry(); // max_concurrent_agents=3
    let _g1 = reg.register_root_for_test("a").unwrap();
    let _g2 = reg.register_root_for_test("b").unwrap();
    let _g3 = reg.register_root_for_test("c").unwrap();
    let err = reg
        .spawn_subagent_internal("a", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::GlobalConcurrencyExceeded { .. });
}

#[tokio::test]
async fn max_children_per_agent_enforced() {
    let reg = small_registry(); // max_children_per_agent=2
    let _g = reg.register_root_for_test("root").unwrap();
    // 阻塞 future 占位让 children 累计
    let started = Arc::new(AtomicU32::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    for _ in 0..2 {
        let reg_clone = Arc::clone(&reg);
        let started_clone = Arc::clone(&started);
        let release_clone = Arc::clone(&release);
        tokio::spawn(async move {
            reg_clone
                .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
                    started_clone.fetch_add(1, Ordering::Relaxed);
                    release_clone.notified().await;
                    SubagentOutcome {
                        child_session_id: ctx.child_session_id,
                        subagent_type: ctx.subagent_type,
                        outcome_label: SubagentOutcomeLabel::Completed,
                        error_message: None,
                    }
                })
                .await
                .unwrap();
        });
    }
    // 等两个子都 started
    let mut waited = Duration::from_secs(2);
    while started.load(Ordering::Relaxed) < 2 && waited > Duration::ZERO {
        tokio::time::sleep(Duration::from_millis(5)).await;
        waited = waited.saturating_sub(Duration::from_millis(5));
    }
    assert_eq!(started.load(Ordering::Relaxed), 2);

    // 第三次 spawn 应被 per-agent 限流拦下
    let err = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::ChildrenPerAgentExceeded { .. });

    // 释放阻塞任务
    release.notify_waiters();
    release.notify_waiters();
}

#[tokio::test]
async fn subagent_panic_does_not_kill_parent() {
    let reg = fresh_registry();
    let _g = reg.register_root_for_test("root").unwrap();
    let err = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |_ctx| async move {
            panic!("intentional panic");
            #[allow(unreachable_code)]
            SubagentOutcome {
                child_session_id: String::new(),
                subagent_type: SubagentType::Reviewer,
                outcome_label: SubagentOutcomeLabel::Failed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::Panic(_));

    // 父侧 active count 已回到 1（panic 路径也走 unregister）
    assert_eq!(reg.active_count(), 1);

    // 父 abort_signal 未被污染
    let parent = reg.handles.read().get("root").cloned().unwrap();
    assert!(!parent.is_aborted());

    // 还能继续 spawn
    let outcome = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap();
    assert_eq!(outcome.outcome_label, SubagentOutcomeLabel::Completed);
}

#[tokio::test]
async fn parent_aborted_blocks_new_spawn() {
    let reg = fresh_registry();
    let _g = reg.register_root_for_test("root").unwrap();
    reg.cascade_abort("root");
    let err = reg
        .spawn_subagent_internal("root", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::ParentAborted(_));
}

#[tokio::test]
async fn parent_not_found_returns_err() {
    let reg = fresh_registry();
    let err = reg
        .spawn_subagent_internal("ghost", SubagentType::Reviewer, |ctx| async move {
            SubagentOutcome {
                child_session_id: ctx.child_session_id,
                subagent_type: ctx.subagent_type,
                outcome_label: SubagentOutcomeLabel::Completed,
                error_message: None,
            }
        })
        .await
        .unwrap_err();
    matches!(err, SpawnError::ParentNotFound(_));
}

#[test]
fn cascade_abort_skips_unknown_session() {
    let reg = fresh_registry();
    // 不应 panic、不应卡死
    reg.cascade_abort("nonexistent");
}
