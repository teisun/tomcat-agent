//! Shared, narrow UI preference persistence. Never serialize environment overrides to disk.
use super::{with_config_lock, UiLanguage};
use crate::infra::i18n::{set_locale, tr, Locale};
use crate::{write_file_atomic, AppError};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferences {
    pub language: UiLanguage,
    pub effective: Locale,
    pub env_override: bool,
}

pub fn parse_language(value: &str) -> Result<UiLanguage, AppError> {
    match value {
        "auto" => Ok(UiLanguage::Auto),
        "en" => Ok(UiLanguage::En),
        "zh-CN" => Ok(UiLanguage::ZhCn),
        _ => Err(AppError::Config(tr(
            "ui.invalid_language",
            &[("language", value)],
        ))),
    }
}

/// Pure resolution is separately testable without mutating process environment or language.
pub fn resolve_preferences(
    language: UiLanguage,
    env: Option<&str>,
    host: Option<&str>,
    system: &str,
) -> Result<UiPreferences, AppError> {
    let override_language = env.map(parse_language).transpose()?;
    let selected = override_language.unwrap_or(language);
    let effective = selected.explicit_locale().unwrap_or_else(|| {
        Locale::from_language_tag(host.filter(|v| !v.trim().is_empty()).unwrap_or(system))
    });
    Ok(UiPreferences {
        language,
        effective,
        env_override: override_language.is_some(),
    })
}

pub fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".into())
}

/// Help/bootstrap only needs the preference, not credentials, models or validated runtime config.
pub fn read_language(path: &Path) -> Result<UiLanguage, AppError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(UiLanguage::Auto),
        Err(error) => return Err(error.into()),
    };
    let value: toml::Value = content
        .parse()
        .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
    let Some(ui) = value.get("ui") else {
        return Ok(UiLanguage::Auto);
    };
    let table = ui
        .as_table()
        .ok_or_else(|| AppError::Config(tr("ui.invalid_table", &[])))?;
    match table.get("language") {
        None => Ok(UiLanguage::Auto),
        Some(toml::Value::String(language)) => parse_language(language),
        Some(_) => Err(AppError::Config(tr(
            "ui.invalid_language",
            &[("language", &tr("ui.nonString", &[]))],
        ))),
    }
}

pub fn preferences(path: &Path, host: Option<&str>) -> Result<UiPreferences, AppError> {
    resolve_preferences(
        read_language(path)?,
        std::env::var("TOMCAT__UI__LANGUAGE").ok().as_deref(),
        host,
        &system_language(),
    )
}

/// Startup remains usable when config is absent/corrupt; regular commands still validate it.
pub fn bootstrap_language(host: Option<&str>) {
    let language = crate::normalize_path(crate::DEFAULT_CONFIG_PATH)
        .ok()
        .and_then(|path| read_language(&path).ok())
        .unwrap_or_default();
    let fallback = Locale::from_language_tag(host.unwrap_or(&system_language()));
    let effective = resolve_preferences(
        language,
        std::env::var("TOMCAT__UI__LANGUAGE").ok().as_deref(),
        host,
        &system_language(),
    )
    .map(|p| p.effective)
    .unwrap_or(fallback);
    set_locale(effective);
}

pub fn write_language(path: &Path, language: UiLanguage) -> Result<(), AppError> {
    with_config_lock(path, || {
        let content = std::fs::read_to_string(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::Config(tr(
                    "ui.config_missing",
                    &[("path", &path.display().to_string())],
                ))
            } else {
                AppError::Io(error)
            }
        })?;
        let mut document: toml::Value = content
            .parse()
            .map_err(|e: toml::de::Error| AppError::Config(e.to_string()))?;
        let root = document
            .as_table_mut()
            .ok_or_else(|| AppError::Config(tr("ui.invalid_table", &[])))?;
        let ui = root
            .entry("ui")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let table = ui
            .as_table_mut()
            .ok_or_else(|| AppError::Config(tr("ui.invalid_table", &[])))?;
        table.insert(
            "language".into(),
            toml::Value::String(language.as_str().into()),
        );
        let rendered =
            toml::to_string_pretty(&document).map_err(|e| AppError::Config(e.to_string()))?;
        write_file_atomic(path, rendered.as_bytes())
    })
}
