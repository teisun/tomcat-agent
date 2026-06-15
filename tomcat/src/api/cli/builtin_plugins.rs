use crate::infra::resolve_plugins_dir;
use crate::{AppConfig, AppError};

const WEB_SEARCH_BACKENDS_DIR: &str = "web-search-backends";
const WEB_SEARCH_BACKENDS_MANIFEST: &str =
    include_str!("../../../assets/plugins/web-search-backends/plugin.json");
const WEB_SEARCH_BACKENDS_MAIN: &str =
    include_str!("../../../assets/plugins/web-search-backends/main.js");
const WEB_SEARCH_BACKENDS_README: &str =
    include_str!("../../../assets/plugins/web-search-backends/README.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinPluginsStatus {
    Created,
    UpdatedExistingPlugin,
    AlreadyPresent,
}

pub(crate) fn ensure_builtin_plugins(cfg: &AppConfig) -> Result<BuiltinPluginsStatus, AppError> {
    let plugins_root = resolve_plugins_dir(cfg)?;
    let plugin_dir = plugins_root.join(WEB_SEARCH_BACKENDS_DIR);
    let existed = plugin_dir.exists();
    std::fs::create_dir_all(&plugin_dir).map_err(AppError::Io)?;

    let mut wrote_any = false;
    let mut merged_manifest = false;
    wrote_any |= write_if_missing(
        &plugin_dir.join("plugin.json"),
        WEB_SEARCH_BACKENDS_MANIFEST,
    )?;
    if !wrote_any {
        merged_manifest |= merge_manifest_file(
            &plugin_dir.join("plugin.json"),
            WEB_SEARCH_BACKENDS_MANIFEST,
        )?;
    }
    wrote_any |= write_if_missing(&plugin_dir.join("main.js"), WEB_SEARCH_BACKENDS_MAIN)?;
    wrote_any |= write_if_missing(&plugin_dir.join("README.md"), WEB_SEARCH_BACKENDS_README)?;

    Ok(match (existed, wrote_any || merged_manifest) {
        (false, _) => BuiltinPluginsStatus::Created,
        (true, true) => BuiltinPluginsStatus::UpdatedExistingPlugin,
        (true, false) => BuiltinPluginsStatus::AlreadyPresent,
    })
}

fn write_if_missing(path: &std::path::Path, contents: &str) -> Result<bool, AppError> {
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(path, contents).map_err(AppError::Io)?;
    Ok(true)
}

fn merge_manifest_file(path: &std::path::Path, bundled_manifest: &str) -> Result<bool, AppError> {
    let existing_text = std::fs::read_to_string(path).map_err(AppError::Io)?;
    let mut existing: serde_json::Value = serde_json::from_str(&existing_text).map_err(|err| {
        AppError::Plugin(format!(
            "builtin plugin manifest parse error ({}): {}",
            path.display(),
            err
        ))
    })?;
    let bundled: serde_json::Value = serde_json::from_str(bundled_manifest).map_err(|err| {
        AppError::Plugin(format!("embedded builtin manifest parse error: {}", err))
    })?;
    if !merge_manifest_fields(&mut existing, &bundled)? {
        return Ok(false);
    }
    let rendered = serde_json::to_string_pretty(&existing).map_err(AppError::Serialize)?;
    std::fs::write(path, rendered).map_err(AppError::Io)?;
    Ok(true)
}

fn merge_manifest_fields(
    existing: &mut serde_json::Value,
    bundled: &serde_json::Value,
) -> Result<bool, AppError> {
    let existing_obj = existing.as_object_mut().ok_or_else(|| {
        AppError::Plugin("builtin plugin manifest must be a JSON object".to_string())
    })?;
    let bundled_obj = bundled.as_object().ok_or_else(|| {
        AppError::Plugin("embedded builtin plugin manifest must be a JSON object".to_string())
    })?;

    let mut changed = false;
    for field in ["requiredPermissions", "requiredSecrets", "allowedHosts"] {
        let Some(bundled_value) = bundled_obj.get(field) else {
            continue;
        };
        let bundled_array = bundled_value.as_array().ok_or_else(|| {
            AppError::Plugin(format!(
                "embedded builtin manifest `{field}` must be an array"
            ))
        })?;
        match existing_obj.get_mut(field) {
            Some(existing_value) => {
                let existing_array = existing_value.as_array_mut().ok_or_else(|| {
                    AppError::Plugin(format!(
                        "builtin plugin manifest `{field}` must be an array"
                    ))
                })?;
                for item in bundled_array {
                    if !existing_array
                        .iter()
                        .any(|existing_item| existing_item == item)
                    {
                        existing_array.push(item.clone());
                        changed = true;
                    }
                }
            }
            None => {
                existing_obj.insert(field.to_string(), bundled_value.clone());
                changed = true;
            }
        }
    }

    Ok(changed)
}

#[cfg(test)]
#[path = "tests/builtin_plugins_test.rs"]
mod tests;
