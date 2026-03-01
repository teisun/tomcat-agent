//! 基于 tracing 的分级日志：控制台与按大小滚动的文件输出。
//! 禁止在日志中打印敏感信息（API 密钥等）。

use std::path::Path;

use tracing_appender::rolling::{Rotation, RollingFileAppender};
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

use crate::config::LogConfig;

/// 使用 LogConfig 初始化 tracing。控制台 + 可选文件（按天滚动，保留最多 5 个文件）。
pub fn init_logging(cfg: &LogConfig) -> Result<(), crate::error::AppError> {
    let level = cfg.level.to_lowercase();
    if !["trace", "debug", "info", "warn", "error"].contains(&level.as_str()) {
        return Err(crate::error::AppError::Config(format!(
            "无效的日志级别: {}",
            cfg.level
        )));
    }
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    let fmt_layer = fmt::layer().with_writer(std::io::stderr).with_filter(filter);

    let file_layer = if cfg.file_enabled {
        let path = Path::new(&cfg.file_path);
        let dir = path.parent().unwrap_or(Path::new("."));
        let prefix = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pi_awsm");
        let file_appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .max_log_files(5_usize)
            .filename_prefix(prefix)
            .build(dir)
            .map_err(|e| crate::error::AppError::Io(std::io::Error::other(e.to_string())))?;
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        let file_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(&cfg.level));
        let layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_filter(file_filter);
        Some(layer)
    } else {
        None
    };

    if let Some(file_layer) = file_layer {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry().with(fmt_layer).init();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_config_default_level() {
        let cfg = LogConfig::default();
        assert_eq!(cfg.level, "info");
    }

    #[test]
    fn invalid_log_level_returns_error() {
        let cfg = LogConfig {
            level: "not_a_level".to_string(),
            file_enabled: false,
            ..Default::default()
        };
        let r = init_logging(&cfg);
        assert!(r.is_err());
    }
}
