//! Per-message, first-write file backups. Independent of ShadowGit checkpoints.
//! Capture is best effort; only successful native writes publish a manifest row.
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
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

pub(crate) const KEEP_PREFIX: &str = "keep-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Baseline {
    pub(crate) path: PathBuf,
    pub(crate) backup: Option<String>,
    pub(crate) git_head: Option<String>,
    pub(crate) at: i64,
    pub(crate) permissions: Option<BackupPermissions>,
}

// Permissions belong to each path, not to its deduplicated content.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct BackupPermissions {
    #[cfg(unix)]
    mode: u32,
    #[cfg(not(unix))]
    readonly: bool,
}
impl BackupPermissions {
    fn capture(permissions: fs::Permissions) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Self {
                mode: permissions.mode(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                readonly: permissions.readonly(),
            }
        }
    }
    fn restore(self, path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            fs::Permissions::from_mode(self.mode)
        };
        #[cfg(not(unix))]
        let permissions = {
            let mut p = fs::metadata(path)?.permissions();
            p.set_readonly(self.readonly);
            p
        };
        fs::set_permissions(path, permissions)
    }
}

pub(crate) fn valid_backup_name(name: &str) -> bool {
    name.strip_prefix("sha256-")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Keep is a timestamp-only, empty directory. Old snapshot directories are ignored.
pub(crate) fn latest_keep(transcript: &Path) -> Result<Option<(i64, String)>, AppError> {
    latest_keep_in(&session_dir(transcript))
}
fn latest_keep_in(root: &Path) -> Result<Option<(i64, String)>, AppError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut latest = None;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(at) = name
            .strip_prefix(KEEP_PREFIX)
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        if at < 0
            || name != format!("{KEEP_PREFIX}{at}")
            || fs::read_dir(entry.path())?.next().is_some()
        {
            continue;
        }
        let candidate = (at, name);
        if latest.as_ref().is_none_or(|old| &candidate > old) {
            latest = Some(candidate);
        }
    }
    Ok(latest)
}

/// Wait across a same-millisecond boundary, never invent a counter or wait out clock rollback.
pub(crate) fn timestamp_after(after: Option<i64>) -> Result<i64, AppError> {
    for _ in 0..100 {
        let now = chrono::Utc::now().timestamp_millis();
        match after {
            Some(at) if now < at => break,
            Some(at) if now == at => std::thread::sleep(Duration::from_millis(1)),
            _ => return Ok(now),
        }
    }
    Err(AppError::Config("baseline_clock_not_advanced".into()))
}

fn capture_content(path: &Path, directory: &Path) -> Result<String, AppError> {
    let mut input = fs::File::open(path)?;
    let mut copy = tempfile::NamedTempFile::new_in(directory)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
        copy.write_all(&chunk[..n])?;
    }
    let name = format!("sha256-{:x}", hash.finalize());
    let target = directory.join(&name);
    match copy.persist_noclobber(&target) {
        Ok(_) => {}
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&target)?.is_file() || !files_equal(e.file.path(), &target)? {
                return Err(AppError::Config("invalid_baseline_content".into()));
            }
        }
        Err(e) => return Err(e.error.into()),
    }
    Ok(name)
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

pub(crate) fn read_rows(dir: &Path) -> Result<Vec<Baseline>, AppError> {
    let path = dir.join("baselines.jsonl");
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        return Err(AppError::Config("invalid_baseline".into()));
    }
    let text = fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.is_object() && value.get("at").is_none() {
            continue;
        }
        let row: Baseline = serde_json::from_value(value)?;
        if row.at < 0
            || !row.path.is_absolute()
            || row
                .backup
                .as_ref()
                .is_some_and(|name| !valid_backup_name(name))
        {
            return Err(AppError::Config("invalid_baseline".into()));
        }
        rows.push(row);
    }
    Ok(rows)
}
fn rows(dir: &Path) -> Vec<Baseline> {
    read_rows(dir).unwrap_or_default()
}

pub struct TurnFileBaselines {
    pub message_id: String,
    directory: PathBuf,
    keep_at: Option<i64>,
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
        if !safe_id(message_id) || message_id.starts_with(KEEP_PREFIX) {
            return None;
        }
        let directory = session_dir(transcript).join(message_id);
        let keep_at = latest_keep(transcript).ok()?.map(|(at, _)| at);
        let old: Vec<_> = rows(&directory)
            .into_iter()
            .filter(|row| keep_at.is_none_or(|at| row.at > at))
            .collect();
        Some(Arc::new(Self {
            message_id: message_id.to_string(),
            directory,
            keep_at,
            cwd,
            state: Mutex::new(CaptureState {
                seen: old.iter().map(|r| r.path.clone()).collect(),
                head: old.first().map(|r| r.git_head.clone()),
            }),
        }))
    }

    pub fn keep_is_current(&self) -> bool {
        self.directory.parent().is_some_and(|root| {
            latest_keep_in(root).is_ok_and(|keep| keep.map(|(at, _)| at) == self.keep_at)
        })
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
            let at = timestamp_after(self.keep_at)?;
            let (backup, permissions) = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() => (
                    Some(capture_content(&path, &self.directory)?),
                    Some(BackupPermissions::capture(meta.permissions())),
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
                Ok(_) => {
                    return Err(AppError::Config(crate::infra::i18n::tr(
                        "baseline.regularFile",
                        &[],
                    )))
                }
                Err(error) => return Err(error.into()),
            };
            Ok(Baseline {
                path: path.clone(),
                backup,
                git_head: head,
                at,
                permissions,
            })
        })();
        match result {
            Ok(row) => Some(PendingBaseline {
                owner: self.clone(),
                row,
                committed: false,
            }),
            Err(error) => {
                state.seen.remove(&path);
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

pub(crate) enum RestoreSource {
    Backup(PathBuf, Option<BackupPermissions>),
    Content(Vec<u8>),
    Absent,
}

pub struct RestoreFiles {
    files: BTreeMap<PathBuf, RestoreSource>,
}
impl RestoreFiles {
    pub(crate) fn selected(files: BTreeMap<PathBuf, RestoreSource>) -> Self {
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
                    RestoreSource::Backup(backup, permissions) => match fs::read(backup) {
                        Ok(bytes) => {
                            write_file_atomic(path, &bytes)?;
                            if let Some(permissions) = permissions {
                                permissions.restore(path)?;
                            }
                            Ok(())
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e.into()),
                    },
                    RestoreSource::Content(bytes) => write_file_atomic(path, bytes),
                    RestoreSource::Absent => match fs::remove_file(path) {
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
                Some(name) if valid_backup_name(&name) => Some(directory.join(name)),
                None => None,
                _ => continue,
            };
            // Earliest baseline owns the path, even if its backing file is missing.
            files.entry(row.path).or_insert_with(|| match backup {
                Some(path) => RestoreSource::Backup(path, row.permissions),
                None => RestoreSource::Absent,
            });
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

/// A disk-restoring rewind also reverses later acceptance boundaries.
pub fn discard_keeps_after(transcript: &Path, timestamp_ms: i64) {
    let Ok(entries) = fs::read_dir(session_dir(transcript)) else {
        return;
    };
    for entry in entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
    {
        let name = entry.file_name();
        if name
            .to_str()
            .and_then(|s| s.strip_prefix(KEEP_PREFIX))
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|t| {
                t >= 0 && t > timestamp_ms && name.to_string_lossy() == format!("{KEEP_PREFIX}{t}")
            })
        {
            // A new Keep is empty. Never recursively remove legacy snapshots or unexpected data.
            let _ = fs::remove_dir(entry.path());
        }
    }
}

/// Active sessions retain the complete review history. Only an idle/deleted session expires.
pub fn prune(sessions_dir: &Path, retention_days: u32, now: SystemTime) {
    let Ok(sessions) = fs::read_dir(sessions_dir.join("file-baselines")) else {
        return;
    };
    let retention = Duration::from_secs(u64::from(retention_days) * 86400);
    for session in sessions
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
    {
        let Some(id) = session
            .file_name()
            .to_str()
            .filter(|s| safe_id(s))
            .map(str::to_owned)
        else {
            continue;
        };
        let expired = match fs::symlink_metadata(sessions_dir.join(format!("{id}.jsonl"))) {
            Ok(meta) if meta.is_file() => meta
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= retention),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            _ => false, // I/O errors and unexpected path types are not proof of expiry.
        };
        if expired {
            let _ = fs::remove_dir_all(session.path());
        }
    }
}
