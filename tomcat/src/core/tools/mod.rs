//! # 工具系统
//!
//! 四层架构：
//! - [`contract`]：LLM 与 host 之间的工具契约（catalog / registry / confirmation）。
//! - [`primitive`]：5 原语 + 安全流水（受信内核执行通道）。
//! - [`config_tool`]：`config_get` / `config_set` 工具通道（走 ConfigBackend）。
//! - [`pipeline`]：跨工具的纯算法与会话状态（edit_normalize / read_state）。

pub mod config_tool;
pub mod contract;
pub mod pipeline;
pub mod plan_tool;
pub mod primitive;
pub mod web_fetch;
pub mod web_search;

#[cfg(test)]
mod tests;
