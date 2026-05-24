//! # `~/.tomcat/plans/*.plan.md` 计划文件持久化
//!
//! `PlanFile` = `PlanFileFrontmatter`（YAML） + 正文（自由 markdown）。
//! 写盘走「advisory file lock → write tmp → fsync tmp → rename tmp→final →
//! release lock」原子序列，避免并发改 plan 文件 / 中断半态。
//!
//! 与 [`crate::infra::config::lock`]（`tomcat.config.toml` 锁）独立：plan 文件
//! 的锁文件位于 `<plan_path>.lock`，永久落在 `~/.tomcat/plans/` 下。
//!
//! Schema：见 [`plan-runtime.md §5.4`](../../../../docs/architecture/plan-runtime.md#54-planfile-单文件落盘格式)
//! 和 [`create-plan.md §5.2`](../../../../docs/architecture/tools/create-plan.md#52-frontmatter-schema)。
//! 当前 `schema_version = 1`；未来升级在 `read_plan` 中按版本分支。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::safety::assert_plan_id_safe_for_disk;

/// Plan 文件当前 schema 版本；不匹配的文件被拒。
pub const PLAN_FILE_SCHEMA_VERSION: i32 = 1;

/// `<plan_path>.lock` 抢锁默认上限，对应 `[plan] lock_timeout_ms = 2000`。
pub const DEFAULT_LOCK_TIMEOUT_MS: u64 = 2000;

/// 抢锁 retry 间隔（指数退避起点）。
const LOCK_RETRY_BASE_MS: u64 = 5;
/// 抢锁 retry 间隔上限。
const LOCK_RETRY_MAX_MS: u64 = 80;

// ─── 错误类型 ───────────────────────────────────────────────────────────────

/// PlanFile 解析 / 落盘错误，**不** panic。
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// 抢 advisory file lock 超时；`waited_ms` 是实际等待毫秒数；
    /// `holder_pid` 为侧车 lock 文件内当前持有锁的进程 pid（用于调试）。
    #[error("plan 文件锁繁忙（等待 {waited_ms} ms 仍未释放；可能由 pid={holder_pid:?} 持有）")]
    LockBusy {
        waited_ms: u64,
        holder_pid: Option<i32>,
    },

    /// 目标 plan 文件不存在。
    #[error("plan 文件不存在: {path}")]
    NotFound { path: String },

    /// 找不到 frontmatter 开头/结尾的 `---` 分隔符。
    #[error("plan 文件缺少 frontmatter 分隔符 ---")]
    FrontmatterDelimMissing,

    /// `serde_yaml` 反序列化失败。
    #[error("frontmatter YAML 解析失败: {0}")]
    YamlParse(String),

    /// 必填字段缺失（serde 已用 default 兜底，runtime 显式校验）。
    #[error("frontmatter 缺少必填字段: {field}")]
    MissingField { field: String },

    /// `schema_version` 与当前 runtime 不兼容。
    #[error("frontmatter schema_version 不兼容: 实际 {actual}, 期望 {expected}")]
    SchemaVersion { actual: i32, expected: i32 },

    /// `plan_id` 未通过 `assert_plan_id_safe_for_disk`（路径穿越 / 非法字符）。
    #[error("非法 plan_id: {reason}")]
    InvalidPlanId { reason: String },

    /// 单一文件最多一个 in_progress；写盘前校验。
    #[error("plan 文件最多允许一个 in_progress todo，当前: {count}")]
    MultipleInProgress { count: usize },

    /// todo id 在单文件内必须唯一。
    #[error("plan 文件 todo id 重复: {id}")]
    DuplicateTodoId { id: String },

    /// 写盘 IO 错误（rename / open / write 等）。
    #[error("plan 文件 IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

// ─── frontmatter / 子结构 ──────────────────────────────────────────────────

/// PlanFile.frontmatter.mode 的枚举形态；与 [`super::PlanMode`] 没有 1:1 映射
/// （后者是 runtime in-memory 状态机，含 Chat）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanFileMode {
    Planning,
    Executing,
    Completed,
    Pending,
}

impl PlanFileMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanFileMode::Planning => "planning",
            PlanFileMode::Executing => "executing",
            PlanFileMode::Completed => "completed",
            PlanFileMode::Pending => "pending",
        }
    }
}

/// 单个 todo 的状态。**单一文件**最多一个 `in_progress`（写盘前校验）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

/// PlanFile 顶部 YAML frontmatter；**v1 schema**。
///
/// 未声明字段通过 `#[serde(flatten)]` 兜底到 `unknown`，写盘时保留，
/// 满足 §9.3B `plan_file_round_trip_preserves_unknown_keys`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanFileFrontmatter {
    pub plan_id: String,
    pub goal: String,
    pub mode: PlanFileMode,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub created_at: String,
    pub schema_version: i32,
    pub todos: Vec<TodoItem>,
    /// 未来扩展字段；read 时收集，write 时原样写回（保前向兼容）。
    #[serde(flatten)]
    pub unknown: serde_yaml::Mapping,
}

/// `PlanFile = frontmatter + 自由 body`。
#[derive(Debug, Clone)]
pub struct PlanFile {
    pub frontmatter: PlanFileFrontmatter,
    /// frontmatter `---` 之后的全部正文（包含 `## Goal` / `## Draft` 等 markdown 段）。
    pub body: String,
}

// ─── 路径 / lock 工具 ──────────────────────────────────────────────────────

/// 返回 `~/.tomcat/plans` 目录路径（不创建）。
pub fn plans_dir() -> Result<PathBuf, std::io::Error> {
    crate::infra::config::resolve_plans_dir().map_err(|e| std::io::Error::other(e.to_string()))
}

/// `~/.tomcat/plans/<plan_id>.plan.md`；落盘前已 `assert_plan_id_safe_for_disk`。
pub fn plan_path_for_id(plan_id: &str) -> Result<PathBuf, PlanError> {
    assert_plan_id_safe_for_disk(plan_id).map_err(|e| PlanError::InvalidPlanId {
        reason: e.to_string(),
    })?;
    let dir = plans_dir().map_err(PlanError::Io)?;
    Ok(dir.join(format!("{plan_id}.plan.md")))
}

fn lock_path_for(plan_path: &Path) -> PathBuf {
    let parent = plan_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let name = plan_path
        .file_name()
        .map(|n| format!("{}.lock", n.to_string_lossy()))
        .unwrap_or_else(|| ".plan.md.lock".to_string());
    parent.join(name)
}

// ─── 序列化 / 反序列化 ─────────────────────────────────────────────────────

/// 把 PlanFile 序列化为带 `---` 分隔符的文本（适合写入磁盘）。
pub fn serialize_plan_file(plan: &PlanFile) -> Result<String, PlanError> {
    validate_frontmatter_invariants(&plan.frontmatter)?;
    let yaml = serde_yaml::to_string(&plan.frontmatter)
        .map_err(|e| PlanError::YamlParse(e.to_string()))?;
    // serde_yaml::to_string 默认以 `---\n` 起头；统一裁掉前导分隔符再手动包裹。
    let yaml_trimmed = yaml.trim_start_matches("---\n").trim_end().to_string();
    let mut out = String::with_capacity(yaml_trimmed.len() + plan.body.len() + 16);
    out.push_str("---\n");
    out.push_str(&yaml_trimmed);
    out.push_str("\n---\n");
    if !plan.body.is_empty() {
        out.push_str(&plan.body);
        if !plan.body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// 从磁盘文本反序列化 PlanFile（分离 frontmatter / body）。
pub fn parse_plan_file(text: &str) -> Result<PlanFile, PlanError> {
    let stripped = text
        .strip_prefix("---\n")
        .ok_or(PlanError::FrontmatterDelimMissing)?;
    let end = stripped
        .find("\n---")
        .ok_or(PlanError::FrontmatterDelimMissing)?;
    let yaml = &stripped[..end];
    let mut body_start = end + "\n---".len();
    // body 可能跟 `\n` 或 EOF
    if let Some(rest) = stripped.get(body_start..) {
        if let Some(rest) = rest.strip_prefix('\n') {
            body_start = end + "\n---\n".len();
            let _ = rest;
        }
    }
    let body = stripped.get(body_start..).unwrap_or("").to_string();
    let frontmatter: PlanFileFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| PlanError::YamlParse(e.to_string()))?;
    enforce_required_fields(&frontmatter)?;
    if frontmatter.schema_version != PLAN_FILE_SCHEMA_VERSION {
        return Err(PlanError::SchemaVersion {
            actual: frontmatter.schema_version,
            expected: PLAN_FILE_SCHEMA_VERSION,
        });
    }
    Ok(PlanFile { frontmatter, body })
}

/// runtime 显式必填字段校验（避免 serde default 兜底导致空 plan_id / goal）。
fn enforce_required_fields(fm: &PlanFileFrontmatter) -> Result<(), PlanError> {
    if fm.plan_id.trim().is_empty() {
        return Err(PlanError::MissingField {
            field: "plan_id".into(),
        });
    }
    if fm.goal.trim().is_empty() {
        return Err(PlanError::MissingField {
            field: "goal".into(),
        });
    }
    if fm.created_at.trim().is_empty() {
        return Err(PlanError::MissingField {
            field: "created_at".into(),
        });
    }
    Ok(())
}

/// 写盘前对 frontmatter 做不变量校验（单 in_progress / id 唯一）。
pub fn validate_frontmatter_invariants(fm: &PlanFileFrontmatter) -> Result<(), PlanError> {
    enforce_required_fields(fm)?;
    let in_progress_count = fm
        .todos
        .iter()
        .filter(|t| matches!(t.status, TodoStatus::InProgress))
        .count();
    if in_progress_count > 1 {
        return Err(PlanError::MultipleInProgress {
            count: in_progress_count,
        });
    }
    let mut seen = std::collections::HashSet::with_capacity(fm.todos.len());
    for t in &fm.todos {
        if !seen.insert(&t.id) {
            return Err(PlanError::DuplicateTodoId { id: t.id.clone() });
        }
    }
    Ok(())
}

// ─── 读 / 写 / lock（高层 API） ────────────────────────────────────────────

/// 读取 plan 文件（不上锁，read-only path）。
pub fn read_plan(path: &Path) -> Result<PlanFile, PlanError> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PlanError::NotFound {
                path: path.display().to_string(),
            }
        } else {
            PlanError::Io(e)
        }
    })?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    parse_plan_file(&text)
}

/// 写盘：抢锁 → 原子 rename → 释放锁。
///
/// `lock_timeout_ms` 通常来自 `[plan] lock_timeout_ms`；典型值 2000。
pub fn write_plan(path: &Path, plan: &PlanFile, lock_timeout_ms: u64) -> Result<(), PlanError> {
    let serialized = serialize_plan_file(plan)?;
    let parent = path.parent().ok_or_else(|| {
        PlanError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "plan 路径无父目录",
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(PlanError::Io)?;
    let lock_path = lock_path_for(path);
    with_advisory_lock(&lock_path, lock_timeout_ms, || {
        let tmp = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            next_tmp_seq()
        ));
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(PlanError::Io)?;
        file.write_all(serialized.as_bytes())
            .map_err(PlanError::Io)?;
        file.sync_all().map_err(PlanError::Io)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            PlanError::Io(e)
        })?;
        Ok(())
    })
}

/// 抢 plan 文件 advisory lock；超 `lock_timeout_ms` 不放弃则返回 `LockBusy`。
///
/// 公开供 reviewer dispatch / `update_plan` 共享同一锁文件，保证 R5 串行性。
pub fn with_advisory_lock<R>(
    lock_path: &Path,
    lock_timeout_ms: u64,
    f: impl FnOnce() -> Result<R, PlanError>,
) -> Result<R, PlanError> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(PlanError::Io)?;
    }
    let mut lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(PlanError::Io)?;
    let start = Instant::now();
    let timeout = Duration::from_millis(lock_timeout_ms);
    let mut backoff = LOCK_RETRY_BASE_MS;
    loop {
        match lock_file.try_lock_exclusive() {
            Ok(()) => break,
            Err(_) => {
                if start.elapsed() >= timeout {
                    // N11：尝试读侧车 lock 文件已有内容（持锁进程在抢到锁后会写 pid）。
                    let holder_pid = read_lock_holder_pid(lock_path);
                    return Err(PlanError::LockBusy {
                        waited_ms: start.elapsed().as_millis() as u64,
                        holder_pid,
                    });
                }
                std::thread::sleep(Duration::from_millis(backoff));
                backoff = (backoff * 2).min(LOCK_RETRY_MAX_MS);
            }
        }
    }
    // 写入当前进程 pid 供其它进程读取（N11 调试线索）。
    use std::io::Write;
    let _ = lock_file.set_len(0);
    let _ = writeln!(lock_file, "{}", std::process::id());
    let _ = lock_file.flush();
    let res = f();
    let _ = FileExt::unlock(&lock_file);
    res
}

/// N11：从侧车 lock 文件读取首行 pid（容错——读不到/解析失败返回 `None`）。
fn read_lock_holder_pid(lock_path: &Path) -> Option<i32> {
    let s = std::fs::read_to_string(lock_path).ok()?;
    s.lines().next()?.trim().parse::<i32>().ok()
}

/// 单调递增的 tmp 文件后缀，避免同进程并发 write_plan 命名冲突。
fn next_tmp_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

