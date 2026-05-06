//! # `ConfigBackend` 抽象（plan §6 / PR-7）
//!
//! `tool_exec::execute_tool` 内分发 `config_get` / `config_set` 这两条 LLM 工具时，
//! 需要把"读 / 写 pi.config.toml"的实现注入进来：具体实现位于
//! [`crate::core::tools::config_tool`]。
//!
//! 本模块只声明一个小契约 trait：调用方通过
//! `Option<Arc<dyn ConfigBackend>>` 注入实现。`AgentLoop` 中字段为
//! `Option<...>`，便于测试与不需要工具的场景（CLI 单测、子模块测试）
//! 继续不传。
//!
//! ## 错误语义
//!
//! - 配置未启用（注入为 `None`）：execute_tool 直接返回 `is_error=true`，
//!   payload 提示"未启用 config 工具"。
//! - 白名单 / 硬黑名单拒绝：返回 [`AppError::Permission`]，execute_tool 包装为
//!   `is_error=true`，文案不暴露白名单具体内容（`tools::config_tool` 内部已自带文案）。
//! - 配置文件 IO / TOML 错误：返回 [`AppError::Io`] / [`AppError::Config`]，原样上抛。

use async_trait::async_trait;
use std::sync::Arc;

use crate::infra::error::AppError;

/// 把 `config_get` / `config_set` 工具调用透传到具体后端的契约。
///
/// 实现见 [`crate::core::tools::config_tool::ChatConfigBackend`]。
#[async_trait]
pub trait ConfigBackend: Send + Sync + 'static {
    /// 读取一个配置项；返回值会被工具直接序列化给 LLM。
    async fn config_get(&self, key: &str) -> Result<serde_json::Value, AppError>;

    /// 写入（或追加）一个配置项；返回 `(applied, message)`，由工具序列化给 LLM。
    async fn config_set(&self, key: &str, value: &str) -> Result<(bool, String), AppError>;
}

/// 类型别名：方便 `AgentLoop` / `tool_exec` 处使用。
pub type SharedConfigBackend = Arc<dyn ConfigBackend>;
