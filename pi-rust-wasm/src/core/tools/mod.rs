//! # 工具系统
//!
//! 四层架构：
//! - [`contract`]：LLM 与 host 之间的工具契约（catalog / registry / confirmation）。
//! - [`primitive`]：5 原语 + 安全流水（受信内核执行通道）。
//! - [`config_tool`]：`config_get` / `config_set` 工具通道（走 ConfigBackend）。
//! - [`pipeline`]：跨工具的纯算法与会话状态（edit_normalize / read_state）。

pub mod pipeline;
pub mod primitive;
pub mod contract;
pub mod config_tool;

#[cfg(test)]
mod tests;
