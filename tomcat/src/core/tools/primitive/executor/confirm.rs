//! # `require_user_confirmation` 兼容入口
//!
//! 路径授权统一走 `gate_check_path` / `gate_check_bash`，本入口仅保留供
//! 外部直接调用 `confirmation.confirm` 的兼容场景：
//! - Read 操作不需要 confirm；
//! - `auto_confirm = true` 时直接放行；
//! - 其他情况转发给底层 [`crate::core::tools::contract::confirmation::UserConfirmationProvider::confirm`]。

use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::error::AppError;

pub(super) async fn require_user_confirmation_impl(
    executor: &DefaultPrimitiveExecutor,
    operation: PrimitiveOperation,
    preview: &str,
    plugin_id: &str,
) -> Result<bool, AppError> {
    if matches!(operation, PrimitiveOperation::Read) || executor.config.auto_confirm {
        return Ok(true);
    }
    executor
        .confirmation
        .confirm(operation, preview, plugin_id)
        .await
}
