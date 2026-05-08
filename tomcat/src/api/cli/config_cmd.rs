//! `tomcat config` 子命令实现：get / set / edit。

use std::path::PathBuf;

use crate::infra::config::with_config_lock;
use crate::{load_config, normalize_path, validate_config, write_file_atomic, AppConfig, AppError};

use super::{ConfigSub, DEFAULT_CONFIG_PATH};

pub(crate) fn config_file_path() -> Result<PathBuf, AppError> {
    normalize_path(DEFAULT_CONFIG_PATH)
}

pub(crate) fn resolve_toml_key<'a>(val: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut current = val;
    for seg in key.split('.') {
        current = current.get(seg)?;
    }
    Some(current)
}

pub(crate) fn set_toml_key(
    val: &mut toml::Value,
    key: &str,
    raw_value: &str,
) -> Result<(), AppError> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.is_empty() {
        return Err(AppError::Config("配置项路径不能为空".to_string()));
    }

    let mut current = val;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            let table = current
                .as_table_mut()
                .ok_or_else(|| AppError::Config(format!("配置路径无效: {} 不是表", seg)))?;
            let entry = table.get(seg.to_owned()).ok_or_else(|| {
                let available: Vec<&String> = table.keys().collect();
                AppError::Config(format!(
                    "配置项不存在: {}。同级可用项: {:?}",
                    seg, available
                ))
            })?;
            let new_val =
                match entry {
                    toml::Value::Integer(_) => raw_value
                        .parse::<i64>()
                        .map(toml::Value::Integer)
                        .map_err(|_| {
                        AppError::Config(format!("无法将 '{}' 转换为整数类型", raw_value))
                    })?,
                    toml::Value::Boolean(_) => raw_value
                        .parse::<bool>()
                        .map(toml::Value::Boolean)
                        .map_err(|_| {
                            AppError::Config(format!(
                                "无法将 '{}' 转换为布尔类型（期望 true/false）",
                                raw_value
                            ))
                        })?,
                    toml::Value::Float(_) => raw_value
                        .parse::<f64>()
                        .map(toml::Value::Float)
                        .map_err(|_| {
                            AppError::Config(format!("无法将 '{}' 转换为浮点类型", raw_value))
                        })?,
                    _ => toml::Value::String(raw_value.to_string()),
                };
            table.insert(seg.to_string(), new_val);
            return Ok(());
        }
        current = current
            .get_mut(*seg)
            .ok_or_else(|| AppError::Config(format!("配置路径无效: 不存在的中间节点 {}", seg)))?;
    }
    Ok(())
}

pub(crate) fn run_config(sub: ConfigSub, cfg: &AppConfig) -> Result<(), AppError> {
    match sub {
        ConfigSub::Get { key } => {
            if let Some(k) = key {
                let val =
                    toml::Value::try_from(cfg).map_err(|e| AppError::Config(e.to_string()))?;
                match resolve_toml_key(&val, &k) {
                    Some(v) => println!("{}", v),
                    None => {
                        let parent_key = k.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
                        let parent = if parent_key.is_empty() {
                            Some(&val)
                        } else {
                            resolve_toml_key(&val, parent_key)
                        };
                        let hint = parent
                            .and_then(|p| p.as_table())
                            .map(|t| {
                                let keys: Vec<&String> = t.keys().collect();
                                format!("同级可用项: {:?}", keys)
                            })
                            .unwrap_or_default();
                        println!("未找到配置项: {}", k);
                        if !hint.is_empty() {
                            println!("  {}", hint);
                        }
                    }
                }
            } else {
                let toml_str =
                    toml::to_string_pretty(&cfg).map_err(|e| AppError::Config(e.to_string()))?;
                println!("{}", toml_str);
            }
        }
        ConfigSub::Set { key, value } => {
            let path = config_file_path()?;
            if !path.exists() {
                println!("配置文件不存在: {}。请先运行: tomcat init", path.display());
                return Ok(());
            }
            with_config_lock(&path, || {
                let content = std::fs::read_to_string(&path).map_err(AppError::Io)?;
                let mut val: toml::Value = content
                    .parse()
                    .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
                set_toml_key(&mut val, &key, &value)?;
                let new_toml =
                    toml::to_string_pretty(&val).map_err(|e| AppError::Config(e.to_string()))?;
                let check: Result<AppConfig, _> = toml::from_str(&new_toml);
                match check {
                    Ok(ref c) => {
                        if let Err(e) = validate_config(c) {
                            println!("值无效: {}，未修改配置", e);
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        println!("值无效: {}，未修改配置", e);
                        return Ok(());
                    }
                }
                write_file_atomic(&path, new_toml.as_bytes())?;
                println!("已设置 {} = {}", key, value);
                Ok(())
            })?;
        }
        ConfigSub::Edit => {
            let path = config_file_path()?;
            if !path.exists() {
                println!("配置文件不存在: {}。请先运行: tomcat init", path.display());
                return Ok(());
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
                if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            });
            match std::process::Command::new(&editor).arg(&path).status() {
                Ok(status) if status.success() => match load_config(Some(path.as_path())) {
                    Ok(ref c) => {
                        if let Err(e) = validate_config(c) {
                            println!("警告：编辑后的配置不合法: {}，请重新编辑修复", e);
                        } else {
                            println!("配置已更新");
                        }
                    }
                    Err(e) => {
                        println!("警告：编辑后的配置解析失败: {}，请重新编辑修复", e);
                    }
                },
                Ok(status) => {
                    println!("编辑器退出码: {}，配置可能未修改", status);
                }
                Err(e) => {
                    println!(
                        "无法启动编辑器 '{}': {}。请设置 EDITOR 环境变量或手动编辑 {}",
                        editor,
                        e,
                        path.display()
                    );
                }
            }
        }
    }
    Ok(())
}
