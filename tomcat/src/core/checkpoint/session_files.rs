//! A derived view of the last editing turn. No additional snapshot store.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::file_baselines::{self, Baseline, RestoreFiles};
use crate::core::tools::primitive::{line_diff_stat, MAX_DIFF_INPUT_BYTES, MAX_DIFF_INPUT_LINES};
use crate::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionFileStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionFileBlockedReason {
    HeadMoved,
    BackupMissing,
    NotRegularFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub path: String,
    pub status: SessionFileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    pub restorable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<SessionFileBlockedReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFilesResponse {
    pub session_id: String,
    pub source_turn_id: Option<String>,
    pub files: Vec<SessionFile>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileBaselineResponse {
    pub session_id: String,
    pub source_turn_id: String,
    pub path: String,
    pub existed: bool,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFilesRestoreResponse {
    pub session_id: String,
    pub source_turn_id: String,
    pub restored: Vec<String>,
}

fn error(code: &str) -> AppError {
    AppError::Config(code.into())
}

fn turn_dir(transcript: &Path, source: &str) -> Result<PathBuf, AppError> {
    if !file_baselines::safe_id(source) {
        return Err(error("unknown_path"));
    }
    let root = file_baselines::session_dir(transcript);
    let root = root.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            error("unknown_path")
        } else {
            e.into()
        }
    })?;
    let dir = root.join(source);
    for ancestor in dir.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(error("unknown_path"));
        }
    }
    Ok(dir)
}

fn read_rows(dir: &Path) -> Result<Vec<Baseline>, AppError> {
    let path = dir.join("baselines.jsonl");
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        return Err(error("unknown_path"));
    }
    let text = fs::read_to_string(path)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Baseline = serde_json::from_str(line)?;
        if !row.path.is_absolute()
            || row
                .backup
                .as_ref()
                .is_some_and(|b| b.len() != 64 || !b.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(error("invalid_baseline"));
        }
        if seen.insert(row.path.clone()) {
            out.push(row);
        }
    }
    Ok(out)
}

/// Select by published manifest metadata, without reading chat or other manifests.
/// Superseded turns are removed by the existing history rewrite paths.
pub fn latest_editing_turn(transcript: &Path) -> Result<Option<String>, AppError> {
    let root = file_baselines::session_dir(transcript);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    for entry in entries {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() || !file_baselines::safe_id(&id) {
            continue;
        }
        let dir = turn_dir(transcript, &id)?;
        let metadata = match fs::symlink_metadata(dir.join("baselines.jsonl")) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        let candidate = (metadata.modified()?, id);
        latest = Some(match latest {
            Some(previous) => previous.max(candidate),
            None => candidate,
        });
    }
    Ok(latest.map(|(_, id)| id))
}

fn backup_path(dir: &Path, row: &Baseline) -> Option<PathBuf> {
    row.backup.as_ref().map(|b| dir.join(b))
}

fn backup_available(backup: &Path) -> bool {
    file_baselines::require_regular_path(backup).is_ok()
        && fs::symlink_metadata(backup).is_ok_and(|m| m.is_file())
}

fn text(path: &Path) -> Result<String, AppError> {
    file_baselines::require_regular_path(path)?;
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_DIFF_INPUT_BYTES as u64 {
        return Err(error("too_large"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_DIFF_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DIFF_INPUT_BYTES {
        return Err(error("too_large"));
    }
    if bytes.contains(&0) {
        return Err(error("binary"));
    }
    let text = String::from_utf8(bytes).map_err(|_| error("binary"))?;
    if text.lines().count() > MAX_DIFF_INPUT_LINES {
        return Err(error("too_large"));
    }
    Ok(text)
}

pub fn list(transcript: &Path, session_id: &str) -> Result<SessionFilesResponse, AppError> {
    let source = latest_editing_turn(transcript)?;
    let mut files = Vec::new();
    if let Some(ref source) = source {
        let dir = turn_dir(transcript, source)?;
        let head =
            file_baselines::session_cwd(transcript).and_then(|cwd| file_baselines::git_head(&cwd));
        for row in read_rows(&dir)? {
            let backup = backup_path(&dir, &row);
            let current_meta = fs::symlink_metadata(&row.path);
            let exists = match &current_meta {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(error(&format!("Cannot read {}: {e}", row.path.display()))),
            };
            let missing = backup.as_ref().is_some_and(|b| !backup_available(b));
            let regular = file_baselines::require_regular_path(&row.path).is_ok();
            if !missing && regular {
                match backup.as_ref() {
                    None if !exists => continue,
                    Some(b) if exists && file_baselines::files_equal(b, &row.path)? => continue,
                    _ => {}
                }
            }
            let blocked = if missing {
                Some(SessionFileBlockedReason::BackupMissing)
            } else if !regular {
                Some(SessionFileBlockedReason::NotRegularFile)
            } else if row.git_head != head {
                Some(SessionFileBlockedReason::HeadMoved)
            } else {
                None
            };
            let before = match backup.as_ref() {
                Some(b) => text(b),
                None => Ok(String::new()),
            };
            let after = if exists {
                text(&row.path)
            } else {
                Ok(String::new())
            };
            let counts = before
                .ok()
                .zip(after.ok())
                .filter(|(a, b)| a.len().saturating_add(b.len()) <= MAX_DIFF_INPUT_BYTES)
                .map(|(a, b)| line_diff_stat(&a, &b));
            files.push(SessionFile {
                path: row.path.to_string_lossy().into_owned(),
                status: if row.backup.is_none() {
                    SessionFileStatus::Added
                } else if !exists {
                    SessionFileStatus::Deleted
                } else {
                    SessionFileStatus::Modified
                },
                added: counts.map(|c| c.0),
                removed: counts.map(|c| c.1),
                restorable: blocked.is_none(),
                blocked_reason: blocked,
            });
        }
    }
    Ok(SessionFilesResponse {
        session_id: session_id.into(),
        source_turn_id: source,
        files,
    })
}

fn selected_row(
    transcript: &Path,
    source: &str,
    path: &str,
) -> Result<(PathBuf, Baseline), AppError> {
    let dir = turn_dir(transcript, source)?;
    let rows = read_rows(&dir).map_err(|e| match e {
        AppError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => error("unknown_path"),
        e => e,
    })?;
    let row = rows
        .into_iter()
        .find(|r| r.path == Path::new(path))
        .ok_or_else(|| error("unknown_path"))?;
    Ok((dir, row))
}

pub fn baseline(
    transcript: &Path,
    session: &str,
    source: &str,
    path: &str,
) -> Result<SessionFileBaselineResponse, AppError> {
    let (dir, row) = selected_row(transcript, source, path)?;
    let value = match backup_path(&dir, &row) {
        Some(b) if backup_available(&b) => text(&b)?,
        Some(_) => return Err(error("unavailable")),
        None => String::new(),
    };
    Ok(SessionFileBaselineResponse {
        session_id: session.into(),
        source_turn_id: source.into(),
        path: row.path.to_string_lossy().into_owned(),
        existed: row.backup.is_some(),
        text: value,
    })
}

/// Preflight the exact confirmed subset, then reuse the file-only Revert kernel.
pub fn restore(
    transcript: &Path,
    session: &str,
    source: &str,
    paths: &[String],
) -> Result<SessionFilesRestoreResponse, (String, serde_json::Value)> {
    let map_error = |e: AppError| {
        (
            match e {
                AppError::Config(code) => code,
                e => e.to_string(),
            },
            serde_json::json!({}),
        )
    };
    let head =
        file_baselines::session_cwd(transcript).and_then(|cwd| file_baselines::git_head(&cwd));
    let mut files = BTreeMap::new();
    for path in paths {
        let (dir, row) = selected_row(transcript, source, path).map_err(map_error)?;
        if row.git_head != head {
            return Err(("head_moved".into(), serde_json::json!({})));
        }
        file_baselines::require_regular_path(&row.path).map_err(map_error)?;
        let backup = backup_path(&dir, &row);
        if backup.as_ref().is_some_and(|b| !backup_available(b)) {
            return Err(("unavailable".into(), serde_json::json!({})));
        }
        files.insert(row.path, backup);
    }
    let restore = RestoreFiles::selected(files);
    let restored = restore.paths();
    restore.restore().map_err(|(path, reason)| {
        (
            "restore_failed".into(),
            serde_json::json!({"path":path,"reason":reason}),
        )
    })?;
    Ok(SessionFilesRestoreResponse {
        session_id: session.into(),
        source_turn_id: source.into(),
        restored,
    })
}
