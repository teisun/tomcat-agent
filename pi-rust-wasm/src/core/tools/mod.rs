//! # 工具系统
//!
//! 聚合工具注册中心、LLM 配置工具后端，以及文件 / shell primitive 执行器。

pub mod catalog;
pub mod config;
pub mod edit_normalize;
pub mod primitive;
pub mod read_state;
mod registry;

pub use registry::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};

#[cfg(test)]
mod tests;
