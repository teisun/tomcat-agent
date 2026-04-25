//! # 用户确认接口
//!
//! 4 原语执行前（write/edit/bash）由本 trait 向用户请求确认；
//! CLI/chat（011）实现具体交互，本模块仅定义契约。

use crate::core::primitives::PrimitiveOperation;
use crate::infra::error::AppError;
use async_trait::async_trait;

/// 用户确认提供方：高危操作前由宿主（如 CLI）实现弹窗或命令行确认。
#[async_trait]
pub trait UserConfirmationProvider: Send + Sync + 'static {
    /// 请求用户确认。返回 `Ok(true)` 表示同意，`Ok(false)` 表示拒绝。
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError>;
}

/// 测试或自动化场景：始终允许，不弹窗。
#[derive(Debug, Default)]
pub struct AllowAllConfirmation;

#[async_trait]
impl UserConfirmationProvider for AllowAllConfirmation {
    async fn confirm(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 测试用：始终拒绝（用于边界测试“用户拒绝确认”）。
#[derive(Debug, Default)]
pub struct DenyAllConfirmation;

#[async_trait]
impl UserConfirmationProvider for DenyAllConfirmation {
    async fn confirm(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(false)
    }
}
