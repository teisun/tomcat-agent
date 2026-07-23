//! # `AgentRegistry` — 多 Agent 派生的唯一入口（multi-agent.md §14 Phase 2/3 最小子集）
//!
//! reviewer 内联子 Agent 与未来 `dispatch_agent` 都通过 [`AgentRegistry::spawn_subagent_internal`]
//! 完成子 [`AgentLoop`] 构造。设计目标：
//!
//! - **唯一子 loop 构造点**：除顶层 `chat_loop`，整个仓库内 `AgentLoop::new` 仅在
//!   `spawn_subagent_internal` 内调用（grep 锚点）。
//! - **CascadeAbort**：父 Agent 持层级化 `CancellationToken`；子 Agent 由
//!   `child_token()` 派生。父 `cancel()` 一次立即扩散到所有后代（无需逐级通知）。
//! - **资源限流**：`MAX_SPAWN_DEPTH`、`MAX_CONCURRENT_AGENTS`、`MAX_CHILDREN_PER_AGENT`
//!   三道闸门防止 fork bomb / 内存膨胀。
//! - **panic 隔离**：子 spawn 走 `tokio::spawn + JoinHandle.await`；JoinError 转 `SpawnError::Panic`，
//!   父循环继续；不会因为子 panic 杀掉父进程。
//! - **生命周期事件**：`SubAgentStart` / `SubAgentEnd` 通过 `EventBus::emit`；不阻塞父循环。
//!
//! 注：本模块**不**与 [`AgentLoop`] 直接耦合（避免循环依赖）。spawn 函数以闭包形式注入，
//! 集成 reviewer 时由调用方包装真实的 `AgentLoop::new + run()`；测试时可用 mock 闭包。

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::SubagentType;
use crate::infra::event_bus::ScopedEventEmitter;
use crate::infra::events::AgentEvent;
use crate::EventBus;

#[cfg(test)]
mod tests;

// ─── 配置常量 ───────────────────────────────────────────────────────────────

/// 派生深度上限：root chat_loop=0 / reviewer=1 / reviewer 内套娃=2 拒。
pub const MAX_SPAWN_DEPTH: u32 = 2;
/// 全局并发 Agent 数上限（顶层 + 所有子）。
pub const MAX_CONCURRENT_AGENTS: u32 = 16;
/// 单一父 Agent 可同时持有的直接子数量。
pub const MAX_CHILDREN_PER_AGENT: u32 = 8;

/// Registry 内部限流配置（可注入；测试用低阈值）。
#[derive(Debug, Clone, Copy)]
pub struct AgentRegistryConfig {
    pub max_spawn_depth: u32,
    pub max_concurrent_agents: u32,
    pub max_children_per_agent: u32,
}

impl Default for AgentRegistryConfig {
    fn default() -> Self {
        Self {
            max_spawn_depth: MAX_SPAWN_DEPTH,
            max_concurrent_agents: MAX_CONCURRENT_AGENTS,
            max_children_per_agent: MAX_CHILDREN_PER_AGENT,
        }
    }
}

// ─── 错误 ────────────────────────────────────────────────────────────────────

/// `spawn_subagent_internal` 失败原因。
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("派生深度超限：parent depth {parent_depth} + 1 > {max}")]
    DepthExceeded { parent_depth: u32, max: u32 },
    #[error("全局并发 Agent 数超限：当前 {current} / 上限 {max}")]
    GlobalConcurrencyExceeded { current: u32, max: u32 },
    #[error("父 Agent {parent} 子数超限：当前 {current} / 上限 {max}")]
    ChildrenPerAgentExceeded {
        parent: String,
        current: u32,
        max: u32,
    },
    #[error("父 Agent {0} 未在 registry（已 unregister 或从未 register）")]
    ParentNotFound(String),
    #[error("父 Agent {0} 已被请求 abort，拒绝派生新子")]
    ParentAborted(String),
    #[error("子 Agent 执行或结果解析阶段 panic：{0}")]
    Panic(String),
    #[error("子 spawn 内部错误: {0}")]
    Internal(String),
}

/// `register` 失败原因（独立于 `SpawnError`，便于父循环对 chat_loop 自身 register 单独处理）。
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("session_id {0} 已存在")]
    DuplicateSessionId(String),
}

// ─── 数据结构 ───────────────────────────────────────────────────────────────

/// 单个 Agent 实例的注册记录。
pub struct AgentHandle {
    pub session_id: String,
    pub subagent_type: SubagentType,
    pub spawn_depth: u32,
    pub parent_session_id: Option<String>,
    /// 当前 Agent 子树的根 token。root handle 在每回合开始会被替换成新的 token，
    /// 子 handle 则在 spawn 时从父 token 派生出独立 child_token。
    pub cancel_token: Mutex<CancellationToken>,
    /// 直接子 session_id 列表（仅用于 MAX_CHILDREN_PER_AGENT 计数与 unregister 清理）。
    children: Mutex<Vec<String>>,
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("session_id", &self.session_id)
            .field("subagent_type", &self.subagent_type)
            .field("spawn_depth", &self.spawn_depth)
            .field("parent_session_id", &self.parent_session_id)
            .field("aborted", &self.is_aborted())
            .field("children", &*self.children.lock())
            .finish()
    }
}

impl AgentHandle {
    pub fn is_aborted(&self) -> bool {
        self.cancel_token.lock().is_cancelled()
    }

    pub fn token(&self) -> CancellationToken {
        self.cancel_token.lock().clone()
    }

    pub fn child_token(&self) -> CancellationToken {
        self.cancel_token.lock().child_token()
    }

    pub fn replace_token(&self, token: CancellationToken) {
        *self.cancel_token.lock() = token;
    }

    pub fn cancel(&self) {
        self.cancel_token.lock().cancel();
    }
}

/// 子 Agent 的运行结果（由调用方闭包决定 success / interrupted / failed 的语义）。
#[derive(Debug, Clone)]
pub struct SubagentOutcome {
    pub child_session_id: String,
    pub subagent_type: SubagentType,
    pub outcome_label: SubagentOutcomeLabel,
    /// 失败 / abort 摘要（成功为 None）。
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentOutcomeLabel {
    Completed,
    Interrupted,
    Failed,
}

impl SubagentOutcomeLabel {
    fn as_str(self) -> &'static str {
        match self {
            SubagentOutcomeLabel::Completed => "completed",
            SubagentOutcomeLabel::Interrupted => "interrupted",
            SubagentOutcomeLabel::Failed => "failed",
        }
    }
}

/// 提供给 spawn 闭包的子 Agent 运行上下文。调用方据此构造 `AgentLoopConfig`
/// 并调用 `AgentLoop::new(...).run().await`。
pub struct SubagentSpawnContext {
    pub child_session_id: String,
    pub parent_session_id: String,
    pub subagent_type: SubagentType,
    pub spawn_depth: u32,
    /// 子 Agent 的层级化取消 token。调用方应把它直接传给 `AgentLoop::new(..., token)`，
    /// 让父 turn cancel / root cascade_abort 自然扩散到子 Agent。
    pub cancel_token: CancellationToken,
}

struct SpawnCleanupGuard {
    registry: Arc<AgentRegistry>,
    child_session_id: String,
}

impl Drop for SpawnCleanupGuard {
    fn drop(&mut self) {
        if let Some(handle) = self
            .registry
            .handles
            .read()
            .get(&self.child_session_id)
            .cloned()
        {
            handle.cancel();
        }
        self.registry.unregister(&self.child_session_id);
    }
}

// ─── Registry ───────────────────────────────────────────────────────────────

/// 进程内单例（按需注入，不是 OnceCell）；`ChatContext::from_config` 在装配阶段
/// 构造一次并 `Arc<AgentRegistry>` 注入到 `ChatContext`。
pub struct AgentRegistry {
    handles: RwLock<HashMap<String, Arc<AgentHandle>>>,
    /// 全局并发计数（O(1) 累计，避免每次 spawn 都 lock handles）。
    active: AtomicU32,
    config: AgentRegistryConfig,
    /// session_id 唯一性辅助（uuid v4 简化版；测试用确定性 prefix）。
    next_seq: AtomicU32,
    /// 事件总线；用于 `SubAgentStart` / `SubAgentEnd`。`None` 表示禁用事件（单元测试默认）。
    event_bus: Option<Arc<dyn EventBus>>,
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("active", &self.active.load(Ordering::Relaxed))
            .field("config", &self.config)
            .field(
                "session_ids",
                &self.handles.read().keys().cloned().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl AgentRegistry {
    pub fn new() -> Arc<Self> {
        Self::with_config(AgentRegistryConfig::default())
    }

    pub fn with_config(config: AgentRegistryConfig) -> Arc<Self> {
        Arc::new(Self {
            handles: RwLock::new(HashMap::new()),
            active: AtomicU32::new(0),
            config,
            next_seq: AtomicU32::new(0),
            event_bus: None,
        })
    }

    /// 注入事件总线（注入后所有 spawn 都发射 `SubAgentStart/End`）。
    pub fn attach_event_bus(self: &Arc<Self>, bus: Arc<dyn EventBus>) -> Arc<Self> {
        // 因为 Arc 内部不可变，这里 clone Self 字段重组（开销可忽略；只有装配阶段调用）。
        Arc::new(Self {
            handles: RwLock::new(std::mem::take(&mut *self.handles.write())),
            active: AtomicU32::new(self.active.load(Ordering::Relaxed)),
            config: self.config,
            next_seq: AtomicU32::new(self.next_seq.load(Ordering::Relaxed)),
            event_bus: Some(bus),
        })
    }

    pub fn config(&self) -> AgentRegistryConfig {
        self.config
    }

    /// 当前活跃 Agent 数。
    pub fn active_count(&self) -> u32 {
        self.active.load(Ordering::Relaxed)
    }

    /// 注册 handle；返回 `RegistrationGuard`，Drop 时自动 unregister，避免泄漏。
    ///
    /// 顶层 `chat_loop` 在装配阶段 register 自身 handle；reviewer 子 Agent 由
    /// `spawn_subagent_internal` 内部 register（不暴露给调用方）。
    pub fn register(
        self: &Arc<Self>,
        handle: Arc<AgentHandle>,
    ) -> Result<RegistrationGuard, RegisterError> {
        let mut handles = self.handles.write();
        if handles.contains_key(&handle.session_id) {
            return Err(RegisterError::DuplicateSessionId(handle.session_id.clone()));
        }
        handles.insert(handle.session_id.clone(), Arc::clone(&handle));
        self.active.fetch_add(1, Ordering::Relaxed);
        Ok(RegistrationGuard {
            registry: Arc::clone(self),
            session_id: handle.session_id.clone(),
        })
    }

    /// 立即注销（一般通过 `RegistrationGuard::drop` 自动调用）。
    pub fn unregister(&self, session_id: &str) {
        let handle = self.handles.write().remove(session_id);
        if let Some(h) = handle {
            // 同步从父的 children 列表中移除
            if let Some(parent_id) = h.parent_session_id.clone() {
                if let Some(parent) = self.handles.read().get(&parent_id) {
                    parent.children.lock().retain(|c| c != session_id);
                }
            }
            self.active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// cancel 指定 root handle 的 token；tokio 会把取消沿 token 树自动扩散到所有后代。
    pub fn cascade_abort(&self, root_session_id: &str) {
        let root = {
            let snapshot = self.handles.read();
            snapshot.get(root_session_id).cloned()
        };
        if let Some(root) = root {
            root.cancel();
        }
    }

    /// 在新回合开始时为 root handle 安装新的 token。
    /// 只允许作用于 root agent（`parent_session_id == None`）。
    pub fn rearm_root(
        &self,
        root_session_id: &str,
        token: CancellationToken,
    ) -> Result<(), SpawnError> {
        let root = {
            let handles = self.handles.read();
            handles
                .get(root_session_id)
                .cloned()
                .ok_or_else(|| SpawnError::ParentNotFound(root_session_id.to_string()))?
        };
        if root.parent_session_id.is_some() {
            return Err(SpawnError::Internal(format!(
                "session {root_session_id} is not a root agent"
            )));
        }
        root.replace_token(token);
        Ok(())
    }

    /// **唯一的子 Agent 构造点**。
    ///
    /// 调用方通过 `spawn` 闭包接收 [`SubagentSpawnContext`]，在其中构造
    /// `AgentLoopConfig`（透传 `parent_session_id` / `spawn_depth` / `subagent_type`
    /// 与层级化 `CancellationToken`），再调用 `AgentLoop::new(...).run().await`。
    ///
    /// Registry 责任：
    /// - 三道闸门（depth / global / per-parent）
    /// - 注册子 handle、emit `SubAgentStart`
    /// - panic 隔离（`tokio::spawn` + `JoinHandle.await`）
    /// - 终值时 unregister + emit `SubAgentEnd`
    pub async fn spawn_subagent_internal<F, Fut>(
        self: &Arc<Self>,
        parent_session_id: &str,
        subagent_type: SubagentType,
        spawn: F,
    ) -> Result<SubagentOutcome, SpawnError>
    where
        F: FnOnce(SubagentSpawnContext) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = SubagentOutcome> + Send + 'static,
    {
        let (child_handle, _parent_arc) =
            self.preflight_and_register(parent_session_id, subagent_type)?;
        let child_session_id = child_handle.session_id.clone();
        let cancel_token = child_handle.token();
        let spawn_depth = child_handle.spawn_depth;
        let cleanup_guard = SpawnCleanupGuard {
            registry: Arc::clone(self),
            child_session_id: child_session_id.clone(),
        };

        // emit SubAgentStart
        self.emit(
            Some(&child_session_id),
            AgentEvent::SubAgentStart {
                parent_session_id: parent_session_id.to_string(),
                child_session_id: child_session_id.clone(),
                subagent_type: subagent_type.as_str().to_string(),
                spawn_depth,
            },
        );

        let ctx = SubagentSpawnContext {
            child_session_id: child_session_id.clone(),
            parent_session_id: parent_session_id.to_string(),
            subagent_type,
            spawn_depth,
            cancel_token,
        };

        // panic 隔离：tokio::spawn + JoinHandle.await，JoinError(panic) → SpawnError::Panic
        // （覆盖 child run() 本体或 spawn 收尾/结果解析阶段的 panic）
        let join = tokio::spawn(async move { spawn(ctx).await });
        let outcome_result = join.await;

        // 不论结果，都 unregister + emit End；future 被 drop 时 guard 也会 cancel + unregister。
        drop(cleanup_guard);

        match outcome_result {
            Ok(outcome) => {
                self.emit(
                    Some(&outcome.child_session_id),
                    AgentEvent::SubAgentEnd {
                        parent_session_id: parent_session_id.to_string(),
                        child_session_id: outcome.child_session_id.clone(),
                        subagent_type: outcome.subagent_type.as_str().to_string(),
                        outcome: outcome.outcome_label.as_str().to_string(),
                        error_message: outcome.error_message.clone(),
                    },
                );
                Ok(outcome)
            }
            Err(join_err) => {
                let msg = format!("{join_err}");
                self.emit(
                    Some(&child_session_id),
                    AgentEvent::SubAgentEnd {
                        parent_session_id: parent_session_id.to_string(),
                        child_session_id: child_session_id.clone(),
                        subagent_type: subagent_type.as_str().to_string(),
                        outcome: "failed".to_string(),
                        error_message: Some(format!("panic: {msg}")),
                    },
                );
                Err(SpawnError::Panic(msg))
            }
        }
    }

    /// 同步路径：限流 + 注册子 handle + 锁父 children；返回 (子 handle, 父 handle Arc)。
    fn preflight_and_register(
        self: &Arc<Self>,
        parent_session_id: &str,
        subagent_type: SubagentType,
    ) -> Result<(Arc<AgentHandle>, Arc<AgentHandle>), SpawnError> {
        let mut handles = self.handles.write();
        // 1) 父存在
        let parent = handles
            .get(parent_session_id)
            .cloned()
            .ok_or_else(|| SpawnError::ParentNotFound(parent_session_id.to_string()))?;

        // 2) 父未 abort
        if parent.is_aborted() {
            return Err(SpawnError::ParentAborted(parent_session_id.to_string()));
        }

        // 3) depth
        let new_depth = parent.spawn_depth + 1;
        if new_depth > self.config.max_spawn_depth {
            return Err(SpawnError::DepthExceeded {
                parent_depth: parent.spawn_depth,
                max: self.config.max_spawn_depth,
            });
        }

        // 4) 全局并发：与 insert 共持 `handles.write()`，避免多个并发 spawn 同时越过配额。
        let current = handles.len() as u32;
        if current >= self.config.max_concurrent_agents {
            return Err(SpawnError::GlobalConcurrencyExceeded {
                current,
                max: self.config.max_concurrent_agents,
            });
        }

        // 注册子 handle（child_token 与父 token 形成层级树，父 cancel 一次后代全可见）
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let child_session_id = format!(
            "{}-child-{}-{}",
            parent_session_id,
            subagent_type.as_str(),
            seq
        );
        let child = Arc::new(AgentHandle {
            session_id: child_session_id.clone(),
            subagent_type,
            spawn_depth: new_depth,
            parent_session_id: Some(parent_session_id.to_string()),
            cancel_token: Mutex::new(parent.child_token()),
            children: Mutex::new(Vec::new()),
        });

        // 5) 父的 children 上限：与 push 共持父 children 锁，避免同父并发越过 max_children。
        {
            let mut children = parent.children.lock();
            if children.len() as u32 >= self.config.max_children_per_agent {
                return Err(SpawnError::ChildrenPerAgentExceeded {
                    parent: parent_session_id.to_string(),
                    current: children.len() as u32,
                    max: self.config.max_children_per_agent,
                });
            }
            handles.insert(child_session_id.clone(), Arc::clone(&child));
            children.push(child_session_id.clone());
        }
        self.active.fetch_add(1, Ordering::Relaxed);

        Ok((child, parent))
    }

    fn emit(&self, session_id: Option<&str>, ev: AgentEvent) {
        let Some(bus) = self.event_bus.as_ref() else {
            return;
        };
        let emitter =
            ScopedEventEmitter::new_optional(Arc::clone(bus), session_id.map(str::to_string));
        let _ = emitter.emit(ev);
    }

    /// 构造一个顶层 root handle（spawn_depth=0、subagent_type=User）并注册。
    /// 生产路径由 `ChatContext::from_config` 在装配阶段调用；
    /// `RegistrationGuard` 与 `ChatContext` 同生命周期，drop 时自动注销。
    pub fn register_root(
        self: &Arc<Self>,
        session_id: impl Into<String>,
    ) -> Result<RegistrationGuard, RegisterError> {
        let session_id = session_id.into();
        let handle = Arc::new(AgentHandle {
            session_id,
            subagent_type: SubagentType::User,
            spawn_depth: 0,
            parent_session_id: None,
            cancel_token: Mutex::new(CancellationToken::new()),
            children: Mutex::new(Vec::new()),
        });
        self.register(handle)
    }

    /// 测试用别名（保持向后兼容；与 [`Self::register_root`] 行为一致）。
    pub fn register_root_for_test(
        self: &Arc<Self>,
        session_id: impl Into<String>,
    ) -> Result<RegistrationGuard, RegisterError> {
        self.register_root(session_id)
    }
}

/// 自动注销 guard。`chat_loop` 持有它直到本轮回合结束；reviewer 由 spawn_subagent_internal
/// 内部管理（手动 unregister + emit End）。
#[derive(Debug)]
pub struct RegistrationGuard {
    registry: Arc<AgentRegistry>,
    session_id: String,
}

impl RegistrationGuard {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        self.registry.unregister(&self.session_id);
    }
}

// ─── 等待小工具（测试可见） ─────────────────────────────────────────────────
