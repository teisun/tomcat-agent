//! # 工具系统
//!
//! 聚合工具注册中心、LLM 配置工具后端，以及文件 / shell primitive 执行器。

pub mod config;
pub mod primitive;
mod registry;

pub use registry::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
