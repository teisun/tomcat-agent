//! # 用户确认接口
//!
//! 4 原语执行前（write/edit/bash）由本 trait 向用户请求确认；
//! CLI/chat（011）实现具体交互，本模块仅定义契约。
//!
//! ## v2：[`ConfirmDecision`] 三态
//!
//! 旧 [`UserConfirmationProvider::confirm`] 只能回 `bool`；
//! 工作区权限分级（plan §3）需要"加入工作区(永久)/本次允许/拒绝"三选项。
//!
//! 兼容策略：
//! - `confirm` -> `bool` 作为 trait 兼容入口保留，默认三态实现会委托它。
//! - executor 当前路径使用 `confirm_decision` -> [`ConfirmDecision`]，
//!   默认实现委托给 `confirm`（`true` ↦ `AllowOnce`，`false` ↦ `Deny`）。
//! - CLI 端 `CliConfirmation` 在 PR-4（drag UX）/PR-2（executor 接入）中
//!   override `confirm_decision` 给出 3 选项 UI。

use crate::core::primitives::PrimitiveOperation;
use crate::infra::error::AppError;
use async_trait::async_trait;
use std::path::PathBuf;

/// 用户确认结果（v2 三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmDecision {
    /// 仅本次允许（写入 SessionGrants，会话结束失效）。
    AllowOnce,
    /// 允许并把 `root` 写入 `workspace.extra_roots`（持久化）。
    AllowAndPersistRoot { root: PathBuf },
    /// 拒绝。
    Deny,
}

impl ConfirmDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAndPersistRoot { .. })
    }
}

/// 用户确认提供方：高危操作前由宿主（如 CLI）实现弹窗或命令行确认。
#[async_trait]
pub trait UserConfirmationProvider: Send + Sync + 'static {
    /// 旧接口：bool 二态确认。新代码请优先使用 [`Self::confirm_decision`]。
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError>;

    /// 新接口：三态确认（plan §3 / §4.3 拖入 UX）。
    ///
    /// `suggested_root` 不为 `None` 时，UI 应展示"加入工作区"选项；
    /// 否则只展示"本次允许 / 拒绝"两项（如 bash_approval 命中场景）。
    ///
    /// 默认实现委托给 [`Self::confirm`]：`true` -> `AllowOnce`，`false` -> `Deny`。
    /// CLI 端在 PR-4 / PR-2 override 给出真正的 3 选项 UI。
    async fn confirm_decision(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        _suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        match self.confirm(operation, preview, plugin_id).await? {
            true => Ok(ConfirmDecision::AllowOnce),
            false => Ok(ConfirmDecision::Deny),
        }
    }
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

/// 测试用：始终拒绝（用于边界测试"用户拒绝确认"）。
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
