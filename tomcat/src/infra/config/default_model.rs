use crate::infra::i18n::tr;
use std::path::Path;

use crate::infra::config::with_config_lock;
use crate::infra::platform::write_file_atomic;
use crate::{validate_config, AppConfig, AppError};

pub fn write_default_model(config_path: &Path, model_id: &str) -> Result<(), AppError> {
    if !config_path.exists() {
        return Err(AppError::Config(tr(
            "config.fileMissing",
            &[("path", &config_path.display().to_string())],
        )));
    }

    with_config_lock(config_path, || {
        let content = std::fs::read_to_string(config_path).map_err(AppError::Io)?;
        let mut value: toml::Value = content
            .parse()
            .map_err(|error: toml::de::Error| AppError::Config(error.to_string()))?;
        let root = value
            .as_table_mut()
            .ok_or_else(|| AppError::Config(tr("config.rootTable", &[])))?;
        let llm = root
            .get_mut("llm")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| AppError::Config(tr("config.llmTableMissing", &[])))?;
        llm.insert(
            "default_model".to_string(),
            toml::Value::String(model_id.to_string()),
        );

        let rendered =
            toml::to_string_pretty(&value).map_err(|error| AppError::Config(error.to_string()))?;
        let check: AppConfig =
            toml::from_str(&rendered).map_err(|error| AppError::Config(error.to_string()))?;
        validate_config(&check)?;
        write_file_atomic(config_path, rendered.as_bytes())
    })
}

/// 清掉磁盘配置中恰好等于被删除模型的选择，并返回被清理的配置键。
///
/// 删除用户模型时必须修改真实 TOML，而不是把启动时的 AppConfig 整体写回：后者会把
/// 环境变量覆盖和值不属于本次操作的字段一起带回磁盘。
pub fn clear_model_references(
    config_path: &Path,
    model_id: &str,
) -> Result<Vec<&'static str>, AppError> {
    match std::fs::metadata(config_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AppError::Io(error)),
    }
    with_config_lock(config_path, || {
        let content = std::fs::read_to_string(config_path).map_err(AppError::Io)?;
        let mut value: toml::Value = content
            .parse()
            .map_err(|error: toml::de::Error| AppError::Config(error.to_string()))?;
        let root = value
            .as_table_mut()
            .ok_or_else(|| AppError::Config(tr("config.rootTable", &[])))?;
        let mut cleared = Vec::new();
        if clear_model_value(root, "llm", "default_model", model_id)? {
            cleared.push("llm.default_model");
        }
        if clear_model_value(root, "llm", "vision_model", model_id)? {
            cleared.push("llm.vision_model");
        }
        if clear_model_value(root, "llm", "title_model", model_id)? {
            cleared.push("llm.title_model");
        }
        if clear_model_value(root, "context", "compaction_model", model_id)? {
            cleared.push("context.compaction_model");
        }
        if cleared.is_empty() {
            return Ok(cleared);
        }
        let rendered =
            toml::to_string_pretty(&value).map_err(|error| AppError::Config(error.to_string()))?;
        let check: AppConfig =
            toml::from_str(&rendered).map_err(|error| AppError::Config(error.to_string()))?;
        validate_config(&check)?;
        write_file_atomic(config_path, rendered.as_bytes())?;
        Ok(cleared)
    })
}

fn clear_model_value(
    root: &mut toml::map::Map<String, toml::Value>,
    section: &str,
    key: &str,
    model_id: &str,
) -> Result<bool, AppError> {
    let Some(table) = root.get_mut(section) else {
        return Ok(false);
    };
    let table = table
        .as_table_mut()
        .ok_or_else(|| AppError::Config(tr("config.sectionTable", &[("section", section)])))?;
    let Some(current) = table.get(key) else {
        return Ok(false);
    };
    let current = current.as_str().ok_or_else(|| {
        AppError::Config(tr(
            "config.fieldString",
            &[("field", &format!("{section}.{key}"))],
        ))
    })?;
    if current.trim() != model_id {
        return Ok(false);
    }
    // String 和 Option<String> 的 resolver 都把空串视作“未选择”，并会走场景自身的回退。
    table.insert(key.to_string(), toml::Value::String(String::new()));
    Ok(true)
}
