//! `pi chat` 子命令入口：启动交互式对话模式。

use crate::{AppConfig, AppError};

pub(super) fn run_chat(resume: bool, cfg: &AppConfig) -> Result<(), AppError> {
    let ctx = super::super::chat::ChatContext::from_config(cfg.clone())?;

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Config(format!("创建 tokio 运行时失败: {}", e)))?;

    let cancelled = ctx.cancelled.clone();
    ctrlc::set_handler(move || {
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .ok();

    rt.block_on(super::super::chat::chat_loop(&ctx, resume))
}
