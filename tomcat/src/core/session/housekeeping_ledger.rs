//! Small, discardable cache for startup housekeeping.
//!
//! The session transcripts remain the source of truth. This file records only
//! fingerprints and results derived from them, so a missing or malformed ledger
//! costs one full pass but can never make an old format unreadable.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic_with;

const LEDGER_FILE_NAME: &str = ".housekeeping.json";
const LEDGER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileFingerprint {
    pub len: u64,
    /// Nanoseconds since the Unix epoch. Together with `len`, this detects an
    /// append without reading the file itself.
    pub modified_ns: u64,
}

impl FileFingerprint {
    pub(crate) fn for_path(path: &Path) -> Result<Option<Self>, AppError> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AppError::Io(error)),
        };
        let modified_ns = metadata
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        Ok(Some(Self {
            len: metadata.len(),
            modified_ns,
        }))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscriptLedgerEntry {
    pub fingerprint: FileFingerprint,
    pub blob_shas: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarLedgerEntry {
    pub fingerprint: FileFingerprint,
    /// Oldest display record that still contains a diff. `None` means every
    /// display is already compacted (or there are no file displays).
    pub oldest_uncompacted_ts: Option<String>,
    /// An invalid timestamp cannot safely establish a next compaction deadline.
    /// Keep rescanning that sidecar rather than silently retaining a diff forever.
    pub requires_rescan: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionHousekeepingEntry {
    pub transcript: Option<TranscriptLedgerEntry>,
    pub sidecar: Option<SidecarLedgerEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HousekeepingLedger {
    version: u32,
    #[serde(default)]
    sessions: HashMap<String, SessionHousekeepingEntry>,
}

impl Default for HousekeepingLedger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            sessions: HashMap::new(),
        }
    }
}

impl HousekeepingLedger {
    pub(crate) fn load(sessions_dir: &Path) -> Self {
        let path = sessions_dir.join(LEDGER_FILE_NAME);
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(
                    ledger = %path.display(),
                    error = %error,
                    "sessions: ignoring unreadable housekeeping ledger"
                );
                return Self::default();
            }
        };
        match serde_json::from_slice::<Self>(&content) {
            Ok(ledger) if ledger.version == LEDGER_VERSION => ledger,
            Ok(ledger) => {
                warn!(
                    ledger = %path.display(),
                    found_version = ledger.version,
                    expected_version = LEDGER_VERSION,
                    "sessions: ignoring incompatible housekeeping ledger"
                );
                Self::default()
            }
            Err(error) => {
                warn!(
                    ledger = %path.display(),
                    error = %error,
                    "sessions: ignoring malformed housekeeping ledger"
                );
                Self::default()
            }
        }
    }

    pub(crate) fn save(&self, sessions_dir: &Path) -> Result<(), AppError> {
        let path = sessions_dir.join(LEDGER_FILE_NAME);
        let content = serde_json::to_vec_pretty(self)?;
        write_file_atomic_with(&path, |writer| {
            use std::io::Write;

            writer.write_all(&content).map_err(AppError::Io)?;
            writer.write_all(b"\n").map_err(AppError::Io)
        })
    }

    pub(crate) fn entry(&self, session_id: &str) -> Option<&SessionHousekeepingEntry> {
        self.sessions.get(session_id)
    }

    pub(crate) fn entry_mut(&mut self, session_id: &str) -> &mut SessionHousekeepingEntry {
        self.sessions.entry(session_id.to_string()).or_default()
    }

    pub(crate) fn prune_to(&mut self, session_ids: &HashSet<String>) {
        self.sessions
            .retain(|session_id, _| session_ids.contains(session_id));
    }
}

pub(crate) fn session_id_from_transcript_path(path: &Path) -> Option<&str> {
    path.file_stem()?.to_str()
}
