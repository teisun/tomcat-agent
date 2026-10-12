//! `config_set` 工具实现与落盘辅助。

use std::path::Path;

use crate::core::permission::{PathRule, PermissionDecision};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::{
    append_path_rule_to_disk, append_workspace_entry_to_disk, append_workspace_root_to_disk,
    load_config, load_config_toml_file, with_config_lock, AppConfig, WorkspaceEntry,
};
use crate::infra::error::AppError;
use crate::infra::i18n::tr;
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
        return Err(AppError::Permission(tr(
            "configTool.writeDenied",
            &[("key", key)],
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
    let preview = tr(
        "configTool.appendPreview",
        &[("key", key), ("value", value)],
    );

    match key {
        "workspace.workspace_roots" => {
            let abs = parse_string_element(value)?;
            let normalized = normalize_path(&abs).map_err(|e| {
                AppError::Config(tr("configTool.pathInvalid", &[("detail", &e.to_string())]))
            })?;
            ensure_path_not_denied(ctx, &normalized)?;
            let abs_path = normalized.to_string_lossy().to_string();
            let suggested = Some(normalized.clone());
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    None,
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
                message: tr("configTool.pathAllowed", &[("value", value)]),
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
                message: tr(
                    "configTool.appended",
                    &[("key", "workspace.entries"), ("value", value)],
                ),
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
                message: tr("configTool.pathRules", &[("value", value)]),
            })
        }
        "primitive.bash_approval_required" | "primitive.bash_forbidden" => {
            let regex_str = parse_string_element(value)?;
            // 提前编译验证 regex；坏 regex 直接拒绝（避免污染 effective_bash_*）。
            regex::Regex::new(&regex_str).map_err(|e| {
                AppError::Config(tr("configTool.regexInvalid", &[("detail", &e.to_string())]))
            })?;
            let decision = ctx
                .confirmation
                .confirm_decision(
                    PrimitiveOperation::Write,
                    &preview,
                    CONFIG_TOOL_PLUGIN_ID,
                    None,
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
                message: tr(
                    "configTool.appended",
                    &[("key", key), ("value", &regex_str)],
                ),
            })
        }
        _ => Err(AppError::Config(tr(
            "configTool.appendUnsupported",
            &[("key", key)],
        ))),
    }
}

async fn handle_scalar_replace(
    key: &str,
    value: &str,
    ctx: &ConfigToolContext,
) -> Result<ConfigSetOutcome, AppError> {
    let cfg_before = load_config(Some(&ctx.config_path))?;
    let val_before = toml::Value::try_from(&cfg_before).map_err(|e| {
        AppError::Config(tr(
            "configTool.serializeFailed",
            &[("detail", &e.to_string())],
        ))
    })?;
    let prev = resolve_toml_path(&val_before, key)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "<not_set>".to_string());

    let preview = tr(
        "configTool.replacePreview",
        &[("key", key), ("previous", prev.trim()), ("value", value)],
    );
    let decision = ctx
        .confirmation
        .confirm_decision(
            PrimitiveOperation::Write,
            &preview,
            CONFIG_TOOL_PLUGIN_ID,
            None,
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
        message: tr("cli.config.set", &[("key", key), ("value", value)]),
    })
}

fn ensure_path_not_denied(ctx: &ConfigToolContext, path: &Path) -> Result<(), AppError> {
    let Some(gate) = ctx.gate.as_ref() else {
        return Ok(());
    };
    match gate.check(PrimitiveOperation::Read, &path.to_string_lossy())? {
        PermissionDecision::Deny { reason } => Err(AppError::Permission(tr(
            "configTool.pathDenied",
            &[("path", &path.display().to_string()), ("reason", &reason)],
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
        AppError::Config(tr(
            "configTool.parseFailed",
            &[
                ("type", type_name),
                ("detail", &e.to_string()),
                ("value", value),
            ],
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
                return Err(AppError::Config(tr(
                    "configTool.regexKeyUnsupported",
                    &[("key", key)],
                )))
            }
        };
        if target.iter().any(|s| s == &regex_str) {
            return Ok(());
        }
        target.push(regex_str);
        let toml_str = toml::to_string_pretty(&cfg).map_err(|e| {
            AppError::Config(tr(
                "configTool.serializeFailed",
                &[("detail", &e.to_string())],
            ))
        })?;
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
        return Err(AppError::Config(tr("cli.config.emptyPath", &[])));
    }
    let mut cur = val;
    for (i, seg) in segs.iter().enumerate() {
        if i == segs.len() - 1 {
            let table = cur
                .as_table_mut()
                .ok_or_else(|| AppError::Config(tr("cli.config.notTable", &[("name", seg)])))?;
            let new_val = if let Some(existing) = table.get(*seg) {
                coerce_scalar(existing, raw_value)?
            } else {
                infer_scalar_from_raw(raw_value)
            };
            table.insert((*seg).to_string(), new_val);
            return Ok(());
        }
        let table = cur
            .as_table_mut()
            .ok_or_else(|| AppError::Config(tr("cli.config.notTable", &[("name", seg)])))?;
        if !table.contains_key(*seg) {
            table.insert((*seg).to_string(), toml::Value::Table(Default::default()));
        }
        cur = table.get_mut(*seg).ok_or_else(|| {
            AppError::Config(tr("cli.config.intermediateMissing", &[("name", seg)]))
        })?;
        if !cur.is_table() {
            return Err(AppError::Config(tr(
                "cli.config.notTable",
                &[("name", seg)],
            )));
        }
    }
    Ok(())
}

fn coerce_scalar(existing: &toml::Value, raw: &str) -> Result<toml::Value, AppError> {
    match existing {
        toml::Value::Integer(_) => raw
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|_| AppError::Config(tr("cli.config.intInvalid", &[("value", raw)]))),
        toml::Value::Boolean(_) => raw
            .parse::<bool>()
            .map(toml::Value::Boolean)
            .map_err(|_| AppError::Config(tr("cli.config.boolInvalid", &[("value", raw)]))),
        toml::Value::Float(_) => raw
            .parse::<f64>()
            .map(toml::Value::Float)
            .map_err(|_| AppError::Config(tr("cli.config.floatInvalid", &[("value", raw)]))),
        _ => Ok(toml::Value::String(raw.to_string())),
    }
}

fn infer_scalar_from_raw(raw: &str) -> toml::Value {
    if let Ok(v) = raw.parse::<bool>() {
        toml::Value::Boolean(v)
    } else if let Ok(v) = raw.parse::<i64>() {
        toml::Value::Integer(v)
    } else if let Ok(v) = raw.parse::<f64>() {
        toml::Value::Float(v)
    } else {
        toml::Value::String(raw.to_string())
    }
}
