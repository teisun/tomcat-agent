use super::ManifestFunction;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFunction {
    pub plugin_id: String,
    pub plugin_root: PathBuf,
    pub point: String,
    pub function: String,
}

#[derive(Debug, Default)]
pub struct FunctionRegistry {
    by_point: RwLock<BTreeMap<String, Vec<RegisteredFunction>>>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, function: RegisteredFunction) {
        self.by_point
            .write()
            .entry(function.point.clone())
            .or_default()
            .push(function);
    }

    pub fn register_plugin_functions(
        &self,
        plugin_id: &str,
        plugin_root: impl AsRef<Path>,
        functions: &[ManifestFunction],
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
                    point: function.point.clone(),
                    function: function.function.clone(),
                });
        }
    }

    pub fn replace_all(&self, functions: impl IntoIterator<Item = RegisteredFunction>) {
        let mut next = BTreeMap::<String, Vec<RegisteredFunction>>::new();
        for function in functions {
            next.entry(function.point.clone())
                .or_default()
                .push(function);
        }
        *self.by_point.write() = next;
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
        removed
    }
}

fn canonicalize_or_keep(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
