//! Crash-recovery cache for one computed but not-yet-applied preheat summary.
//!
//! The transcript remains the authoritative event log: it contains the cut marker and, once
//! applied, the completed summary body. This sidecar only covers the expensive gap between the
//! background LLM completing and the next foreground application opportunity.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::session::manager::CompactionResult;
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic_with;

const PREHEAT_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreheatCacheRecord {
    schema_version: u32,
    for_id: String,
    covered_start_id: String,
    covered_end_id: String,
    covered_count: usize,
    summary_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_covered_tokens_before: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_summary_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_tokens_saved: Option<usize>,
    #[serde(default)]
    preheat_elapsed_ms: u64,
}

/// Sidecar lives beside the transcript and always holds at most one record.
pub(crate) fn preheat_cache_path(transcript_path: &Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("session");
    transcript_path.with_file_name(format!("{stem}.preheat.jsonl"))
}

/// Atomically replaces the previous pending result. `Preheat` permits at most one such result,
/// so append-and-scan would only add stale state and unbounded growth.
pub(crate) fn write_preheat_cache(
    transcript_path: &Path,
    result: &CompactionResult,
) -> Result<(), AppError> {
    if transcript_path.as_os_str().is_empty() {
        return Ok(());
    }
    let for_id = result
        .transcript_compaction_entry_id
        .clone()
        .ok_or_else(|| AppError::internal("preheat cache requires a marker id"))?;
    let record = PreheatCacheRecord {
        schema_version: PREHEAT_CACHE_SCHEMA_VERSION,
        for_id,
        covered_start_id: result.covered_start_id.clone(),
        covered_end_id: result.covered_end_id.clone(),
        covered_count: result.covered_count,
        summary_text: result.summary_text.clone(),
        estimated_covered_tokens_before: result.estimated_covered_tokens_before,
        estimated_summary_tokens: result.estimated_summary_tokens,
        estimated_tokens_saved: result.estimated_tokens_saved,
        preheat_elapsed_ms: result.preheat_elapsed_ms,
    };
    let path = preheat_cache_path(transcript_path);
    write_file_atomic_with(&path, |writer| {
        serde_json::to_writer(&mut *writer, &record)?;
        writer.write_all(b"\n").map_err(AppError::Io)?;
        Ok(())
    })
}

/// Reads a cache record only when its schema and contents are valid. Callers must still verify
/// the matching transcript marker/body and live covered end before restoring it.
pub(crate) fn read_preheat_cache(transcript_path: &Path) -> Option<CompactionResult> {
    if transcript_path.as_os_str().is_empty() {
        return None;
    }
    let path = preheat_cache_path(transcript_path);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn!(cache = %path.display(), %error, "could not read preheat cache; recomputing if needed");
            return None;
        }
    };
    let record = match serde_json::from_str::<PreheatCacheRecord>(&raw) {
        Ok(record) if record.schema_version == PREHEAT_CACHE_SCHEMA_VERSION => record,
        Ok(record) => {
            warn!(
                cache = %path.display(),
                schema_version = record.schema_version,
                "unsupported preheat cache schema; ignoring cache"
            );
            return None;
        }
        Err(error) => {
            warn!(cache = %path.display(), %error, "corrupt preheat cache; ignoring cache");
            return None;
        }
    };
    if record.for_id.is_empty()
        || record.covered_start_id.is_empty()
        || record.covered_end_id.is_empty()
        || record.summary_text.is_empty()
    {
        warn!(cache = %path.display(), "incomplete preheat cache; ignoring cache");
        return None;
    }
    Some(CompactionResult {
        summary_text: record.summary_text,
        covered_start_id: record.covered_start_id,
        covered_end_id: record.covered_end_id,
        covered_count: record.covered_count,
        transcript_compaction_entry_id: Some(record.for_id),
        estimated_covered_tokens_before: record.estimated_covered_tokens_before,
        estimated_summary_tokens: record.estimated_summary_tokens,
        estimated_tokens_saved: record.estimated_tokens_saved,
        preheat_elapsed_ms: record.preheat_elapsed_ms,
    })
}

#[cfg(test)]
#[path = "tests/preheat_cache_test.rs"]
mod tests;
