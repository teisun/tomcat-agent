//! 配置加载、校验与路径解析函数。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::brand::{DEFAULT_WORK_DIR, ENV_PREFIX};
use super::super::error::AppError;
use super::super::platform::normalize_path;
use super::types::AppConfig;

/// 从可选配置文件与环境变量加载并合并为 [`AppConfig`]。
///
/// 合并顺序：若提供且存在的配置文件先加载，再叠加环境变量 `TOMCAT__*`（`__` 表示嵌套）。未提供或不存在文件时仅用默认值与环境变量。
/// **注意**：仓库与代码**不**设置任何 `TOMCAT__*` 默认值；若本机 shell 中设置了 `TOMCAT__LLM__DEFAULT_MODEL` 等变量，会覆盖配置文件中的同名字段（例如把模型固定为旧值）。集成测试会通过 `env_remove` 避免宿主环境泄漏。
///
/// # Arguments
/// * `config_path` - 配置文件路径，如 `Some(Path::new("tomcat.config.toml"))`；`None` 表示仅用默认与环境变量。
///
/// # Returns
/// 合并后的 [`AppConfig`]，可直接用于 [`validate_config`] 校验。
///
/// # Errors
/// * [`AppError::Config`] - 配置文件解析失败或反序列化到 [`AppConfig`] 失败时返回。
pub fn load_config(config_path: Option<&std::path::Path>) -> Result<AppConfig, AppError> {
    // 安全护栏（plan §5）：敏感 env vars 必须由 TOML 主导，不允许 env 覆盖。
    // 若 TOML 文件存在且声明了对应 key，则在 layered.build 之前 unset env，
    // 防止 shell 中误设的 `TOMCAT__SECURITY__*` / `TOMCAT__LLM__API_KEY*`
    // 静默覆盖磁盘配置造成提权。
    sanitize_sensitive_env(config_path);

    let mut builder = ::config::Config::builder();
    if let Some(p) = config_path {
        if p.exists() {
            reject_legacy_whitelist_keys(p)?;
            builder = builder.add_source(::config::File::from(p));
        }
    }
    builder = builder.add_source(
        ::config::Environment::with_prefix(ENV_PREFIX)
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

/// 把 TOML 文件中已声明的敏感 key 对应的 env vars 从进程环境中移除，避免
/// `TOMCAT__SECURITY__*` 等 env 静默覆盖磁盘 TOML 引发的提权风险。
///
/// 当前覆盖：
/// - `TOMCAT__SECURITY__*` 全部
/// - `TOMCAT__LLM__API_KEY*`
/// - `TOMCAT__PRIMITIVE__PATH_RULES*`
/// - `TOMCAT__PRIMITIVE__BASH_FORBIDDEN*`
/// - `TOMCAT__PRIMITIVE__BASH_APPROVAL_REQUIRED*`
fn sanitize_sensitive_env(config_path: Option<&std::path::Path>) {
    if config_path.is_none_or(|p| !p.exists()) {
        return;
    }
    let blocked_prefixes = [
        "TOMCAT__SECURITY__",
        "TOMCAT__LLM__API_KEY",
        "TOMCAT__PRIMITIVE__PATH_RULES",
        "TOMCAT__PRIMITIVE__BASH_FORBIDDEN",
        "TOMCAT__PRIMITIVE__BASH_APPROVAL_REQUIRED",
    ];
    let to_remove: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if blocked_prefixes.iter().any(|p| k.starts_with(p)) {
                Some(k)
            } else {
                None
            }
        })
        .collect();
    for k in &to_remove {
        // SAFETY: env_remove 在 std::env 中是安全函数；多线程下 Rust 1.85+ 标记为 unsafe
        // 但本调用发生在 load_config 启动早期单线程上下文。
        std::env::remove_var(k);
        tracing::warn!(target: "config", "已 unset 敏感 env var: {}", k);
    }
}

/// 仅从 TOML 配置文件解析 [`AppConfig`]（**不**合并环境变量），供需整表写回的场景（如 `tomcat workspace`）。
pub fn load_config_toml_file(path: &Path) -> Result<AppConfig, AppError> {
    reject_legacy_whitelist_keys(path)?;
    let content = std::fs::read_to_string(path).map_err(AppError::Io)?;
    toml::from_str(&content).map_err(|e| AppError::Config(e.to_string()))
}

fn reject_legacy_whitelist_keys(path: &Path) -> Result<(), AppError> {
    let content = std::fs::read_to_string(path).map_err(AppError::Io)?;
    let Ok(value) = content.parse::<toml::Value>() else {
        return Ok(());
    };
    let Some(primitive) = value.get("primitive").and_then(|v| v.as_table()) else {
        return Ok(());
    };
    let legacy = [
        (
            "path_whitelist",
            "workspace.workspace_roots（持久允许根）或 primitive.path_rules（deny/readonly）",
        ),
        (
            "bash_whitelist",
            "primitive.bash_forbidden / primitive.bash_approval_required 的显式规则",
        ),
        (
            "auto_confirm_whitelist",
            "删除该字段；现由 primitive.auto_confirm 控制（默认 true）",
        ),
    ];
    let hits = legacy
        .iter()
        .filter(|(key, _)| primitive.contains_key(*key))
        .map(|(key, target)| format!("primitive.{key} -> {target}"))
        .collect::<Vec<_>>();
    if hits.is_empty() {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "配置包含已删除的 legacy whitelist 字段：{}。请按提示迁移后重试。",
        hits.join("; ")
    )))
}

/// 校验并解析 `workspace.workspace_roots`：忽略仅空白项；每项须可规范化为已存在的目录；规范路径去重。
pub fn resolve_workspace_roots_paths(cfg: &AppConfig) -> Result<Vec<PathBuf>, AppError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in &cfg.workspace.workspace_roots {
        let t = s.trim();
        if t.is_empty() {
            continue;
        }
        let p = normalize_path(t)?;
        let canon = std::fs::canonicalize(&p).map_err(|_| {
            AppError::Config(format!(
                "workspace.workspace_roots 路径无效或不可访问: {}",
                t
            ))
        })?;
        if !canon.is_dir() {
            return Err(AppError::Config(format!(
                "workspace.workspace_roots 不是目录: {}",
                canon.display()
            )));
        }
        if !seen.insert(canon.clone()) {
            return Err(AppError::Config(format!(
                "workspace.workspace_roots 存在重复: {}",
                canon.display()
            )));
        }
        out.push(canon);
    }
    push_builtin_workspace_root(&mut seen, &mut out)?;
    Ok(out)
}

/// 把内置可写根并入 `workspace_roots` 解析结果（当前为 `~/.tomcat/temp/`）。
fn push_builtin_workspace_root(
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    let temp = resolve_dot_tomcat_temp_dir()?;
    std::fs::create_dir_all(&temp).map_err(AppError::Io)?;
    let canon = std::fs::canonicalize(&temp).unwrap_or(temp);
    if !canon.is_dir() {
        return Err(AppError::Config(format!(
            "内置 workspace 根不是目录: {}",
            canon.display()
        )));
    }
    if seen.insert(canon.clone()) {
        out.push(canon);
    }
    Ok(())
}

/// 解析工作根目录：若配置了 `storage.work_dir` 则规范化后返回，否则默认 `~/.tomcat/`。
///
/// 详见 docs/architecture/work-dir-and-data-layout.md。
pub fn get_work_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref s) = cfg.storage.work_dir {
        let s = s.trim();
        if !s.is_empty() {
            return normalize_path(s);
        }
    }
    normalize_path(DEFAULT_WORK_DIR)
}

/// 解析计划目录：固定为默认工作根 `DEFAULT_WORK_DIR/plans/`。
///
/// 计划文件按架构约定始终落在默认 `~/.tomcat/plans/`，**不**随 `storage.work_dir`
/// 覆盖而漂移，避免与 agent 运行态轨迹目录或用户项目工作区混在一起。
pub fn resolve_plans_dir() -> Result<PathBuf, AppError> {
    Ok(normalize_path(DEFAULT_WORK_DIR)?.join("plans"))
}

/// 解析临时/测试工作目录：固定为 `~/.tomcat/temp/`。
///
/// 与 [`resolve_plans_dir`] 相同，始终落在默认 `~/.tomcat/` 下，不随 `storage.work_dir`
/// 覆盖漂移；默认并入 [`resolve_workspace_roots_paths`] 供工具读写授权。
pub fn resolve_dot_tomcat_temp_dir() -> Result<PathBuf, AppError> {
    Ok(normalize_path(DEFAULT_WORK_DIR)?.join("temp"))
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

/// `work_dir/agents/{id}` — Agent 运行态轨迹目录。
pub fn resolve_agent_trail_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("agents").join(&cfg.agent.id))
}

/// `work_dir/agents/{id}/sessions` — 独立推导，不经 agent_dir。
pub fn resolve_sessions_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_agent_trail_dir(cfg)?.join("sessions"))
}

/// `work_dir/plugins` — 全局共享插件目录。
pub fn resolve_plugins_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("plugins"))
}

/// `work_dir/agents/{id}/tmp` — 临时目录，保留签名兼容。
pub fn resolve_tmp_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_agent_trail_dir(cfg)?.join("tmp"))
}

/// `work_dir/agents/{id}/logs` — 独立推导，不经 agent_dir。
pub fn resolve_log_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_agent_trail_dir(cfg)?.join("logs"))
}

/// `work_dir/agents/{id}/audit` — 独立审计日志目录，专用 JSONL 存储。
pub fn resolve_audit_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_agent_trail_dir(cfg)?.join("audit"))
}

/// `work_dir/agents/{id}/checkpoints` — 影子 git checkpoint 根目录。
pub fn resolve_checkpoints_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(resolve_agent_trail_dir(cfg)?.join("checkpoints"))
}

/// agent 设计态目录。优先 `cfg.agent.workspace`，否则 `work_dir/workspace-{id}`。
pub fn resolve_agent_definition_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(ref ws) = cfg.agent.workspace {
        let w = ws.trim();
        if !w.is_empty() {
            return normalize_path(w);
        }
    }
    Ok(get_work_dir(cfg)?.join(format!("workspace-{}", cfg.agent.id)))
}

/// agent 设计态工作区目录。保留旧函数名作为兼容 wrapper，新代码优先使用
/// [`resolve_agent_definition_dir`]。
pub fn resolve_workspace_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    resolve_agent_definition_dir(cfg)
}

/// `work_dir/memory` — 向量检索索引目录。
pub fn resolve_memory_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("memory"))
}

/// `work_dir/assets` — 全局资产目录。
pub fn resolve_assets_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("assets"))
}

/// 启动时创建完整新布局目录树。若目录已存在则跳过（幂等）。
///
/// 创建：`agent_dir`（可配置覆盖）、`work_dir/agents/{id}/sessions|logs|audit|todos`、
/// `workspace-{id}`（可配置覆盖）、全局目录 `memory|credentials|media|subagents|plugins`、
/// 以及 `assets/`。
pub fn ensure_work_dir_structure(cfg: &AppConfig) -> Result<(), AppError> {
    let work = get_work_dir(cfg)?;
    let agent_dir = resolve_agent_dir(cfg)?;
    std::fs::create_dir_all(&agent_dir).map_err(AppError::Io)?;

    let agent_base = resolve_agent_trail_dir(cfg)?;
    for sub in [
        "sessions",
        "logs",
        "audit",
        "todos",
        "tmp",
        "skills",
        "packages",
        "tool-results",
        "checkpoints",
    ] {
        std::fs::create_dir_all(agent_base.join(sub)).map_err(AppError::Io)?;
    }

    let ws = resolve_agent_definition_dir(cfg)?;
    std::fs::create_dir_all(&ws).map_err(AppError::Io)?;

    for dir in [
        "memory",
        "credentials",
        "media",
        "subagents",
        "plugins",
        "packages",
        "skills",
    ] {
        std::fs::create_dir_all(work.join(dir)).map_err(AppError::Io)?;
    }

    std::fs::create_dir_all(work.join("assets")).map_err(AppError::Io)?;
    std::fs::create_dir_all(resolve_dot_tomcat_temp_dir()?).map_err(AppError::Io)?;
    std::fs::create_dir_all(resolve_plans_dir()?).map_err(AppError::Io)?;
    Ok(())
}

/// 配置合法性校验入口，应在启动时对 [`load_config`] 得到的配置调用。
///
/// # Arguments
/// * `cfg` - 待校验的 [`AppConfig`]。
///
/// # Errors
/// * [`AppError::Config`] - `audit_log_retention_days` 为 0、`log.level` 非法、`llm.proxy` 格式非法（非 `http://`/`https://` 开头），或 `llm.files.expires_after_seconds` 越界（非 0 且不在 [3600, 2592000]）时返回。
pub fn validate_config(cfg: &AppConfig) -> Result<(), AppError> {
    if cfg.security.audit_log_retention_days == 0 {
        return Err(AppError::Config(
            "audit_log_retention_days 必须大于 0".to_string(),
        ));
    }
    if cfg.checkpoint.retention_max == 0 {
        return Err(AppError::Config(
            "checkpoint.retention_max 必须大于 0".to_string(),
        ));
    }
    if cfg.checkpoint.retention_days == 0 {
        return Err(AppError::Config(
            "checkpoint.retention_days 必须大于 0".to_string(),
        ));
    }
    let session_mode = cfg.session.default_mode.trim().to_ascii_lowercase();
    if !["code", "claw"].contains(&session_mode.as_str()) {
        return Err(AppError::Config(format!(
            "session.default_mode 非法: {}（允许 code / claw）",
            cfg.session.default_mode
        )));
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
    let stream_idle_timeout = cfg.llm.stream_timeout_sec;
    if stream_idle_timeout != 0 && !(5..=3_600).contains(&stream_idle_timeout) {
        return Err(AppError::Config(format!(
            "llm.stream_timeout_sec 非法: {}（允许 0 或 [5, 3600]）",
            stream_idle_timeout
        )));
    }
    let non_stream_stale_timeout = cfg.llm.non_stream_stale_timeout_sec;
    if non_stream_stale_timeout != 0 && !(5..=3_600).contains(&non_stream_stale_timeout) {
        return Err(AppError::Config(format!(
            "llm.non_stream_stale_timeout_sec 非法: {}（允许 0 或 [5, 3600]）",
            non_stream_stale_timeout
        )));
    }
    let http_read_timeout = cfg.llm.http_read_timeout_sec;
    if http_read_timeout != 0 && !(5..=3_600).contains(&http_read_timeout) {
        return Err(AppError::Config(format!(
            "llm.http_read_timeout_sec 非法: {}（允许 0 或 [5, 3600]）",
            http_read_timeout
        )));
    }
    let expires = cfg.llm.files.expires_after_seconds;
    if expires != 0 && !(3_600..=2_592_000).contains(&expires) {
        return Err(AppError::Config(format!(
            "llm.files.expires_after_seconds 非法: {}（允许 0 或 [3600, 2592000]）",
            expires
        )));
    }
    if cfg.skills.prompt_budget_pct > 100 {
        return Err(AppError::Config(format!(
            "skills.prompt_budget_pct 非法: {}（允许 [0, 100]）",
            cfg.skills.prompt_budget_pct
        )));
    }
    if cfg.skills.prompt_budget_floor_chars == 0 {
        return Err(AppError::Config(
            "skills.prompt_budget_floor_chars 必须大于 0".to_string(),
        ));
    }
    if cfg.skills.max_description_chars == 0 {
        return Err(AppError::Config(
            "skills.max_description_chars 必须大于 0".to_string(),
        ));
    }
    if cfg.skills.max_skills == 0 {
        return Err(AppError::Config("skills.max_skills 必须大于 0".to_string()));
    }
    let web_search_backend = cfg.tools.web_search.backend.trim().to_ascii_lowercase();
    if !["auto", "openai", "tavily", "brave", "serper"].contains(&web_search_backend.as_str()) {
        return Err(AppError::Config(format!(
            "tools.web_search.backend 非法: {}",
            cfg.tools.web_search.backend
        )));
    }
    if !(1..=20).contains(&cfg.tools.web_search.count) {
        return Err(AppError::Config(format!(
            "tools.web_search.count 非法: {}（允许 [1, 20]）",
            cfg.tools.web_search.count
        )));
    }
    if cfg.tools.web_search.cache_capacity == 0 {
        return Err(AppError::Config(
            "tools.web_search.cache_capacity 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_search.cache_ttl_secs == 0 {
        return Err(AppError::Config(
            "tools.web_search.cache_ttl_secs 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_search.timeout_ms == 0 {
        return Err(AppError::Config(
            "tools.web_search.timeout_ms 必须大于 0".to_string(),
        ));
    }
    for (label, value) in [
        (
            "tools.web_search.tavily_base_url",
            cfg.tools.web_search.tavily_base_url.trim(),
        ),
        (
            "tools.web_search.brave_base_url",
            cfg.tools.web_search.brave_base_url.trim(),
        ),
        (
            "tools.web_search.serper_base_url",
            cfg.tools.web_search.serper_base_url.trim(),
        ),
    ] {
        if !value.starts_with("http://") && !value.starts_with("https://") {
            return Err(AppError::Config(format!(
                "{label} 须以 http:// 或 https:// 开头"
            )));
        }
    }
    if cfg.tools.web_fetch.max_redirects == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.max_redirects 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.fetch_timeout_ms == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.fetch_timeout_ms 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.max_http_content_bytes == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.max_http_content_bytes 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.max_markdown_chars == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.max_markdown_chars 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.markdown_head_chars == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.markdown_head_chars 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.cache_ttl_secs == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.cache_ttl_secs 必须大于 0".to_string(),
        ));
    }
    if cfg.tools.web_fetch.cache_capacity_bytes == 0 {
        return Err(AppError::Config(
            "tools.web_fetch.cache_capacity_bytes 必须大于 0".to_string(),
        ));
    }
    resolve_workspace_roots_paths(cfg).map(|_| ())?;
    Ok(())
}
