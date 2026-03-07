//! 基于 tracing 的分级日志：控制台与按大小滚动的文件输出。
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
pub fn init_logging(cfg: &LogConfig) -> Result<(), super::error::AppError> {
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

    /// 仅控制台、成功路径。init_logging 内部会 init 全局 subscriber，进程内只能成功一次；
    /// 全量测试若出现 "global default trace subscriber already set" 可单独跑：
    /// cargo test -p pi_awsm infra::logging::tests -- --test-threads=1
    #[test]
    fn a_init_logging_console_only_succeeds() {
        let cfg = LogConfig {
            level: "info".to_string(),
            file_enabled: false,
            file_path: String::new(),
            ..LogConfig::default()
        };
        let r = init_logging(&cfg);
        assert!(r.is_ok(), "init_logging(console only) should succeed");
    }

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
