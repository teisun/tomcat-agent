//! 基于 tracing 的分级日志：控制台 stderr 与可选按日滚动的文件输出。
//! 禁止在日志中打印敏感信息（API 密钥等）。

use std::path::Path;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use super::config::LogConfig;

/// 使用 [`LogConfig`] 初始化 tracing：控制台 stderr + 可选文件（按天滚动，保留最多 5 个文件）。
///
/// 优先使用环境变量 `RUST_LOG`，未设置时使用 `cfg.level`。禁止在日志中打印敏感信息（如 API 密钥）。
///
/// # Arguments
/// * `cfg` - 日志配置，见 [`LogConfig`]。
///
/// # Errors
/// * [`super::error::AppError::Config`] - `cfg.level` 不在 `trace`/`debug`/`info`/`warn`/`error` 之一时返回。
/// * [`super::error::AppError::Io`] - 启用文件输出且无法创建/打开日志文件时返回。
///
/// `log_dir` — 日志写入目录（由 `resolve_log_dir` 推导），仅 `cfg.file_enabled == true` 时使用。
pub fn init_logging(cfg: &LogConfig, log_dir: Option<&Path>) -> Result<(), super::error::AppError> {
    let level = cfg.level.to_lowercase();
    if !["trace", "debug", "info", "warn", "error"].contains(&level.as_str()) {
        return Err(super::error::AppError::Config(format!(
            "无效的日志级别: {}",
            cfg.level
        )));
    }
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    let fmt_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter);

    let file_layer = if cfg.file_enabled {
        let dir = log_dir.unwrap_or(Path::new("."));
        let file_appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .max_log_files(5_usize)
            .filename_prefix("pi_wasm")
            .build(dir)
            .map_err(|e| super::error::AppError::Io(std::io::Error::other(e.to_string())))?;
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        let file_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));
        let layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_filter(file_filter);
        Some(layer)
    } else {
        None
    };

    // try_init：进程内只允许一个全局 subscriber；重复调用（如单测多次跑 CLI）时忽略已初始化。
    let _ = if let Some(file_layer) = file_layer {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(file_layer)
            .try_init()
    } else {
        tracing_subscriber::registry().with(fmt_layer).try_init()
    };
    Ok(())
}

#[cfg(test)]
mod tests;
