//! # Session Transcript（pi-mono 相容 JSONL 格式）
//!
//! 单 session 落盘的对话记录：与 pi-mono 共享 schema，append-only / BufReader
//! 流式读，禁止全量加载到内存（单 session 可能上百 MB）。是 [`crate::core::compaction`]
//! 与 `--resume` / Checkpoint 体系的物理基座。
//!
//! ## 文件结构
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  ~/.tomcat/sessions/<session_id>.jsonl                                      │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Line 1   { "type":"session", "id":"...", "version":1, "timestamp":..., │
//! │             "cwd":"..." }                          ← SessionHeader      │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Line N   { "type":"message", "id":"...", "parentId":"...",             │
//! │             "timestamp":..., "role":"user|assistant|tool",              │
//! │             "kind":"normal|steering|compactionSummary",                  │
//! │             "content":[{...}], ... }              ← TranscriptEntry     │
//! │  Line N+1 { "type":"branchSummary", ... }                                │
//! │  Line N+2 { "type":"modelChange", ... }                                  │
//! │  ...                                                                     │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  EOF（永远 append-only，原子落盘后立即 fsync 由 platform 层管）         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 9 种 TranscriptEntry（tag = "type"）
//!
//! ```text
//! TranscriptEntry
//! ├─ Message               role + kind + content[]    （主流：user/assistant/tool）
//! ├─ ModelChange           model_id 切换记录          （/model 命令）
//! ├─ ThinkingLevelChange   thinking_level 切换记录    （/thinking 命令）
//! ├─ ThinkingTrace         thinking 独立持久化条目    （`persist=true` 时写入）
//! ├─ BranchSummary         分支摘要 + isBoundary       （Layer-1 压缩落点）
//! ├─ Label                 用户书签                   （/label 命令）
//! ├─ SessionInfo           会话级元数据                （新版会话补充信息）
//! ├─ Custom                透传 JSON                   （扩展逃生舱）
//! └─ EntryBase             公共基座（id / parentId / timestamp）
//! ```
//!
//! ## parent_id 树形结构
//!
//! ```text
//!   header.id → 不参与 parent_id 引用
//!
//!   M1 (parentId=None)          ← 第 1 条根 entry
//!    │
//!    ├── M2 (parentId=M1)       ← 顺序对话
//!    │    │
//!    │    └── M3 (parentId=M2)
//!    │         │
//!    │         ├── BS1 (parentId=M3, isBoundary=true)  ← Layer-1 摘要
//!    │         │
//!    │         └── M4 (parentId=BS1)  ← 压缩后续话
//!    │
//!    └── M2' (parentId=M1)      ← 分支（Steering / 重生成 等）
//! ```
//!
//! ## 公共 API 两类
//!
//! ```text
//! ┌─ 流式读（BufReader 逐行解析，禁止 read_to_end） ────────────────────────┐
//! │  read_header              ► 第 1 行 SessionHeader                       │
//! │  read_entries_tail(cap)   ► 从尾部反向读 cap 条                          │
//! │  get_entry(id)            ► 按 id 线性扫描                               │
//! │  get_branch(leaf_id)      ► 沿 parentId 反向追溯到根                     │
//! │  get_children(parent_id)  ► 全文件扫描收集 parentId 命中项               │
//! │  get_leaf_entry           ► 最后一行                                     │
//! └────────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ 追加 / 插入 / 局部改写 ────────────────────────────────────────────────┐
//! │  append_line(json)            ► 单纯字符串追加，最低开销                 │
//! │  append_entry(&entry)         ► 序列化 + append                         │
//! │  insert_entry_after_message_id ► 为某 message 之后原地插入              │
//! │                                  （compaction 把 BranchSummary 落到这）│
//! │  set_branch_summary_entry_is_boundary_true                              │
//! │                                ► 仅改 BranchSummary 的 isBoundary 标志  │
//! │  remove_branch_summary_entry_by_id                                      │
//! │                                ► 失败摘要回滚（compaction error 路径）  │
//! │  write_header                 ► 重写第 1 行（cwd 变更等罕见场景）        │
//! └────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 设计要点
//!
//! - **Append-only 优先**：90% 写入是 `append_line` / `append_entry`，避免锁全文件。
//! - **insert / set / remove 走 atomic rewrite**：先读全文件（流式）→ 改→ 写
//!   临时 → rename，由 [`crate::infra::platform::write_file_atomic`] 兜底原子性。
//! - **EntryBase + serde flatten**：所有变体共享 `id / parentId / timestamp`，新增
//!   类型只需在 `TranscriptEntry` enum 加一个变体。
//! - **`#[serde(rename_all = "camelCase")]`**：与 pi-mono `transcript.ts` 字段名
//!   完全对齐，跨语言会话可互操作。

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

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
    /// 模型思考链条独立条目：仅在 `llm.thinking.persist=true` 时写入；
    /// **不**参与 hydrate 重放（避免污染 assistant 正文与上行 messages）。
    ThinkingTrace(ThinkingTraceEntry),
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

/// `type=thinking_trace`：单条 assistant 消息流期间累计的 thinking 文本（合并写入），
/// 仅在 `llm.thinking.persist = true` 时由 chat 层 listener 落盘。`signature` 仅当
/// provider 在 `StreamEvent::Thinking` 携带时填入（Anthropic 等），多块/多 signature
/// 场景留给后续 `outbound-transform-followup` 进一步细化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingTraceEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// JSONL `type: branch_summary`：上下文压缩摘要行（原 compaction 语义），含 `S::E` 与 boundary 等字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_covered_tokens_before: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_summary_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_saved: Option<usize>,
    /// T2-P0-002 Phase D：preheat 摘要任务 3 次重试全部失败时记录的最末错误，
    /// 与 `summary == None` 配合形成「失败锚点」，便于运行期与 reload 时定位故障窗口。
    /// 旧 transcript 行（无此字段）反序列化为 `None`，序列化时 `skip_serializing_if`
    /// 保证现有成功路径不再写出新字段，避免 JSONL 行长度膨胀。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// T2-P0-002 Phase D：preheat 摘要任务实际进行的尝试次数（含失败的最末一次），
    /// 通常为 `MAX_PREHEAT_RETRIES = 3`。与 `error` 同步写入，用于 reload 时跳过失败行
    /// 不重建假摘要 ChatMessage（详见 `session::manager::context::fold_entries_to_messages`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
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

/// 在首条 `type=message` 且 `id == anchor_message_id` 的 JSONL 行**之后**插入 `entry`（整文件原子写）。
///
/// §5.7.4：找不到锚点时打 `warn` 并退化为 [`append_entry`]（尾部追加），保证 L1 仍可落盘。
pub fn insert_entry_after_message_id(
    path: &Path,
    anchor_message_id: &str,
    entry: &TranscriptEntry,
) -> Result<(), AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let lines: Vec<String> = reader
        .lines()
        .map(|r| r.map_err(AppError::Io))
        .collect::<Result<Vec<_>, _>>()?;
    if lines.is_empty() {
        return Err(AppError::Config("transcript 文件为空".to_string()));
    }

    let mut anchor_line: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(TranscriptEntry::Message(me)) = serde_json::from_str::<TranscriptEntry>(trimmed) {
            if me.id.as_deref() == Some(anchor_message_id) {
                anchor_line = Some(idx);
                break;
            }
        }
    }

    if anchor_line.is_none() {
        warn!(
            anchor = %anchor_message_id,
            "insert_entry_after_message_id: anchor message not found; falling back to append_entry"
        );
        return append_entry(path, entry);
    }
    let anchor_line = anchor_line.unwrap();
    let new_json = serde_json::to_string(entry)?;
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 1);
    for (i, line) in lines.iter().enumerate() {
        out.push(line.clone());
        if i == anchor_line {
            out.push(new_json.clone());
        }
    }
    let mut content = out.join("\n");
    content.push('\n');
    write_file_atomic(path, content.as_bytes())?;
    Ok(())
}

/// 按 `branch_summary` 行的 `id` 将 `isBoundary` 改为 `true`（重写整文件：仅替换匹配行；其余行保留原始字节）。
///
/// 使用临时文件 + `rename` 原子替换目标路径，避免写入中途崩溃导致 transcript 损坏。
pub fn set_branch_summary_entry_is_boundary_true(
    path: &Path,
    entry_id: &str,
) -> Result<(), AppError> {
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
            Ok(TranscriptEntry::BranchSummary(mut ce)) => {
                if ce.id.as_deref() == Some(entry_id) {
                    ce.is_boundary = Some(true);
                    found = true;
                    Some(serde_json::to_string(&TranscriptEntry::BranchSummary(ce))?)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(json) = replaced {
            out.push(json);
        } else {
            out.push(line);
        }
    }

    if !found {
        return Err(AppError::Config(format!(
            "transcript: branch_summary entry id {entry_id:?} not found"
        )));
    }

    let mut content = out.join("\n");
    content.push('\n');
    write_file_atomic(path, content.as_bytes())?;
    Ok(())
}

/// 按 `branch_summary` 行的 `id` **删除所有匹配行**（重写整文件：省略匹配行；其余行保留原始字节）。
///
/// 与 [`set_branch_summary_entry_is_boundary_true`] 相同：临时文件 + `rename` 原子替换。
pub fn remove_branch_summary_entry_by_id(path: &Path, entry_id: &str) -> Result<(), AppError> {
    let f = std::fs::File::open(path).map_err(AppError::Io)?;
    let reader = BufReader::new(f);
    let lines: Vec<String> = reader
        .lines()
        .map(|r| r.map_err(AppError::Io))
        .collect::<Result<Vec<_>, _>>()?;
    if lines.is_empty() {
        return Err(AppError::Config("transcript 文件为空".to_string()));
    }

    let mut removed = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.push(lines[0].clone());

    for line in lines.into_iter().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(line);
            continue;
        }
        let omit = match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(TranscriptEntry::BranchSummary(ref ce)) => ce.id.as_deref() == Some(entry_id),
            _ => false,
        };
        if omit {
            removed += 1;
        } else {
            out.push(line);
        }
    }

    if removed == 0 {
        return Err(AppError::Config(format!(
            "transcript: branch_summary entry id {entry_id:?} not found for removal"
        )));
    }

    let mut content = out.join("\n");
    content.push('\n');
    write_file_atomic(path, content.as_bytes())?;
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
        TranscriptEntry::ThinkingTrace(e) => e.id.as_deref(),
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
        TranscriptEntry::ThinkingTrace(e) => e.parent_id.as_deref(),
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
