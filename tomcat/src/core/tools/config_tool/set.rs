//! `config_set` 工具实现与落盘辅助。

use std::path::Path;

use crate::core::permission::{PathRule, PermissionDecision};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::{
    append_path_rule_to_disk, append_workspace_entry_to_disk, append_workspace_root_to_disk,
    load_config, load_config_toml_file, with_config_lock, AppConfig, WorkspaceEntry,
};
use crate::infra::error::AppError;
use crate::infra::platform::{normalize_path, write_file_atomic};

use super::allowlist;
use super::get::resolve_toml_path;
use super::ConfigToolContext;

/// 工具触发的 plugin_id 标签——与 `tool_exec::AGENT_PLUGIN_ID` 区分，便于审计追溯
/// "这次 confirm 来自 config_set 工具"。
const CONFIG_TOOL_PLUGIN_ID: &str = "__config_tool__";

/// `config_set` 工具的返回载荷（序列化为 JSON 给 LLM）。
#[derive(Debug, Clone, serde::Serialize)]
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
    if !allowlist::is_writable(key) {
        return Err(AppError::Permission(format!(
            "配置项 '{}' 不在写白名单内或被硬黑名单拦截；如需手动修改请使用 `tomcat config edit`",
            key
        )));
    }

    if allowlist::is_array_field(key) {
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
        let mut cfg = load_config_toml_file(config_path)?;
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
