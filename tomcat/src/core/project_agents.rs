//! Ancestor AGENTS.md files enter the existing project-rule renderer, not conversation history.
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use super::{
    diagnostic_path, read_text_file, readable, safe_path, Discovery, InstructionCard,
    InstructionKind, PromptFile,
};
use crate::core::permission::PermissionGate;
use crate::infra::i18n::tr;
use crate::infra::platform::{canonicalize, open_no_follow, same_file};

pub(super) fn discover(cwd: &Path) -> Discovery {
    let mut result = Discovery::default();
    let paths = (|| {
        // Keep the lexical suffix until safe_path has rejected directory links. Canonicalizing
        // the cwd first would hide a link escaping a repository. Canonicalize the trusted root
        // itself so OS aliases such as macOS /var do not reject otherwise ordinary projects.
        let boundary = cwd
            .ancestors()
            .find(|dir| {
                fs::symlink_metadata(dir.join(".git"))
                    .is_ok_and(|meta| meta.is_dir() || meta.is_file())
            })
            .unwrap_or(cwd);
        let root = canonicalize(boundary).map_err(|e| e.to_string())?;
        let cwd = root.join(cwd.strip_prefix(boundary).map_err(|e| e.to_string())?);
        safe_path(&root, &cwd)?;
        if !cwd.is_dir() {
            return Err(tr("instructions.notRegular", &[]));
        }
        let mut dirs = Vec::new();
        for dir in cwd.ancestors() {
            dirs.push(dir.to_path_buf());
            if dir == root {
                break;
            }
        }
        dirs.reverse();
        Ok::<_, String>((root, dirs))
    })();
    let (root, dirs) = match paths {
        Ok(paths) => paths,
        Err(error) => {
            result
                .diagnostics
                .push(format!("{}: {error}", cwd.display()));
            return result;
        }
    };
    for dir in dirs {
        let path = dir.join("AGENTS.md");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let relative = diagnostic_path(&root, &path);
                result.files.push(PromptFile {
                    card: InstructionCard {
                        id: format!("rule:{relative}"),
                        kind: InstructionKind::Command,
                        name: "AGENTS.md".into(),
                        description: String::new(),
                        source: "AGENTS.md".into(),
                        path: relative,
                    },
                    file_path: path,
                    always_apply: true,
                    agents_root: Some(root.clone()),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => result
                .diagnostics
                .push(format!("{}: {error}", path.display())),
        }
    }
    result
}

fn resolve(root: &Path, path: &Path, gate: &dyn PermissionGate) -> Result<PathBuf, String> {
    readable(gate, path)?;
    safe_path(
        root,
        path.parent()
            .ok_or_else(|| tr("instructions.invalidPath", &[]))?,
    )?;
    let target = canonicalize(path).map_err(|e| e.to_string())?;
    safe_path(root, &target)?;
    readable(gate, &target)?;
    if !fs::symlink_metadata(&target)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err(tr("instructions.notRegular", &[]));
    }
    Ok(target)
}

/// Recheck the opened object, not merely the name checked before open. Leaf links are allowed
/// only to repository-contained regular files; directory links and observed replacements fail.
fn validate_opened(
    root: &Path,
    path: &Path,
    target: &Path,
    file: &File,
    gate: &dyn PermissionGate,
) -> Result<(), String> {
    if resolve(root, path, gate)? != target {
        return Err("Project instruction target changed while reading".into());
    }
    let current = open_no_follow(target).map_err(|e| e.to_string())?;
    if !same_file(file, &current).map_err(|e| e.to_string())? {
        return Err("Project instruction file changed while reading".into());
    }
    Ok(())
}

fn read_opened(
    root: &Path,
    path: &Path,
    target: &Path,
    mut file: File,
    gate: &dyn PermissionGate,
) -> Result<String, String> {
    validate_opened(root, path, target, &file, gate)?;
    let text = read_text_file(&mut file)?;
    validate_opened(root, path, target, &file, gate)?;
    let body = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if body.trim().is_empty() {
        return Err(tr("instructions.empty", &[]));
    }
    Ok(body.to_string())
}

pub(super) fn read_body(
    root: &Path,
    path: &Path,
    gate: &dyn PermissionGate,
) -> Result<String, String> {
    let target = resolve(root, path, gate)?;
    let file = open_no_follow(&target).map_err(|e| e.to_string())?;
    read_opened(root, path, &target, file, gate)
}

#[cfg(test)]
#[path = "tests/project_agents_test.rs"]
mod tests;
