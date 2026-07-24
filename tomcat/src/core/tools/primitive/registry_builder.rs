use std::path::PathBuf;
use std::sync::Arc;

use crate::core::permission::{BashAstChecker, PermissionGate};
use crate::core::tools::contract::confirmation::UserConfirmationProvider;
use crate::infra::config::ToolsBashConfig;
use crate::infra::AuditRecorder;

use super::bash_task::BackgroundBashGuard;
use super::BashTaskRegistry;

/// Build the standard tracked-bash registry used by agent-owned callers.
///
/// Main chat loops and bash-capable subagents must share the same assembly:
/// foreground wait policy from config + background guard for gate / confirmation /
/// audit / AST checks. This helper keeps those call sites from drifting apart.
pub(crate) fn build_bash_task_registry(
    bash_cfg: &ToolsBashConfig,
    persist_dir: PathBuf,
    gate: Arc<dyn PermissionGate>,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    bash_ast: BashAstChecker,
) -> Arc<BashTaskRegistry> {
    Arc::new(
        BashTaskRegistry::new(persist_dir)
            .with_foreground_wait_ms(bash_cfg.foreground_wait_ms)
            .with_background_guard(BackgroundBashGuard::new(
                "__agent__",
                gate,
                confirmation,
                audit,
                bash_ast,
            )),
    )
}
