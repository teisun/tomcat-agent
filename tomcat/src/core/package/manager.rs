use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};

use chrono::Utc;
use serde::Deserialize;

use crate::core::skill::parse as parse_skill_frontmatter;
use crate::ext::parse_manifest as parse_plugin_manifest;
use crate::infra::config::with_config_lock;
use crate::infra::i18n::tr;
use crate::infra::{read_file_utf8, AppError};
use crate::AppConfig;

use super::model::{
    DetectedPackageResource, DetectedPackageSource, DetectedPackageSourceKind, InstallOutcome,
    PackageLayerListing, PackageManifest, PackagePluginRecord, PackageRecord, PackageResourceKind,
    PackageSkillRecord, PackageSourceKind, PackageVisibility, PluginRegistryEntry, PreparedInstall,
    PreparedInstallResource, UninstallOutcome, PACKAGE_MANIFEST_SCHEMA_V1,
};
use super::paths::{resolve_layer_paths, resolve_runtime_layer_paths, LayerPaths};

mod install_fs;
mod registry;

use self::install_fs::{
    cleanup_install_artifacts, install_resource, prepare_force_remove_path, rollback_install,
    InstallFsMutation,
};
use self::registry::RegistrySnapshot;
pub use self::registry::{
    load_package_registry, load_plugin_registry, save_package_registry, save_plugin_registry,
};

#[derive(Debug)]
pub struct PackageManager<'a> {
    cfg: &'a AppConfig,
}

/// Serializes every registry-changing operation in one resource layer. The
/// process-local mutex prevents competing chat/CLI tasks from racing first; the
/// file lock then extends the same order across processes. Callers must obtain
/// it only after user confirmation, never while waiting for a user decision.
pub fn with_resource_transaction_lock<R>(
    registry_path: &Path,
    f: impl FnOnce() -> Result<R, AppError>,
) -> Result<R, AppError> {
    let layer_root = registry_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            AppError::Config(tr(
                "package.layerParent",
                &[("path", &registry_path.display().to_string())],
            ))
        })?
        .to_path_buf();
    let key = layer_root.canonicalize().unwrap_or(layer_root);
    let slot = resource_transaction_coordinator()
        .write()
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _memory_guard = slot.lock();
    with_config_lock(&key.join(".tomcat-resource-transaction"), f)
}

fn resource_transaction_coordinator() -> &'static RwLock<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static COORDINATOR: OnceLock<RwLock<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    COORDINATOR.get_or_init(|| RwLock::new(HashMap::new()))
}

#[cfg(test)]
pub(crate) fn fail_next_registry_save_for_test(path: &Path) {
    registry::fail_next_save_for_test(path);
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
            return Err(AppError::Config(tr(
                "package.unknownSource",
                &[("path", &source.display().to_string())],
            )));
        }

        let Some(file_name) = source.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(tr(
                "package.invalidFilename",
                &[("name", "source"), ("path", &source.display().to_string())],
            )));
        };
        match file_name {
            "package.json" => detect_package_manifest_file(&source, true)?.ok_or_else(|| {
                AppError::Config(tr(
                    "package.tomcatMissing",
                    &[("path", &source.display().to_string())],
                ))
            }),
            "plugin.json" => detect_bare_plugin(&source),
            "SKILL.md" => detect_bare_skill(&source),
            _ => Err(AppError::Config(tr(
                "package.sourceTypes",
                &[("path", &source.display().to_string())],
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
            return Err(AppError::Config(tr(
                "package.exists",
                &[("kind", "package"), ("id", &detected.manifest.name)],
            )));
        }

        let mut warnings =
            collect_cross_layer_warnings(self.cfg, scope_root, visibility, &detected)?;
        let mut resources = Vec::with_capacity(detected.resources.len());
        for resource in ordered_detected_resources(&detected.resources) {
            let destination_dir = resource_target_dir(&layer_paths, resource.kind, &resource.id)?;
            if destination_dir.exists() && !force {
                return Err(AppError::Config(tr(
                    "package.exists",
                    &[("kind", resource.kind.as_str()), ("id", &resource.id)],
                )));
            }
            if destination_dir.exists() && force {
                warnings.push(tr(
                    "package.overwrite",
                    &[("kind", resource.kind.as_str()), ("id", &resource.id)],
                ));
            }
            resources.push(PreparedInstallResource {
                kind: resource.kind,
                id: resource.id.clone(),
                source_path: resource.source_path.clone(),
                source_dir: resource.source_dir.clone(),
                source_digest: install_fs::source_tree_digest(&resource.source_dir)?,
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
        let registry_path = prepared.layer_paths.package_registry_path.clone();
        with_resource_transaction_lock(&registry_path, || self.install_locked(prepared))
    }

    fn install_locked(&self, prepared: PreparedInstall) -> Result<InstallOutcome, AppError> {
        let package_snapshot =
            RegistrySnapshot::capture_package(&prepared.layer_paths.package_registry_path)?;
        let plugin_snapshot =
            RegistrySnapshot::capture_plugin(&prepared.layer_paths.plugin_registry_path)?;
        let mut mutations = Vec::new();

        let install_result = (|| -> Result<InstallOutcome, AppError> {
            let mut package_registry = package_snapshot.package_value()?;
            let mut plugin_registry = plugin_snapshot.plugin_value()?;
            if !prepared.force
                && package_registry
                    .packages
                    .iter()
                    .any(|record| record.name == prepared.detected.manifest.name)
            {
                return Err(AppError::Config(tr(
                    "package.installedDuringWait",
                    &[
                        ("kind", "package"),
                        ("id", &prepared.detected.manifest.name),
                    ],
                )));
            }
            for resource in &prepared.resources {
                let owned_by_other_package = package_registry.packages.iter().any(|record| {
                    record.name != prepared.detected.manifest.name
                        && match resource.kind {
                            PackageResourceKind::Plugin => {
                                record.plugins.iter().any(|plugin| plugin.id == resource.id)
                            }
                            PackageResourceKind::Skill => {
                                record.skills.iter().any(|skill| skill.name == resource.id)
                            }
                        }
                });
                if owned_by_other_package {
                    return Err(AppError::Config(tr(
                        "package.foreignOwner",
                        &[("kind", resource.kind.as_str()), ("id", &resource.id)],
                    )));
                }
                if resource.destination_dir.exists() && !prepared.force {
                    return Err(AppError::Config(tr(
                        "package.installedDuringWait",
                        &[("kind", resource.kind.as_str()), ("id", &resource.id)],
                    )));
                }
                if resource.kind == PackageResourceKind::Plugin
                    && plugin_registry
                        .plugins
                        .iter()
                        .any(|entry| entry.id == resource.id)
                    && !prepared.force
                {
                    return Err(AppError::Config(tr(
                        "package.pluginOwner",
                        &[("id", &resource.id)],
                    )));
                }
            }
            let previous_record = package_registry
                .packages
                .iter()
                .find(|record| record.name == prepared.detected.manifest.name)
                .cloned();
            package_registry
                .packages
                .retain(|record| record.name != prepared.detected.manifest.name);
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
                    if let Some(mutation) = prepare_force_remove_path(&resource_target_dir(
                        &prepared.layer_paths,
                        PackageResourceKind::Plugin,
                        &plugin.id,
                    )?)? {
                        mutations.push(mutation);
                    }
                }
                for skill in previous_record
                    .skills
                    .iter()
                    .filter(|skill| !skill_ids.contains(&skill.name))
                {
                    if let Some(mutation) = prepare_force_remove_path(&resource_target_dir(
                        &prepared.layer_paths,
                        PackageResourceKind::Skill,
                        &skill.name,
                    )?)? {
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
                    Err(AppError::Config(tr(
                        "package.operationFailed",
                        &[("operation", "install"), ("detail", &error.to_string())],
                    )))
                } else {
                    Err(AppError::Config(tr(
                        "package.rollbackIncomplete",
                        &[
                            ("operation", "install"),
                            ("detail", &error.to_string()),
                            ("dirty", &rollback_errors.join(" | ")),
                        ],
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
        let registry_path = layer_paths.package_registry_path.clone();
        with_resource_transaction_lock(&registry_path, || {
            self.uninstall_locked(package_name, layer_paths)
        })
    }

    fn uninstall_locked(
        &self,
        package_name: &str,
        layer_paths: LayerPaths,
    ) -> Result<UninstallOutcome, AppError> {
        let package_snapshot =
            RegistrySnapshot::capture_package(&layer_paths.package_registry_path)?;
        let plugin_snapshot = RegistrySnapshot::capture_plugin(&layer_paths.plugin_registry_path)?;
        let mut mutations = Vec::new();
        let uninstall_result = (|| -> Result<UninstallOutcome, AppError> {
            let mut package_registry = package_snapshot.package_value()?;
            let Some(index) = package_registry
                .packages
                .iter()
                .position(|record| record.name == package_name)
            else {
                return Err(AppError::Config(tr(
                    "package.notInstalled",
                    &[
                        ("layer", &layer_paths.visibility.to_string()),
                        ("name", package_name),
                        (
                            "plugins",
                            &crate::infra::platform::format_home_path(&layer_paths.plugins_dir),
                        ),
                        (
                            "skills",
                            &crate::infra::platform::format_home_path(&layer_paths.skills_dir),
                        ),
                    ],
                )));
            };
            let record = package_registry.packages.remove(index);

            for plugin in &record.plugins {
                let path =
                    resource_target_dir(&layer_paths, PackageResourceKind::Plugin, &plugin.id)?;
                if let Some(mutation) = prepare_force_remove_path(&path)? {
                    mutations.push(mutation);
                }
            }
            for skill in &record.skills {
                let path =
                    resource_target_dir(&layer_paths, PackageResourceKind::Skill, &skill.name)?;
                if let Some(mutation) = prepare_force_remove_path(&path)? {
                    mutations.push(mutation);
                }
            }

            save_package_registry(&layer_paths.package_registry_path, &package_registry)?;
            let mut plugin_registry = plugin_snapshot.plugin_value()?;
            let plugin_ids = record
                .plugins
                .iter()
                .map(|plugin| plugin.id.clone())
                .collect::<HashSet<_>>();
            plugin_registry
                .plugins
                .retain(|entry| !plugin_ids.contains(&entry.id));
            save_plugin_registry(&layer_paths.plugin_registry_path, &plugin_registry)?;

            let removed_paths = mutations
                .iter()
                .filter_map(|mutation| match mutation {
                    InstallFsMutation::Removed { original_path, .. } => Some(original_path.clone()),
                    InstallFsMutation::Installed { .. } => None,
                })
                .collect();
            cleanup_install_artifacts(&mutations);
            Ok(UninstallOutcome {
                record,
                removed_paths,
            })
        })();

        match uninstall_result {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let rollback_errors = rollback_install(
                    &layer_paths,
                    &package_snapshot,
                    &plugin_snapshot,
                    &mutations,
                );
                if rollback_errors.is_empty() {
                    Err(AppError::Config(tr(
                        "package.operationFailed",
                        &[("operation", "uninstall"), ("detail", &error.to_string())],
                    )))
                } else {
                    Err(AppError::Config(tr(
                        "package.rollbackIncomplete",
                        &[
                            ("operation", "uninstall"),
                            ("detail", &error.to_string()),
                            ("dirty", &rollback_errors.join(" | ")),
                        ],
                    )))
                }
            }
        }
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
        .ok_or_else(|| AppError::Config(tr("package.noParent", &[("file", "package.json")])))?
        .canonicalize()
        .map_err(AppError::Io)?;
    let raw = read_file_utf8(manifest_path)?;
    let parsed: RawPackageJson = serde_json::from_str(&raw).map_err(|error| {
        AppError::Config(tr("package.jsonParse", &[("detail", &error.to_string())]))
    })?;
    let Some(tomcat) = parsed.tomcat else {
        return if require_tomcat_block {
            Err(AppError::Config(tr(
                "package.tomcatMissing",
                &[("path", &manifest_path.display().to_string())],
            )))
        } else {
            Ok(None)
        };
    };

    let name = tomcat
        .name
        .or(parsed.name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::Config(tr(
                "package.fieldMissing",
                &[("field", "package.tomcat.name")],
            ))
        })?;
    if tomcat.version.is_some() {
        return Err(AppError::Config(tr("package.deprecatedVersion", &[])));
    }
    let version = parsed
        .version
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::Config(tr(
                "package.fieldMissing",
                &[("field", "package.json.version")],
            ))
        })?;
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
        return Err(AppError::Config(tr("package.emptyResources", &[])));
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
        return Err(AppError::Config(tr(
            "package.namespaceDir",
            &[
                ("namespace", namespace),
                ("path", &namespace_dir.display().to_string()),
            ],
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
        let plugin_path = resolve_package_resource_path(root, plugin, "plugin")?;
        let (plugin_root, plugin_manifest) = resolve_plugin_source(&plugin_path)?;
        let id = plugin_manifest.id.clone();
        if !seen.insert((PackageResourceKind::Plugin.as_str().to_string(), id.clone())) {
            return Err(AppError::Config(tr(
                "package.duplicateResource",
                &[("kind", "plugin"), ("id", &id)],
            )));
        }
        resources.push(DetectedPackageResource {
            kind: PackageResourceKind::Plugin,
            id,
            source_path: plugin.clone(),
            source_dir: plugin_root,
        });
    }
    for skill in &manifest.skills {
        let skill_path = resolve_package_resource_path(root, skill, "skill")?;
        let (skill_root, skill_name, _description) = resolve_skill_source(&skill_path)?;
        if !seen.insert((
            PackageResourceKind::Skill.as_str().to_string(),
            skill_name.clone(),
        )) {
            return Err(AppError::Config(tr(
                "package.duplicateResource",
                &[("kind", "skill"), ("id", &skill_name)],
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
            return Err(AppError::Config(tr(
                "package.missingManifest",
                &[
                    ("kind", "plugin"),
                    ("file", "plugin.json"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        }
        (resolved, manifest_path)
    } else {
        let Some(file_name) = resolved.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(tr(
                "package.invalidFilename",
                &[
                    ("name", "plugin.json"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        };
        if file_name != "plugin.json" {
            return Err(AppError::Config(tr(
                "package.sourceFileOnly",
                &[
                    ("kind", "plugin"),
                    ("file", "plugin.json"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        }
        (
            resolved
                .parent()
                .ok_or_else(|| {
                    AppError::Config(tr("package.noParent", &[("file", "plugin.json")]))
                })?
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
        AppError::Config(tr(
            "package.mainUnreadable",
            &[("path", &manifest.main), ("detail", &error.to_string())],
        ))
    })?;
    if !main_path.starts_with(&root) {
        return Err(AppError::Permission(tr(
            "package.mainOutside",
            &[("path", &main_path.display().to_string())],
        )));
    }
    if !main_path.is_file() {
        return Err(AppError::Config(tr(
            "package.mainFile",
            &[("path", &main_path.display().to_string())],
        )));
    }
    Ok(())
}

fn resolve_skill_source(path: &Path) -> Result<(PathBuf, String, String), AppError> {
    let resolved = canonicalize_existing_path(path)?;
    let (skill_root, skill_file) = if resolved.is_dir() {
        let skill_file = resolved.join("SKILL.md");
        if !skill_file.is_file() {
            return Err(AppError::Config(tr(
                "package.missingManifest",
                &[
                    ("kind", "skill"),
                    ("file", "SKILL.md"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        }
        (resolved, skill_file)
    } else {
        let Some(file_name) = resolved.file_name().and_then(|name| name.to_str()) else {
            return Err(AppError::Config(tr(
                "package.invalidFilename",
                &[
                    ("name", "SKILL.md"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        };
        if file_name != "SKILL.md" {
            return Err(AppError::Config(tr(
                "package.sourceFileOnly",
                &[
                    ("kind", "skill"),
                    ("file", "SKILL.md"),
                    ("path", &resolved.display().to_string()),
                ],
            )));
        }
        (
            resolved
                .parent()
                .ok_or_else(|| AppError::Config(tr("package.noParent", &[("file", "SKILL.md")])))?
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
            let resource_path = resource_target_dir(layer, resource.kind, &resource.id)?;
            if !resource_path.exists() {
                continue;
            }
            let other_index = PackageVisibility::ordered_runtime_layers()
                .iter()
                .position(|visibility| *visibility == layer.visibility)
                .unwrap_or(0);
            if other_index < target_index {
                warnings.push(tr(
                    "package.shadowed",
                    &[
                        ("kind", resource.kind.as_str()),
                        ("id", &resource.id),
                        ("layer", &layer.visibility.to_string()),
                    ],
                ));
            } else {
                warnings.push(tr(
                    "package.shadows",
                    &[
                        ("kind", resource.kind.as_str()),
                        ("id", &resource.id),
                        ("layer", &layer.visibility.to_string()),
                    ],
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

/// Produce a managed resource directory only from a single safe resource identifier.
///
/// Package records are persisted input: validate them again at every destructive IO
/// boundary so a hand-edited or old registry cannot turn `join` into traversal.
fn resource_target_dir(
    layer: &LayerPaths,
    kind: PackageResourceKind,
    id: &str,
) -> Result<PathBuf, AppError> {
    validate_resource_id(id, kind.as_str())?;
    if let Ok(metadata) = fs::symlink_metadata(&layer.layer_root) {
        if metadata.file_type().is_symlink() {
            return Err(AppError::Permission(tr(
                "package.layerSymlink",
                &[("path", &layer.layer_root.display().to_string())],
            )));
        }
    }
    let root = match kind {
        PackageResourceKind::Plugin => &layer.plugins_dir,
        PackageResourceKind::Skill => &layer.skills_dir,
    };
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if metadata.file_type().is_symlink() {
            return Err(AppError::Permission(tr(
                "package.rootSymlink",
                &[
                    ("kind", kind.as_str()),
                    ("path", &root.display().to_string()),
                ],
            )));
        }
    }
    Ok(root.join(id))
}

fn validate_resource_id(id: &str, label: &str) -> Result<(), AppError> {
    let path = Path::new(id);
    if id.trim().is_empty()
        || id.contains('\\')
        || path.is_absolute()
        || !matches!(path.components().next(), Some(Component::Normal(component)) if component == std::ffi::OsStr::new(id))
        || path.components().nth(1).is_some()
    {
        return Err(AppError::Config(tr(
            "package.invalidId",
            &[("kind", label), ("id", &format!("{id:?}"))],
        )));
    }
    Ok(())
}

/// Resolve an explicit package entry without allowing it to escape or pass through
/// a symlink. This happens before parsing manifests and before any destination IO.
fn resolve_package_resource_path(
    root: &Path,
    reference: &str,
    label: &str,
) -> Result<PathBuf, AppError> {
    let relative = Path::new(reference);
    if reference.trim().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::Config(tr(
            "package.relativeRef",
            &[("kind", label), ("reference", &format!("{reference:?}"))],
        )));
    }
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        if let Component::Normal(segment) = component {
            candidate.push(segment);
            if fs::symlink_metadata(&candidate)
                .map_err(AppError::Io)?
                .file_type()
                .is_symlink()
            {
                return Err(AppError::Permission(tr(
                    "package.symlinkRef",
                    &[("kind", label), ("path", &candidate.display().to_string())],
                )));
            }
        }
    }
    let resolved = canonicalize_existing_path(&candidate)?;
    if !resolved.starts_with(root) {
        return Err(AppError::Permission(tr(
            "package.outsideRef",
            &[("kind", label), ("path", &resolved.display().to_string())],
        )));
    }
    Ok(resolved)
}

fn canonicalize_existing_path(path: &Path) -> Result<PathBuf, AppError> {
    path.canonicalize().map_err(|error| {
        AppError::Config(tr(
            "package.pathUnreadable",
            &[
                ("path", &path.display().to_string()),
                ("detail", &error.to_string()),
            ],
        ))
    })
}
