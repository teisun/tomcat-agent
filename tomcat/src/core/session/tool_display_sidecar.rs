//! Durable, append-only UI rendering data kept out of the LLM transcript.
//!
//! A tool result has two consumers with different needs:
//! * the model only needs its textual result;
//! * the UI may need a structured diff.
//!
//! Keeping the latter beside, rather than inside, the transcript prevents a local
//! rendering snapshot from being replayed to the model or making every transcript read
//! pay for it.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::session::transcript::{append_sidecar_line_with_sync, SyncLevel};
use crate::infra::error::AppError;
use crate::infra::events::ToolDisplay;
use crate::infra::platform::write_file_atomic_with;

const REVERSE_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const TOOL_DISPLAY_DIFF_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Main-session and subagent transcript writers share this lock with compaction. Without it, an
/// append between compactor read and atomic rename could be silently lost.
static SIDECAR_LOCKS: OnceLock<DashMap<PathBuf, Arc<Mutex<()>>>> = OnceLock::new();

fn sidecar_lock(path: &Path) -> Arc<Mutex<()>> {
    SIDECAR_LOCKS
        .get_or_init(DashMap::new)
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDisplaySidecarEntry {
    pub tool_call_id: String,
    pub ts: String,
    pub display: ToolDisplay,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ToolDisplayCompaction {
    pub entries_compacted: usize,
    /// The oldest record that still has an unexpired diff after this compaction.
    pub oldest_uncompacted_ts: Option<String>,
    /// An unparseable timestamp with a display prevents establishing a safe
    /// future deadline, so startup housekeeping must inspect it again.
    pub requires_rescan: bool,
}

/// `<session>.tool_display.jsonl`, a derived rendering record rather than a session.
pub(crate) fn tool_display_sidecar_path(transcript_path: &Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("session");
    transcript_path.with_file_name(format!("{stem}.tool_display.jsonl"))
}

/// The sidecar is made durable before its corresponding transcript line is appended.
/// A crash between the two writes may leave an orphan sidecar row, which is harmless:
/// readers only attach rows to tool IDs that exist in the requested transcript page.
pub(crate) fn append_tool_display(
    transcript_path: &Path,
    tool_call_id: &str,
    ts: &str,
    display: &ToolDisplay,
) -> Result<(), AppError> {
    let entry = ToolDisplaySidecarEntry {
        tool_call_id: tool_call_id.to_string(),
        ts: ts.to_string(),
        display: display.clone(),
    };
    let line = serde_json::to_string(&entry)?;
    let path = tool_display_sidecar_path(transcript_path);
    let lock = sidecar_lock(&path);
    let _guard = lock.lock().map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "session.displayLock",
            &[("detail", &error.to_string())],
        ))
    })?;
    std::fs::metadata(transcript_path).map_err(AppError::Io)?;
    append_sidecar_line_with_sync(&path, &line, SyncLevel::SyncData)
}

fn compact_display(display: &mut ToolDisplay) -> bool {
    match display {
        ToolDisplay::File {
            diff,
            diff_truncated,
            expired,
            ..
        } => {
            let changed = !*expired || diff.is_some() || *diff_truncated;
            *diff = None;
            *diff_truncated = false;
            *expired = true;
            changed
        }
        ToolDisplay::Files { files, expired, .. } => {
            let mut changed = !*expired;
            for file in files {
                changed |= !file.expired || file.diff.is_some() || file.diff_truncated;
                file.diff = None;
                file.diff_truncated = false;
                file.expired = true;
            }
            *expired = true;
            changed
        }
        ToolDisplay::Plan { .. } | ToolDisplay::Text { .. } => false,
    }
}

fn display_needs_compaction(display: &ToolDisplay) -> bool {
    match display {
        ToolDisplay::File {
            diff,
            diff_truncated,
            expired,
            ..
        } => !*expired || diff.is_some() || *diff_truncated,
        ToolDisplay::Files { files, expired, .. } => {
            !*expired
                || files
                    .iter()
                    .any(|file| !file.expired || file.diff.is_some() || file.diff_truncated)
        }
        ToolDisplay::Plan { .. } | ToolDisplay::Text { .. } => false,
    }
}

#[derive(Debug, Default)]
struct SidecarCompactionInspection {
    needs_compaction: bool,
    oldest_uncompacted_ts: Option<(DateTime<Utc>, String)>,
    requires_rescan: bool,
}

impl SidecarCompactionInspection {
    fn into_report(self) -> ToolDisplayCompaction {
        ToolDisplayCompaction {
            entries_compacted: 0,
            oldest_uncompacted_ts: self.oldest_uncompacted_ts.map(|(_, timestamp)| timestamp),
            requires_rescan: self.requires_rescan,
        }
    }
}

/// Reads a sidecar without modifying it and determines both whether a rewrite
/// is needed now and the next time at which it can become necessary.
fn inspect_sidecar_for_compaction(
    path: &Path,
    cutoff: DateTime<Utc>,
) -> Result<SidecarCompactionInspection, AppError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SidecarCompactionInspection::default());
        }
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut inspection = SidecarCompactionInspection::default();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(AppError::Io)?;
        let entry = match serde_json::from_str::<ToolDisplaySidecarEntry>(&line) {
            Ok(entry) => entry,
            Err(error) => {
                warn!(
                    sidecar = %path.display(),
                    error = %error,
                    "preserving malformed tool-display sidecar row during compaction"
                );
                continue;
            }
        };
        if !display_needs_compaction(&entry.display) {
            continue;
        }
        match DateTime::parse_from_rfc3339(&entry.ts) {
            Ok(timestamp) => {
                let timestamp = timestamp.with_timezone(&Utc);
                if timestamp < cutoff {
                    inspection.needs_compaction = true;
                } else {
                    let oldest = inspection
                        .oldest_uncompacted_ts
                        .get_or_insert_with(|| (timestamp, entry.ts.clone()));
                    if timestamp < oldest.0 {
                        *oldest = (timestamp, entry.ts);
                    }
                }
            }
            Err(error) => {
                warn!(
                    sidecar = %path.display(),
                    timestamp = %entry.ts,
                    error = %error,
                    "keeping tool-display row with unparseable timestamp"
                );
                inspection.requires_rescan = true;
            }
        }
    }
    Ok(inspection)
}

/// Rewrites only expired file-display entries to their small, durable summary.
///
/// The caller is responsible for taking the same per-transcript lock as append. Keeping the
/// compactor here means both migration and startup housekeeping share one JSON shape.
pub(crate) fn compact_tool_display_sidecar(
    transcript_path: &Path,
    now: DateTime<Utc>,
) -> Result<ToolDisplayCompaction, AppError> {
    let path = tool_display_sidecar_path(transcript_path);
    let lock = sidecar_lock(&path);
    let _guard = lock.lock().map_err(|error| {
        AppError::Config(crate::infra::i18n::tr(
            "session.displayLock",
            &[("detail", &error.to_string())],
        ))
    })?;
    let cutoff = now
        - chrono::Duration::from_std(TOOL_DISPLAY_DIFF_RETENTION)
            .expect("seven-day retention fits chrono duration");
    let inspection = inspect_sidecar_for_compaction(&path, cutoff)?;
    let needs_compaction = inspection.needs_compaction;
    let mut report = inspection.into_report();
    if !needs_compaction {
        return Ok(report);
    }
    let file = std::fs::File::open(&path).map_err(AppError::Io)?;

    std::fs::metadata(transcript_path).map_err(AppError::Io)?;
    write_file_atomic_with(&path, |writer| {
        for line in BufReader::new(file).lines() {
            let line = line.map_err(AppError::Io)?;
            let rewritten = match serde_json::from_str::<ToolDisplaySidecarEntry>(&line) {
                Ok(mut entry) => {
                    let expired = DateTime::parse_from_rfc3339(&entry.ts)
                        .map(|timestamp| timestamp.with_timezone(&Utc) < cutoff)
                        .unwrap_or_else(|error| {
                            warn!(
                                sidecar = %path.display(),
                                timestamp = %entry.ts,
                                error = %error,
                                "keeping tool-display row with unparseable timestamp"
                            );
                            false
                        });
                    if expired && compact_display(&mut entry.display) {
                        report.entries_compacted += 1;
                        serde_json::to_string(&entry)?
                    } else {
                        line
                    }
                }
                Err(error) => {
                    warn!(
                        sidecar = %path.display(),
                        error = %error,
                        "preserving malformed tool-display sidecar row during compaction"
                    );
                    line
                }
            };
            writer
                .write_all(rewritten.as_bytes())
                .map_err(AppError::Io)?;
            writer.write_all(b"\n").map_err(AppError::Io)?;
        }
        Ok(())
    })?;
    Ok(report)
}

/// Reads newest records first until every requested tool call is found or the requested
/// transcript page's oldest timestamp has been crossed.
///
/// This deliberately has no in-memory index: old history needs no resident state and a
/// freshly opened page normally reads only its own trailing sidecar lines. The timestamp
/// boundary also guarantees that a page containing tools without displays cannot turn the
/// common case into a full sidecar scan.
pub(crate) fn read_tool_displays_for_calls(
    transcript_path: &Path,
    wanted: &HashSet<String>,
    oldest_page_message_at: Option<DateTime<Utc>>,
) -> Result<HashMap<String, ToolDisplay>, AppError> {
    read_tool_displays_for_calls_with_observer(
        transcript_path,
        wanted,
        oldest_page_message_at,
        |_| {},
    )
}

fn read_tool_displays_for_calls_with_observer(
    transcript_path: &Path,
    wanted: &HashSet<String>,
    oldest_page_message_at: Option<DateTime<Utc>>,
    mut on_scanned: impl FnMut(&ToolDisplaySidecarEntry),
) -> Result<HashMap<String, ToolDisplay>, AppError> {
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let path = tool_display_sidecar_path(transcript_path);
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut pos = file.metadata().map_err(AppError::Io)?.len();
    let mut carry = Vec::new();
    let mut found = HashMap::with_capacity(wanted.len());

    while pos > 0 && found.len() < wanted.len() {
        let read_len = REVERSE_CHUNK_BYTES.min(pos as usize);
        pos -= read_len as u64;
        file.seek(SeekFrom::Start(pos)).map_err(AppError::Io)?;
        let mut chunk = vec![0; read_len];
        file.read_exact(&mut chunk).map_err(AppError::Io)?;
        if !carry.is_empty() {
            chunk.extend_from_slice(&carry);
            carry.clear();
        }

        let mut end = chunk.len();
        for index in (0..chunk.len()).rev() {
            if chunk[index] != b'\n' {
                continue;
            }
            let line = &chunk[index + 1..end];
            end = index;
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<ToolDisplaySidecarEntry>(line) {
                Ok(entry) => {
                    if sidecar_entry_is_older_than_page(&entry, oldest_page_message_at, &path) {
                        return Ok(found);
                    }
                    on_scanned(&entry);
                    if wanted.contains(&entry.tool_call_id) {
                        found.entry(entry.tool_call_id).or_insert(entry.display);
                        if found.len() == wanted.len() {
                            break;
                        }
                    }
                }
                Err(error) => warn!(
                    sidecar = %path.display(),
                    error = %error,
                    "skipping malformed tool-display sidecar row"
                ),
            }
        }
        carry = chunk[..end].to_vec();
    }
    // The first JSONL row has no preceding newline. It remains in `carry` after the
    // reverse scan, including when the sidecar contains exactly one record.
    if !carry.is_empty() && found.len() < wanted.len() {
        match serde_json::from_slice::<ToolDisplaySidecarEntry>(&carry) {
            Ok(entry) => {
                if !sidecar_entry_is_older_than_page(&entry, oldest_page_message_at, &path) {
                    on_scanned(&entry);
                    if wanted.contains(&entry.tool_call_id) {
                        found.entry(entry.tool_call_id).or_insert(entry.display);
                    }
                }
            }
            Err(error) => warn!(
                sidecar = %path.display(),
                error = %error,
                "skipping malformed tool-display sidecar row"
            ),
        }
    }
    Ok(found)
}

fn sidecar_entry_is_older_than_page(
    entry: &ToolDisplaySidecarEntry,
    oldest_page_message_at: Option<DateTime<Utc>>,
    path: &Path,
) -> bool {
    let Some(oldest_page_message_at) = oldest_page_message_at else {
        return false;
    };
    match DateTime::parse_from_rfc3339(&entry.ts) {
        Ok(timestamp) => timestamp.with_timezone(&Utc) < oldest_page_message_at,
        Err(error) => {
            warn!(
                sidecar = %path.display(),
                timestamp = %entry.ts,
                error = %error,
                "ignoring invalid tool-display timestamp as a page scan boundary"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tools::primitive::{DiffTag, FileDiffLine};

    fn file_display(text: &str) -> ToolDisplay {
        ToolDisplay::File {
            file: "src/lib.rs".to_string(),
            added: Some(1),
            removed: Some(0),
            diff: Some(vec![FileDiffLine {
                tag: DiffTag::Add,
                old_line: None,
                new_line: Some(1),
                skipped_lines: None,
                text: text.to_string(),
            }]),
            diff_truncated: false,
            expired: false,
        }
    }

    #[test]
    fn compaction_expires_only_old_diffs_and_is_idempotent() {
        let temp = tempfile::TempDir::new().unwrap();
        let transcript = temp.path().join("session.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let now = Utc::now();
        append_tool_display(
            &transcript,
            "old",
            &(now - chrono::Duration::days(8)).to_rfc3339(),
            &file_display("old change"),
        )
        .unwrap();
        append_tool_display(
            &transcript,
            "fresh",
            &(now - chrono::Duration::days(6)).to_rfc3339(),
            &file_display("fresh change"),
        )
        .unwrap();

        assert_eq!(
            compact_tool_display_sidecar(&transcript, now)
                .unwrap()
                .entries_compacted,
            1
        );
        let sidecar = tool_display_sidecar_path(&transcript);
        let rows: Vec<ToolDisplaySidecarEntry> =
            BufReader::new(std::fs::File::open(&sidecar).unwrap())
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect();
        let ToolDisplay::File {
            diff: old_diff,
            expired: old_expired,
            ..
        } = &rows[0].display
        else {
            panic!("expected file display");
        };
        assert!(old_expired);
        assert!(old_diff.is_none());
        let ToolDisplay::File {
            diff: fresh_diff,
            expired: fresh_expired,
            ..
        } = &rows[1].display
        else {
            panic!("expected file display");
        };
        assert!(!fresh_expired);
        assert_eq!(fresh_diff.as_ref().map(Vec::len), Some(1));

        let once = std::fs::read(&sidecar).unwrap();
        let modified_before_noop = std::fs::metadata(&sidecar).unwrap().modified().unwrap();
        assert_eq!(
            compact_tool_display_sidecar(&transcript, now)
                .unwrap()
                .entries_compacted,
            0
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), once);
        assert_eq!(
            std::fs::metadata(&sidecar).unwrap().modified().unwrap(),
            modified_before_noop,
            "a no-op compaction must not atomically replace the sidecar"
        );
    }

    #[test]
    fn reverse_read_stops_before_rows_older_than_requested_page() {
        let temp = tempfile::TempDir::new().unwrap();
        let transcript = temp.path().join("session.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let old = "2026-09-04T00:00:00Z";
        let page = "2026-09-04T00:01:00Z";
        append_tool_display(&transcript, "old-sentinel", old, &file_display("old")).unwrap();
        append_tool_display(&transcript, "shown", page, &file_display("shown")).unwrap();

        let wanted = HashSet::from(["shown".to_string(), "no-display".to_string()]);
        let oldest_page_message_at = DateTime::parse_from_rfc3339(page)
            .unwrap()
            .with_timezone(&Utc);
        let mut scanned = Vec::new();
        let displays = read_tool_displays_for_calls_with_observer(
            &transcript,
            &wanted,
            Some(oldest_page_message_at),
            |entry| scanned.push(entry.tool_call_id.clone()),
        )
        .unwrap();

        assert_eq!(displays.len(), 1);
        assert!(displays.contains_key("shown"));
        assert_eq!(scanned, vec!["shown"]);
        assert!(
            !scanned.iter().any(|id| id == "old-sentinel"),
            "the old sidecar row must not be read after crossing the page boundary"
        );
    }
}
