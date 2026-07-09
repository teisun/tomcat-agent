use std::path::Path;

use crate::infra::config::with_config_lock;
use crate::infra::platform::write_file_atomic;
use crate::{validate_config, AppConfig, AppError};

pub fn write_default_model(config_path: &Path, model_id: &str) -> Result<(), AppError> {
    if !config_path.exists() {
        return Err(AppError::Config(format!(
            "配置文件不存在: {}。请先运行: tomcat init",
            config_path.display()
        )));
    }

    with_config_lock(config_path, || {
        let content = std::fs::read_to_string(config_path).map_err(AppError::Io)?;
        let mut value: toml::Value = content
            .parse()
            .map_err(|error: toml::de::Error| AppError::Config(error.to_string()))?;
        let root = value
            .as_table_mut()
            .ok_or_else(|| AppError::Config("配置文件根节点必须是 TOML 表".to_string()))?;
        let llm = root
            .get_mut("llm")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| AppError::Config("配置文件缺少 llm 表".to_string()))?;
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
