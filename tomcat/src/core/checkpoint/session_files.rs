//! A cumulative review view since the latest Keep All, independent of message ownership.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::file_baselines::{self, Baseline, RestoreFiles, RestoreSource, KEEP_PREFIX};
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

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFilesKeepResponse {
    pub session_id: String,
    pub source_turn_id: String,
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

/// A row's capture time is immutable even when its old owner becomes writable again.
fn scope_rows(
    transcript: &Path,
    expected: Option<&str>,
) -> Result<(Option<String>, Vec<(PathBuf, Baseline)>), AppError> {
    let keep = file_baselines::latest_keep(transcript)?;
    let root = file_baselines::session_dir(transcript);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut rows = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir()
            || !file_baselines::safe_id(&id)
            || id.starts_with(KEEP_PREFIX)
        {
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
        // Record timestamps win even if a copied manifest has an older mtime.
        // Only an unreadable, definitively pre-Keep manifest may be ignored.
        let captured = match file_baselines::read_rows(&dir) {
            Ok(rows) => rows,
            Err(AppError::Serialize(ref e))
                if (e.is_syntax() || e.is_eof())
                    && keep.as_ref().is_some_and(|(at, _)| {
                        metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .is_some_and(|t| t.as_millis() < *at as u128)
                    }) =>
            {
                continue
            }
            Err(e) => return Err(e),
        };
        for row in captured {
            if keep.as_ref().is_none_or(|(at, _)| row.at > *at) {
                rows.push((dir.clone(), row));
            }
        }
    }
    rows.sort_by(|(a, ar), (b, br)| ar.at.cmp(&br.at).then_with(|| a.cmp(b)));
    let source = keep.map(|(_, id)| id).or_else(|| {
        rows.first()
            .and_then(|(dir, _)| dir.file_name())
            .map(|id| id.to_string_lossy().into_owned())
    });
    if expected.is_some_and(|id| source.as_deref() != Some(id)) {
        return Err(error("unknown_path"));
    }
    let mut seen = HashSet::new();
    rows.retain(|(_, row)| seen.insert(row.path.clone()));
    Ok((source, rows))
}

/// Accept only a timestamp. Capture the next version lazily, before the next AI write.
pub fn keep(
    transcript: &Path,
    session: &str,
    source: &str,
) -> Result<SessionFilesKeepResponse, AppError> {
    scope_rows(transcript, Some(source))?;
    let previous = file_baselines::latest_keep(transcript)?.map(|(at, _)| at);
    let timestamp = file_baselines::timestamp_after(previous)?;
    let id = format!("{KEEP_PREFIX}{timestamp}");
    fs::create_dir(file_baselines::session_dir(transcript).join(&id))?;
    Ok(SessionFilesKeepResponse {
        session_id: session.into(),
        source_turn_id: id,
    })
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

fn source_text(source: &RestoreSource) -> Result<String, AppError> {
    match source {
        RestoreSource::Absent => Ok(String::new()),
        RestoreSource::Backup(path, _) if backup_available(path) => text(path),
        RestoreSource::Backup(..) => Err(error("unavailable")),
        RestoreSource::Content(bytes) => {
            if bytes.len() > MAX_DIFF_INPUT_BYTES {
                return Err(error("too_large"));
            }
            if bytes.contains(&0) {
                return Err(error("binary"));
            }
            let value = std::str::from_utf8(bytes).map_err(|_| error("binary"))?;
            if value.lines().count() > MAX_DIFF_INPUT_LINES {
                return Err(error("too_large"));
            }
            Ok(value.into())
        }
    }
}

fn source_available(source: &RestoreSource) -> bool {
    !matches!(source, RestoreSource::Backup(path, _) if !backup_available(path))
}

fn source_matches(source: &RestoreSource, path: &Path, exists: bool) -> Result<bool, AppError> {
    match source {
        RestoreSource::Absent => Ok(!exists),
        _ if !exists => Ok(false),
        RestoreSource::Backup(backup, _) => Ok(file_baselines::files_equal(backup, path)?),
        RestoreSource::Content(bytes) => {
            let mut file = fs::File::open(path)?;
            if file.metadata()?.len() != bytes.len() as u64 {
                return Ok(false);
            }
            let mut offset = 0;
            let mut chunk = [0u8; 8192];
            loop {
                let n = file.read(&mut chunk)?;
                if n == 0 {
                    return Ok(offset == bytes.len());
                }
                if bytes.get(offset..offset + n) != Some(&chunk[..n]) {
                    return Ok(false);
                }
                offset += n;
            }
        }
    }
}

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("--literal-pathspecs").arg("-C").arg(root);
    command
}

/// Resolve committed versions once for all consumers. No durable Git bookkeeping.
fn effective_baselines(
    transcript: &Path,
    rows: Vec<(PathBuf, Baseline)>,
) -> Result<Vec<(PathBuf, RestoreSource)>, AppError> {
    let mut result: Vec<_> = rows
        .iter()
        .map(|(dir, row)| {
            (
                row.path.clone(),
                match backup_path(dir, row) {
                    Some(path) => RestoreSource::Backup(path, row.permissions),
                    None => RestoreSource::Absent,
                },
            )
        })
        .collect();
    let Some(cwd) = file_baselines::session_cwd(transcript) else {
        return Ok(result);
    };
    let Some(head) = file_baselines::git_head(&cwd) else {
        return Ok(result);
    };
    if rows
        .iter()
        .all(|(_, row)| row.git_head.as_ref() == Some(&head))
    {
        return Ok(result);
    }
    let root = git(&cwd).args(["rev-parse", "--show-toplevel"]).output()?;
    if !root.status.success() {
        return Ok(result);
    }
    let root = PathBuf::from(
        String::from_utf8(root.stdout)
            .map_err(|_| error("invalid_git_path"))?
            .trim_end_matches('\n'),
    );
    let root = root.canonicalize()?;
    let mut groups: BTreeMap<Option<String>, Vec<(usize, String)>> = BTreeMap::new();
    for (index, (_, row)) in rows.iter().enumerate() {
        if row.git_head.as_ref() == Some(&head) {
            continue;
        }
        let Some(relative) = row.path.strip_prefix(&root).ok().and_then(Path::to_str) else {
            continue;
        };
        // cat-file's batch input is newline-delimited. Such paths retain their backups.
        if relative.contains('\n') {
            continue;
        }
        groups
            .entry(row.git_head.clone())
            .or_default()
            .push((index, relative.replace(std::path::MAIN_SEPARATOR, "/")));
    }
    let mut committed = Vec::new();
    for (old, paths) in groups {
        let valid_oid = old
            .as_ref()
            .is_none_or(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()));
        let output = if valid_oid {
            let range = old
                .as_ref()
                .map(|old| format!("{old}...{head}"))
                .unwrap_or_else(|| head.clone());
            Some(
                git(&root)
                    .args(["log", "--format=", "--name-only", "-z", "--no-renames"])
                    .arg(range)
                    .arg("--")
                    .args(paths.iter().map(|(_, path)| path))
                    .output()?,
            )
        } else {
            None
        };
        let missing_old = output.as_ref().is_none_or(|o| !o.status.success());
        let changed: HashSet<Vec<u8>> = output
            .as_ref()
            .filter(|o| o.status.success())
            .map(|o| {
                o.stdout
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_vec())
                    .collect()
            })
            .unwrap_or_default();
        for (index, path) in paths {
            if missing_old || changed.contains(path.as_bytes()) {
                committed.push((index, path, missing_old));
            }
        }
    }
    if committed.is_empty() {
        return Ok(result);
    }
    // Batch metadata is unfiltered and safely framed. Filtered output is read separately:
    // Git's --batch --filters header reports the RAW blob size, not the filtered byte count.
    let mut child = git(&root)
        .args(["cat-file", "--batch-check"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("git_stdin_unavailable"))?;
    let input = committed
        .iter()
        .map(|(_, path, _)| format!("{head}:{path}\n"))
        .collect::<String>();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output();
    let written = writer
        .join()
        .map_err(|_| error("git_batch_writer_failed"))?;
    let output = output?;
    written?;
    if !output.status.success() {
        return Err(error("git_baseline_failed"));
    }
    let mut reader = std::io::Cursor::new(output.stdout);
    for (index, path, missing_old) in committed {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.ends_with(" missing\n") {
            // Untracked/outside Git remains a normal backup, even if the old commit is gone.
            if !missing_old {
                result[index].1 = RestoreSource::Absent;
            }
            continue;
        }
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 || fields[1] != "blob" {
            return Err(error("invalid_git_baseline"));
        }
        let content = git(&root)
            .args(["cat-file", "--filters"])
            .arg(format!("{head}:{path}"))
            .output()?;
        if !content.status.success() {
            return Err(error("git_baseline_failed"));
        }
        result[index].1 = RestoreSource::Content(content.stdout);
    }
    Ok(result)
}

pub fn list(transcript: &Path, session_id: &str) -> Result<SessionFilesResponse, AppError> {
    let (source, rows) = scope_rows(transcript, None)?;
    let mut files = Vec::new();
    for (path, baseline) in effective_baselines(transcript, rows)? {
        let exists = match fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        let missing = !source_available(&baseline);
        let regular = file_baselines::require_regular_path(&path).is_ok();
        if !missing && regular && source_matches(&baseline, &path, exists)? {
            continue;
        }
        let blocked = if missing {
            Some(SessionFileBlockedReason::BackupMissing)
        } else if !regular {
            Some(SessionFileBlockedReason::NotRegularFile)
        } else {
            None
        };
        let before = source_text(&baseline);
        let after = if exists {
            text(&path)
        } else {
            Ok(String::new())
        };
        let counts = before
            .ok()
            .zip(after.ok())
            .filter(|(a, b)| a.len().saturating_add(b.len()) <= MAX_DIFF_INPUT_BYTES)
            .map(|(a, b)| line_diff_stat(&a, &b));
        files.push(SessionFile {
            path: path.to_string_lossy().into_owned(),
            status: if matches!(baseline, RestoreSource::Absent) {
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
    Ok(SessionFilesResponse {
        session_id: session_id.into(),
        source_turn_id: source,
        files,
    })
}

pub fn baseline(
    transcript: &Path,
    session: &str,
    source: &str,
    path: &str,
) -> Result<SessionFileBaselineResponse, AppError> {
    let (_, rows) = scope_rows(transcript, Some(source))?;
    let rows = rows
        .into_iter()
        .filter(|(_, r)| r.path == Path::new(path))
        .collect();
    let (path, value) = effective_baselines(transcript, rows)?
        .into_iter()
        .next()
        .ok_or_else(|| error("unknown_path"))?;
    Ok(SessionFileBaselineResponse {
        session_id: session.into(),
        source_turn_id: source.into(),
        path: path.to_string_lossy().into_owned(),
        existed: !matches!(value, RestoreSource::Absent),
        text: source_text(&value)?,
    })
}

/// Preflight every requested path before any disk write; rewind's separate HEAD gate is unchanged.
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
    let (_, rows) = scope_rows(transcript, Some(source)).map_err(map_error)?;
    let requested: HashSet<_> = paths.iter().map(|p| Path::new(p)).collect();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|(_, r)| requested.contains(r.path.as_path()))
        .collect();
    if rows.len() != requested.len() {
        return Err(map_error(error("unknown_path")));
    }
    let mut files = BTreeMap::new();
    for (path, baseline) in effective_baselines(transcript, rows).map_err(map_error)? {
        file_baselines::require_regular_path(&path).map_err(map_error)?;
        if !source_available(&baseline) {
            return Err(map_error(error("unavailable")));
        }
        files.insert(path, baseline);
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
