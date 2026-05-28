use tracing::{info, warn};

use crate::api::chat::context::ChatContext;
use crate::infra::error::AppError;

pub(super) fn ensure_session(ctx: &ChatContext) -> Result<(), AppError> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let _ = ctx.session.ensure_current_session(cwd)?;
    Ok(())
}

pub(crate) async fn cleanup_openai_files_on_session_end(ctx: &ChatContext, reason: &str) {
    let Some(runtime) = ctx.openai_files_runtime.as_ref() else {
        return;
    };
    let summary = runtime.cleanup_registered_files(reason).await;
    if summary.total == 0 {
        return;
    }
    if summary.failed > 0 {
        warn!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            failed = summary.failed,
            "openai files cleanup finished with failures"
        );
    } else {
        info!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            "openai files cleanup completed"
        );
    }
}
