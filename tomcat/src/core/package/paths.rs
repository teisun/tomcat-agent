use std::path::{Path, PathBuf};

use crate::infra::config::{get_work_dir, resolve_agent_trail_dir};
use crate::infra::error::AppError;
use crate::AppConfig;

use super::model::PackageVisibility;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerPaths {
    pub visibility: PackageVisibility,
    pub layer_root: PathBuf,
    pub scope_root: Option<PathBuf>,
    pub packages_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub package_registry_path: PathBuf,
    pub plugin_registry_path: PathBuf,
}

pub fn canonical_scope_root(path: &Path) -> Result<PathBuf, AppError> {
    path.canonicalize()
        .map_err(|error| AppError::Config(format!("scope_root 无法 canonicalize: {error}")))
}

pub fn resolve_layer_paths(
    cfg: &AppConfig,
    visibility: PackageVisibility,
    scope_root: Option<&Path>,
) -> Result<LayerPaths, AppError> {
    match visibility {
        PackageVisibility::Global => {
            let root = get_work_dir(cfg)?;
            Ok(build_layer_paths(visibility, root, None))
        }
        PackageVisibility::Agent => {
            let root = resolve_agent_trail_dir(cfg)?;
            Ok(build_layer_paths(visibility, root, None))
        }
        PackageVisibility::Scope => {
            let scope_root = scope_root.ok_or_else(|| {
                AppError::Config("scope 安装/查询必须提供 scope_root".to_string())
            })?;
            let canonical_root = canonical_scope_root(scope_root)?;
            let layer_root = canonical_root.join(".tomcat");
            Ok(build_layer_paths(
                visibility,
                layer_root,
                Some(canonical_root),
            ))
        }
    }
}

pub fn resolve_runtime_layer_paths(
    cfg: &AppConfig,
    scope_root: Option<&Path>,
) -> Result<Vec<LayerPaths>, AppError> {
    let mut out = Vec::with_capacity(3);
    for visibility in PackageVisibility::ordered_runtime_layers() {
        match visibility {
            PackageVisibility::Scope => {
                let Some(scope_root) = scope_root else {
                    continue;
                };
                out.push(resolve_layer_paths(cfg, visibility, Some(scope_root))?);
            }
            _ => out.push(resolve_layer_paths(cfg, visibility, scope_root)?),
        }
    }
    Ok(out)
}

fn build_layer_paths(
    visibility: PackageVisibility,
    layer_root: PathBuf,
    scope_root: Option<PathBuf>,
) -> LayerPaths {
    let packages_dir = layer_root.join("packages");
    let plugins_dir = layer_root.join("plugins");
    let skills_dir = layer_root.join("skills");
    LayerPaths {
        visibility,
        layer_root,
        scope_root,
        package_registry_path: packages_dir.join("registry.json"),
        plugin_registry_path: plugins_dir.join("registry.json"),
        packages_dir,
        plugins_dir,
        skills_dir,
    }
}
