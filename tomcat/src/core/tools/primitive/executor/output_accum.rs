//! # bash 输出累积器（T2-P0-016 PR-E.3，bash.md §2.4.3 / §8）
//!
//! ## 一句话
//! 接收一段（已读完的）原始字节流，按字符数上限做 **EndTruncatingAccumulator 风格的
//! 头尾保留**；超限时把**完整原文**落盘到 `~/.tomcat/agents/<id>/tool-results/`，回传
//! 截断文本 + 落盘路径，供 `BashResult.persisted_output_path` 上 wire 给 LLM。
//!
//! ## 与 cc-fork-01 对齐
//! - 默认 `max_chars` = 30_000（[`crate::infra::DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS`]，
//!   与 cc-fork-01 `BASH_MAX_OUTPUT_DEFAULT` 同档）；
//! - 上限硬封顶 [`crate::infra::MAX_TOOLS_BASH_MAX_OUTPUT_CHARS`] = 150_000；
//! - 落盘文件名采用 `<prefix>-<unix_ms>-<rand6>.txt`，避免并发碰撞；
//!   写入 **完整原文** 而非截断后文本（落盘的目的就是「让模型可以再 read 取回」）。
//!
//! ## 字符 vs. 字节
//! 输入是 `&str`，已经经过 `String::from_utf8_lossy` 处理，**不会**在多字节字符中间
//! 切断（[`truncate_head_tail`] 按 `char_indices` 切）。

use std::path::{Path, PathBuf};

/// 单次 bash 调用的「输出累积处置」结果。
#[derive(Debug, Clone)]
pub struct AccumOutcome {
    /// 实际向 LLM 上 wire 的文本（≤ `max_chars` 字符；超限走头尾保留）。
    pub text: String,
    /// 是否截断；与 `BashResult.truncated` 同源。
    pub truncated: bool,
    /// 完整原文落盘路径；仅在 `truncated == true && persist_dir.is_some()` 时为 `Some`。
    /// 落盘失败（IO 错）退化为 `None` 并 `tracing::warn!`，**不**整把 fail bash 调用。
    pub persisted_path: Option<PathBuf>,
}

/// 把原文按 `max_chars` 字符上限做头尾截断；超限时把**完整原文**落盘到 `persist_dir`。
///
/// `persist_prefix` 用于文件名前缀（例如 `"bash-stdout"` / `"bash-stderr"`），便于在
/// `tool-results/` 目录里肉眼区分；同 `agent_id` 默认隐含在 `persist_dir` 路径里。
///
/// **不**对路径做 mkdir：调用方应在装配 `DefaultPrimitiveExecutor` 时（或最早一次
/// 启动时）通过 [`crate::infra::ensure_work_dir_structure`] 创建好 `tool-results/`。
pub fn accumulate_with_persist(
    raw: &str,
    max_chars: usize,
    persist_dir: Option<&Path>,
    persist_prefix: &str,
) -> AccumOutcome {
    let total_chars = raw.chars().count();
    if total_chars <= max_chars || max_chars < 16 {
        return AccumOutcome {
            text: raw.to_string(),
            truncated: false,
            persisted_path: None,
        };
    }

    let truncated_text = truncate_head_tail(raw, max_chars, total_chars);
    let persisted_path = persist_dir.and_then(|dir| match write_persist_file(dir, persist_prefix, raw) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), prefix = persist_prefix, "bash output persist failed; downgrading to in-memory truncation only");
            None
        }
    });

    AccumOutcome {
        text: truncated_text,
        truncated: true,
        persisted_path,
    }
}

/// 头尾保留：前 `max_chars / 2` 字符 + 中间分隔说明 + 后 `max_chars / 2` 字符。
///
/// 与 [`crate::core::tools::primitive::executor::bash`] 中 Phase-E.2 临时实现等价，
/// 但落到独立模块便于单测与后续扩展（例如「按行而不是按字符」切，或 ANSI escape 截断）。
fn truncate_head_tail(s: &str, max_chars: usize, total_chars: usize) -> String {
    let half = max_chars / 2;
    let total: Vec<(usize, char)> = s.char_indices().collect();
    let head_end = total.get(half).map(|(i, _)| *i).unwrap_or(s.len());
    let tail_start = total
        .get(total.len().saturating_sub(half))
        .map(|(i, _)| *i)
        .unwrap_or(0);
    let truncated_chars = total_chars.saturating_sub(max_chars);
    format!(
        "{}\n... [truncated {} chars; full output persisted to disk if available] ...\n{}",
        &s[..head_end],
        truncated_chars,
        &s[tail_start..]
    )
}

/// 把完整原文写到 `persist_dir/<prefix>-<unix_ms>-<rand6>.txt`，返回绝对路径。
///
/// 文件名加随机后缀避免同毫秒并发命中同名（极小概率，仍兜一下）。
fn write_persist_file(dir: &Path, prefix: &str, content: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand6 = simple_rand6();
    let filename = format!("{}-{}-{}.txt", prefix, now_ms, rand6);
    let path = dir.join(filename);
    let mut f = std::fs::File::create(&path)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    Ok(path)
}

/// 6 字符 base36 随机串。仅用于文件名去重，不要求密码学随机。
fn simple_rand6() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut x = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64
        ^ std::process::id() as u64;
    let mut s = String::with_capacity(6);
    const ALPH: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    for _ in 0..6 {
        s.push(ALPH[(x % 36) as usize] as char);
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }
    s
}
