//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `PI_AWSM__`，分隔符 `__`）。

use serde::{Deserialize, Serialize};

use super::error::AppError;

/// 插件或操作的权限等级，用于 4 原语与工具访问控制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    /// 受限，仅允许白名单内操作。
    #[default]
    Restricted,
    /// 普通，需审批的操作按配置处理。
    Normal,
    /// 可信，放宽审批要求。
    Trusted,
}

/// 日志配置：级别、是否写文件、路径与滚动大小。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file_enabled: bool,
    #[serde(default = "default_log_path")]
    pub file_path: String,
    #[serde(default = "default_log_roll_size")]
    pub file_roll_size_mb: u64,
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_path() -> String {
    "pi_awsm.log".to_string()
}
fn default_log_roll_size() -> u64 {
    10
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file_enabled: false,
            file_path: default_log_path(),
            file_roll_size_mb: default_log_roll_size(),
        }
    }
}

/// LLM 接入配置：提供商、API 地址、密钥环境变量、默认模型、限流与重试。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_llm_model")]
    pub default_model: String,
    /// 最大并发 LLM 请求数，0 表示不限制（不推荐）。
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: u32,
    /// 非流式请求失败时的重试次数（仅对可重试错误如 429/5xx）。
    #[serde(default = "default_llm_retry_count")]
    pub retry_count: u32,
    /// 流式请求单次读取超时秒数。
    #[serde(default = "default_stream_timeout_sec")]
    pub stream_timeout_sec: u64,
}

fn default_llm_provider() -> String {
    "openai".to_string()
}
fn default_llm_model() -> String {
    "gpt-4o-mini".to_string()
}
fn default_max_concurrent_requests() -> u32 {
    4
}
fn default_llm_retry_count() -> u32 {
    3
}
fn default_stream_timeout_sec() -> u64 {
    60
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            api_base: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            default_model: default_llm_model(),
            max_concurrent_requests: default_max_concurrent_requests(),
            retry_count: default_llm_retry_count(),
            stream_timeout_sec: default_stream_timeout_sec(),
        }
    }
}

/// 存储配置：会话目录等。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    #[serde(default = "default_sessions_dir")]
    pub sessions_dir: String,
}

fn default_sessions_dir() -> String {
    "~/.pi/agent/sessions".to_string()
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            sessions_dir: default_sessions_dir(),
        }
    }
}

/// 插件配置：插件目录与启动时自动加载的插件列表。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginConfig {
    #[serde(default = "default_plugins_dir")]
    pub plugins_dir: String,
    #[serde(default)]
    pub auto_load: Vec<String>,
}

fn default_plugins_dir() -> String {
    "~/.pi/agent/plugins".to_string()
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            plugins_dir: default_plugins_dir(),
            auto_load: Vec::new(),
        }
    }
}

/// 4 原语配置：路径/命令白名单与黑名单、审批与禁止列表、是否需用户确认等。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrimitiveConfig {
    #[serde(default)]
    pub path_whitelist: Vec<String>,
    #[serde(default)]
    pub path_blacklist: Vec<String>,
    #[serde(default)]
    pub bash_whitelist: Vec<String>,
    #[serde(default)]
    pub bash_approval_required: Vec<String>,
    #[serde(default)]
    pub bash_forbidden: Vec<String>,
    #[serde(default)]
    pub auto_confirm: bool,
    #[serde(default)]
    pub auto_confirm_whitelist: Vec<String>,
    #[serde(default)]
    pub require_approval_for_all_write: bool,
    #[serde(default)]
    pub require_approval_for_all_bash: bool,
}

impl Default for PrimitiveConfig {
    fn default() -> Self {
        Self {
            path_whitelist: Vec::new(),
            path_blacklist: Vec::new(),
            bash_whitelist: Vec::new(),
            bash_approval_required: Vec::new(),
            bash_forbidden: Vec::new(),
            auto_confirm: false,
            auto_confirm_whitelist: Vec::new(),
            require_approval_for_all_write: true,
            require_approval_for_all_bash: true,
        }
    }
}

/// 安全与审计配置：默认插件权限、审计日志开关与保留天数、安全扫描等。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    #[serde(default)]
    pub default_plugin_permission_level: PermissionLevel,
    #[serde(default = "default_true")]
    pub enable_audit_log: bool,
    #[serde(default = "default_audit_retention_days")]
    pub audit_log_retention_days: u32,
    #[serde(default)]
    pub enable_plugin_safety_scan: bool,
}

fn default_true() -> bool {
    true
}
fn default_audit_retention_days() -> u32 {
    90
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_plugin_permission_level: PermissionLevel::Restricted,
            enable_audit_log: true,
            audit_log_retention_days: 90,
            enable_plugin_safety_scan: false,
        }
    }
}

/// 应用顶层配置，聚合 log / llm / storage / plugin / security / primitive 子配置。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub primitive: PrimitiveConfig,
}

/// 从可选配置文件与环境变量加载并合并为 [`AppConfig`]。
///
/// 合并顺序：若提供且存在的配置文件先加载，再叠加环境变量 `PI_AWSM__*`（`__` 表示嵌套）。未提供或不存在文件时仅用默认值与环境变量。
///
/// # Arguments
/// * `config_path` - 配置文件路径，如 `Some(Path::new("config.toml"))`；`None` 表示仅用默认与环境变量。
///
/// # Returns
/// 合并后的 [`AppConfig`]，可直接用于 [`validate_config`] 校验。
///
/// # Errors
/// * [`AppError::Config`] - 配置文件解析失败或反序列化到 [`AppConfig`] 失败时返回。
pub fn load_config(config_path: Option<&std::path::Path>) -> Result<AppConfig, AppError> {
    let mut builder = ::config::Config::builder();
    if let Some(p) = config_path {
        if p.exists() {
            builder = builder.add_source(::config::File::from(p));
        }
    }
    builder = builder.add_source(
        ::config::Environment::with_prefix("PI_AWSM")
            .separator("__")
            .try_parsing(true),
    );
    let layered = builder
        .build()
        .map_err(|e| AppError::Config(e.to_string()))?;
    let merged: AppConfig = layered
        .try_deserialize()
        .map_err(|e| AppError::Config(e.to_string()))?;
    Ok(merged)
}

/// 配置合法性校验入口，应在启动时对 [`load_config`] 得到的配置调用。
///
/// # Arguments
/// * `cfg` - 待校验的 [`AppConfig`]。
///
/// # Errors
/// * [`AppError::Config`] - `audit_log_retention_days` 为 0 或 `log.level` 不在 `trace`/`debug`/`info`/`warn`/`error` 之一时返回。
pub fn validate_config(cfg: &AppConfig) -> Result<(), AppError> {
    if cfg.security.audit_log_retention_days == 0 {
        return Err(AppError::Config(
            "audit_log_retention_days 必须大于 0".to_string(),
        ));
    }
    let level = cfg.log.level.to_lowercase();
    if !["trace", "debug", "info", "warn", "error"].contains(&level.as_str()) {
        return Err(AppError::Config(format!(
            "无效的 log.level: {}",
            cfg.log.level
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_app_config_roundtrip() {
        let cfg = AppConfig::default();
        let j = serde_json::to_string(&cfg).unwrap();
        let _: AppConfig = serde_json::from_str(&j).unwrap();
    }

    #[test]
    fn security_config_default() {
        let _ = SecurityConfig::default();
    }

    #[test]
    fn validate_config_accepts_valid() {
        let mut cfg = AppConfig::default();
        cfg.log.level = "info".to_string();
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn validate_config_rejects_invalid_log_level() {
        let mut cfg = AppConfig::default();
        cfg.log.level = "invalid".to_string();
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_zero_audit_retention() {
        let mut cfg = AppConfig::default();
        cfg.security.audit_log_retention_days = 0;
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn load_config_none_path_returns_default_or_env() {
        let r = load_config(None);
        assert!(r.is_ok());
    }

    #[test]
    fn load_config_from_existing_file() {
        let dir = std::env::temp_dir().join("pi_awsm_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"[log]\nlevel = \"debug\"\n").unwrap();
        drop(f);
        let r = load_config(Some(path.as_path()));
        assert!(r.is_ok());
        let cfg = r.unwrap();
        assert!(validate_config(&cfg).is_ok());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn deserialize_security_config_uses_default_helpers() {
        let s = r#"{"security":{}}"#;
        let cfg: AppConfig = serde_json::from_str(s).unwrap();
        assert!(cfg.security.enable_audit_log);
        assert_eq!(cfg.security.audit_log_retention_days, 90);
    }
}
