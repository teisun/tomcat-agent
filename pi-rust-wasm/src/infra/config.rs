//! # 配置模块 (Config)
//!
//! 配置结构体、加载与合并、合法性校验。多源合并顺序：默认值 → 配置文件 → 环境变量（前缀 `PI_WASM__`，分隔符 `__`）。
//! 内嵌资源管理：wasmedge_quickjs.wasm + assets/modules/ 在启动时自动释放到 work_dir。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::AppError;
use super::platform::normalize_path;

// ---------------------------------------------------------------------------
// Embedded resources & compile-time SHA-256
// ---------------------------------------------------------------------------

const EMBEDDED_QUICKJS_WASM: &[u8] = include_bytes!("../../assets/wasm/wasmedge_quickjs.wasm");
static EMBEDDED_MODULES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/modules");

const EMBEDDED_WASM_SHA256: &str = env!("EMBEDDED_WASM_SHA256");
const EMBEDDED_MODULES_SHA256: &str = env!("EMBEDDED_MODULES_SHA256");

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

/// 全局默认 LLM 模型 id（`LlmConfig` 默认值、`pi init` 首次写入与文档一致）。
/// 可通过 `pi.config.toml` 中 `[llm] default_model` 或环境变量 `PI_WASM__LLM__DEFAULT_MODEL` 覆盖（后者优先级更高，见 [`load_config`]）。
pub const DEFAULT_LLM_MODEL: &str = "gpt-5.2";

fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
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
    /// 工作根目录；默认 `~/.pi_/`。支持 `~` 与相对路径。
    #[serde(default)]
    pub work_dir: Option<String>,
}

/// Agent 配置：标识、身份目录、工作区目录。
/// `agent_dir` 和 `workspace` 均为可选，未配置时由 resolve 函数从 work_dir 推导。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    /// agent 标识，影响运行时数据目录和 session key 命名。
    #[serde(default = "default_agent_id")]
    pub id: String,
    /// agent 身份与凭据目录（auth-profiles.json、models.json 等）。
    /// 未配置时从 work_dir 推导为 `{work_dir}/agents/{id}/agent`。
    #[serde(default)]
    pub agent_dir: Option<String>,
    /// agent 工作区目录（AGENTS.md、SOUL.md 等设计态文件）。
    /// 未配置时从 work_dir 推导为 `{work_dir}/workspace-{id}`。
    #[serde(default)]
    pub workspace: Option<String>,
}

fn default_agent_id() -> String {
    "main".to_string()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: default_agent_id(),
            agent_dir: None,
            workspace: None,
        }
    }
}

/// 插件配置：启动时自动加载的插件列表。插件目录由 [`resolve_plugins_dir`] 从 work_dir 推导。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub auto_load: Vec<String>,
}

/// 全局工作区授权：额外可访问根路径列表，**所有 agent 共用**，与 `[agent]` 下的 `workspace`（设计态目录）不同。
///
/// 持久化在 `pi.config.toml` 的 `[workspace]` 表；由 `pi workspace add/list/remove` 或手编维护。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WorkspaceConfig {
    /// 每项为路径字符串（通常为绝对路径）；空串在解析时忽略。
    #[serde(default)]
    pub extra_roots: Vec<String>,
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
    /// `execute_bash` 在 Unix 上 `sh -c` 前可选 source 的 env 脚本路径；`None` 时默认 `$HOME/.wasmedge/env`。
    #[serde(default)]
    pub wasmedge_env_path: Option<String>,
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
            wasmedge_env_path: None,
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

/// 应用顶层配置，聚合 log / llm / storage / agent / plugin / security / primitive / wasm 子配置。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
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
/// **注意**：仓库与代码**不**设置任何 `PI_WASM__*` 默认值；若本机 shell 中设置了 `PI_WASM__LLM__DEFAULT_MODEL` 等变量，会覆盖配置文件中的同名字段（例如把模型固定为旧值）。集成测试会通过 `env_remove` 避免宿主环境泄漏。
///
/// # Arguments
/// * `config_path` - 配置文件路径，如 `Some(Path::new("pi.config.toml"))`；`None` 表示仅用默认与环境变量。
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

/// 仅从 TOML 配置文件解析 [`AppConfig`]（**不**合并环境变量），供需整表写回的场景（如 `pi workspace`）。
pub fn load_config_toml_file(path: &Path) -> Result<AppConfig, AppError> {
    let content = std::fs::read_to_string(path).map_err(AppError::Io)?;
    toml::from_str(&content).map_err(|e| AppError::Config(e.to_string()))
}

/// 校验并解析 `workspace.extra_roots`：忽略仅空白项；每项须可规范化为已存在的目录；规范路径去重。
pub fn resolve_extra_roots_paths(cfg: &AppConfig) -> Result<Vec<PathBuf>, AppError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in &cfg.workspace.extra_roots {
        let t = s.trim();
        if t.is_empty() {
            continue;
        }
        let p = normalize_path(t)?;
        let canon = std::fs::canonicalize(&p).map_err(|_| {
            AppError::Config(format!("workspace.extra_roots 路径无效或不可访问: {}", t))
        })?;
        if !canon.is_dir() {
            return Err(AppError::Config(format!(
                "workspace.extra_roots 不是目录: {}",
                canon.display()
            )));
        }
        if !seen.insert(canon.clone()) {
            return Err(AppError::Config(format!(
                "workspace.extra_roots 存在重复: {}",
                canon.display()
            )));
        }
        out.push(canon);
    }
    Ok(out)
}

/// 解析工作根目录：若配置了 `storage.work_dir` 则规范化后返回，否则默认 `~/.pi_/`。
///
/// 详见 openspec/specs/architecture/work-dir-and-data-layout.md。
pub fn get_work_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref s) = cfg.storage.work_dir {
        let s = s.trim();
        if !s.is_empty() {
            return normalize_path(s);
        }
    }
    normalize_path("~/.pi_/")
}

// ---------------------------------------------------------------------------
// resolve 函数：从 work_dir 按架构推导 agent 系统子目录
// sessions/logs/audit 始终从 work_dir/agents/{id}/ 独立推导，不经 agent_dir。
// agent_dir 和 workspace 支持配置覆盖。
// 参考 openclaw 的独立推导模式。
// ---------------------------------------------------------------------------

/// agent 身份与凭据目录。优先 `cfg.agent.agent_dir`，否则 `work_dir/agents/{id}/agent`。
pub fn resolve_agent_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref dir) = cfg.agent.agent_dir {
        let d = dir.trim();
        if !d.is_empty() {
            return normalize_path(d);
        }
    }
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join(&cfg.agent.id)
        .join("agent"))
}

/// `work_dir/agents/{id}/sessions` — 独立推导，不经 agent_dir。
pub fn resolve_sessions_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join(&cfg.agent.id)
        .join("sessions"))
}

/// `work_dir/plugins` — 全局共享插件目录。
pub fn resolve_plugins_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("plugins"))
}

/// `work_dir/agents/{id}/tmp` — 临时目录，保留签名兼容。
pub fn resolve_tmp_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join(&cfg.agent.id)
        .join("tmp"))
}

/// `work_dir/agents/{id}/logs` — 独立推导，不经 agent_dir。
pub fn resolve_log_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join(&cfg.agent.id)
        .join("logs"))
}

/// `work_dir/agents/{id}/audit` — 独立审计日志目录，专用 JSONL 存储。
pub fn resolve_audit_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?
        .join("agents")
        .join(&cfg.agent.id)
        .join("audit"))
}

/// agent 工作区目录。优先 `cfg.agent.workspace`，否则 `work_dir/workspace-{id}`。
pub fn resolve_workspace_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref ws) = cfg.agent.workspace {
        let w = ws.trim();
        if !w.is_empty() {
            return normalize_path(w);
        }
    }
    Ok(get_work_dir(cfg)?.join(format!("workspace-{}", cfg.agent.id)))
}

/// `work_dir/memory` — 向量检索索引目录。
pub fn resolve_memory_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("memory"))
}

/// `work_dir/assets` — 全局资源目录（含 wasm/ 和 modules/ 子目录）。
pub fn resolve_assets_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("assets"))
}

/// 查找 quickjs wasm：`work_dir/assets/wasm/wasmedge_quickjs.wasm`。
pub fn resolve_quickjs_path(cfg: &AppConfig) -> Option<PathBuf> {
    if let Ok(work) = get_work_dir(cfg) {
        let p = work
            .join("assets")
            .join("wasm")
            .join("wasmedge_quickjs.wasm");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 启动时创建完整新布局目录树。若目录已存在则跳过（幂等）。
///
/// 创建：`agent_dir`（可配置覆盖）、`work_dir/agents/{id}/sessions|logs|audit`、
/// `workspace-{id}`（可配置覆盖）、全局目录 `memory|credentials|media|subagents|plugins`、
/// 以及 `assets/wasm|modules`。
pub fn ensure_work_dir_structure(cfg: &AppConfig) -> Result<(), AppError> {
    let work = get_work_dir(cfg)?;
    let id = &cfg.agent.id;

    let agent_dir = resolve_agent_dir(cfg)?;
    std::fs::create_dir_all(&agent_dir).map_err(AppError::Io)?;

    let agent_base = work.join("agents").join(id);
    for sub in ["sessions", "logs", "audit"] {
        std::fs::create_dir_all(agent_base.join(sub)).map_err(AppError::Io)?;
    }

    let ws = resolve_workspace_dir(cfg)?;
    std::fs::create_dir_all(&ws).map_err(AppError::Io)?;

    for dir in ["memory", "credentials", "media", "subagents", "plugins"] {
        std::fs::create_dir_all(work.join(dir)).map_err(AppError::Io)?;
    }

    std::fs::create_dir_all(work.join("assets").join("wasm")).map_err(AppError::Io)?;
    std::fs::create_dir_all(work.join("assets").join("modules")).map_err(AppError::Io)?;
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
    resolve_extra_roots_paths(cfg).map(|_| ())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SHA-256 helpers
// ---------------------------------------------------------------------------

fn compute_file_sha256(path: &Path) -> Result<String, AppError> {
    let data = std::fs::read(path).map_err(AppError::Io)?;
    Ok(format!("{:x}", Sha256::digest(&data)))
}

fn compute_dir_sha256(dir: &Path) -> Result<String, AppError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_dir_hashes(dir, dir, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, file_hash) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update(file_hash.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_dir_hashes(
    base: &Path,
    current: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), AppError> {
    let entries = std::fs::read_dir(current).map_err(AppError::Io)?;
    for entry in entries {
        let entry = entry.map_err(AppError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            collect_dir_hashes(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let hash = compute_file_sha256(&path)?;
            out.push((rel, hash));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Atomic write + file locking (6.6)
// ---------------------------------------------------------------------------

fn write_atomic(target: &Path, content: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, content).map_err(AppError::Io)?;
    std::fs::rename(&tmp, target).or_else(|_| {
        std::fs::copy(&tmp, target).map_err(AppError::Io)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(())
    })
}

fn acquire_assets_lock(work_dir: &Path) -> Result<std::fs::File, AppError> {
    use fs2::FileExt;
    let lock_dir = work_dir.join("assets");
    std::fs::create_dir_all(&lock_dir).map_err(AppError::Io)?;
    let lock_path = lock_dir.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(AppError::Io)?;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(_) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                return Err(AppError::Config(
                    "资源锁超时（10s），请检查是否有其他 pi 进程卡住，或手动删除 ~/.pi_/assets/.lock"
                        .to_string(),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Embedded asset extraction (6.2, 6.3)
// ---------------------------------------------------------------------------

fn extract_wasm_if_needed(work_dir: &Path) -> Result<PathBuf, AppError> {
    let target = work_dir
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    if target.exists() && !EMBEDDED_WASM_SHA256.is_empty() {
        if let Ok(disk_sha) = compute_file_sha256(&target) {
            if disk_sha == EMBEDDED_WASM_SHA256 {
                return Ok(target);
            }
        }
    }
    std::fs::create_dir_all(target.parent().unwrap()).map_err(AppError::Io)?;
    write_atomic(&target, EMBEDDED_QUICKJS_WASM)?;
    Ok(target)
}

fn extract_modules_if_needed(work_dir: &Path) -> Result<PathBuf, AppError> {
    let target_dir = work_dir.join("assets").join("modules");
    if target_dir.is_dir() && !EMBEDDED_MODULES_SHA256.is_empty() {
        if let Ok(disk_sha) = compute_dir_sha256(&target_dir) {
            if disk_sha == EMBEDDED_MODULES_SHA256 {
                return Ok(target_dir);
            }
        }
    }
    extract_include_dir(&EMBEDDED_MODULES, &target_dir)?;
    Ok(target_dir)
}

fn extract_include_dir(dir: &Dir<'_>, base_target: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(base_target).map_err(AppError::Io)?;
    for file in dir.files() {
        let dest = base_target.join(file.path());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        std::fs::write(&dest, file.contents()).map_err(AppError::Io)?;
    }
    for sub in dir.dirs() {
        extract_include_dir(sub, base_target)?;
    }
    Ok(())
}

fn write_versions_json(work_dir: &Path) -> Result<(), AppError> {
    let versions = serde_json::json!({
        "wasm_sha256": EMBEDDED_WASM_SHA256,
        "modules_sha256": EMBEDDED_MODULES_SHA256,
        "extracted_at": chrono::Utc::now().to_rfc3339(),
    });
    let content =
        serde_json::to_string_pretty(&versions).map_err(|e| AppError::Config(e.to_string()))?;
    let path = work_dir.join("assets").join(".versions.json");
    write_atomic(&path, content.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unified entry point (6.4)
// ---------------------------------------------------------------------------

/// 确保内嵌资源已释放到 `work_dir/assets/`。
/// 在 `ensure_work_dir_structure` 之后、正式业务逻辑之前调用。
/// 通过文件锁保证多进程安全；SHA-256 比对避免重复写入。
pub fn ensure_embedded_assets(cfg: &AppConfig) -> Result<(), AppError> {
    let work_dir = get_work_dir(cfg)?;
    let _lock = acquire_assets_lock(&work_dir)?;
    extract_wasm_if_needed(&work_dir)?;
    extract_modules_if_needed(&work_dir)?;
    write_versions_json(&work_dir)?;
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
    fn validate_config_rejects_duplicate_extra_roots() {
        let dir = tempfile::tempdir().unwrap();
        let c = std::fs::canonicalize(dir.path()).unwrap();
        let s = c.to_string_lossy().into_owned();
        let mut cfg = AppConfig::default();
        cfg.log.level = "info".to_string();
        cfg.workspace.extra_roots = vec![s.clone(), s];
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_nonexistent_extra_root() {
        let mut cfg = AppConfig::default();
        cfg.log.level = "info".to_string();
        cfg.workspace
            .extra_roots
            .push("/nonexistent/pi_workspace_root_test_path".to_string());
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_accepts_extra_roots_when_dirs_exist() {
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.log.level = "info".to_string();
        cfg.workspace.extra_roots = vec![
            d1.path().to_str().unwrap().to_string(),
            d2.path().to_str().unwrap().to_string(),
        ];
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn resolve_extra_roots_skips_blank_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.workspace.extra_roots =
            vec!["  ".to_string(), dir.path().to_str().unwrap().to_string()];
        let roots = resolve_extra_roots_paths(&cfg).unwrap();
        assert_eq!(roots.len(), 1);
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
        let example_path = manifest_dir.join("pi.config.toml.example");
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
            panic!(
                "pi.config.toml.example 内容应可被 load_config 反序列化: {}",
                e
            )
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

    fn cfg_with_work_dir(dir: &std::path::Path) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(dir.to_string_lossy().to_string());
        cfg
    }

    #[test]
    fn compute_file_sha256_returns_hex() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.bin");
        std::fs::write(&file, b"hello").unwrap();
        let hash = compute_file_sha256(&file).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_file_sha256_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.bin");
        let f2 = dir.path().join("b.bin");
        std::fs::write(&f1, b"same content").unwrap();
        std::fs::write(&f2, b"same content").unwrap();
        assert_eq!(
            compute_file_sha256(&f1).unwrap(),
            compute_file_sha256(&f2).unwrap()
        );
    }

    #[test]
    fn compute_dir_sha256_deterministic() {
        let d1 = tempfile::tempdir().unwrap();
        std::fs::write(d1.path().join("a.txt"), b"aaa").unwrap();
        std::fs::write(d1.path().join("b.txt"), b"bbb").unwrap();

        let d2 = tempfile::tempdir().unwrap();
        std::fs::write(d2.path().join("a.txt"), b"aaa").unwrap();
        std::fs::write(d2.path().join("b.txt"), b"bbb").unwrap();

        assert_eq!(
            compute_dir_sha256(d1.path()).unwrap(),
            compute_dir_sha256(d2.path()).unwrap()
        );
    }

    #[test]
    fn compute_dir_sha256_changes_on_content_diff() {
        let d1 = tempfile::tempdir().unwrap();
        std::fs::write(d1.path().join("a.txt"), b"aaa").unwrap();

        let d2 = tempfile::tempdir().unwrap();
        std::fs::write(d2.path().join("a.txt"), b"bbb").unwrap();

        assert_ne!(
            compute_dir_sha256(d1.path()).unwrap(),
            compute_dir_sha256(d2.path()).unwrap()
        );
    }

    #[test]
    fn write_atomic_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub").join("output.bin");
        write_atomic(&target, b"data").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"data");
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.bin");
        std::fs::write(&target, b"old").unwrap();
        write_atomic(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn acquire_assets_lock_creates_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = acquire_assets_lock(dir.path()).unwrap();
        assert!(dir.path().join("assets").join(".lock").exists());
    }

    #[test]
    fn ensure_embedded_assets_extracts_wasm_and_modules() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_work_dir(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        ensure_embedded_assets(&cfg).unwrap();

        let wasm_path = dir
            .path()
            .join("assets")
            .join("wasm")
            .join("wasmedge_quickjs.wasm");
        assert!(wasm_path.exists(), "wasm file should be extracted");
        assert!(wasm_path.metadata().unwrap().len() > 0);

        let modules_dir = dir.path().join("assets").join("modules");
        assert!(modules_dir.is_dir(), "modules dir should be extracted");
        let count = std::fs::read_dir(&modules_dir).unwrap().count();
        assert!(count > 0, "modules dir should contain files");

        let versions = dir.path().join("assets").join(".versions.json");
        assert!(versions.exists(), ".versions.json should be created");
        let content = std::fs::read_to_string(&versions).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(!v["wasm_sha256"].as_str().unwrap_or("").is_empty());
        assert!(!v["modules_sha256"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn ensure_embedded_assets_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_work_dir(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        ensure_embedded_assets(&cfg).unwrap();
        ensure_embedded_assets(&cfg).unwrap();

        let wasm_path = dir
            .path()
            .join("assets")
            .join("wasm")
            .join("wasmedge_quickjs.wasm");
        assert!(wasm_path.exists());
    }

    #[test]
    fn ensure_embedded_assets_upgrades_on_sha_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_work_dir(dir.path());
        ensure_work_dir_structure(&cfg).unwrap();
        ensure_embedded_assets(&cfg).unwrap();

        let wasm_path = dir
            .path()
            .join("assets")
            .join("wasm")
            .join("wasmedge_quickjs.wasm");
        let original = std::fs::read(&wasm_path).unwrap();

        std::fs::write(&wasm_path, b"tampered content").unwrap();
        assert_ne!(std::fs::read(&wasm_path).unwrap(), original);

        ensure_embedded_assets(&cfg).unwrap();
        assert_eq!(
            std::fs::read(&wasm_path).unwrap(),
            original,
            "tampered wasm should be overwritten with embedded version"
        );
    }

    #[test]
    fn extract_wasm_skips_when_sha_matches() {
        let dir = tempfile::tempdir().unwrap();
        extract_wasm_if_needed(dir.path()).unwrap();

        let wasm_path = dir
            .path()
            .join("assets")
            .join("wasm")
            .join("wasmedge_quickjs.wasm");
        let mtime_before = std::fs::metadata(&wasm_path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let result = extract_wasm_if_needed(dir.path()).unwrap();
        assert_eq!(result, wasm_path);

        let mtime_after = std::fs::metadata(&wasm_path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "file should not be rewritten when SHA matches"
        );
    }

    #[test]
    fn embedded_sha256_constants_are_nonempty() {
        assert!(
            !EMBEDDED_WASM_SHA256.is_empty(),
            "compile-time wasm SHA-256 must be set"
        );
        assert!(
            !EMBEDDED_MODULES_SHA256.is_empty(),
            "compile-time modules SHA-256 must be set"
        );
        assert_eq!(EMBEDDED_WASM_SHA256.len(), 64);
        assert_eq!(EMBEDDED_MODULES_SHA256.len(), 64);
    }

    #[test]
    fn concurrent_lock_does_not_deadlock() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();

        for _ in 0..2 {
            let p = Arc::clone(&path);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                let _lock = acquire_assets_lock(&p).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }
}
