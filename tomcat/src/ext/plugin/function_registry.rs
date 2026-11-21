use super::{ManifestFunction, PluginSource};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFunction {
    pub plugin_id: String,
    pub plugin_root: PathBuf,
    pub source: PluginSource,
    pub point: String,
    pub function: String,
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default)]
pub struct FunctionRegistry {
    by_point: RwLock<BTreeMap<String, Vec<RegisteredFunction>>>,
    revision: AtomicU64,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, function: RegisteredFunction) {
        let mut guard = self.by_point.write();
        guard
            .entry(function.point.clone())
            .or_default()
            .push(function);
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Direct registration appends raw manifest entries for a plugin and does
    /// not apply point-level override resolution. Production layered override is
    /// resolved before calling `replace_all()`.
    pub fn register_plugin_functions(
        &self,
        plugin_id: &str,
        plugin_root: impl AsRef<Path>,
        functions: &[ManifestFunction],
        source: PluginSource,
    ) {
        let plugin_root = canonicalize_or_keep(plugin_root.as_ref());
        self.remove_by_plugin(plugin_id);
        let mut guard = self.by_point.write();
        for function in functions {
            guard
                .entry(function.point.clone())
                .or_default()
                .push(RegisteredFunction {
                    plugin_id: plugin_id.to_string(),
                    plugin_root: plugin_root.clone(),
                    source,
                    point: function.point.clone(),
                    function: function.function.clone(),
                    metadata: function.metadata.clone(),
                });
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Replace the registry with an already-resolved effective snapshot.
    pub fn replace_all(&self, functions: impl IntoIterator<Item = RegisteredFunction>) {
        let mut next = BTreeMap::<String, Vec<RegisteredFunction>>::new();
        for function in functions {
            next.entry(function.point.clone())
                .or_default()
                .push(function);
        }
        let mut guard = self.by_point.write();
        *guard = next;
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the winner and its cache revision under the same lock.
    pub(crate) fn with_winner<R>(
        &self,
        point: &str,
        f: impl FnOnce(Option<&RegisteredFunction>, u64) -> R,
    ) -> R {
        let guard = self.by_point.read();
        f(
            guard.get(point).and_then(|entries| entries.first()),
            self.revision.load(Ordering::Relaxed),
        )
    }

    pub fn functions_for_point(&self, point: &str) -> Vec<RegisteredFunction> {
        self.by_point.read().get(point).cloned().unwrap_or_default()
    }

    pub fn remove_by_plugin(&self, plugin_id: &str) -> usize {
        let mut removed = 0usize;
        let mut guard = self.by_point.write();
        guard.retain(|_, entries| {
            entries.retain(|entry| {
                let keep = entry.plugin_id != plugin_id;
                if !keep {
                    removed += 1;
                }
                keep
            });
            !entries.is_empty()
        });
        if removed > 0 {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        removed
    }
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    crate::infra::platform::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
