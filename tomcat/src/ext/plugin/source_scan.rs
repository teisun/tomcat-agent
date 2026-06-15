use super::catalog::PluginSource;
use crate::AppConfig;
use crate::infra::config::{get_work_dir, resolve_agent_trail_dir};
use crate::infra::error::AppError;
use std::path::{Path, PathBuf};

pub fn plugin_roots(
    cfg: &AppConfig,
    agent_workspace_dir: &Path,
) -> Result<Vec<(PluginSource, PathBuf)>, AppError> {
    Ok(vec![
        (
            PluginSource::Project,
            agent_workspace_dir.join(".tomcat").join("plugins"),
        ),
        (
            PluginSource::Agent,
            resolve_agent_trail_dir(cfg)?.join("plugins"),
        ),
        (PluginSource::Managed, get_work_dir(cfg)?.join("plugins")),
    ])
}

pub fn host_root_plugin_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(get_work_dir(cfg)?.join("plugins"))
}
