use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::paths::LayerPaths;

pub const PACKAGE_REGISTRY_SCHEMA_V1: &str = "tomcat.package.registry.v1";
pub const PACKAGE_MANIFEST_SCHEMA_V1: &str = "tomcat.package.v1";

fn default_package_registry_schema() -> String {
    PACKAGE_REGISTRY_SCHEMA_V1.to_string()
}

fn default_package_manifest_schema() -> String {
    PACKAGE_MANIFEST_SCHEMA_V1.to_string()
}

fn default_package_source_kind() -> PackageSourceKind {
    PackageSourceKind::Local
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackageVisibility {
    Scope,
    Agent,
    Global,
}

impl PackageVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Agent => "agent",
            Self::Global => "global",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::Scope => "current-project",
            Self::Agent => "agent",
            Self::Global => "global",
        }
    }

    pub fn ordered_runtime_layers() -> [Self; 3] {
        [Self::Scope, Self::Agent, Self::Global]
    }
}

impl std::fmt::Display for PackageVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackageSourceKind {
    #[serde(alias = "package", alias = "barePlugin", alias = "bareSkill")]
    Local,
}

impl PackageSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectedPackageSourceKind {
    Package,
    BarePlugin,
    BareSkill,
}

impl DetectedPackageSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::BarePlugin => "barePlugin",
            Self::BareSkill => "bareSkill",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PackageResourceKind {
    Plugin,
    Skill,
}

impl PackageResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Skill => "skill",
        }
    }

    pub fn registry_dir(self) -> &'static str {
        match self {
            Self::Plugin => "plugins",
            Self::Skill => "skills",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    #[serde(default = "default_package_manifest_schema")]
    pub schema: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
}

impl PackageManifest {
    pub fn single_plugin(
        package_name: String,
        version: String,
        description: Option<String>,
        plugin_ref: String,
    ) -> Self {
        Self {
            schema: default_package_manifest_schema(),
            name: package_name,
            version,
            description,
            plugins: vec![plugin_ref],
            skills: vec![],
        }
    }

    pub fn single_skill(
        package_name: String,
        description: Option<String>,
        skill_ref: String,
    ) -> Self {
        Self {
            schema: default_package_manifest_schema(),
            name: package_name,
            version: "0.0.0".to_string(),
            description,
            plugins: vec![],
            skills: vec![skill_ref],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePluginRecord {
    pub id: String,
    pub relative_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSkillRecord {
    pub name: String,
    pub relative_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyPackageResource {
    pub kind: PackageResourceKind,
    pub id: String,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub install_subpath: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_package_source_kind")]
    pub source_kind: PackageSourceKind,
    pub visibility: PackageVisibility,
    #[serde(alias = "source_path")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_root: Option<String>,
    pub installed_at: String,
    #[serde(default)]
    pub plugins: Vec<PackagePluginRecord>,
    #[serde(default)]
    pub skills: Vec<PackageSkillRecord>,
    #[serde(default, rename = "resources", skip_serializing)]
    pub(crate) legacy_resources: Vec<LegacyPackageResource>,
}

impl PackageRecord {
    pub fn resource_count(&self) -> usize {
        self.plugins.len() + self.skills.len()
    }

    pub fn resource_descriptors(&self) -> Vec<(PackageResourceKind, String)> {
        let mut out = Vec::with_capacity(self.resource_count());
        out.extend(
            self.plugins
                .iter()
                .map(|plugin| (PackageResourceKind::Plugin, plugin.id.clone())),
        );
        out.extend(
            self.skills
                .iter()
                .map(|skill| (PackageResourceKind::Skill, skill.name.clone())),
        );
        out
    }

    pub(crate) fn normalize_legacy_resources(&mut self) {
        if !self.legacy_resources.is_empty() {
            for resource in self.legacy_resources.drain(..) {
                let relative_dir = if resource.source_path.trim().is_empty() {
                    legacy_relative_dir(&resource)
                } else {
                    resource.source_path
                };
                match resource.kind {
                    PackageResourceKind::Plugin => {
                        if self.plugins.iter().any(|plugin| plugin.id == resource.id) {
                            continue;
                        }
                        self.plugins.push(PackagePluginRecord {
                            id: resource.id,
                            relative_dir,
                        });
                    }
                    PackageResourceKind::Skill => {
                        if self.skills.iter().any(|skill| skill.name == resource.id) {
                            continue;
                        }
                        self.skills.push(PackageSkillRecord {
                            name: resource.id,
                            relative_dir,
                        });
                    }
                }
            }
        }
    }
}

fn legacy_relative_dir(resource: &LegacyPackageResource) -> String {
    if resource.install_subpath.trim().is_empty() {
        return ".".to_string();
    }
    resource
        .install_subpath
        .split_once('/')
        .map(|(_, suffix)| suffix.to_string())
        .unwrap_or_else(|| ".".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRegistryFile {
    #[serde(default = "default_package_registry_schema")]
    pub schema: String,
    #[serde(default)]
    pub packages: Vec<PackageRecord>,
}

impl Default for PackageRegistryFile {
    fn default() -> Self {
        Self {
            schema: default_package_registry_schema(),
            packages: Vec::new(),
        }
    }
}

impl PackageRegistryFile {
    pub fn normalize(&mut self) {
        if self.schema.trim().is_empty() {
            self.schema = default_package_registry_schema();
        }
        for record in &mut self.packages {
            record.normalize_legacy_resources();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRegistryEntry {
    pub id: String,
    pub path: String,
    pub enabled: bool,
    pub loaded_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRegistryFile {
    #[serde(default)]
    pub plugins: Vec<PluginRegistryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPackageResource {
    pub kind: PackageResourceKind,
    pub id: String,
    pub source_path: String,
    pub source_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPackageSource {
    pub kind: DetectedPackageSourceKind,
    pub source_root: PathBuf,
    pub manifest: PackageManifest,
    pub resources: Vec<DetectedPackageResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInstallResource {
    pub kind: PackageResourceKind,
    pub id: String,
    pub source_path: String,
    pub source_dir: PathBuf,
    pub destination_dir: PathBuf,
    pub install_subpath: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInstall {
    pub detected: DetectedPackageSource,
    pub visibility: PackageVisibility,
    pub layer_paths: LayerPaths,
    pub warnings: Vec<String>,
    pub force: bool,
    pub resources: Vec<PreparedInstallResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub record: PackageRecord,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallOutcome {
    pub record: PackageRecord,
    pub removed_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLayerListing {
    pub visibility: PackageVisibility,
    pub records: Vec<PackageRecord>,
}
