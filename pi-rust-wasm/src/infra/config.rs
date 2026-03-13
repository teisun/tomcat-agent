//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `PI_WASM__`，分隔符 `__`）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::AppError;
use super::platform::normalize_path;

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

/// 日志配置：级别、是否写文件、滚动大小。日志目录由 [`resolve_log_dir`] 从 work_dir 推导。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file_enabled: bool,
    #[serde(default = "default_log_roll_size")]
    pub file_roll_size_mb: u64,
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_roll_size() -> u64 {
    10
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file_enabled: false,
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
    /// 显式 HTTP 代理 URL（如 `http://127.0.0.1:7890`）。设置后所有 LLM 请求经该代理；未设置时仍使用环境变量 HTTPS_PROXY/HTTP_PROXY（若存在）。
    #[serde(default)]
    pub proxy: Option<String>,
    /// 当对主 api_base 请求不通（连接失败、超时等）时，自动用该 URL 重试；示例 `https://api.chatanywhere.tech`。留空则关闭自动降级。
    #[serde(default)]
    pub api_base_fallback: Option<String>,
}

fn default_llm_provider() -> String {
    "openai".to_string()
}
fn default_llm_model() -> String {
    "gpt-5.2".to_string()
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
            proxy: None,
            api_base_fallback: None,
        }
    }
}

/// 存储配置：仅 work_dir。agent 系统子目录由 resolve 函数从 work_dir 推导。
/// 详见 openspec/specs/architecture/work-dir-and-data-layout.md。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StorageConfig {
    /// 工作根目录；默认 `~/.pi_wasm/`。支持 `~` 与相对路径。
    #[serde(default)]
    pub work_dir: Option<String>,
}

/// 插件配置：启动时自动加载的插件列表。插件目录由 [`resolve_plugins_dir`] 从 work_dir 推导。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub auto_load: Vec<String>,
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

/// Wasm 运行时配置（feature "wasmedge" 时使用）。
/// quickjs wasm 路径由 [`resolve_quickjs_path`] 从 work_dir 推导，回退到环境变量。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WasmConfig {}

/// 应用顶层配置，聚合 log / llm / storage / plugin / security / primitive / wasm 子配置。
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
    #[serde(default)]
    pub wasm: WasmConfig,
}

/// 从可选配置文件与环境变量加载并合并为 [`AppConfig`]。
///
/// 合并顺序：若提供且存在的配置文件先加载，再叠加环境变量 `PI_WASM__*`（`__` 表示嵌套）。未提供或不存在文件时仅用默认值与环境变量。
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
        ::config::Environment::with_prefix("PI_WASM")
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

/// 解析工作根目录：若配置了 `storage.work_dir` 则规范化后返回，否则默认 `~/.pi_wasm/`。
///
/// 详见 openspec/specs/architecture/work-dir-and-data-layout.md。
pub fn get_work_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref s) = cfg.storage.work_dir {
        let s = s.trim();
        if !s.is_empty() {
            return normalize_path(s);
        }
    }
    normalize_path("~/.pi_wasm/")
}

// ---------------------------------------------------------------------------
// resolve 函数：从 work_dir 按架构推导 agent 系统子目录
// ---------------------------------------------------------------------------

/// `work_dir/agents/default/sessions`
pub fn resolve_sessions_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("sessions"))
}

/// `work_dir/agents/default/plugins`
pub fn resolve_plugins_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("plugins"))
}

/// `work_dir/agents/default/tmp`
pub fn resolve_tmp_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("tmp"))
}

/// `work_dir/agents/default/logs`
pub fn resolve_log_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("logs"))
}

/// `work_dir/agents/default/audit` — 独立审计日志目录，专用 JSONL 存储。
pub fn resolve_audit_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("audit"))
}

/// `work_dir/agents/default/workspace`
pub fn resolve_workspace_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join("default")
        .join("workspace"))
}

/// 查找 quickjs wasm：`work_dir/wasm/wasmedge_quickjs.wasm` → 环境变量 `WASMEDGE_QUICKJS_PATH`。
pub fn resolve_quickjs_path(cfg: &AppConfig) -> Option<PathBuf> {
    if let Ok(work) = get_work_dir(cfg) {
        let p = work.join("wasm").join("wasmedge_quickjs.wasm");
        if p.exists() {
            return Some(p);
        }
    }
    std::env::var("WASMEDGE_QUICKJS_PATH")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

/// 启动时创建工作根目录及多 agent 子目录（当前仅 agentId=default）。若目录已存在则跳过。
///
/// 创建：`work_dir`、`work_dir/agents/default/sessions`、`plugins`、`tmp`、`logs`、`audit`、`workspace`，
/// 以及 `work_dir/wasm`（全局运行时引擎）、`work_dir/plugins`（全局共享插件）。
pub fn ensure_work_dir_structure(cfg: &AppConfig) -> Result<(), AppError> {
    let work = get_work_dir(cfg)?;
    let default_agent = work.join("agents").join("default");
    for sub in ["sessions", "plugins", "tmp", "logs", "audit", "workspace"] {
        std::fs::create_dir_all(default_agent.join(sub)).map_err(AppError::Io)?;
    }
    std::fs::create_dir_all(work.join("wasm")).map_err(AppError::Io)?;
    std::fs::create_dir_all(work.join("plugins")).map_err(AppError::Io)?;
    Ok(())
}

/// 配置合法性校验入口，应在启动时对 [`load_config`] 得到的配置调用。
///
/// # Arguments
/// * `cfg` - 待校验的 [`AppConfig`]。
///
/// # Errors
/// * [`AppError::Config`] - `audit_log_retention_days` 为 0、`log.level` 非法，或 `llm.proxy` 格式非法（非 `http://`/`https://` 开头）时返回。
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
    if let Some(ref proxy) = cfg.llm.proxy {
        let u = proxy.trim();
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err(AppError::Config(format!(
                "llm.proxy 须以 http:// 或 https:// 开头: {}",
                proxy
            )));
        }
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
    fn validate_config_rejects_invalid_proxy() {
        let mut cfg = AppConfig::default();
        cfg.log.level = "info".to_string();
        cfg.llm.proxy = Some("socks5://127.0.0.1:1080".to_string());
        assert!(validate_config(&cfg).is_err());
        cfg.llm.proxy = Some("http://127.0.0.1:7890".to_string());
        assert!(validate_config(&cfg).is_ok());
        cfg.llm.proxy = Some("https://proxy.example.com".to_string());
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn load_config_none_path_returns_default_or_env() {
        let r = load_config(None);
        assert!(r.is_ok());
    }

    #[test]
    fn load_config_from_existing_file() {
        let dir = std::env::temp_dir().join("pi_wasm_config_test");
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
    fn load_config_from_example_file() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let example_path = manifest_dir.join("config.toml.example");
        if !example_path.exists() {
            return;
        }
        // config 库按扩展名识别格式，.example 不被识别；将内容复制到临时 .toml 再加载以验证示例内容合法
        let content = std::fs::read_to_string(&example_path).unwrap();
        let dir = std::env::temp_dir().join("pi_wasm_example_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let temp_toml = dir.join("config.toml");
        std::fs::write(&temp_toml, &content).unwrap();
        let r = load_config(Some(temp_toml.as_path()));
        let _ = std::fs::remove_file(&temp_toml);
        let _ = std::fs::remove_dir(&dir);
        let cfg = r.unwrap_or_else(|e| {
            panic!("config.toml.example 内容应可被 load_config 反序列化: {}", e)
        });
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn deserialize_security_config_uses_default_helpers() {
        let s = r#"{"security":{}}"#;
        let cfg: AppConfig = serde_json::from_str(s).unwrap();
        assert!(cfg.security.enable_audit_log);
        assert_eq!(cfg.security.audit_log_retention_days, 90);
    }
}
