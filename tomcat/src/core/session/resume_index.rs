use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::core::session::manager::{PlanEventKind, PlanEventRef};
use crate::core::session::transcript::{
    read_entries_tail_with_stats, TranscriptEntry, TranscriptReadStats,
};
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

const RESUME_INDEX_SCHEMA_VERSION: u32 = 1;
const RECENT_TURN_LIMIT: usize = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ResumeIndexIoStats {
    pub bytes_scanned: u64,
    pub entries_scanned: usize,
    pub max_live_bytes: usize,
}

impl ResumeIndexIoStats {
    pub(crate) fn add_read_stats(&mut self, other: TranscriptReadStats) {
        self.bytes_scanned += other.bytes_scanned;
        self.entries_scanned += other.entries_scanned;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeIndexSource {
    Existing,
    Rebuilt,
}

#[derive(Debug, Clone)]
pub(crate) struct ResumeIndexLoad {
    pub index: ResumeIndex,
    pub stats: ResumeIndexIoStats,
    pub source: ResumeIndexSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResumeEntryKind {
    Message,
    ModelChange,
    ThinkingLevelChange,
    ThinkingTrace,
    BranchSummary,
    Label,
    SessionInfo,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ResumeAnchor {
    pub entry_id: Option<String>,
    pub ordinal: usize,
    pub timestamp: String,
    pub entry_kind: ResumeEntryKind,
}

impl ResumeAnchor {
    fn from_entry(entry: &TranscriptEntry, ordinal: usize) -> Self {
        match entry {
            TranscriptEntry::Message(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::Message,
            },
            TranscriptEntry::ModelChange(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::ModelChange,
            },
            TranscriptEntry::ThinkingLevelChange(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::ThinkingLevelChange,
            },
            TranscriptEntry::ThinkingTrace(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::ThinkingTrace,
            },
            TranscriptEntry::BranchSummary(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::BranchSummary,
            },
            TranscriptEntry::Label(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::Label,
            },
            TranscriptEntry::SessionInfo(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::SessionInfo,
            },
            TranscriptEntry::Custom(e) => Self {
                entry_id: e.id.clone(),
                ordinal,
                timestamp: e.timestamp.clone(),
                entry_kind: ResumeEntryKind::Custom,
            },
        }
    }

    pub(crate) fn matches_entry(&self, entry: &TranscriptEntry) -> bool {
        let entry_anchor = Self::from_entry(entry, self.ordinal);
        if self.entry_id.is_some() {
            return self.entry_id == entry_anchor.entry_id;
        }
        self.entry_kind == entry_anchor.entry_kind && self.timestamp == entry_anchor.timestamp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResumeDayAnchor {
    pub date: String,
    pub first_entry: ResumeAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredPlanEventKind {
    Create,
    Build,
    Update,
}

impl From<PlanEventKind> for StoredPlanEventKind {
    fn from(value: PlanEventKind) -> Self {
        match value {
            PlanEventKind::Create => Self::Create,
            PlanEventKind::Build => Self::Build,
            PlanEventKind::Update => Self::Update,
        }
    }
}

impl From<StoredPlanEventKind> for PlanEventKind {
    fn from(value: StoredPlanEventKind) -> Self {
        match value {
            StoredPlanEventKind::Create => Self::Create,
            StoredPlanEventKind::Build => Self::Build,
            StoredPlanEventKind::Update => Self::Update,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPlanEventRef {
    pub kind: StoredPlanEventKind,
    pub plan_id: String,
    pub path: String,
    pub entry_id: Option<String>,
    pub ordinal: usize,
    pub timestamp: String,
}

impl StoredPlanEventRef {
    fn from_plan_event_ref(plan: PlanEventRef, anchor: &ResumeAnchor) -> Self {
        Self {
            kind: plan.kind.into(),
            plan_id: plan.plan_id,
            path: plan.path.to_string_lossy().to_string(),
            entry_id: anchor.entry_id.clone(),
            ordinal: anchor.ordinal,
            timestamp: anchor.timestamp.clone(),
        }
    }

    pub(crate) fn to_plan_event_ref(&self) -> Option<PlanEventRef> {
        Some(PlanEventRef {
            kind: self.kind.clone().into(),
            plan_id: self.plan_id.clone(),
            path: crate::infra::platform::normalize_path(&self.path).ok()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResumeIndex {
    pub schema_version: u32,
    pub transcript_size: u64,
    pub transcript_mtime_ms: u64,
    pub total_entries: usize,
    pub last_entry_id: Option<String>,
    pub latest_boundary: Option<ResumeAnchor>,
    pub recent_turn_starts: Vec<ResumeAnchor>,
    pub latest_day_first_entry: Option<ResumeDayAnchor>,
    pub latest_plan_event: Option<StoredPlanEventRef>,
}

impl ResumeIndex {
    pub(crate) fn latest_plan_event_ref(&self) -> Option<PlanEventRef> {
        self.latest_plan_event
            .as_ref()
            .and_then(StoredPlanEventRef::to_plan_event_ref)
    }
}

#[cfg(test)]
thread_local! {
    static LAST_INLINE_REBUILD_STATS: std::cell::RefCell<Option<ResumeIndexIoStats>> =
        const { std::cell::RefCell::new(None) };
}

fn transcript_mtime_ms(path: &Path) -> Result<u64, AppError> {
    let meta = fs::metadata(path).map_err(AppError::Io)?;
    let modified = meta.modified().map_err(AppError::Io)?;
    let since_epoch = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_millis(0));
    Ok(since_epoch.as_millis() as u64)
}

fn parse_entry_id(entry: &TranscriptEntry) -> Option<String> {
    match entry {
        TranscriptEntry::Message(e) => e.id.clone(),
        TranscriptEntry::ModelChange(e) => e.id.clone(),
        TranscriptEntry::ThinkingLevelChange(e) => e.id.clone(),
        TranscriptEntry::ThinkingTrace(e) => e.id.clone(),
        TranscriptEntry::BranchSummary(e) => e.id.clone(),
        TranscriptEntry::Label(e) => e.id.clone(),
        TranscriptEntry::SessionInfo(e) => e.id.clone(),
        TranscriptEntry::Custom(e) => e.id.clone(),
    }
}

fn parse_entry_timestamp(entry: &TranscriptEntry) -> &str {
    match entry {
        TranscriptEntry::Message(e) => &e.timestamp,
        TranscriptEntry::ModelChange(e) => &e.timestamp,
        TranscriptEntry::ThinkingLevelChange(e) => &e.timestamp,
        TranscriptEntry::ThinkingTrace(e) => &e.timestamp,
        TranscriptEntry::BranchSummary(e) => &e.timestamp,
        TranscriptEntry::Label(e) => &e.timestamp,
        TranscriptEntry::SessionInfo(e) => &e.timestamp,
        TranscriptEntry::Custom(e) => &e.timestamp,
    }
}

fn parse_entry_date(entry: &TranscriptEntry) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(parse_entry_timestamp(entry))
        .ok()
        .map(|dt| dt.date_naive())
}

fn is_user_turn_start(entry: &TranscriptEntry) -> bool {
    match entry {
        TranscriptEntry::Message(me) => {
            me.message.get("role").and_then(|v| v.as_str()) == Some("user")
        }
        _ => false,
    }
}

fn is_boundary(entry: &TranscriptEntry) -> bool {
    matches!(entry, TranscriptEntry::BranchSummary(ce) if ce.is_boundary == Some(true))
}

fn maybe_plan_event(entry: &TranscriptEntry) -> Option<PlanEventRef> {
    match entry {
        TranscriptEntry::Custom(custom) => PlanEventRef::from_custom_event(&custom.extra),
        _ => None,
    }
}

pub(crate) fn resume_index_path(transcript_path: &Path) -> PathBuf {
    let stem = transcript_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    transcript_path.with_file_name(format!("{stem}.resume-index.json"))
}

fn read_sidecar_raw(transcript_path: &Path) -> Result<Option<ResumeIndex>, AppError> {
    let path = resume_index_path(transcript_path);
    match fs::read_to_string(&path) {
        Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::Io(error)),
    }
}

fn write_sidecar(transcript_path: &Path, index: &ResumeIndex) -> Result<(), AppError> {
    let path = resume_index_path(transcript_path);
    let body = serde_json::to_vec_pretty(index)?;
    write_file_atomic(&path, &body)
}

fn apply_entry(index: &mut ResumeIndex, entry: &TranscriptEntry, ordinal: usize) {
    let anchor = ResumeAnchor::from_entry(entry, ordinal);
    index.total_entries = ordinal + 1;
    index.last_entry_id = parse_entry_id(entry);

    if is_boundary(entry) {
        index.latest_boundary = Some(anchor.clone());
    }
    if is_user_turn_start(entry) {
        index.recent_turn_starts.push(anchor.clone());
        if index.recent_turn_starts.len() > RECENT_TURN_LIMIT {
            let extra = index.recent_turn_starts.len() - RECENT_TURN_LIMIT;
            index.recent_turn_starts.drain(0..extra);
        }
    }
    if let Some(date) = parse_entry_date(entry) {
        let date_str = date.to_string();
        match &index.latest_day_first_entry {
            Some(day_anchor) if day_anchor.date == date_str => {}
            Some(day_anchor) => {
                if date > NaiveDate::parse_from_str(&day_anchor.date, "%Y-%m-%d").unwrap_or(date) {
                    index.latest_day_first_entry = Some(ResumeDayAnchor {
                        date: date_str,
                        first_entry: anchor.clone(),
                    });
                }
            }
            None => {
                index.latest_day_first_entry = Some(ResumeDayAnchor {
                    date: date_str,
                    first_entry: anchor.clone(),
                });
            }
        }
    }
    if let Some(plan_event) = maybe_plan_event(entry) {
        index.latest_plan_event =
            Some(StoredPlanEventRef::from_plan_event_ref(plan_event, &anchor));
    }
}

fn build_empty_index(transcript_path: &Path) -> Result<ResumeIndex, AppError> {
    let meta = fs::metadata(transcript_path).map_err(AppError::Io)?;
    Ok(ResumeIndex {
        schema_version: RESUME_INDEX_SCHEMA_VERSION,
        transcript_size: meta.len(),
        transcript_mtime_ms: transcript_mtime_ms(transcript_path)?,
        total_entries: 0,
        last_entry_id: None,
        latest_boundary: None,
        recent_turn_starts: Vec::new(),
        latest_day_first_entry: None,
        latest_plan_event: None,
    })
}

fn build_index_from_lines(
    transcript_path: &Path,
    lines: &[String],
    mut stats: ResumeIndexIoStats,
) -> Result<(ResumeIndex, ResumeIndexIoStats), AppError> {
    let mut index = build_empty_index(transcript_path)?;
    stats.max_live_bytes = stats
        .max_live_bytes
        .max(lines.iter().map(|line| line.len()).max().unwrap_or(0));

    let mut ordinal = 0usize;
    for line in lines.iter().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(entry) => {
                stats.entries_scanned += 1;
                apply_entry(&mut index, &entry, ordinal);
                ordinal += 1;
            }
            Err(error) => {
                tracing::warn!(line = trimmed, error = %error, "skipping unparseable JSONL entry while rebuilding resume index");
            }
        }
    }

    write_sidecar(transcript_path, &index)?;
    Ok((index, stats))
}

pub(crate) fn rebuild_resume_index(
    transcript_path: &Path,
) -> Result<(ResumeIndex, ResumeIndexIoStats), AppError> {
    let f = std::fs::File::open(transcript_path).map_err(AppError::Io)?;
    let mut reader = BufReader::new(f);
    let mut stats = ResumeIndexIoStats::default();
    let mut header = String::new();
    stats.max_live_bytes = header.capacity();
    stats.bytes_scanned += reader.read_line(&mut header).map_err(AppError::Io)? as u64;
    let mut lines = vec![header];
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).map_err(AppError::Io)?;
        if bytes == 0 {
            break;
        }
        stats.bytes_scanned += bytes as u64;
        stats.max_live_bytes = stats.max_live_bytes.max(line.len());
        lines.push(line.clone());
    }

    build_index_from_lines(transcript_path, &lines, stats)
}

pub(crate) fn rebuild_resume_index_from_lines(
    transcript_path: &Path,
    lines: &[String],
) -> Result<(ResumeIndex, ResumeIndexIoStats), AppError> {
    let (index, stats) =
        build_index_from_lines(transcript_path, lines, ResumeIndexIoStats::default())?;
    #[cfg(test)]
    LAST_INLINE_REBUILD_STATS.with(|slot| {
        *slot.borrow_mut() = Some(stats);
    });
    Ok((index, stats))
}

fn validate_sidecar(
    index: &ResumeIndex,
    transcript_path: &Path,
) -> Result<(bool, ResumeIndexIoStats), AppError> {
    if index.schema_version != RESUME_INDEX_SCHEMA_VERSION {
        return Ok((false, ResumeIndexIoStats::default()));
    }

    let meta = fs::metadata(transcript_path).map_err(AppError::Io)?;
    if index.transcript_size != meta.len()
        || index.transcript_mtime_ms != transcript_mtime_ms(transcript_path)?
    {
        return Ok((false, ResumeIndexIoStats::default()));
    }

    let (tail, tail_stats) = read_entries_tail_with_stats(transcript_path, 1)?;
    let mut stats = ResumeIndexIoStats::default();
    stats.add_read_stats(tail_stats);
    let last_entry_id = tail.last().and_then(parse_entry_id);
    Ok((last_entry_id == index.last_entry_id, stats))
}

pub(crate) fn load_or_rebuild_resume_index(
    transcript_path: &Path,
) -> Result<ResumeIndexLoad, AppError> {
    if let Some(index) = read_sidecar_raw(transcript_path)? {
        let (valid, stats) = validate_sidecar(&index, transcript_path)?;
        if valid {
            return Ok(ResumeIndexLoad {
                index,
                stats,
                source: ResumeIndexSource::Existing,
            });
        }
    }

    let (index, stats) = rebuild_resume_index(transcript_path)?;
    Ok(ResumeIndexLoad {
        index,
        stats,
        source: ResumeIndexSource::Rebuilt,
    })
}

pub(crate) fn update_resume_index_after_append(
    transcript_path: &Path,
    entry: &TranscriptEntry,
) -> Result<(), AppError> {
    let mut index = match read_sidecar_raw(transcript_path)? {
        Some(index) if index.schema_version == RESUME_INDEX_SCHEMA_VERSION => index,
        _ => {
            let _ = rebuild_resume_index(transcript_path)?;
            return Ok(());
        }
    };

    let ordinal = index.total_entries;
    apply_entry(&mut index, entry, ordinal);
    index.transcript_size = fs::metadata(transcript_path).map_err(AppError::Io)?.len();
    index.transcript_mtime_ms = transcript_mtime_ms(transcript_path)?;
    write_sidecar(transcript_path, &index)
}

pub(crate) fn remove_resume_index(transcript_path: &Path) -> Result<(), AppError> {
    match fs::remove_file(resume_index_path(transcript_path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Io(error)),
    }
}

#[cfg(test)]
pub(crate) fn take_last_inline_rebuild_stats_for_tests() -> Option<ResumeIndexIoStats> {
    LAST_INLINE_REBUILD_STATS.with(|slot| slot.borrow_mut().take())
}
