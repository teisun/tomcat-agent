//! 对话 transcript（pi 系 JSONL）：SessionHeader、TranscriptEntry 及流式读/追加写。
//!
//! 首行 session header，后续每行一条 entry；禁止全量加载，使用 BufReader 逐行读。

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::infra::error::AppError;

/// 首行：session header，与 pi-mono 格式一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub r#type: String, // "session"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    pub id: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// 公共基座：id、parentId、timestamp，树形结构。预留供后续树形操作使用。
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryBase {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// 单行 JSONL 条目的联合类型，通过 type 字段区分（snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEntry {
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    Custom(CustomEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelChangeEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub thinking_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_start_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_end_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_boundary: Option<bool>,
    /// 与 `id` 一致时可自指，便于阅读端识别 preheat 行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preheat_compaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// 从路径流式读取首行并解析为 SessionHeader；文件不存在或空返回错误。
pub fn read_header(path: &Path) -> Result<SessionHeader, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let mut reader = BufReader::new(f);
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(AppError::Io)? == 0 {
        return Err(AppError::Config("transcript 文件为空".to_string()));
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("transcript 首行为空".to_string()));
    }
    let header: SessionHeader = serde_json::from_str(trimmed)?;
    Ok(header)
}

/// 逐行读取 transcript，仅解析最近 `cap` 条 entry（避免全量加载）；从文件末尾往前取。
/// 返回的 Vec 顺序为从旧到新（与文件顺序一致）。
pub fn read_entries_tail(path: &Path, cap: usize) -> Result<Vec<TranscriptEntry>, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let mut lines: Vec<String> = reader
        .lines()
        .map(|r| r.map_err(AppError::Io))
        .collect::<Result<Vec<_>, _>>()?;
    // 首行是 header，跳过
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    lines.remove(0);
    let mut entries = Vec::with_capacity(cap.min(lines.len()));
    let start = if lines.len() <= cap {
        0
    } else {
        lines.len() - cap
    };
    for line in lines.drain(start..) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                warn!(line = trimmed, error = %e, "skipping unparseable JSONL entry");
                continue;
            }
        }
    }
    Ok(entries)
}

/// 追加一行 JSON 到 transcript 文件末尾（append-only）。
pub fn append_line(path: &Path, json: &str) -> Result<(), AppError> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(AppError::Io)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(AppError::Io)?;
    writeln!(f, "{}", json).map_err(AppError::Io)?;
    Ok(())
}

/// 追加一条 TranscriptEntry 到文件。
pub fn append_entry(path: &Path, entry: &TranscriptEntry) -> Result<(), AppError> {
    let json = serde_json::to_string(entry)?;
    append_line(path, &json)
}

/// 按 `Compaction` 行的 `id` 将 `isBoundary` 原地改为 `true`（重写整文件除首行 header 外仅替换匹配行）。
pub fn set_compaction_entry_is_boundary_true(path: &Path, entry_id: &str) -> Result<(), AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let lines: Vec<String> = reader
        .lines()
        .map(|r| r.map_err(AppError::Io))
        .collect::<Result<Vec<_>, _>>()?;
    if lines.is_empty() {
        return Err(AppError::Config("transcript 文件为空".to_string()));
    }

    let mut found = false;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.push(lines[0].clone());

    for line in lines.into_iter().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(line);
            continue;
        }
        let replaced = match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(TranscriptEntry::Compaction(mut ce)) => {
                if ce.id.as_deref() == Some(entry_id) {
                    ce.is_boundary = Some(true);
                    found = true;
                    Some(serde_json::to_string(&TranscriptEntry::Compaction(ce))?)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(json) = replaced {
            out.push(json);
        } else {
            out.push(trimmed.to_string());
        }
    }

    if !found {
        return Err(AppError::Config(format!(
            "transcript: compaction entry id {entry_id:?} not found"
        )));
    }

    let mut content = out.join("\n");
    content.push('\n');
    std::fs::write(path, content).map_err(AppError::Io)?;
    Ok(())
}

/// 追加 SessionHeader 作为首行（仅当文件不存在或为空时调用）。
pub fn write_header(path: &Path, header: &SessionHeader) -> Result<(), AppError> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(AppError::Io)?;
    let content = serde_json::to_string(header)?;
    std::fs::write(path, format!("{}\n", content)).map_err(AppError::Io)?;
    Ok(())
}

/// 从 TranscriptEntry 取 id（用于树形查询）。
fn entry_id(entry: &TranscriptEntry) -> Option<&str> {
    match entry {
        TranscriptEntry::Message(e) => e.id.as_deref(),
        TranscriptEntry::ModelChange(e) => e.id.as_deref(),
        TranscriptEntry::ThinkingLevelChange(e) => e.id.as_deref(),
        TranscriptEntry::Compaction(e) => e.id.as_deref(),
        TranscriptEntry::BranchSummary(e) => e.id.as_deref(),
        TranscriptEntry::Label(e) => e.id.as_deref(),
        TranscriptEntry::SessionInfo(e) => e.id.as_deref(),
        TranscriptEntry::Custom(e) => e.id.as_deref(),
    }
}

fn entry_parent_id(entry: &TranscriptEntry) -> Option<&str> {
    match entry {
        TranscriptEntry::Message(e) => e.parent_id.as_deref(),
        TranscriptEntry::ModelChange(e) => e.parent_id.as_deref(),
        TranscriptEntry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
        TranscriptEntry::Compaction(e) => e.parent_id.as_deref(),
        TranscriptEntry::BranchSummary(e) => e.parent_id.as_deref(),
        TranscriptEntry::Label(e) => e.parent_id.as_deref(),
        TranscriptEntry::SessionInfo(e) => e.parent_id.as_deref(),
        TranscriptEntry::Custom(e) => e.parent_id.as_deref(),
    }
}

/// 流式查找：按 id 返回第一条匹配的 entry；未找到返回 None。
pub fn get_entry(path: &Path, id: &str) -> Result<Option<TranscriptEntry>, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let mut lines = reader.lines();
    lines.next(); // 跳过 header
    for line in lines {
        let line = line.map_err(AppError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(trimmed) {
            if entry_id(&entry) == Some(id) {
                return Ok(Some(entry));
            }
        }
    }
    Ok(None)
}

/// 收集 parent_id 为给定值的 entries，最多 cap 条（避免无界）。
pub fn get_children(
    path: &Path,
    parent_id: &str,
    cap: usize,
) -> Result<Vec<TranscriptEntry>, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let mut lines = reader.lines();
    lines.next();
    let mut out = Vec::with_capacity(cap.min(256));
    for line in lines {
        if out.len() >= cap {
            break;
        }
        let line = line.map_err(AppError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(trimmed) {
            if entry_parent_id(&entry) == Some(parent_id) {
                out.push(entry);
            }
        }
    }
    Ok(out)
}

/// 返回 transcript 中最后一条 entry（文件末尾）；无 entry 返回 None。
pub fn get_leaf_entry(path: &Path) -> Result<Option<TranscriptEntry>, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let mut lines = reader.lines();
    lines.next();
    let mut last = None;
    for line in lines {
        let line = line.map_err(AppError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(trimmed) {
            last = Some(entry);
        }
    }
    Ok(last)
}

/// 从 leaf_id 沿 parent 链回溯到根，返回路径上的 entries（从根到叶）；最多 max_entries 条。
pub fn get_branch(
    path: &Path,
    leaf_id: &str,
    max_entries: usize,
) -> Result<Vec<TranscriptEntry>, AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let mut lines = reader.lines();
    lines.next();
    let mut by_id: std::collections::HashMap<String, TranscriptEntry> =
        std::collections::HashMap::with_capacity(max_entries.min(4096));
    for line in lines {
        if by_id.len() >= max_entries {
            break;
        }
        let line = line.map_err(AppError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(trimmed) {
            if let Some(id) = entry_id(&entry) {
                by_id.insert(id.to_string(), entry);
            }
        }
    }
    let mut branch = Vec::new();
    let mut current_id: Option<String> = Some(leaf_id.to_string());
    while let Some(id) = current_id {
        let entry = match by_id.get(&id) {
            Some(e) => e.clone(),
            None => break,
        };
        current_id = entry_parent_id(&entry).map(String::from);
        branch.push(entry);
        if branch.len() >= max_entries {
            break;
        }
    }
    branch.reverse();
    Ok(branch)
}

#[cfg(test)]
mod tests;
