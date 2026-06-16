use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;

use crate::core::skill::parse as parse_skill_frontmatter;
use crate::ext::parse_manifest as parse_plugin_manifest;
use crate::infra::{read_file_utf8, AppError};
use crate::AppConfig;

use super::model::{
    DetectedPackageResource, DetectedPackageSource, DetectedPackageSourceKind, InstallOutcome,
    PackageLayerListing, PackageManifest, PackagePluginRecord, PackageRecord, PackageResourceKind,
    PackageSkillRecord, PackageSourceKind, PackageVisibility, PluginRegistryEntry, PreparedInstall,
    PreparedInstallResource, UninstallOutcome, PACKAGE_MANIFEST_SCHEMA_V1,
};
use super::paths::{resolve_layer_paths, resolve_runtime_layer_paths};

mod install_fs;
mod registry;

use self::install_fs::{
    cleanup_install_artifacts, install_resource, prepare_force_remove_path, remove_path_if_exists,
    rollback_install,
};
use self::registry::RegistrySnapshot;
pub use self::registry::{
    load_package_registry, load_plugin_registry, save_package_registry, save_plugin_registry,
};

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
        let package_registry = load_package_registry(&layer_paths.package_registry_path)?;
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
            let mut package_registry = package_snapshot.package_value()?;
            let previous_record = package_registry
                .packages
                .iter()
                .find(|record| record.name == prepared.detected.manifest.name)
                .cloned();
            package_registry
                .packages
                .retain(|record| record.name != prepared.detected.manifest.name);
            let mut plugin_registry = plugin_snapshot.plugin_value()?;
            let plugin_ids = prepared
                .resources
                .iter()
                .filter(|resource| resource.kind == PackageResourceKind::Plugin)
                .map(|resource| resource.id.clone())
                .collect::<HashSet<_>>();
            let skill_ids = prepared
                .resources
                .iter()
                .filter(|resource| resource.kind == PackageResourceKind::Skill)
                .map(|resource| resource.id.clone())
                .collect::<HashSet<_>>();
            let removed_plugin_ids = previous_record
                .as_ref()
                .map(|record| {
                    record
                        .plugins
                        .iter()
                        .map(|plugin| plugin.id.clone())
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default();
            let all_replaced_plugin_ids = removed_plugin_ids
                .union(&plugin_ids)
                .cloned()
                .collect::<HashSet<_>>();
            plugin_registry
                .plugins
                .retain(|entry| !all_replaced_plugin_ids.contains(&entry.id));

            if let Some(previous_record) = &previous_record {
                for plugin in previous_record
                    .plugins
                    .iter()
                    .filter(|plugin| !plugin_ids.contains(&plugin.id))
                {
                    if let Some(mutation) = prepare_force_remove_path(
                        &prepared.layer_paths.plugins_dir.join(&plugin.id),
                    )? {
                        mutations.push(mutation);
                    }
                }
                for skill in previous_record
                    .skills
                    .iter()
                    .filter(|skill| !skill_ids.contains(&skill.name))
                {
                    if let Some(mutation) = prepare_force_remove_path(
                        &prepared.layer_paths.skills_dir.join(&skill.name),
                    )? {
                        mutations.push(mutation);
                    }
                }
            }

            for resource in &prepared.resources {
                mutations.push(install_resource(resource, prepared.force)?);
            }

            let installed_at = Utc::now().to_rfc3339();
            let record = PackageRecord {
                name: prepared.detected.manifest.name.clone(),
                version: prepared.detected.manifest.version.clone(),
                description: prepared.detected.manifest.description.clone(),
                source_kind: PackageSourceKind::Local,
                visibility: prepared.visibility,
                source: prepared.detected.source_root.display().to_string(),
                scope_root: prepared
                    .layer_paths
                    .scope_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                installed_at: installed_at.clone(),
                plugins: prepared
                    .resources
                    .iter()
                    .filter(|resource| resource.kind == PackageResourceKind::Plugin)
                    .map(|resource| PackagePluginRecord {
                        id: resource.id.clone(),
                        relative_dir: resource.source_path.clone(),
                    })
                    .collect(),
                skills: prepared
                    .resources
                    .iter()
                    .filter(|resource| resource.kind == PackageResourceKind::Skill)
                    .map(|resource| PackageSkillRecord {
                        name: resource.id.clone(),
                        relative_dir: resource.source_path.clone(),
                    })
                    .collect(),
                legacy_resources: Vec::new(),
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

            cleanup_install_artifacts(&mutations);

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
        let mut package_registry = load_package_registry(&layer_paths.package_registry_path)?;
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
        for plugin in &record.plugins {
            let path = layer_paths.plugins_dir.join(&plugin.id);
            if remove_path_if_exists(&path)? {
                removed_paths.push(path);
            }
        }
        for skill in &record.skills {
            let path = layer_paths.skills_dir.join(&skill.name);
            if remove_path_if_exists(&path)? {
                removed_paths.push(path);
            }
        }

        save_package_registry(&layer_paths.package_registry_path, &package_registry)?;

        let mut plugin_registry = load_plugin_registry(&layer_paths.plugin_registry_path)?;
        let plugin_ids = record
            .plugins
            .iter()
            .map(|plugin| plugin.id.clone())
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
        let mut listings = Vec::with_capacity(layers.len());
        for layer in layers {
            let registry = load_package_registry(&layer.package_registry_path)?;
            listings.push(PackageLayerListing {
                visibility: layer.visibility,
                records: registry.packages,
            });
        }
        Ok(listings)
    }
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
    schema: Option<String>,
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
    if tomcat.version.is_some() {
        return Err(AppError::Config(
            "tomcat.version 已废弃，请改用外层 package.json.version".to_string(),
        ));
    }
    let version = parsed
        .version
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Config("package.json.version 缺失".to_string()))?;
    let description = tomcat.description.or(parsed.description);
    let schema = tomcat
        .schema
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| PACKAGE_MANIFEST_SCHEMA_V1.to_string());
    let (plugins, skills) = if tomcat.plugins.is_empty() && tomcat.skills.is_empty() {
        auto_detect_package_entries(&root)?
    } else {
        (tomcat.plugins, tomcat.skills)
    };

    let manifest = PackageManifest {
        schema,
        name,
        version,
        description,
        plugins,
        skills,
    };
    if manifest.plugins.is_empty() && manifest.skills.is_empty() {
        return Err(AppError::Config(
            "package.tomcat.plugins / skills 不能同时为空，且未发现 plugins/* 或 skills/*"
                .to_string(),
        ));
    }

    let resources = resolve_package_resources(&root, &manifest)?;
    Ok(Some(DetectedPackageSource {
        kind: DetectedPackageSourceKind::Package,
        source_root: root,
        manifest,
        resources,
    }))
}

fn auto_detect_package_entries(root: &Path) -> Result<(Vec<String>, Vec<String>), AppError> {
    Ok((
        scan_package_entry_dirs(root, "plugins")?,
        scan_package_entry_dirs(root, "skills")?,
    ))
}

fn scan_package_entry_dirs(root: &Path, namespace: &str) -> Result<Vec<String>, AppError> {
    let namespace_dir = root.join(namespace);
    if !namespace_dir.exists() {
        return Ok(Vec::new());
    }
    if !namespace_dir.is_dir() {
        return Err(AppError::Config(format!(
            "package 根目录下的 {namespace} 必须是目录: {}",
            namespace_dir.display()
        )));
    }

    let mut entries = fs::read_dir(&namespace_dir)
        .map_err(AppError::Io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Io)?;
    entries.sort_by_key(|entry| entry.path());

    let mut out = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(AppError::Io)?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        out.push(format!(
            "{namespace}/{}",
            entry.file_name().to_string_lossy()
        ));
    }
    Ok(out)
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
        kind: DetectedPackageSourceKind::BarePlugin,
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
        kind: DetectedPackageSourceKind::BareSkill,
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

fn canonicalize_existing_path(path: &Path) -> Result<PathBuf, AppError> {
    path.canonicalize().map_err(|error| {
        AppError::Config(format!("路径不存在或不可读: {} ({error})", path.display()))
    })
}
