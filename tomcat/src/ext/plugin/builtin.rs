//! Materialize the executable's bundled plugin into the managed layer.

use crate::infra::{platform::write_file_atomic, resolve_plugins_dir};
use crate::{AppConfig, AppError};

pub(super) const DIRECTORY: &str = "web-search-backends";
pub(super) const ID: &str = "tomcat.web-search-backends";

const FILES: [(&str, &[u8]); 3] = [
    (
        "main.js",
        include_bytes!("../../../assets/plugins/web-search-backends/main.js"),
    ),
    (
        "README.md",
        include_bytes!("../../../assets/plugins/web-search-backends/README.md"),
    ),
    (
        "plugin.json",
        include_bytes!("../../../assets/plugins/web-search-backends/plugin.json"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinPluginsStatus {
    Created,
    UpdatedExistingPlugin,
    AlreadyPresent,
}

pub(crate) fn materialize_builtin_plugins(
    cfg: &AppConfig,
) -> Result<BuiltinPluginsStatus, AppError> {
    let directory = resolve_plugins_dir(cfg)?.join(DIRECTORY);
    let existed = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "managed plugin path is not a regular directory: {}",
                    directory.display()
                ),
            )
            .into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let mut changed = false;
    for (name, bytes) in FILES {
        let path = directory.join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
            && std::fs::read(&path).ok().as_deref() == Some(bytes)
        {
            continue;
        }
        write_file_atomic(&path, bytes)?;
        changed = true;
    }
    Ok(match (existed, changed) {
        (false, _) => BuiltinPluginsStatus::Created,
        (true, true) => BuiltinPluginsStatus::UpdatedExistingPlugin,
        (true, false) => BuiltinPluginsStatus::AlreadyPresent,
    })
}

#[cfg(test)]
#[path = "tests/builtin_test.rs"]
mod tests;
