use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::paths::LayerPaths;

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
    Package,
    BarePlugin,
    BareSkill,
}

impl PackageSourceKind {
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
            name: package_name,
            version: "0.0.0".to_string(),
            description,
            plugins: vec![],
            skills: vec![skill_ref],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageResource {
    pub kind: PackageResourceKind,
    pub id: String,
    pub source_path: String,
    pub install_subpath: String,
}

impl PackageResource {
    pub fn new(
        kind: PackageResourceKind,
        id: impl Into<String>,
        source_path: impl Into<String>,
        install_subpath: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            source_path: source_path.into(),
            install_subpath: install_subpath.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source_kind: PackageSourceKind,
    pub visibility: PackageVisibility,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_root: Option<String>,
    pub installed_at: String,
    #[serde(default)]
    pub resources: Vec<PackageResource>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRegistryFile {
    #[serde(default)]
    pub packages: Vec<PackageRecord>,
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
    pub kind: PackageSourceKind,
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
