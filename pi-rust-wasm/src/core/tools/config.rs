//! # `config_get` / `config_set` LLM 工具实现（plan §6）
//!
//! 让 Agent 能够"用自然语言改配置"，但同时通过键级双向白名单 + 硬黑名单
//! 防止 Agent 自我提权或泄漏敏感信息：
//!
//! - **读路径**：`CONFIG_READ_ALLOWLIST` ∪ 否定 `CONFIG_HARDCODED_READ_DENY`。
//! - **写路径**：`CONFIG_WRITE_ALLOWLIST` ∪ 否定 `CONFIG_HARDCODED_WRITE_DENY`。
//! - **数组语义**：单元素追加 only；删除 / 整数组替换返回错误并引导 `pi config edit`。
//! - **二次 confirm**：每次 `config_set` 都强制走 `UserConfirmationProvider::confirm`
//!   并展示 unified diff，用户拒绝直接返回 `applied=false`。
//!
//! ## 与 CLI 的职责区分
//!
//! `pi config get/set/edit` 是用户特权通道，**不**受这里的白名单约束；本模块仅约束
//! LLM 通过 `config_get` / `config_set` 工具的访问。两条通道共享底层 `append_*_to_disk`
//! 落盘函数 + `with_config_lock` 文件锁，保证一致性。
//!
//! ## 测试位置
//!
//! 集中在本模块的 `tests`：
//! - 白名单 / hardcoded deny 矩阵
//! - 数组单元素追加正反案例
//! - confirm AllowOnce / Deny / AllowAndPersistRoot 分支

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::agent_loop::ConfigBackend;
use crate::core::permission::{PathRule, PermissionDecision, PermissionGate};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::core::tools::primitive::UserConfirmationProvider;
use crate::infra::config::{
    append_path_rule_to_disk, append_workspace_entry_to_disk, append_workspace_root_to_disk,
    load_config, with_config_lock, AppConfig, WorkspaceEntry,
};
use crate::infra::error::AppError;
use crate::infra::platform::{normalize_path, write_file_atomic};

/// 工具触发的 plugin_id 标签——与 `tool_exec::AGENT_PLUGIN_ID` 区分，便于审计追溯
/// "这次 confirm 来自 config_set 工具"。
const CONFIG_TOOL_PLUGIN_ID: &str = "__config_tool__";

// ─── 键级白名单 / 硬黑名单（plan §6.2） ───────────────────────────────────────

/// 读白名单：精确匹配（点号路径）；只有精确命中才允许读。
const CONFIG_READ_ALLOWLIST: &[&str] = &[
    "workspace",
    "workspace.workspace_roots",
    "workspace.entries",
    "agent.id",
    "agent.workspace",
    "agent.agent_dir",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
    "primitive.auto_confirm",
    "llm.default_model",
    "llm.provider",
    "context.context_window",
    "context.max_output_tokens",
    "context.compaction_turns",
    "context.keep_recent_turns",
    "context.compaction_model",
    "context.compaction_max_tokens",
    "log.level",
    "preflight.auto_install_search_tools",
];

/// 读硬黑名单：通配前缀，优先级高于 [`CONFIG_READ_ALLOWLIST`]，即使误列也会被拦。
const CONFIG_HARDCODED_READ_DENY: &[&str] = &[
    "llm.api_key_env",
    "llm.api_key",
    "llm.api_base",
    "llm.api_base_fallback",
    "llm.proxy",
    "security.",
    "storage.",
];

/// 写白名单：精确匹配；其余字段一律拒绝。所有数组字段语义为「单元素追加」。
const CONFIG_WRITE_ALLOWLIST: &[&str] = &[
    "workspace.workspace_roots",
    "workspace.entries",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
    "llm.default_model",
    "log.level",
    "preflight.auto_install_search_tools",
    "context.compaction_turns",
    "context.keep_recent_turns",
    "context.compaction_max_tokens",
];

/// 写硬黑名单：优先级高于 [`CONFIG_WRITE_ALLOWLIST`]；任何敏感 / 自我提权字段一律拒绝。
const CONFIG_HARDCODED_WRITE_DENY: &[&str] = &[
    "llm.",
    "security.",
    "storage.",
    "agent.id",
    "agent.workspace",
    "agent.agent_dir",
    "primitive.auto_confirm",
    "primitive.path_whitelist",
    "primitive.bash_whitelist",
    "primitive.auto_confirm_whitelist",
];

/// 数组字段集合（单元素追加语义）；其余白名单内字段视为标量替换。
const ARRAY_FIELDS: &[&str] = &[
    "workspace.workspace_roots",
    "workspace.entries",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
];

fn matches_prefix_list(key: &str, list: &[&str]) -> bool {
    list.iter().any(|p| {
        if let Some(stripped) = p.strip_suffix('.') {
            key.starts_with(stripped)
                && key
                    .get(stripped.len()..)
                    .is_some_and(|r| r.starts_with('.'))
                || key == stripped
        } else if p.ends_with('.') {
            key.starts_with(p)
        } else {
            // 也作为前缀（用于 `llm.` 等以 dot 结尾的 hardcoded deny）
            // 这里的入口就是 `llm.`、`security.` 等，已在上面分支处理；
            // 兜底退化为完全相等（保持与精确匹配一致）。
            key == *p
        }
    })
}

fn matches_exact_list(key: &str, list: &[&str]) -> bool {
    list.contains(&key)
}

/// 是否允许通过 `config_get` 读取 `key`。
pub fn is_readable(key: &str) -> bool {
    if matches_prefix_list(key, CONFIG_HARDCODED_READ_DENY) {
        return false;
    }
    matches_exact_list(key, CONFIG_READ_ALLOWLIST)
}

/// 是否允许通过 `config_set` 写入 `key`。
pub fn is_writable(key: &str) -> bool {
    if matches_prefix_list(key, CONFIG_HARDCODED_WRITE_DENY) {
        return false;
    }
    matches_exact_list(key, CONFIG_WRITE_ALLOWLIST)
}

/// 是否为「单元素追加」语义的数组字段。
pub fn is_array_field(key: &str) -> bool {
    ARRAY_FIELDS.contains(&key)
}

// ─── ConfigToolContext ────────────────────────────────────────────────────────

/// `config_get` / `config_set` 工具运行所需的上下文。
///
/// chat 启动时由 `ChatContext::from_config` 构造一次；之后每次工具调用都重新
/// 从 `config_path` 读取最新配置（避免内存中的 `AppConfig` 与磁盘漂移）。
pub struct ConfigToolContext {
    /// `pi.config.toml` 绝对路径；写盘 / 读盘均经由 `with_config_lock` 串行化。
    pub config_path: PathBuf,
    /// 二次 confirm 提供方；与 primitive 层共享同一 `CliConfirmation` 实例。
    pub confirmation: Arc<dyn UserConfirmationProvider>,
    /// 当前 chat 共享的权限 gate；用于阻止 config_set 绕过 deny，并让 path_rules 热生效。
    pub gate: Option<Arc<dyn PermissionGate>>,
}

impl ConfigToolContext {
    pub fn new(config_path: PathBuf, confirmation: Arc<dyn UserConfirmationProvider>) -> Self {
        Self {
            config_path,
            confirmation,
            gate: None,
        }
    }

    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }
}

// ─── ConfigBackend impl（plan §6.5 + 6.6） ────────────────────────────────────

/// `core::agent_loop` 注入用的 `ConfigBackend` 适配器。
///
/// 与 [`ConfigToolContext`] 1:1：把 `config_path` + `confirmation` 包装为
/// trait 对象，方便 `AgentLoop::with_config_backend(Arc::new(ChatConfigBackend{...}))`。
/// `config_get` 每次都重新 `load_config`，避免内存视图与磁盘漂移；写盘走
/// `with_config_lock` 串行化。
pub struct ChatConfigBackend {
    pub ctx: ConfigToolContext,
}

#[async_trait]
impl ConfigBackend for ChatConfigBackend {
    async fn config_get(&self, key: &str) -> Result<serde_json::Value, AppError> {
        let cfg = load_config(Some(&self.ctx.config_path))?;
        config_get_impl(key, &cfg)
    }

    async fn config_set(&self, key: &str, value: &str) -> Result<(bool, String), AppError> {
        let outcome = config_set_impl(key, value, &self.ctx).await?;
        Ok((outcome.applied, outcome.message))
    }
}

// ─── config_get_impl ─────────────────────────────────────────────────────────

/// 处理 `config_get` 工具调用。
///
/// 返回 JSON 形式的当前值；若 key 不存在但白名单允许，返回 `"not_set"` 字符串。
pub fn config_get_impl(key: &str, cfg: &AppConfig) -> Result<serde_json::Value, AppError> {
    if !is_readable(key) {
        return Err(AppError::Permission(format!(
            "配置项 '{}' 不在读白名单内或被硬黑名单拦截",
            key
        )));
    }
    let toml_val = toml::Value::try_from(cfg)
        .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
    match resolve_toml_path(&toml_val, key) {
        Some(v) => toml_to_json(v).map_err(|e| AppError::Config(format!("转换 JSON 失败: {}", e))),
        None => Ok(serde_json::Value::String("not_set".to_string())),
    }
}

fn resolve_toml_path<'a>(val: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = val;
    for seg in key.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

fn toml_to_json(v: &toml::Value) -> Result<serde_json::Value, String> {
    let s = serde_json::to_value(v).map_err(|e| e.to_string())?;
    Ok(s)
}

// ─── config_set_impl ─────────────────────────────────────────────────────────

/// `config_set` 工具的返回载荷（序列化为 JSON 给 LLM）。
#[derive(Debug, Clone)]
pub struct ConfigSetOutcome {
    pub applied: bool,
    pub message: String,
}

/// 处理 `config_set` 工具调用。
///
/// 流程（plan §6.3 / §6.4）：
/// 1. 写白名单 + 硬黑名单守卫
/// 2. 数组字段：解析单元素 → confirm → `append_*_to_disk` 落盘
/// 3. 标量字段：解析新值 → confirm（diff 预览） → `with_config_lock` 替换 + 写盘
pub async fn config_set_impl(
    key: &str,
    value: &str,
    ctx: &ConfigToolContext,
) -> Result<ConfigSetOutcome, AppError> {
    if !is_writable(key) {
        return Err(AppError::Permission(format!(
            "配置项 '{}' 不在写白名单内或被硬黑名单拦截；如需手动修改请使用 `pi config edit`",
            key
        )));
    }

    if is_array_field(key) {
        return handle_array_append(key, value, ctx).await;
    }

    handle_scalar_replace(key, value, ctx).await
}

async fn handle_array_append(
    key: &str,
    value: &str,
    ctx: &ConfigToolContext,
) -> Result<ConfigSetOutcome, AppError> {
    let preview = format!(
        "配置变更确认\n  字段: {}\n  类型: 追加 1 项\n  新值: {}\n",
        key, value
    );

    match key {
        "workspace.workspace_roots" => {
            let abs = parse_string_element(value)?;
            let normalized =
                normalize_path(&abs).map_err(|e| AppError::Config(format!("路径无效: {}", e)))?;
            ensure_path_not_denied(ctx, &normalized)?;
            let abs_path = normalized.to_string_lossy().to_string();
            let suggested = Some(normalized.clone());
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    suggested,
                )
                .await?;
            if !decision.is_allow() {
                return Ok(ConfigSetOutcome {
                    applied: false,
                    message: "user_denied".into(),
                });
            }
            append_workspace_root_to_disk(&ctx.config_path, abs_path)?;
            Ok(ConfigSetOutcome {
                applied: true,
                message: format!("已更新配置：以后允许访问 {}", value),
            })
        }
        "workspace.entries" => {
            let entry: WorkspaceEntry = parse_json_element(value, "WorkspaceEntry")?;
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    None,
                )
                .await?;
            if !decision.is_allow() {
                return Ok(ConfigSetOutcome {
                    applied: false,
                    message: "user_denied".into(),
                });
            }
            append_workspace_entry_to_disk(&ctx.config_path, entry)?;
            Ok(ConfigSetOutcome {
                applied: true,
                message: format!("已追加 workspace.entries: {}", value),
            })
        }
        "primitive.path_rules" => {
            let rule: PathRule = parse_json_element(value, "PathRule")?;
            let rule_for_runtime = rule.clone();
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    None,
                )
                .await?;
            if !decision.is_allow() {
                return Ok(ConfigSetOutcome {
                    applied: false,
                    message: "user_denied".into(),
                });
            }
            append_path_rule_to_disk(&ctx.config_path, rule)?;
            if let Some(gate) = ctx.gate.as_ref() {
                gate.grant_path_rule(rule_for_runtime);
            }
            Ok(ConfigSetOutcome {
                applied: true,
                message: format!("已更新访问规则：{}", value),
            })
        }
        "primitive.bash_approval_required" | "primitive.bash_forbidden" => {
            let regex_str = parse_string_element(value)?;
            // 提前编译验证 regex；坏 regex 直接拒绝（避免污染 effective_bash_*）。
            regex::Regex::new(&regex_str)
                .map_err(|e| AppError::Config(format!("无效正则: {}", e)))?;
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    None,
                )
                .await?;
            if !decision.is_allow() {
                return Ok(ConfigSetOutcome {
                    applied: false,
                    message: "user_denied".into(),
                });
            }
            append_bash_regex_to_disk(&ctx.config_path, key, regex_str.clone())?;
            Ok(ConfigSetOutcome {
                applied: true,
                message: format!("已追加 {}: {}", key, regex_str),
            })
        }
        _ => Err(AppError::Config(format!("数组字段 '{}' 暂未实现追加", key))),
    }
}

async fn handle_scalar_replace(
    key: &str,
    value: &str,
    ctx: &ConfigToolContext,
) -> Result<ConfigSetOutcome, AppError> {
    let cfg_before = load_config(Some(&ctx.config_path))?;
    let val_before = toml::Value::try_from(&cfg_before)
        .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
    let prev = resolve_toml_path(&val_before, key)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "<not_set>".to_string());

    let preview = format!(
        "配置变更确认\n  字段: {}\n  类型: 替换标量\n  - 旧值: {}\n  + 新值: {}\n",
        key,
        prev.trim(),
        value
    );
    let decision = ctx
        .confirmation
        .confirm_decision(
            PrimitiveOperation::Write,
            &preview,
            CONFIG_TOOL_PLUGIN_ID,
            None,
        )
        .await?;
    if !decision.is_allow() {
        return Ok(ConfigSetOutcome {
            applied: false,
            message: "user_denied".into(),
        });
    }

    write_scalar_to_disk(&ctx.config_path, key, value)?;
    Ok(ConfigSetOutcome {
        applied: true,
        message: format!("已设置 {} = {}", key, value),
    })
}

fn ensure_path_not_denied(ctx: &ConfigToolContext, path: &Path) -> Result<(), AppError> {
    let Some(gate) = ctx.gate.as_ref() else {
        return Ok(());
    };
    match gate.check(PrimitiveOperation::Read, &path.to_string_lossy())? {
        PermissionDecision::Deny { reason } => Err(AppError::Permission(format!(
            "该路径已被禁止访问，无法写入 workspace.workspace_roots：{} ({})",
            path.display(),
            reason
        ))),
        _ => Ok(()),
    }
}

fn parse_string_element(value: &str) -> Result<String, AppError> {
    // 优先按 JSON 字符串解析（支持工具明确传 `"\"path\""`）；
    // 退化按裸字符串处理（兼容 LLM 传 `path` 不带引号）。
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(s) = v.as_str() {
            return Ok(s.to_string());
        }
    }
    Ok(value.to_string())
}

fn parse_json_element<T: serde::de::DeserializeOwned>(
    value: &str,
    type_name: &str,
) -> Result<T, AppError> {
    serde_json::from_str::<T>(value).map_err(|e| {
        AppError::Config(format!(
            "无法将 value 解析为 {}：{}；value={}",
            type_name, e, value
        ))
    })
}

fn append_bash_regex_to_disk(
    config_path: &Path,
    key: &str,
    regex_str: String,
) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let mut cfg = load_config(Some(config_path))?;
        let target = match key {
            "primitive.bash_approval_required" => &mut cfg.primitive.bash_approval_required,
            "primitive.bash_forbidden" => &mut cfg.primitive.bash_forbidden,
            _ => {
                return Err(AppError::Config(format!(
                    "append_bash_regex_to_disk: 不支持的 key {}",
                    key
                )))
            }
        };
        if target.iter().any(|s| s == &regex_str) {
            return Ok(());
        }
        target.push(regex_str);
        let toml_str = toml::to_string_pretty(&cfg)
            .map_err(|e| AppError::Config(format!("序列化配置失败: {}", e)))?;
        write_file_atomic(config_path, toml_str.as_bytes())?;
        Ok(())
    })
}

fn write_scalar_to_disk(config_path: &Path, key: &str, raw_value: &str) -> Result<(), AppError> {
    with_config_lock(config_path, || {
        let content = std::fs::read_to_string(config_path).map_err(AppError::Io)?;
        let mut val: toml::Value = content
            .parse()
            .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
        set_toml_scalar(&mut val, key, raw_value)?;
        let new_toml = toml::to_string_pretty(&val).map_err(|e| AppError::Config(e.to_string()))?;
        // 反序列化校验类型 / 业务约束。
        let parsed: AppConfig =
            toml::from_str(&new_toml).map_err(|e| AppError::Config(e.to_string()))?;
        crate::infra::config::validate_config(&parsed)?;
        write_file_atomic(config_path, new_toml.as_bytes())?;
        Ok(())
    })
}

fn set_toml_scalar(val: &mut toml::Value, key: &str, raw_value: &str) -> Result<(), AppError> {
    let segs: Vec<&str> = key.split('.').collect();
    if segs.is_empty() {
        return Err(AppError::Config("配置键不能为空".into()));
    }
    let mut cur = val;
    for (i, seg) in segs.iter().enumerate() {
        if i == segs.len() - 1 {
            let table = cur
                .as_table_mut()
                .ok_or_else(|| AppError::Config(format!("配置路径无效: {} 不是表", seg)))?;
            let new_val = if let Some(existing) = table.get(*seg) {
                coerce_scalar(existing, raw_value)?
            } else {
                toml::Value::String(raw_value.to_string())
            };
            table.insert((*seg).to_string(), new_val);
            return Ok(());
        }
        cur = cur
            .get_mut(*seg)
            .ok_or_else(|| AppError::Config(format!("配置路径无效: 缺中间节点 {}", seg)))?;
    }
    Ok(())
}

fn coerce_scalar(existing: &toml::Value, raw: &str) -> Result<toml::Value, AppError> {
    match existing {
        toml::Value::Integer(_) => raw
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|_| AppError::Config(format!("无法将 '{}' 转换为整数", raw))),
        toml::Value::Boolean(_) => raw.parse::<bool>().map(toml::Value::Boolean).map_err(|_| {
            AppError::Config(format!("无法将 '{}' 转换为布尔（期望 true/false）", raw))
        }),
        toml::Value::Float(_) => raw
            .parse::<f64>()
            .map(toml::Value::Float)
            .map_err(|_| AppError::Config(format!("无法将 '{}' 转换为浮点", raw))),
        _ => Ok(toml::Value::String(raw.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_allowlist_covers_documented_keys() {
        for k in [
            "workspace",
            "workspace.workspace_roots",
            "primitive.path_rules",
            "agent.id",
            "log.level",
        ] {
            assert!(is_readable(k), "{k} should be readable");
        }
    }

    #[test]
    fn read_hardcoded_deny_overrides_allowlist() {
        for k in [
            "llm.api_key",
            "llm.api_key_env",
            "security.audit_log_retention_days",
            "storage.work_dir",
        ] {
            assert!(!is_readable(k), "{k} must be denied");
        }
    }

    #[test]
    fn write_allowlist_subset() {
        for k in [
            "workspace.workspace_roots",
            "primitive.path_rules",
            "primitive.bash_forbidden",
            "log.level",
        ] {
            assert!(is_writable(k), "{k} should be writable");
        }
    }

    #[test]
    fn write_hardcoded_deny_blocks_self_escalation() {
        for k in [
            "primitive.bash_whitelist",
            "primitive.auto_confirm",
            "primitive.path_whitelist",
            "primitive.auto_confirm_whitelist",
            "agent.id",
            "agent.workspace",
            "llm.api_key",
            "security.enable_audit_log",
        ] {
            assert!(!is_writable(k), "{k} must be denied");
        }
    }

    #[test]
    fn array_fields_classification() {
        assert!(is_array_field("workspace.workspace_roots"));
        assert!(is_array_field("primitive.path_rules"));
        assert!(is_array_field("primitive.bash_forbidden"));
        assert!(!is_array_field("log.level"));
        assert!(!is_array_field("llm.default_model"));
    }

    use crate::core::tools::primitive::AllowAllConfirmation;
    use tempfile::TempDir;

    fn empty_config(dir: &TempDir) -> std::path::PathBuf {
        let p = dir.path().join("pi.config.toml");
        std::fs::write(
            &p,
            "[agent]\nid='main'\nworkspace='/tmp'\n\n[storage]\nwork_dir='/tmp'\n\n[llm]\nprovider='openai'\ndefault_model='gpt-4o'\n\n[workspace]\nworkspace_roots=[]\nentries=[]\n\n[primitive]\npath_rules=[]\nbash_approval_required=[]\nbash_forbidden=[]\nauto_confirm=true",
        ).unwrap();
        p
    }

    #[tokio::test]
    async fn config_get_returns_value_for_allowlisted_key() {
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let cfg = load_config(Some(&p)).unwrap();
        let v = config_get_impl("llm.default_model", &cfg).unwrap();
        assert_eq!(v.as_str(), Some("gpt-4o"));
    }

    #[tokio::test]
    async fn config_get_denies_sensitive_key() {
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let cfg = load_config(Some(&p)).unwrap();
        let err = config_get_impl("llm.api_key", &cfg).unwrap_err();
        assert!(matches!(err, AppError::Permission(_)));
    }

    #[tokio::test]
    async fn config_set_appends_extra_root_with_allow_all_confirm() {
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let extra = dir.path().join("proj");
        std::fs::create_dir_all(&extra).unwrap();
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
        let ctx = ConfigToolContext::new(p.clone(), confirm);
        let outcome = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
            .await
            .unwrap();
        assert!(outcome.applied);
        let cfg = load_config(Some(&p)).unwrap();
        assert_eq!(cfg.workspace.workspace_roots.len(), 1);
    }

    #[tokio::test]
    async fn config_set_extra_root_cannot_override_runtime_deny() {
        use crate::core::permission::{
            DefaultPermissionGate, GateConfig, PathRuleMode, SessionGrants,
        };

        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let extra = dir.path().join("denied");
        std::fs::create_dir_all(&extra).unwrap();
        let gate = DefaultPermissionGate::new(
            GateConfig {
                agent_definition_dir: dir.path().join("workspace-temp"),
                workspace_roots: vec![],
                agent_trail_readonly_dirs: vec![],
                user_path_rules: vec![PathRule::new(
                    extra.to_string_lossy().to_string(),
                    PathRuleMode::Deny,
                )],
                user_bash_forbidden: vec![],
                user_bash_approval: vec![],
                auto_confirm: false,
            },
            SessionGrants::new(),
        )
        .into_arc();
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
        let ctx = ConfigToolContext::new(p.clone(), confirm).with_gate(gate);

        let err = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Permission(_)));
        let cfg = load_config(Some(&p)).unwrap();
        assert!(cfg.workspace.workspace_roots.is_empty());
    }

    #[tokio::test]
    async fn config_set_denies_self_escalation_keys() {
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
        let ctx = ConfigToolContext::new(p, confirm);
        for k in [
            "primitive.bash_whitelist",
            "primitive.path_whitelist",
            "primitive.auto_confirm_whitelist",
            "primitive.auto_confirm",
            "agent.id",
            "llm.api_key",
        ] {
            let err = config_set_impl(k, "anything", &ctx).await.unwrap_err();
            assert!(
                matches!(err, AppError::Permission(_)),
                "{k} must be denied as self-escalation, got {:?}",
                err
            );
        }
    }

    #[tokio::test]
    async fn config_set_user_denied_returns_applied_false() {
        use crate::core::tools::primitive::DenyAllConfirmation;
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let extra = dir.path().join("proj2");
        std::fs::create_dir_all(&extra).unwrap();
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
        let ctx = ConfigToolContext::new(p.clone(), confirm);
        let outcome = config_set_impl("workspace.workspace_roots", &extra.to_string_lossy(), &ctx)
            .await
            .unwrap();
        assert!(!outcome.applied);
        assert_eq!(outcome.message, "user_denied");
        let cfg = load_config(Some(&p)).unwrap();
        assert!(cfg.workspace.workspace_roots.is_empty());
    }

    #[tokio::test]
    async fn config_set_array_path_rule_appends_with_json_value() {
        use crate::core::permission::{
            DefaultPermissionGate, GateConfig, PermissionDecision, SessionGrants,
        };
        use crate::core::tools::primitive::PrimitiveOperation;

        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
        let gate = DefaultPermissionGate::new(
            GateConfig {
                agent_definition_dir: dir.path().join("workspace-temp"),
                workspace_roots: vec![],
                agent_trail_readonly_dirs: vec![],
                user_path_rules: vec![],
                user_bash_forbidden: vec![],
                user_bash_approval: vec![],
                auto_confirm: false,
            },
            SessionGrants::new(),
        )
        .into_arc();
        let ctx = ConfigToolContext::new(p.clone(), confirm).with_gate(gate.clone());
        let rule = format!(
            r#"{{"path":"{}","mode":"deny"}}"#,
            blocked.to_string_lossy()
        );
        let outcome = config_set_impl("primitive.path_rules", &rule, &ctx)
            .await
            .unwrap();
        assert!(outcome.applied);
        let cfg = load_config(Some(&p)).unwrap();
        assert_eq!(cfg.primitive.path_rules.len(), 1);
        assert_eq!(cfg.primitive.path_rules[0].path, blocked.to_string_lossy());

        let decision = gate
            .check(
                PrimitiveOperation::Read,
                blocked.join("secret.txt").to_str().unwrap(),
            )
            .unwrap();
        assert!(
            matches!(decision, PermissionDecision::Deny { .. }),
            "config_set primitive.path_rules 后，同一会话 gate 必须立即 deny，实际: {:?}",
            decision
        );
    }

    #[tokio::test]
    async fn config_set_bash_forbidden_rejects_invalid_regex() {
        let dir = TempDir::new().unwrap();
        let p = empty_config(&dir);
        let confirm: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
        let ctx = ConfigToolContext::new(p.clone(), confirm);
        // 不平衡的括号 → regex 编译失败
        let err = config_set_impl("primitive.bash_forbidden", "(unbalanced", &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
        let cfg = load_config(Some(&p)).unwrap();
        assert!(cfg.primitive.bash_forbidden.is_empty());
    }
}
