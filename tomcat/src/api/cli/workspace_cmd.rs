//! `tomcat workspace` 子命令实现：add / list / remove。

use crate::infra::i18n::tr;
use std::path::PathBuf;

use crate::{
    load_config, load_config_toml_file, normalize_path, resolve_workspace_roots_paths,
    validate_config, write_file_atomic, AppConfig, AppError,
};

use super::{config_file_path, WorkspaceSub};

pub(crate) fn run_workspace(sub: WorkspaceSub, _cfg: &AppConfig) -> Result<(), AppError> {
    let config_path = config_file_path()?;

    match sub {
        WorkspaceSub::List => {
            if !config_path.exists() {
                println!(
                    "{}",
                    tr(
                        "cli.config.fileMissing",
                        &[("path", &config_path.display().to_string())]
                    )
                );
                return Ok(());
            }
            let list_cfg = load_config(Some(&config_path))?;
            let mut any = false;
            for s in &list_cfg.workspace.workspace_roots {
                let t = s.trim();
                if t.is_empty() {
                    continue;
                }
                any = true;
                match normalize_path(t)
                    .ok()
                    .and_then(|p| std::fs::canonicalize(p).ok())
                {
                    Some(c) => println!("{}", c.display()),
                    None => println!("{}", t),
                }
            }
            if !any {
                println!("{}", tr("cli.workspace.empty", &[]));
            }
        }
        WorkspaceSub::Add {
            path: add_path,
            cwd,
        } => {
            if !config_path.exists() {
                println!(
                    "{}",
                    tr(
                        "cli.config.fileMissing",
                        &[("path", &config_path.display().to_string())]
                    )
                );
                return Ok(());
            }
            let target = if cwd {
                std::env::current_dir().map_err(|e| {
                    AppError::Config(tr("cli.workspace.cwdFailed", &[("detail", &e.to_string())]))
                })?
            } else if let Some(p) = add_path {
                PathBuf::from(p)
            } else {
                return Err(AppError::Config(tr("cli.workspace.required", &[])));
            };
            let abs = std::fs::canonicalize(&target).map_err(|_| {
                AppError::Config(tr(
                    "cli.workspace.inaccessible",
                    &[("path", &target.display().to_string())],
                ))
            })?;
            if !abs.is_dir() {
                return Err(AppError::Config(tr(
                    "cli.workspace.notDir",
                    &[("path", &abs.display().to_string())],
                )));
            }
            let mut file_cfg = load_config_toml_file(&config_path)?;
            let existing = resolve_workspace_roots_paths(&file_cfg)?;
            if existing.contains(&abs) {
                println!(
                    "{}",
                    tr(
                        "cli.workspace.exists",
                        &[("path", &abs.display().to_string())]
                    )
                );
                return Ok(());
            }
            file_cfg
                .workspace
                .workspace_roots
                .push(abs.to_string_lossy().into_owned());
            validate_config(&file_cfg)?;
            let toml_str =
                toml::to_string_pretty(&file_cfg).map_err(|e| AppError::Config(e.to_string()))?;
            write_file_atomic(&config_path, toml_str.as_bytes())?;
            println!(
                "{}",
                tr(
                    "cli.workspace.added",
                    &[("path", &abs.display().to_string())]
                )
            );
        }
        WorkspaceSub::Remove { path: path_arg } => {
            if !config_path.exists() {
                println!(
                    "{}",
                    tr(
                        "cli.config.fileMissing",
                        &[("path", &config_path.display().to_string())]
                    )
                );
                return Ok(());
            }
            let mut file_cfg = load_config_toml_file(&config_path)?;
            let norm_user = normalize_path(&path_arg)?;
            let canon_user = std::fs::canonicalize(&norm_user).ok();

            let before_len = file_cfg.workspace.workspace_roots.len();
            file_cfg.workspace.workspace_roots.retain(|entry| {
                let t = entry.trim();
                if t.is_empty() {
                    return true;
                }
                let matches = if let Some(ref cu) = canon_user {
                    normalize_path(t)
                        .ok()
                        .and_then(|p| std::fs::canonicalize(p).ok())
                        .map(|ct| &ct == cu)
                        .unwrap_or(false)
                } else {
                    normalize_path(t).map(|nt| nt == norm_user).unwrap_or(false)
                };
                !matches
            });
            if file_cfg.workspace.workspace_roots.len() == before_len {
                println!(
                    "{}",
                    tr(
                        "cli.workspace.notFound",
                        &[("path", &norm_user.display().to_string())]
                    )
                );
                return Ok(());
            }
            validate_config(&file_cfg)?;
            let toml_str =
                toml::to_string_pretty(&file_cfg).map_err(|e| AppError::Config(e.to_string()))?;
            write_file_atomic(&config_path, toml_str.as_bytes())?;
            println!(
                "{}",
                tr(
                    "cli.workspace.removed",
                    &[("path", &norm_user.display().to_string())]
                )
            );
        }
    }
    Ok(())
}
