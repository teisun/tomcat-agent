//! `config_get` 工具实现。

use crate::infra::config::AppConfig;
use crate::infra::error::AppError;

use super::allowlist;

/// 处理 `config_get` 工具调用。
///
/// 返回 JSON 形式的当前值；若 key 不存在但白名单允许，返回 `"not_set"` 字符串。
pub fn config_get_impl(key: &str, cfg: &AppConfig) -> Result<serde_json::Value, AppError> {
    if !allowlist::is_readable(key) {
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

pub(crate) fn resolve_toml_path<'a>(val: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
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
