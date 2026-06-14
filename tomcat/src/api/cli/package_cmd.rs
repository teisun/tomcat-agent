use std::io::IsTerminal;
use std::path::PathBuf;

use dialoguer::{theme::ColorfulTheme, Select};

use crate::core::package::{PackageLayerListing, PackageManager, PackageVisibility};
use crate::{normalize_path, AppConfig, AppError};

use super::PackageVisibilityArg;

pub(crate) fn run_install(
    source: String,
    visibility: Option<PackageVisibilityArg>,
    scope_root: Option<String>,
    force: bool,
    cfg: &AppConfig,
) -> Result<(), AppError> {
    let scope_context = resolve_scope_context(scope_root.as_deref())?;
    let visibility = resolve_target_visibility(visibility)?;
    let manager = PackageManager::new(cfg);
    let prepared = manager.prepare_install(&source, visibility, Some(&scope_context), force)?;
    let outcome = manager.install(prepared)?;

    println!(
        "已安装 package: {}@{} -> {}",
        outcome.record.name, outcome.record.version, visibility
    );
    for resource in &outcome.record.resources {
        println!("  - {}: {}", resource.kind.as_str(), resource.id);
    }
    print_warnings(&outcome.warnings);
    Ok(())
}

pub(crate) fn run_uninstall(
    package: String,
    visibility: Option<PackageVisibilityArg>,
    scope_root: Option<String>,
    cfg: &AppConfig,
) -> Result<(), AppError> {
    let scope_context = resolve_scope_context(scope_root.as_deref())?;
    let visibility = resolve_target_visibility(visibility)?;
    let manager = PackageManager::new(cfg);
    let outcome = manager.uninstall(&package, visibility, Some(&scope_context))?;

    println!("已卸载 package: {} <- {}", outcome.record.name, visibility);
    for removed in &outcome.removed_paths {
        println!("  - removed {}", removed.display());
    }
    Ok(())
}

pub(crate) fn run_packages(
    visibility: Option<PackageVisibilityArg>,
    scope_root: Option<String>,
    cfg: &AppConfig,
) -> Result<(), AppError> {
    let scope_context = resolve_scope_context(scope_root.as_deref())?;
    let manager = PackageManager::new(cfg);
    let listings = manager.list_packages(
        Some(&scope_context),
        visibility.map(PackageVisibilityArg::into_visibility),
    )?;
    render_package_listings(&listings);
    Ok(())
}

fn resolve_target_visibility(
    visibility: Option<PackageVisibilityArg>,
) -> Result<PackageVisibility, AppError> {
    if let Some(visibility) = visibility {
        return Ok(visibility.into_visibility());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(PackageVisibility::Scope);
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("请选择安装/卸载目标层")
        .default(0)
        .items(&["current-project (scope)", "agent", "global"])
        .interact_opt()
        .map_err(|error| AppError::Config(format!("visibility chooser 失败: {error}")))?;

    match selection {
        Some(0) => Ok(PackageVisibility::Scope),
        Some(1) => Ok(PackageVisibility::Agent),
        Some(2) => Ok(PackageVisibility::Global),
        Some(_) => Err(AppError::internal("unexpected visibility selection")),
        None => Err(AppError::Config("已取消选择目标层".to_string())),
    }
}

fn resolve_scope_context(scope_root: Option<&str>) -> Result<PathBuf, AppError> {
    match scope_root {
        Some(scope_root) => normalize_path(scope_root),
        None => std::env::current_dir().map_err(AppError::Io),
    }
}

fn render_package_listings(listings: &[PackageLayerListing]) {
    for listing in listings {
        println!("{}:", listing.visibility);
        if listing.records.is_empty() {
            println!("  (none)");
            continue;
        }
        for record in &listing.records {
            println!(
                "  - {}@{} [{}] source={} installed_at={}",
                record.name,
                record.version,
                record.source_kind.as_str(),
                record.source_path,
                record.installed_at
            );
            if !record.resources.is_empty() {
                let resources = record
                    .resources
                    .iter()
                    .map(|resource| format!("{}:{}", resource.kind.as_str(), resource.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("    resources: {resources}");
            }
        }
    }
}

fn print_warnings(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    println!("warnings:");
    for warning in warnings {
        println!("  - {warning}");
    }
}
