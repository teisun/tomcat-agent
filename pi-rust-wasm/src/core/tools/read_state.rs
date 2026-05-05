//! # `read` 工具的会话级状态表（PR-RF · T2-b/c）
//!
//! 实现 `openspec/specs/architecture/tools/read.md` §3.2 的 **dedup（重复读阻断）**
//! 与 **staleness（陈旧检测）** 共用底座：一张 `path → ReadStamp` 哈希表，挂在
//! [`crate::core::agent_loop::AgentLoopConfig`] 上，**每个会话独立** —— `AgentLoop`
//! 析构时自动随之释放（**无需** 显式 `clear()`；详见 §3.2.3 「cleanup on session end」）。
//!
//! ## 双重职责（共用同一张表）
//!
//! ```text
//!                     ┌────────────────────────┐
//!  read 出口 ─────►   │   ReadFileState (本表)  │ ◄──── edit / write 入口（T3 起接入）
//!  put_stamp(path,…)  └────────────────────────┘            check_stamp(path)
//!         │                       │
//!         ▼                       ▼
//!   dedup：同 key 命中且 mtime+size       staleness：mtime/size 与上次 read 不一致
//!   未变 → FILE_UNCHANGED stub             → 拒绝并要求重 read（防误改外部修改过的文件）
//! ```
//!
//! ## 选型说明（与决策表 §0.A.3 R5 对齐）
//!
//! - **mtime + size 作为「快速指纹」**：99% 场景文件改动 mtime 必变；偶发
//!   `touch -r` / `git checkout` 保留时间戳的边角 case 由 T3 hashline 兜底
//!   （详见 `read.md` §4.4）。
//! - **content_hash 仍计算并存储**：用于诊断 + 给 hashline_edit 复用，
//!   但 dedup 路径**不**强制比对（避免每次 read 之前再读一遍文件计算 hash）。
//! - **`(offset, limit)` 进 key**：同一文件的「前 50 行」与「100..150 行」
//!   是不同窗口；window 不同视为不同 read，互不命中 dedup。
//!
//! ## 并发模型
//!
//! 内部 `parking_lot::RwLock<HashMap>`：read（lookup）走读锁，write（put）
//! 走写锁。**单 session 内** 工具调用是顺序的，竞争可忽略；多 agent 共享
//! 同一 session 时也可正确互斥。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;

/// 一条「上次成功 read 的指纹」。
///
/// 字段顺序与 [`ReadFileState::put_stamp`] 入参顺序一致，便于 grep 对照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadStamp {
    /// 文件 mtime，毫秒；从 `std::fs::Metadata::modified()` 推导。
    pub mtime_ms: i64,
    /// 文件 metadata 大小（字节）；与 mtime 一同用于「文件未变」廉价判定。
    pub size: u64,
    /// 上次 read 内容的 64 位指纹（`std::collections::hash_map::DefaultHasher`）；
    /// 用于诊断 + T3 hashline 互补 staleness。dedup 路径**不**强制比对。
    pub content_hash: u64,
    /// 上次 read 的 `offset`（1-based 行号），`None` 等价于「整文件 / 无窗口」。
    pub offset: Option<u64>,
    /// 上次 read 的 `limit`（行数），`None` 等价于「整文件 / 默认上限」。
    pub limit: Option<u64>,
    /// 上次 read 是否为分窗读（`true` ⇔ 至少有一个 `offset` / `limit` 被显式传入）。
    /// 影响 §3.2.3 的「partial view 不与 full read 互相命中」语义。
    pub is_partial_view: bool,
}

impl ReadStamp {
    /// 判断「同一窗口的下一次 read 是否可短路成 `FILE_UNCHANGED` stub」。
    ///
    /// 命中条件（与 `read.md` §3.2.2 一致）：
    /// - mtime + size 都未变（文件主体未被 touch / 改写）；
    /// - 请求的 `(offset, limit)` 与上次完全一致（窗口对齐）；
    /// - `is_partial_view` 也一致（避免「整文件 vs 分窗」误命中）。
    ///
    /// **不**比对 `content_hash`：哈希在每次 read **之后** 才能算出，dedup 想做的
    /// 就是「跳过这次 read」，所以前提里不能再要求读一遍文件。
    pub fn matches_request(
        &self,
        current_mtime_ms: i64,
        current_size: u64,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> bool {
        let request_partial = offset.is_some() || limit.is_some();
        self.mtime_ms == current_mtime_ms
            && self.size == current_size
            && self.offset == offset
            && self.limit == limit
            && self.is_partial_view == request_partial
    }
}

/// 会话级 `path → ReadStamp` 表（dedup + staleness 共用底座）。
///
/// 由 [`crate::core::agent_loop::AgentLoopConfig::read_file_state`] 持有；
/// 测试可直接 `ReadFileState::default()` + `Arc::new` 注入。
#[derive(Debug, Default)]
pub struct ReadFileState {
    inner: RwLock<HashMap<PathBuf, ReadStamp>>,
}

impl ReadFileState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查找 `path` 上次 read 的 stamp（`None` ⇔ 未 read 过）。
    pub fn get(&self, path: &Path) -> Option<ReadStamp> {
        self.inner.read().get(path).cloned()
    }

    /// 落 stamp。同 path 重复 put 直接覆盖（最新一次 read 的窗口为准）。
    pub fn put(&self, path: PathBuf, stamp: ReadStamp) {
        self.inner.write().insert(path, stamp);
    }

    /// 强制让某个 path 的 stamp 失效（如外部检测到文件被改）。
    /// 主要给 edit/write 端调用（T3+）；本 PR 暂未使用，留接口避免后续改 trait。
    #[allow(dead_code)]
    pub fn invalidate(&self, path: &Path) {
        self.inner.write().remove(path);
    }

    /// 清空整张表；语义上对应「会话结束」的一次性回收。
    ///
    /// 注意：**正常路径不需要显式调用** —— `AgentLoop` 析构时 `Arc<ReadFileState>`
    /// 引用计数归零、整个表自动释放（`Drop` 链：`AgentLoop` → `AgentLoopConfig`
    /// → `Arc<ReadFileState>` → `RwLock<HashMap<...>>`）。该方法主要供
    /// 「同 process 内 session 复用同一 `Arc`」的边角场景使用，并方便测试。
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// 当前缓存条目数（仅供测试 / 诊断）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// 缓存为空判定（与 `len() == 0` 等价；clippy `len_without_is_empty` 要求并存）。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

/// PR-RF（T2-c）`FILE_UNCHANGED` 软 stub 的统一文案。
///
/// 与 cc-fork `FILE_UNCHANGED_STUB` 一字对齐英文版本（`read.md` §3.2.3）。
/// 模型已在前轮拿到完整内容，本轮**应**直接复用，不用再翻 token。
pub const FILE_UNCHANGED_STUB: &str =
    "File unchanged since last read. Refer to the earlier read result.";

/// 计算字符串内容的 64 位 hash（`std::collections::hash_map::DefaultHasher`）。
///
/// 选用 std 的 `DefaultHasher` 而非 xxhash / blake3：
/// - dedup 路径**不**强制比对，hash 仅用于诊断 / hashline 互补；
/// - 不引新 crate，编译时间零增长；
/// - 64 位空间在「单 session 同文件多次窗口」量级下碰撞率可忽略。
pub fn hash_content(content: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// 把 `std::fs::Metadata::modified()` 转成毫秒级 unix 时间戳。
///
/// 失败 / 平台不支持 mtime 时回退到 `0`：
/// 此时 dedup 仍能跑（`0 == 0` 命中），只是失去「外部修改使 stamp 失效」的能力。
/// 这条退化路径与 cc-fork 行为一致（`mtime ?? 0`）。
pub fn metadata_mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(mtime: i64, size: u64, off: Option<u64>, lim: Option<u64>) -> ReadStamp {
        let partial = off.is_some() || lim.is_some();
        ReadStamp {
            mtime_ms: mtime,
            size,
            content_hash: 0,
            offset: off,
            limit: lim,
            is_partial_view: partial,
        }
    }

    #[test]
    fn matches_request_dedup_hits_when_window_and_metadata_align() {
        let s = stamp(100, 1024, Some(1), Some(50));
        assert!(s.matches_request(100, 1024, Some(1), Some(50)));
    }

    #[test]
    fn matches_request_misses_when_mtime_changes() {
        let s = stamp(100, 1024, Some(1), Some(50));
        assert!(!s.matches_request(101, 1024, Some(1), Some(50)));
    }

    #[test]
    fn matches_request_misses_when_size_changes() {
        let s = stamp(100, 1024, None, None);
        assert!(!s.matches_request(100, 1025, None, None));
    }

    #[test]
    fn matches_request_misses_when_window_differs() {
        let s = stamp(100, 1024, Some(1), Some(50));
        assert!(!s.matches_request(100, 1024, Some(1), Some(60)));
        assert!(!s.matches_request(100, 1024, Some(2), Some(50)));
    }

    #[test]
    fn matches_request_separates_full_vs_partial() {
        // 上次是「整文件」读，本次是「分窗」读 → 不命中（视为不同 read 意图）。
        let s = stamp(100, 1024, None, None);
        assert!(!s.matches_request(100, 1024, Some(1), Some(50)));
    }

    #[test]
    fn put_and_get_roundtrip() {
        let state = ReadFileState::new();
        let p = std::path::PathBuf::from("/tmp/x");
        assert_eq!(state.len(), 0);
        state.put(p.clone(), stamp(1, 2, None, None));
        assert_eq!(state.len(), 1);
        assert_eq!(state.get(&p), Some(stamp(1, 2, None, None)));
    }

    #[test]
    fn invalidate_removes_entry() {
        let state = ReadFileState::new();
        let p = std::path::PathBuf::from("/tmp/x");
        state.put(p.clone(), stamp(1, 2, None, None));
        state.invalidate(&p);
        assert!(state.get(&p).is_none());
    }

    #[test]
    fn clear_drops_all_entries_session_end_cleanup() {
        let state = ReadFileState::new();
        state.put(std::path::PathBuf::from("/a"), stamp(1, 1, None, None));
        state.put(
            std::path::PathBuf::from("/b"),
            stamp(2, 2, Some(1), Some(10)),
        );
        assert_eq!(state.len(), 2);
        state.clear();
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn hash_content_is_deterministic_and_distinct() {
        assert_eq!(hash_content(b"hello"), hash_content(b"hello"));
        assert_ne!(hash_content(b"hello"), hash_content(b"world"));
    }
}
