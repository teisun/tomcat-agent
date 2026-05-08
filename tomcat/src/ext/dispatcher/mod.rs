//! # 宿主 API 统一分发器 (HostApiDispatcher)
//!
//! 单入口多路复用：根据 HostRequest 的 module/method 路由到对应 Processor。
//! 与 Architecture 宿主API层（host-api-layer）3.3 一致；支持 4 原语、LLM、工具、事件、会话 API。
//!
//! ## 结构示意
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                        HostApiDispatcher                                     │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │ 注入的 Processor（Option）                                                    │
//! │   event_bus ──────► 事件 on/off/emit/once                                     │
//! │   primitive ──────► 4 原语 readFile/writeFile/editFile/executeBash           │
//! │   tools ───────────► 工具 register/call/list/getActive/setActive             │
//! │   llm ─────────────► LLM createChatCompletion / createChatCompletionStream    │
//! │   session ─────────► 会话 getCurrent/getMessages/sendMessage                 │
//! │   audit ───────────► 每笔 Hostcall 记录                                       │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │ 异步基础设施                                                                  │
//! │   async_results: DashMap<callId, AsyncCallStatus>   ► 异步任务结果缓存        │
//! │   instance_calls: DashMap<instance_id, [callId]>   ► 实例→callId 映射（清理） │
//! │   tokio_handle ───► 共享 Runtime，同步路径 block_on / 异步路径 spawn          │
//! │   llm_semaphore ──► 限制 LLM 并发（默认 5）                                   │
//! │   async_timeout ──► 异步任务超时（默认 30s）                                  │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```

mod dispatch;
mod helpers;
mod ops;
mod session_ops;
mod types;

#[cfg(test)]
mod tests;

pub use types::{AsyncCallStatus, HostApiDispatcher};
