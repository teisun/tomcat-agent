use std::io::IsTerminal;
use std::path::PathBuf;

use dialoguer::{theme::ColorfulTheme, Select};

use crate::core::package::{PackageLayerListing, PackageManager, PackageVisibility};
use crate::infra::i18n::tr;
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
        "{}",
        tr(
            "cli.package.installed",
            &[
                ("name", &outcome.record.name),
                ("version", &outcome.record.version),
                ("visibility", &visibility.to_string())
            ]
        )
    );
    for (kind, id) in outcome.record.resource_descriptors() {
        println!("  - {}: {}", kind.as_str(), id);
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

    println!(
        "{}",
        tr(
            "cli.package.uninstalled",
            &[
                ("name", &outcome.record.name),
                ("visibility", &visibility.to_string())
            ]
        )
    );
    for removed in &outcome.removed_paths {
        println!(
            "{}",
            tr(
                "cli.package.removed",
                &[("path", &removed.display().to_string())]
            )
        );
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
        .with_prompt(tr("cli.package.selectLayer", &[]))
        .default(0)
        .items(&["current-project (scope)", "agent", "global"])
        .interact_opt()
        .map_err(|error| {
            AppError::Config(tr(
                "cli.package.selectFailed",
                &[("detail", &error.to_string())],
            ))
        })?;

    match selection {
        Some(0) => Ok(PackageVisibility::Scope),
        Some(1) => Ok(PackageVisibility::Agent),
        Some(2) => Ok(PackageVisibility::Global),
        Some(_) => Err(AppError::internal("unexpected visibility selection")),
        None => Err(AppError::Config(tr("cli.package.selectCancelled", &[]))),
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
            println!("{}", tr("cli.package.empty", &[]));
            continue;
        }
        for record in &listing.records {
            println!(
                "  - {}@{} [{}] source={} installed_at={}",
                record.name,
                record.version,
                record.source_kind.as_str(),
                record.source,
                record.installed_at
            );
            let resource_descriptors = record.resource_descriptors();
            if !resource_descriptors.is_empty() {
                let resources = resource_descriptors
                    .into_iter()
                    .map(|(kind, id)| format!("{}:{id}", kind.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("{}", tr("cli.package.resources", &[("items", &resources)]));
            }
        }
    }
}

fn print_warnings(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    println!("{}", tr("cli.package.warnings", &[]));
    for warning in warnings {
        println!("  - {warning}");
    }
}
