use super::source_scan::plugin_roots;
use super::{parse_manifest, PluginManifest};
use crate::infra::error::AppError;
use crate::AppConfig;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSource {
    Project,
    Agent,
    Managed,
}

impl PluginSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Agent => "agent",
            Self::Managed => "managed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginCatalogDiagnostic {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub manifest: PluginManifest,
    pub manifest_path: PathBuf,
    pub plugin_root: PathBuf,
    pub source: PluginSource,
}

#[derive(Debug, Clone, Default)]
pub struct PluginCatalog {
    entries: BTreeMap<String, CatalogEntry>,
    pub warnings: Vec<String>,
    pub diagnostics: Vec<PluginCatalogDiagnostic>,
}

impl PluginCatalog {
    pub fn discover(cfg: &AppConfig, agent_workspace_dir: &Path) -> Result<Self, AppError> {
        let mut catalog = Self::default();
        for (source, root) in plugin_roots(cfg, agent_workspace_dir)? {
            scan_root(&root, source, &mut catalog);
        }
        Ok(catalog)
    }

    pub fn get(&self, plugin_id: &str) -> Option<&CatalogEntry> {
        self.entries.get(plugin_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &CatalogEntry)> {
        self.entries.iter()
    }

    fn insert_entry(&mut self, entry: CatalogEntry) {
        let plugin_id = entry.manifest.id.clone();
        if let Some(existing) = self.entries.get(&plugin_id) {
            self.warnings.push(format!(
                "plugin_shadowed:{} by {}",
                plugin_id,
                existing.source.as_str()
            ));
            return;
        }
        self.entries.insert(plugin_id, entry);
    }
}

fn scan_root(root: &Path, source: PluginSource, catalog: &mut PluginCatalog) {
    if !root.exists() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(root) else {
        catalog.diagnostics.push(PluginCatalogDiagnostic {
            path: root.to_path_buf(),
            reason: "plugin 根目录不可读取".to_string(),
        });
        return;
    };

    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            catalog.diagnostics.push(PluginCatalogDiagnostic {
                path: entry.path(),
                reason: "无法读取目录项类型".to_string(),
            });
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let Some((manifest_path, plugin_root)) = resolve_candidate(&path, file_type.is_dir())
        else {
            continue;
        };

        match read_catalog_entry(&manifest_path, &plugin_root, source) {
            Ok(catalog_entry) => catalog.insert_entry(catalog_entry),
            Err(error) => catalog.diagnostics.push(PluginCatalogDiagnostic {
                path: manifest_path,
                reason: error.to_string(),
            }),
        }
    }
}

fn resolve_candidate(path: &Path, is_dir: bool) -> Option<(PathBuf, PathBuf)> {
    if is_dir {
        let pi_manifest = path.join("pi-plugin.json");
        if pi_manifest.is_file() {
            return Some((pi_manifest, path.to_path_buf()));
        }
        let manifest = path.join("plugin.json");
        if manifest.is_file() {
            return Some((manifest, path.to_path_buf()));
        }
        return None;
    }

    let file_name = path.file_name()?.to_string_lossy();
    if file_name == "pi-plugin.json" || file_name == "plugin.json" {
        return path
            .parent()
            .map(|parent| (path.to_path_buf(), parent.to_path_buf()));
    }
    None
}

fn read_catalog_entry(
    manifest_path: &Path,
    plugin_root: &Path,
    source: PluginSource,
) -> Result<CatalogEntry, AppError> {
    let manifest_json = std::fs::read_to_string(manifest_path)
        .map_err(|error| AppError::Plugin(format!("read manifest failed: {error}")))?;
    let manifest = parse_manifest(&manifest_json)?;
    Ok(CatalogEntry {
        manifest,
        manifest_path: manifest_path.to_path_buf(),
        plugin_root: plugin_root.to_path_buf(),
        source,
    })
}
