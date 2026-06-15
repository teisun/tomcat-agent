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
    UpdatedMissingFiles,
    AlreadyPresent,
}

pub(crate) fn ensure_builtin_plugins(cfg: &AppConfig) -> Result<BuiltinPluginsStatus, AppError> {
    let plugins_root = resolve_plugins_dir(cfg)?;
    let plugin_dir = plugins_root.join(WEB_SEARCH_BACKENDS_DIR);
    let existed = plugin_dir.exists();
    std::fs::create_dir_all(&plugin_dir).map_err(AppError::Io)?;

    let mut wrote_any = false;
    wrote_any |= write_if_missing(
        &plugin_dir.join("plugin.json"),
        WEB_SEARCH_BACKENDS_MANIFEST,
    )?;
    wrote_any |= write_if_missing(&plugin_dir.join("main.js"), WEB_SEARCH_BACKENDS_MAIN)?;
    wrote_any |= write_if_missing(&plugin_dir.join("README.md"), WEB_SEARCH_BACKENDS_README)?;

    Ok(match (existed, wrote_any) {
        (false, _) => BuiltinPluginsStatus::Created,
        (true, true) => BuiltinPluginsStatus::UpdatedMissingFiles,
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

#[cfg(test)]
#[path = "tests/builtin_plugins_test.rs"]
mod tests;
