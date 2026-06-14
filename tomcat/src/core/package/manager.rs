use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::core::skill::parse as parse_skill_frontmatter;
use crate::ext::parse_manifest as parse_plugin_manifest;
use crate::infra::{read_file_utf8, write_file_atomic, AppError};
use crate::AppConfig;

use super::model::{
    DetectedPackageResource, DetectedPackageSource, InstallOutcome, PackageLayerListing,
    PackageManifest, PackageRecord, PackageRegistryFile, PackageResource, PackageResourceKind,
    PackageSourceKind, PackageVisibility, PluginRegistryEntry, PluginRegistryFile, PreparedInstall,
    PreparedInstallResource, UninstallOutcome,
};
use super::paths::{resolve_layer_paths, resolve_runtime_layer_paths, LayerPaths};

#[derive(Debug)]
pub struct PackageManager<'a> {
    cfg: &'a AppConfig,
}

impl<'a> PackageManager<'a> {
    pub fn new(cfg: &'a AppConfig) -> Self {
        Self { cfg }
    }

    pub fn detect_source(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<DetectedPackageSource, AppError> {
        let source = canonicalize_existing_path(source.as_ref())?;
        let metadata = fs::metadata(&source).map_err(AppError::Io)?;
        if metadata.is_dir() {
            if let Some(detected) = try_detect_package_manifest_dir(&source)? {
                return Ok(detected);
            }
            let plugin_manifest = source.join("plugin.json");
            if plugin_manifest.is_file() {
                return detect_bare_plugin(&plugin_manifest);
            }
            let skill_file = source.join("SKILL.md");
            if skill_file.is_file() {
                return detect_bare_skill(&skill_file);
            }
            return Err(AppError::Config(format!(
                "source 无法识别为 Tomcat package/plugin/skill: {}",
                source.display()
            )));
        }

        let Some(file_name) = source.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(format!(
                "source 文件名无效: {}",
                source.display()
            )));
        };
        match file_name {
            "package.json" => detect_package_manifest_file(&source, true)?.ok_or_else(|| {
                AppError::Config(format!(
                    "package.json 缺少顶层 tomcat 块: {}",
                    source.display()
                ))
            }),
            "plugin.json" => detect_bare_plugin(&source),
            "SKILL.md" => detect_bare_skill(&source),
            _ => Err(AppError::Config(format!(
                "source 只支持 package.json / plugin.json / SKILL.md 或其所在目录: {}",
                source.display()
            ))),
        }
    }

    pub fn prepare_install(
        &self,
        source: impl AsRef<Path>,
        visibility: PackageVisibility,
        scope_root: Option<&Path>,
        force: bool,
    ) -> Result<PreparedInstall, AppError> {
        let detected = self.detect_source(source)?;
        self.prepare_detected_install(detected, visibility, scope_root, force)
    }

    pub fn prepare_detected_install(
        &self,
        detected: DetectedPackageSource,
        visibility: PackageVisibility,
        scope_root: Option<&Path>,
        force: bool,
    ) -> Result<PreparedInstall, AppError> {
        let layer_paths = resolve_layer_paths(self.cfg, visibility, scope_root)?;
        let package_registry = load_package_registry(&layer_paths.package_registry_path);
        if package_registry
            .packages
            .iter()
            .any(|record| record.name == detected.manifest.name)
            && !force
        {
            return Err(AppError::Config(format!(
                "同层 package 已存在，需加 --force: {}",
                detected.manifest.name
            )));
        }

        let mut warnings =
            collect_cross_layer_warnings(self.cfg, scope_root, visibility, &detected)?;
        let mut resources = Vec::with_capacity(detected.resources.len());
        for resource in ordered_detected_resources(&detected.resources) {
            let destination_dir = match resource.kind {
                PackageResourceKind::Plugin => layer_paths.plugins_dir.join(&resource.id),
                PackageResourceKind::Skill => layer_paths.skills_dir.join(&resource.id),
            };
            if destination_dir.exists() && !force {
                return Err(AppError::Config(format!(
                    "同层 {} 已存在，需加 --force: {}",
                    resource.kind.as_str(),
                    resource.id
                )));
            }
            if destination_dir.exists() && force {
                warnings.push(format!(
                    "将覆盖当前层已存在的 {} `{}`",
                    resource.kind.as_str(),
                    resource.id
                ));
            }
            resources.push(PreparedInstallResource {
                kind: resource.kind,
                id: resource.id.clone(),
                source_path: resource.source_path.clone(),
                source_dir: resource.source_dir.clone(),
                install_subpath: format!("{}/{}", resource.kind.registry_dir(), resource.id),
                destination_dir,
            });
        }

        Ok(PreparedInstall {
            detected,
            visibility,
            layer_paths,
            warnings,
            force,
            resources,
        })
    }

    pub fn install(&self, prepared: PreparedInstall) -> Result<InstallOutcome, AppError> {
        let package_snapshot =
            RegistrySnapshot::capture_package(&prepared.layer_paths.package_registry_path);
        let plugin_snapshot =
            RegistrySnapshot::capture_plugin(&prepared.layer_paths.plugin_registry_path);
        let mut mutations = Vec::new();

        let install_result = (|| -> Result<InstallOutcome, AppError> {
            let mut package_registry = package_snapshot.package_value();
            package_registry
                .packages
                .retain(|record| record.name != prepared.detected.manifest.name);
            let mut plugin_registry = plugin_snapshot.plugin_value();
            let plugin_ids = prepared
                .resources
                .iter()
                .filter(|resource| resource.kind == PackageResourceKind::Plugin)
                .map(|resource| resource.id.clone())
                .collect::<HashSet<_>>();
            plugin_registry
                .plugins
                .retain(|entry| !plugin_ids.contains(&entry.id));

            for resource in &prepared.resources {
                mutations.push(install_resource(resource, prepared.force)?);
            }

            let installed_at = Utc::now().to_rfc3339();
            let record = PackageRecord {
                name: prepared.detected.manifest.name.clone(),
                version: prepared.detected.manifest.version.clone(),
                description: prepared.detected.manifest.description.clone(),
                source_kind: prepared.detected.kind,
                visibility: prepared.visibility,
                source_path: prepared.detected.source_root.display().to_string(),
                scope_root: prepared
                    .layer_paths
                    .scope_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                installed_at: installed_at.clone(),
                resources: prepared
                    .resources
                    .iter()
                    .map(|resource| {
                        PackageResource::new(
                            resource.kind,
                            resource.id.clone(),
                            resource.source_path.clone(),
                            resource.install_subpath.clone(),
                        )
                    })
                    .collect(),
            };

            package_registry.packages.push(record.clone());
            save_package_registry(
                &prepared.layer_paths.package_registry_path,
                &package_registry,
            )?;

            for resource in &prepared.resources {
                if resource.kind != PackageResourceKind::Plugin {
                    continue;
                }
                plugin_registry.plugins.push(PluginRegistryEntry {
                    id: resource.id.clone(),
                    path: resource.destination_dir.display().to_string(),
                    enabled: true,
                    loaded_at: installed_at.clone(),
                });
            }
            save_plugin_registry(&prepared.layer_paths.plugin_registry_path, &plugin_registry)?;

            Ok(InstallOutcome {
                record,
                warnings: prepared.warnings.clone(),
            })
        })();

        match install_result {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let rollback_errors = rollback_install(
                    &prepared.layer_paths,
                    &package_snapshot,
                    &plugin_snapshot,
                    &mutations,
                );
                if rollback_errors.is_empty() {
                    Err(AppError::Config(format!("package install 失败: {error}")))
                } else {
                    Err(AppError::Config(format!(
                        "package install 失败且 rollback 不完整: {error}; dirty_state: {}",
                        rollback_errors.join(" | ")
                    )))
                }
            }
        }
    }

    pub fn uninstall(
        &self,
        package_name: &str,
        visibility: PackageVisibility,
        scope_root: Option<&Path>,
    ) -> Result<UninstallOutcome, AppError> {
        let layer_paths = resolve_layer_paths(self.cfg, visibility, scope_root)?;
        let mut package_registry = load_package_registry(&layer_paths.package_registry_path);
        let Some(index) = package_registry
            .packages
            .iter()
            .position(|record| record.name == package_name)
        else {
            return Err(AppError::Config(format!(
                "package 未安装在 {visibility}: {package_name}"
            )));
        };
        let record = package_registry.packages.remove(index);

        let mut removed_paths = Vec::new();
        for resource in record.resources.iter().rev() {
            let path = layer_paths.layer_root.join(&resource.install_subpath);
            if remove_path_if_exists(&path)? {
                removed_paths.push(path);
            }
        }

        save_package_registry(&layer_paths.package_registry_path, &package_registry)?;

        let mut plugin_registry = load_plugin_registry(&layer_paths.plugin_registry_path);
        let plugin_ids = record
            .resources
            .iter()
            .filter(|resource| resource.kind == PackageResourceKind::Plugin)
            .map(|resource| resource.id.clone())
            .collect::<HashSet<_>>();
        plugin_registry
            .plugins
            .retain(|entry| !plugin_ids.contains(&entry.id));
        save_plugin_registry(&layer_paths.plugin_registry_path, &plugin_registry)?;

        Ok(UninstallOutcome {
            record,
            removed_paths,
        })
    }

    pub fn list_packages(
        &self,
        scope_root: Option<&Path>,
        visibility: Option<PackageVisibility>,
    ) -> Result<Vec<PackageLayerListing>, AppError> {
        let layers = match visibility {
            Some(visibility) => vec![resolve_layer_paths(self.cfg, visibility, scope_root)?],
            None => resolve_runtime_layer_paths(self.cfg, scope_root)?,
        };
        Ok(layers
            .into_iter()
            .map(|layer| PackageLayerListing {
                visibility: layer.visibility,
                records: load_package_registry(&layer.package_registry_path).packages,
            })
            .collect())
    }
}

pub fn load_package_registry(path: &Path) -> PackageRegistryFile {
    load_registry(path).unwrap_or_default()
}

pub fn save_package_registry(path: &Path, registry: &PackageRegistryFile) -> Result<(), AppError> {
    save_registry(path, registry)
}

pub fn load_plugin_registry(path: &Path) -> PluginRegistryFile {
    load_registry(path).unwrap_or_default()
}

pub fn save_plugin_registry(path: &Path, registry: &PluginRegistryFile) -> Result<(), AppError> {
    save_registry(path, registry)
}

#[derive(Debug, Deserialize)]
struct RawPackageJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tomcat: Option<RawTomcatPackageBlock>,
}

#[derive(Debug, Deserialize)]
struct RawTomcatPackageBlock {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    plugins: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
}

fn try_detect_package_manifest_dir(root: &Path) -> Result<Option<DetectedPackageSource>, AppError> {
    let manifest_path = root.join("package.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    detect_package_manifest_file(&manifest_path, false)
}

fn detect_package_manifest_file(
    manifest_path: &Path,
    require_tomcat_block: bool,
) -> Result<Option<DetectedPackageSource>, AppError> {
    let root = manifest_path
        .parent()
        .ok_or_else(|| AppError::Config("package.json 缺少父目录".to_string()))?
        .canonicalize()
        .map_err(AppError::Io)?;
    let raw = read_file_utf8(manifest_path)?;
    let parsed: RawPackageJson = serde_json::from_str(&raw)
        .map_err(|error| AppError::Config(format!("package.json 解析失败: {error}")))?;
    let Some(tomcat) = parsed.tomcat else {
        return if require_tomcat_block {
            Err(AppError::Config(format!(
                "package.json 缺少顶层 tomcat 块: {}",
                manifest_path.display()
            )))
        } else {
            Ok(None)
        };
    };

    let name = tomcat
        .name
        .or(parsed.name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Config("package.tomcat.name 缺失".to_string()))?;
    let version = tomcat
        .version
        .or(parsed.version)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Config("package.tomcat.version 缺失".to_string()))?;
    let description = tomcat.description.or(parsed.description);

    let manifest = PackageManifest {
        name,
        version,
        description,
        plugins: tomcat.plugins,
        skills: tomcat.skills,
    };
    if manifest.plugins.is_empty() && manifest.skills.is_empty() {
        return Err(AppError::Config(
            "package.tomcat.plugins / skills 不能同时为空".to_string(),
        ));
    }

    let resources = resolve_package_resources(&root, &manifest)?;
    Ok(Some(DetectedPackageSource {
        kind: PackageSourceKind::Package,
        source_root: root,
        manifest,
        resources,
    }))
}

fn detect_bare_plugin(manifest_path: &Path) -> Result<DetectedPackageSource, AppError> {
    let (plugin_root, manifest) = resolve_plugin_source(manifest_path)?;
    let plugin_id = manifest.id.clone();
    let version = manifest.version.clone();
    let description = if manifest.description.trim().is_empty() {
        None
    } else {
        Some(manifest.description.clone())
    };
    let manifest =
        PackageManifest::single_plugin(plugin_id.clone(), version, description, ".".to_string());
    Ok(DetectedPackageSource {
        kind: PackageSourceKind::BarePlugin,
        source_root: plugin_root.clone(),
        manifest,
        resources: vec![DetectedPackageResource {
            kind: PackageResourceKind::Plugin,
            id: plugin_id,
            source_path: ".".to_string(),
            source_dir: plugin_root,
        }],
    })
}

fn detect_bare_skill(skill_file: &Path) -> Result<DetectedPackageSource, AppError> {
    let (skill_root, skill_name, description) = resolve_skill_source(skill_file)?;
    let manifest = PackageManifest::single_skill(
        skill_name.clone(),
        Some(description.clone()),
        ".".to_string(),
    );
    Ok(DetectedPackageSource {
        kind: PackageSourceKind::BareSkill,
        source_root: skill_root.clone(),
        manifest,
        resources: vec![DetectedPackageResource {
            kind: PackageResourceKind::Skill,
            id: skill_name,
            source_path: ".".to_string(),
            source_dir: skill_root,
        }],
    })
}

fn resolve_package_resources(
    root: &Path,
    manifest: &PackageManifest,
) -> Result<Vec<DetectedPackageResource>, AppError> {
    let mut resources = Vec::with_capacity(manifest.plugins.len() + manifest.skills.len());
    let mut seen = HashSet::new();
    for plugin in &manifest.plugins {
        let (plugin_root, plugin_manifest) = resolve_plugin_source(&root.join(plugin))?;
        let id = plugin_manifest.id.clone();
        if !seen.insert((PackageResourceKind::Plugin.as_str().to_string(), id.clone())) {
            return Err(AppError::Config(format!("package 内 plugin 重复: {id}")));
        }
        resources.push(DetectedPackageResource {
            kind: PackageResourceKind::Plugin,
            id,
            source_path: plugin.clone(),
            source_dir: plugin_root,
        });
    }
    for skill in &manifest.skills {
        let (skill_root, skill_name, _description) = resolve_skill_source(&root.join(skill))?;
        if !seen.insert((
            PackageResourceKind::Skill.as_str().to_string(),
            skill_name.clone(),
        )) {
            return Err(AppError::Config(format!(
                "package 内 skill 重复: {skill_name}"
            )));
        }
        resources.push(DetectedPackageResource {
            kind: PackageResourceKind::Skill,
            id: skill_name,
            source_path: skill.clone(),
            source_dir: skill_root,
        });
    }
    Ok(resources)
}

fn resolve_plugin_source(path: &Path) -> Result<(PathBuf, crate::PluginManifest), AppError> {
    let resolved = canonicalize_existing_path(path)?;
    let (plugin_root, manifest_path) = if resolved.is_dir() {
        let manifest_path = resolved.join("plugin.json");
        if !manifest_path.is_file() {
            return Err(AppError::Config(format!(
                "plugin 目录下缺少 plugin.json: {}",
                resolved.display()
            )));
        }
        (resolved, manifest_path)
    } else {
        let Some(file_name) = resolved.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(format!(
                "plugin.json 文件名无效: {}",
                resolved.display()
            )));
        };
        if file_name != "plugin.json" {
            return Err(AppError::Config(format!(
                "plugin source 只支持 plugin.json 或其所在目录: {}",
                resolved.display()
            )));
        }
        (
            resolved
                .parent()
                .ok_or_else(|| AppError::Config("plugin.json 缺少父目录".to_string()))?
                .to_path_buf(),
            resolved,
        )
    };

    let manifest_json = read_file_utf8(&manifest_path)?;
    let manifest = parse_plugin_manifest(&manifest_json)?;
    validate_plugin_main(&plugin_root, &manifest)?;
    Ok((plugin_root, manifest))
}

fn validate_plugin_main(
    plugin_root: &Path,
    manifest: &crate::PluginManifest,
) -> Result<(), AppError> {
    let root = plugin_root.canonicalize().map_err(AppError::Io)?;
    let main_path = canonicalize_existing_path(&root.join(&manifest.main)).map_err(|error| {
        AppError::Config(format!(
            "plugin main 不存在或不可读: {} ({})",
            manifest.main, error
        ))
    })?;
    if !main_path.starts_with(&root) {
        return Err(AppError::Permission(format!(
            "plugin main 不得越出插件根目录: {}",
            main_path.display()
        )));
    }
    if !main_path.is_file() {
        return Err(AppError::Config(format!(
            "plugin main 必须是文件: {}",
            main_path.display()
        )));
    }
    Ok(())
}

fn resolve_skill_source(path: &Path) -> Result<(PathBuf, String, String), AppError> {
    let resolved = canonicalize_existing_path(path)?;
    let (skill_root, skill_file) = if resolved.is_dir() {
        let skill_file = resolved.join("SKILL.md");
        if !skill_file.is_file() {
            return Err(AppError::Config(format!(
                "skill 目录下缺少 SKILL.md: {}",
                resolved.display()
            )));
        }
        (resolved, skill_file)
    } else {
        let Some(file_name) = resolved.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(format!(
                "SKILL.md 文件名无效: {}",
                resolved.display()
            )));
        };
        if file_name != "SKILL.md" {
            return Err(AppError::Config(format!(
                "skill source 只支持 SKILL.md 或其所在目录: {}",
                resolved.display()
            )));
        }
        (
            resolved
                .parent()
                .ok_or_else(|| AppError::Config("SKILL.md 缺少父目录".to_string()))?
                .to_path_buf(),
            resolved,
        )
    };

    let raw = read_file_utf8(&skill_file)?;
    let frontmatter =
        parse_skill_frontmatter(&raw).map_err(|error| AppError::Config(error.to_string()))?;
    Ok((skill_root, frontmatter.name, frontmatter.description))
}

fn collect_cross_layer_warnings(
    cfg: &AppConfig,
    scope_root: Option<&Path>,
    target_visibility: PackageVisibility,
    detected: &DetectedPackageSource,
) -> Result<Vec<String>, AppError> {
    let layers = resolve_runtime_layer_paths(cfg, scope_root)?;
    let target_index = PackageVisibility::ordered_runtime_layers()
        .iter()
        .position(|visibility| *visibility == target_visibility)
        .unwrap_or(0);
    let mut warnings = Vec::new();
    for resource in &detected.resources {
        for layer in &layers {
            if layer.visibility == target_visibility {
                continue;
            }
            let resource_path = match resource.kind {
                PackageResourceKind::Plugin => layer.plugins_dir.join(&resource.id),
                PackageResourceKind::Skill => layer.skills_dir.join(&resource.id),
            };
            if !resource_path.exists() {
                continue;
            }
            let other_index = PackageVisibility::ordered_runtime_layers()
                .iter()
                .position(|visibility| *visibility == layer.visibility)
                .unwrap_or(0);
            if other_index < target_index {
                warnings.push(format!(
                    "{} `{}` 在更高优先级层已存在（{}），当前安装后会被遮蔽",
                    resource.kind.as_str(),
                    resource.id,
                    layer.visibility
                ));
            } else {
                warnings.push(format!(
                    "{} `{}` 也存在于更低优先级层（{}），当前安装后会覆盖其可见性",
                    resource.kind.as_str(),
                    resource.id,
                    layer.visibility
                ));
            }
        }
    }
    Ok(warnings)
}

fn ordered_detected_resources(
    resources: &[DetectedPackageResource],
) -> impl Iterator<Item = &DetectedPackageResource> {
    let mut ordered = resources.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|resource| match resource.kind {
        PackageResourceKind::Skill => 0_u8,
        PackageResourceKind::Plugin => 1_u8,
    });
    ordered.into_iter()
}

fn install_resource(
    resource: &PreparedInstallResource,
    force: bool,
) -> Result<InstallFsMutation, AppError> {
    let parent = resource
        .destination_dir
        .parent()
        .ok_or_else(|| AppError::Config("目标目录无父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(AppError::Io)?;

    let stage_dir = hidden_sibling_path(parent, &resource.id, "staging");
    copy_dir_recursive(&resource.source_dir, &stage_dir).inspect_err(|_| {
        let _ = remove_path_if_exists(&stage_dir);
    })?;

    let backup_dir = if resource.destination_dir.exists() {
        if !force {
            let _ = remove_path_if_exists(&stage_dir);
            return Err(AppError::Config(format!(
                "目标已存在且未开启 force: {}",
                resource.destination_dir.display()
            )));
        }
        let backup = hidden_sibling_path(parent, &resource.id, "backup");
        fs::rename(&resource.destination_dir, &backup).map_err(AppError::Io)?;
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(&stage_dir, &resource.destination_dir) {
        let _ = remove_path_if_exists(&stage_dir);
        if let Some(backup) = &backup_dir {
            let _ = fs::rename(backup, &resource.destination_dir);
        }
        return Err(AppError::Io(error));
    }

    Ok(InstallFsMutation {
        destination_dir: resource.destination_dir.clone(),
        backup_dir,
        stage_dir,
    })
}

fn rollback_install(
    layer_paths: &LayerPaths,
    package_snapshot: &RegistrySnapshot,
    plugin_snapshot: &RegistrySnapshot,
    mutations: &[InstallFsMutation],
) -> Vec<String> {
    let mut errors = Vec::new();
    for mutation in mutations.iter().rev() {
        if let Err(error) = remove_path_if_exists(&mutation.destination_dir) {
            errors.push(format!(
                "remove {} failed: {error}",
                mutation.destination_dir.display()
            ));
        }
        if let Some(backup_dir) = &mutation.backup_dir {
            if backup_dir.exists() {
                if let Err(error) = fs::rename(backup_dir, &mutation.destination_dir) {
                    errors.push(format!(
                        "restore {} failed: {error}",
                        mutation.destination_dir.display()
                    ));
                }
            }
        }
        if mutation.stage_dir.exists() {
            if let Err(error) = remove_path_if_exists(&mutation.stage_dir) {
                errors.push(format!(
                    "cleanup stage {} failed: {error}",
                    mutation.stage_dir.display()
                ));
            }
        }
    }

    if let Err(error) = package_snapshot.restore(&layer_paths.package_registry_path) {
        errors.push(format!("restore package registry failed: {error}"));
    }
    if let Err(error) = plugin_snapshot.restore(&layer_paths.plugin_registry_path) {
        errors.push(format!("restore plugin registry failed: {error}"));
    }
    errors
}

fn load_registry<T>(path: &Path) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = fs::read_to_string(path).map_err(AppError::Io)?;
    serde_json::from_str(&raw)
        .map_err(|_| AppError::Config(format!("registry 损坏: {}", path.display())))
}

fn save_registry<T>(path: &Path, registry: &T) -> Result<(), AppError>
where
    T: serde::Serialize,
{
    let json = serde_json::to_vec_pretty(registry).map_err(AppError::Serialize)?;
    write_file_atomic(path, &json)
}

fn canonicalize_existing_path(path: &Path) -> Result<PathBuf, AppError> {
    path.canonicalize().map_err(|error| {
        AppError::Config(format!("路径不存在或不可读: {} ({error})", path.display()))
    })
}

fn hidden_sibling_path(parent: &Path, stem: &str, suffix: &str) -> PathBuf {
    parent.join(format!(".{stem}.{suffix}.{}", Uuid::new_v4()))
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(source).map_err(AppError::Io)?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::Permission(format!(
            "不支持复制符号链接目录: {}",
            source.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(AppError::Config(format!(
            "待安装资源必须是目录: {}",
            source.display()
        )));
    }
    fs::create_dir_all(target).map_err(AppError::Io)?;
    for entry in fs::read_dir(source).map_err(AppError::Io)? {
        let entry = entry.map_err(AppError::Io)?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(AppError::Io)?;
        if file_type.is_symlink() {
            return Err(AppError::Permission(format!(
                "不支持复制符号链接文件: {}",
                source_path.display()
            )));
        }
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).map_err(AppError::Io)?;
        }
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<bool, AppError> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(AppError::Io)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(AppError::Io)?;
    } else {
        fs::remove_file(path).map_err(AppError::Io)?;
    }
    Ok(true)
}

#[derive(Debug, Clone)]
struct InstallFsMutation {
    destination_dir: PathBuf,
    backup_dir: Option<PathBuf>,
    stage_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct RegistrySnapshot {
    existed: bool,
    raw_json: String,
}

impl RegistrySnapshot {
    fn capture_package(path: &Path) -> Self {
        Self::capture(path)
    }

    fn capture_plugin(path: &Path) -> Self {
        Self::capture(path)
    }

    fn capture(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(raw_json) => Self {
                existed: true,
                raw_json,
            },
            Err(_) => Self {
                existed: false,
                raw_json: String::new(),
            },
        }
    }

    fn package_value(&self) -> PackageRegistryFile {
        if !self.existed {
            return PackageRegistryFile::default();
        }
        serde_json::from_str(&self.raw_json).unwrap_or_default()
    }

    fn plugin_value(&self) -> PluginRegistryFile {
        if !self.existed {
            return PluginRegistryFile::default();
        }
        serde_json::from_str(&self.raw_json).unwrap_or_default()
    }

    fn restore(&self, path: &Path) -> Result<(), AppError> {
        if self.existed {
            write_file_atomic(path, self.raw_json.as_bytes())
        } else if path.exists() {
            fs::remove_file(path).map_err(AppError::Io)
        } else {
            Ok(())
        }
    }
}
