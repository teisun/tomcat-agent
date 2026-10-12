//! # `read` 工具的会话级状态表（PR-RF · T2-b/c）
//!
//! 实现 `docs/architecture/tools/read.md` §3.2 的 **dedup（重复读阻断）**
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
//! - **行范围 + 渲染形态进 key**：同一文件的「前 50 行」与「100..150 行」是不同
//!   请求；普通行号与 hashline 同样是不同模型输入，任一不同都不可命中 dedup。
//!
//! ## 并发模型
//!
//! 内部 `parking_lot::RwLock<HashMap>`：read（lookup）走读锁，write（put）
//! 走写锁。**单 session 内** 工具调用是顺序的，竞争可忽略；多 agent 共享
//! 同一 session 时也可正确互斥。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;

/// `read` 返回给模型的文本排版。
///
/// 它是 dedup 身份的一部分：同一段源文件，纯文本、普通行号和 hashline 的输出字节
/// 不同；模型为 edit 索要 hashline 时，不能拿先前的普通行号结果冒充。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRenderMode {
    Plain,
    LineNumbers,
    Hashline,
}

impl ReadRenderMode {
    /// 对齐 executor 的渲染优先级：`hashline` 覆盖 `line_numbers`。
    pub const fn resolve(line_numbers: bool, hashline: bool) -> Self {
        if hashline {
            Self::Hashline
        } else if line_numbers {
            Self::LineNumbers
        } else {
            Self::Plain
        }
    }
}

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
    /// 上次 read 返回给模型的文本排版。排版不同即使源文件和行区间相同也不可 dedup：
    /// 例如普通行号不能替代 edit 所需的 hashline 锚点。
    pub render_mode: ReadRenderMode,
    /// 上次**实际**读回来的行区间（1-based 闭区间）。非文本结果为 `None`。
    ///
    /// 判定覆盖必须用实际读到的区间而不是请求的窗口：不带 `limit` 的整读同样会被
    /// 默认行数上限截断，拿请求窗口当「读全了」会把没读到的部分也判成命中。
    pub covered_lines: Option<(u64, u64)>,
    /// 上次读是否读到了文件末尾（`!truncated`）。决定「无上界的请求」能否被覆盖。
    pub reached_eof: bool,
    /// 产生这条 stamp 的 tool_call_id。工具结果被落盘或替换成占位符后按它失效 ——
    /// 内容已经不在上下文里了，再回一句「和上次一样」就是在说谎。
    pub tool_call_id: Option<String>,
}

impl ReadStamp {
    /// 判断「这次 read 是否已经被上一次读覆盖」，覆盖则可短路成 `FILE_UNCHANGED` stub。
    ///
    /// 命中条件：
    /// - mtime + size 都未变（文件主体未被 touch / 改写）；
    /// - 请求区间**落在**上次实际读到的区间里。
    /// - 请求与上次的渲染形态相同。
    ///
    /// 用「区间包含」而不是「(offset, limit) 完全相等」：读过 L1684-1733 之后再要
    /// L1684-1703，内容明明就在上下文里，按精确匹配却会判成全新读、白读一遍。
    ///
    /// **不**比对 `content_hash`：哈希在每次 read **之后** 才能算出，dedup 想做的
    /// 就是「跳过这次 read」，所以前提里不能再要求读一遍文件。
    pub fn matches_request(
        &self,
        current_mtime_ms: i64,
        current_size: u64,
        offset: Option<u64>,
        limit: Option<u64>,
        render_mode: ReadRenderMode,
    ) -> bool {
        self.covers(current_mtime_ms, current_size, offset, limit, render_mode)
            .is_some()
    }

    /// 命中时返回上次实际读到的行区间，供提示语写清覆盖关系。
    pub fn covers(
        &self,
        current_mtime_ms: i64,
        current_size: u64,
        offset: Option<u64>,
        limit: Option<u64>,
        render_mode: ReadRenderMode,
    ) -> Option<(u64, u64)> {
        if self.mtime_ms != current_mtime_ms
            || self.size != current_size
            || self.render_mode != render_mode
        {
            return None;
        }
        let (covered_start, covered_end) = self.covered_lines?;
        let requested_start = offset.unwrap_or(1);
        if requested_start < covered_start {
            return None;
        }
        // 上次读到了文件末尾，那么从 covered_start 往后都在上下文里，请求要多少都够；
        // 没读到末尾时，请求必须有明确上界且不超过已读到的最后一行。
        let covered = if self.reached_eof {
            true
        } else {
            match limit {
                Some(limit) => {
                    requested_start.saturating_add(limit).saturating_sub(1) <= covered_end
                }
                None => false,
            }
        };
        covered.then_some((covered_start, covered_end))
    }
}

/// 会话级 `path → ReadStamp` 表（dedup + staleness 共用底座）。
///
/// 由 [`crate::core::agent_loop::AgentLoopConfig::read_file_state`] 持有；
/// 测试可直接 `ReadFileState::default()` + `Arc::new` 注入。
#[derive(Debug)]
pub struct ReadFileState {
    inner: RwLock<HashMap<PathBuf, ReadStamp>>,
    refresh_mutation_stamp: AtomicBool,
}

impl Default for ReadFileState {
    fn default() -> Self {
        Self::with_mutation_stamp_refresh(true)
    }
}

impl ReadFileState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mutation_stamp_refresh(enabled: bool) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            refresh_mutation_stamp: AtomicBool::new(enabled),
        }
    }

    /// 是否允许成功的局部变更把 ReadStamp 前移到刚写入的版本。
    pub fn mutation_stamp_refresh_enabled(&self) -> bool {
        self.refresh_mutation_stamp.load(Ordering::Relaxed)
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

    /// 让某次工具调用产生的所有 stamp 失效，返回失效条数。
    ///
    /// 调用时机是「这条工具结果已经从上下文里消失」（落盘成引用 / 被换成占位符）。
    /// 只删还挂在这次调用名下的 stamp：之后如果有新的 read 覆盖了同一个文件，
    /// 那份内容仍然看得见，不该被旧调用的清理波及。
    pub fn invalidate_tool_call(&self, tool_call_id: &str) -> usize {
        let mut guard = self.inner.write();
        let before = guard.len();
        guard.retain(|_, stamp| stamp.tool_call_id.as_deref() != Some(tool_call_id));
        before - guard.len()
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
