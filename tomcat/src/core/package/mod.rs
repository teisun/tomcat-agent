//! Package 安装核心：统一 source 识别、三层落位、账本与事务。

pub mod manager;
pub mod model;
pub mod paths;

pub use manager::{
    load_package_registry, load_plugin_registry, save_package_registry, save_plugin_registry,
    PackageManager,
};
pub use model::{
    DetectedPackageResource, DetectedPackageSource, InstallOutcome, PackageLayerListing,
    PackageManifest, PackageRecord, PackageRegistryFile, PackageResource, PackageResourceKind,
    PackageSourceKind, PackageVisibility, PluginRegistryEntry, PluginRegistryFile, PreparedInstall,
    PreparedInstallResource, UninstallOutcome,
};
pub use paths::{
    canonical_scope_root, resolve_layer_paths, resolve_runtime_layer_paths, LayerPaths,
};

#[cfg(test)]
mod tests;
