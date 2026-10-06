//! Per-message, first-write file backups. Independent of ShadowGit checkpoints.
//! Capture is best effort; only successful native writes publish a manifest row.
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::infra::platform::write_file_atomic;
use crate::AppError;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevertReason {
    NoBaselines,
    Expired,
    GitHeadMoved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Baseline {
    pub(crate) path: PathBuf,
    pub(crate) backup: Option<String>,
    pub(crate) git_head: Option<String>,
}

pub(crate) fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

pub fn session_dir(transcript: &Path) -> PathBuf {
    transcript
        .parent()
        .unwrap_or(Path::new("."))
        .join("file-baselines")
        .join(transcript.file_stem().unwrap_or_default())
}

pub fn session_cwd(transcript: &Path) -> Option<PathBuf> {
    crate::core::session::transcript::read_header(transcript)
        .ok()?
        .cwd
        .filter(|cwd| !cwd.trim().is_empty())
        .map(PathBuf::from)
}

pub fn git_head(cwd: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "-q", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rows(dir: &Path) -> Vec<Baseline> {
    fs::read_to_string(dir.join("baselines.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub struct TurnFileBaselines {
    pub message_id: String,
    directory: PathBuf,
    cwd: PathBuf,
    state: Mutex<CaptureState>,
}
#[derive(Default)]
struct CaptureState {
    seen: HashSet<PathBuf>,
    head: Option<Option<String>>,
}

impl TurnFileBaselines {
    pub fn new(transcript: &Path, message_id: &str, cwd: PathBuf) -> Option<Arc<Self>> {
        if !safe_id(message_id) {
            return None;
        }
        let directory = session_dir(transcript).join(message_id);
        let old = rows(&directory);
        Some(Arc::new(Self {
            message_id: message_id.to_string(),
            directory,
            cwd,
            state: Mutex::new(CaptureState {
                seen: old.iter().map(|r| r.path.clone()).collect(),
                head: old.first().map(|r| r.git_head.clone()),
            }),
        }))
    }

    /// Caller has completed the native tool's path/read-stamp checks. No file bytes
    /// are exposed to the model; failed/declined tool calls do not publish a baseline.
    pub fn prepare(self: &Arc<Self>, path: &str) -> Option<PendingBaseline> {
        let path = crate::infra::platform::normalize_path(path).ok()?;
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        // Resolve existing parent aliases once (macOS /var and /tmp are symlinks).
        // Store the actual location, then reject any *new* symlink at restore time.
        let path = path.parent()?.ancestors().find_map(|ancestor| {
            ancestor
                .canonicalize()
                .ok()
                .and_then(|base| path.strip_prefix(ancestor).ok().map(|tail| base.join(tail)))
        })?;
        let mut state = self.state.lock();
        if !state.seen.insert(path.clone()) {
            return None;
        }
        let head = state
            .head
            .get_or_insert_with(|| git_head(&self.cwd))
            .clone();
        let result = (|| -> Result<Baseline, AppError> {
            fs::create_dir_all(&self.directory)?;
            let backup = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() => {
                    let name = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
                    fs::copy(&path, self.directory.join(&name))?;
                    Some(name)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Ok(_) => return Err(AppError::Config("baseline requires a regular file".into())),
                Err(error) => return Err(error.into()),
            };
            Ok(Baseline {
                path,
                backup,
                git_head: head,
            })
        })();
        match result {
            Ok(row) => Some(PendingBaseline {
                owner: self.clone(),
                row,
                committed: false,
            }),
            Err(error) => {
                tracing::warn!(%error, "file baseline capture failed; write may continue");
                None
            }
        }
    }
}

pub struct PendingBaseline {
    owner: Arc<TurnFileBaselines>,
    row: Baseline,
    committed: bool,
}
impl PendingBaseline {
    pub fn commit(mut self) {
        // A successful no-op must not replace the last editing turn in Files.
        let unchanged = match &self.row.backup {
            Some(name) => files_equal(&self.owner.directory.join(name), &self.row.path),
            None => match fs::symlink_metadata(&self.row.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
                Ok(_) => Ok(false),
                Err(e) => Err(e),
            },
        };
        match unchanged {
            Ok(true) => return, // Drop releases seen so a later real edit can capture again.
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "cannot compare baseline after write"),
        }
        // Keep seen even if persisting fails: never capture an already modified file later.
        self.committed = true;
        let result = (|| -> Result<(), AppError> {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.owner.directory.join("baselines.jsonl"))?;
            writeln!(file, "{}", serde_json::to_string(&self.row)?)?;
            file.sync_data()?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "file baseline manifest failed");
        }
    }
}
impl Drop for PendingBaseline {
    fn drop(&mut self) {
        if !self.committed {
            self.owner.state.lock().seen.remove(&self.row.path);
        }
    }
}

/// Compare bytes without loading oversized/binary files into memory.
pub(crate) fn files_equal(left: &Path, right: &Path) -> std::io::Result<bool> {
    use std::io::{BufReader, Read};
    let mut left = BufReader::new(fs::File::open(left)?);
    let mut right = BufReader::new(fs::File::open(right)?);
    if left.get_ref().metadata()?.len() != right.get_ref().metadata()?.len() {
        return Ok(false);
    }
    let mut a = [0u8; 8192];
    let mut b = [0u8; 8192];
    loop {
        let n = left.read(&mut a)?;
        if n == 0 {
            return Ok(true);
        }
        right.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] {
            return Ok(false);
        }
    }
}

pub(crate) fn require_regular_path(path: &Path) -> Result<(), AppError> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::Config(
                    "refusing to restore through a symlink".into(),
                ));
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => Err(AppError::Config("not_regular_file".into())),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub struct RestoreFiles {
    files: BTreeMap<PathBuf, Option<PathBuf>>,
}
impl RestoreFiles {
    pub(crate) fn selected(files: BTreeMap<PathBuf, Option<PathBuf>>) -> Self {
        Self { files }
    }

    pub fn paths(&self) -> Vec<String> {
        self.files
            .keys()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }
    pub fn restore(&self) -> Result<(), (String, String)> {
        for (path, backup) in &self.files {
            let result = (|| -> Result<(), AppError> {
                // Security/type checks are not content-conflict checks. Never traverse a
                // newly substituted symlink or recursively remove a directory.
                for ancestor in path.ancestors() {
                    if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                        return Err(AppError::Config(
                            "refusing to restore through a symlink".into(),
                        ));
                    }
                }
                match backup {
                    Some(backup) => match fs::read(backup) {
                        Ok(bytes) => {
                            write_file_atomic(path, &bytes)?;
                            fs::set_permissions(path, fs::metadata(backup)?.permissions())?;
                            Ok(())
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e.into()),
                    },
                    None => match fs::remove_file(path) {
                        Ok(()) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e.into()),
                    },
                }
            })();
            if let Err(e) = result {
                return Err((path.to_string_lossy().into_owned(), e.to_string()));
            }
        }
        Ok(())
    }
}

/// `turns` is in canonical transcript order, starting at the selected live user.
pub fn preview(
    transcript: &Path,
    turns: &[String],
    target_timestamp: &str,
    cwd: &Path,
    retention_days: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RestoreFiles, RevertReason> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(target_timestamp)
        .map_err(|_| RevertReason::Expired)?;
    if now.signed_duration_since(timestamp) >= chrono::Duration::days(i64::from(retention_days)) {
        return Err(RevertReason::Expired);
    }
    let head = git_head(cwd);
    let root = session_dir(transcript);
    let mut files = BTreeMap::new();
    for turn in turns.iter().filter(|id| safe_id(id)) {
        let directory = root.join(turn);
        for row in rows(&directory) {
            if row.git_head != head {
                return Err(RevertReason::GitHeadMoved);
            }
            if !row.path.is_absolute() {
                continue;
            }
            let backup = match row.backup {
                Some(name) if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    Some(directory.join(name))
                }
                None => None,
                _ => continue,
            };
            // Earliest baseline owns the path, even if its backing file is missing.
            files.entry(row.path).or_insert(backup);
        }
    }
    if files.is_empty() {
        return Err(RevertReason::NoBaselines);
    }
    Ok(RestoreFiles { files })
}

pub fn discard_turns(transcript: &Path, turns: &[String]) {
    let root = session_dir(transcript);
    for turn in turns.iter().filter(|id| safe_id(id)) {
        let _ = fs::remove_dir_all(root.join(turn));
    }
}
pub fn discard_session(transcript: &Path) {
    let _ = fs::remove_dir_all(session_dir(transcript));
}

pub fn prune(sessions_dir: &Path, retention_days: u32, now: SystemTime) {
    let Ok(sessions) = fs::read_dir(sessions_dir.join("file-baselines")) else {
        return;
    };
    let retention = Duration::from_secs(u64::from(retention_days) * 86400);
    for session in sessions
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
    {
        let Ok(turns) = fs::read_dir(session.path()) else {
            continue;
        };
        for turn in turns
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        {
            if turn
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= retention)
            {
                let _ = fs::remove_dir_all(turn.path());
            }
        }
    }
}
