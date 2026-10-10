//! # AttachmentBlobStore — 内容寻址的附件字节仓库
//!
//! 本模块是 Rust 侧关于图片附件的**全部**职责：存字节、按哈希取字节、校验大小与格式、
//! 管理未发送字节的租约。它**不做任何图像解码或渲染** —— 缩略图降采样与 SVG 栅格化都在
//! webview 里由 Chromium 完成，因此本模块零图形库依赖。
//!
//! ## 为什么是内容寻址
//!
//! 图片字节的身份是「不可变资源」而不是「应用状态」：同一份字节无论被粘贴几次、
//! 无论属于草稿还是已发送历史，都应该只有一份。用 sha256 当文件名让去重、完整性自校验、
//! 以及「发送时零拷贝提升」三件事同时成立 —— 发送不需要搬运字节，只需要停止把它当作待清理的草稿字节。
//!
//! ## 存储布局
//!
//! ```text
//! sessions_dir/attachments/blobs/<sha256>         ← 全部图片字节，内容寻址，不可变
//! sessions_dir/attachments/thumbs/<sha256>        ← 由 <sha256> 派生的 192px 缩略图，纯派生
//! sessions_dir/attachments/pending/<sid>/<sha256> ← 未发送字节的租约标记（空文件，mtime 即租约时间）
//! ```
//!
//! 只有一个装字节的目录，这是刻意的：宿主拿到一个 sha 时必须能**唯一**算出它的路径，
//! 因为 `<img src>` 没法「试一个不行再试另一个」。原图、SVG 转出的 PNG、从 transcript
//! 物化出来的历史图都进 `blobs/`，各按自己的内容哈希寻址。
//!
//! `thumbs/` 是唯一按「来源哈希」而非「自身哈希」寻址的目录，因为它表达的是一个**映射**
//! （某份字节的缩略图长什么样），而不是一份独立内容。这样宿主只要有 blobSha 就能算出缩略图 URI。
//!
//! 保留策略不靠目录表达，靠**租约与 transcript 引用**（见下节）—— 这也是为什么不需要
//! 第二个目录来区分「权威字节」与「可重建字节」。
//!
//! ## 租约与 GC
//!
//! `pending/<sid>/<sha>` 是一个空标记文件，表示「这份字节属于某个未发送的草稿」。
//!
//! ```text
//! ingest   → put(bytes) 落 blob + mark_pending(sid, sha) 建租约
//! 打字     → 不碰这里（草稿文本在扩展层，见 tomcat-vscode-ext/docs/architecture/image-attachments.md）
//! hydrate  → touch_pending 续期，证明这份草稿还活着
//! send     → promote(sid, sha) 只删租约标记，blob 原地不动（零拷贝）
//! GC       → 租约超过 TTL：先删租约；随后标记所有 transcript 引用，再在 1 小时宽限期后清扫孤儿 blob
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::core::llm::{FILE_MAX_BYTES, IMAGE_MAX_BYTES};
use crate::infra::error::AppError;
use crate::infra::platform::write_file_atomic;

// ── 常量 ──────────────────────────────────────────────────────────────

/// 未发送字节的默认租约时长。超期且未被 transcript 引用的 blob 会被 GC 回收。
///
/// 取 7 天：足够覆盖「周五写了草稿、周一回来接着写」，又不至于让被遗忘的草稿字节长期占盘。
pub const PENDING_BLOB_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Orphans stay for an hour before sweeping. A process that crashes between publishing a
/// blob and appending its transcript reference must not lose the just-sent image.
pub const ORPHAN_BLOB_GRACE: Duration = Duration::from_secs(60 * 60);

/// Thumbnails are derived data and can be recreated, so they alone have an LRU budget.
pub const REBUILDABLE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// sha256 十六进制字符串的长度。
const SHA256_HEX_LEN: usize = 64;

/// 允许的图片 MIME 白名单。SVG 在这里被接受，但它只会被存起来与转发，Rust 不解析它。
const ALLOWED_IMAGE_MIME: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
];

// ── 类型 ──────────────────────────────────────────────────────────────

/// 附件清理步骤的结果，用于日志与测试断言。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobGcReport {
    /// 删除的租约标记数量。
    pub leases_released: usize,
    /// `sweep_orphan_blobs` 连同缩略图删除的 blob 数量。
    pub blobs_deleted: usize,
    /// `sweep_orphan_blobs` 因仍在 live set 而保留的 blob 数量。
    pub blobs_retained: usize,
}

/// 内容寻址的附件字节仓库。
#[derive(Debug, Clone)]
pub struct AttachmentBlobStore {
    root: PathBuf,
}

impl AttachmentBlobStore {
    /// 从 session 根目录创建。
    pub fn new(sessions_dir: &Path) -> Self {
        Self {
            root: sessions_dir.join("attachments"),
        }
    }

    /// 附件根目录。宿主需要它来配置 webview 的 `localResourceRoots`，
    /// 因为这个路径由 agent id 与配置决定，扩展自己算不出来。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 全部附件字节的唯一存放处，按内容哈希寻址。
    ///
    /// 「新粘贴还没发出去的图」与「从老 transcript 里物化出来的历史图」都放这里，
    /// 不分目录。曾经想过分成 `blobs/` 与 `cache/` 两处，但那样同一个哈希就有两个
    /// 可能路径，而宿主拿到哈希时**无从判断该拼哪一个** —— `<img src>` 不能「试一个
    /// 再试另一个」。区别其实只在保留策略上，而保留策略看的是租约与 transcript 引用，
    /// 不需要靠目录来表达。
    pub fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// 缩略图目录。按**来源** sha 寻址，宿主有 blobSha 就能算出这里的路径。
    pub fn thumbs_dir(&self) -> PathBuf {
        self.root.join("thumbs")
    }

    fn pending_dir(&self) -> PathBuf {
        self.root.join("pending")
    }

    fn blob_path(&self, sha: &str) -> PathBuf {
        self.blobs_dir().join(sha)
    }

    fn thumb_path(&self, source_sha: &str) -> PathBuf {
        self.thumbs_dir().join(source_sha)
    }

    fn session_pending_dir(&self, session_id: &str) -> PathBuf {
        self.pending_dir().join(session_id)
    }

    // ── 写入与读取 ────────────────────────────────────────────────────

    /// 落盘一份字节，返回它的 sha256。
    ///
    /// 内容寻址天然去重：同一份字节第二次 `put` 不会重写内容，但会刷新 mtime。
    /// mtime 是 orphan grace window 的活动信号，不是内容版本。
    pub fn put(&self, bytes: &[u8]) -> Result<String, AppError> {
        let sha = sha256_hex(bytes);
        let path = self.blob_path(&sha);
        if path.exists() {
            touch_blob(&path)?;
            return Ok(sha);
        }
        write_file_atomic(&path, bytes)?;
        Ok(sha)
    }

    /// 按 sha 读取字节，并**校验内容与文件名一致**。
    ///
    /// 内容寻址让完整性可以自校验：如果磁盘上的字节被外部改动过，算出的哈希就不再等于文件名。
    /// 这种情况把文件隔离掉并当作不存在，而不是把坏字节喂给 provider。
    ///
    pub fn get(&self, sha: &str) -> Result<Option<Vec<u8>>, AppError> {
        validate_sha(sha)?;
        let path = self.blob_path(sha);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(AppError::Io)?;
        if sha256_hex(&bytes) != sha {
            isolate_corrupt(&path, "sha_mismatch");
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    /// 字节是否可取到。不做内容校验。
    pub fn exists(&self, sha: &str) -> bool {
        validate_sha(sha).is_ok() && self.blob_path(sha).exists()
    }

    // ── 缩略图 ───────────────────────────────────────────────────────

    /// 记录某份字节的缩略图。
    ///
    /// 缩略图由 webview 里的 Chromium 生成（`createImageBitmap` 在解码期就降采样），
    /// Rust 只负责收下来存好 —— 这就是本模块零图形库依赖的原因。
    pub fn put_thumbnail(&self, source_sha: &str, thumb: &[u8]) -> Result<(), AppError> {
        validate_sha(source_sha)?;
        write_file_atomic(&self.thumb_path(source_sha), thumb)
    }

    /// 某份字节是否已有缩略图。
    pub fn has_thumbnail(&self, source_sha: &str) -> bool {
        validate_sha(source_sha).is_ok() && self.thumb_path(source_sha).exists()
    }

    pub fn get_thumbnail(&self, source_sha: &str) -> Result<Option<Vec<u8>>, AppError> {
        validate_sha(source_sha)?;
        let path = self.thumb_path(source_sha);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read(&path).map_err(AppError::Io)?))
    }

    // ── 租约 ─────────────────────────────────────────────────────────

    /// 为某个 session 的未发送字节建立租约。
    pub fn mark_pending(&self, session_id: &str, sha: &str) -> Result<(), AppError> {
        validate_sha(sha)?;
        let dir = self.session_pending_dir(validate_session_id(session_id)?);
        std::fs::create_dir_all(&dir).map_err(AppError::Io)?;
        std::fs::write(dir.join(sha), []).map_err(AppError::Io)?;
        Ok(())
    }

    /// 续期租约（扩展层 hydrate 草稿时调用，证明这份草稿还活着）。
    ///
    /// 租约不存在时重新建立 —— 草稿仍在但租约已被 GC 回收是可能的，
    /// 只要 blob 还在就应该续上，而不是让下一轮 GC 把它删掉。
    pub fn touch_pending(&self, session_id: &str, sha: &str) -> Result<(), AppError> {
        if !self.exists(sha) {
            return Ok(());
        }
        self.mark_pending(session_id, sha)
    }

    fn acquire_pending_marker(&self, session_id: &str, sha: &str) -> Result<bool, AppError> {
        let dir = self.session_pending_dir(validate_session_id(session_id)?);
        std::fs::create_dir_all(&dir).map_err(AppError::Io)?;
        let path = dir.join(validate_sha_value(sha)?);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if path.is_file() {
                    Ok(false)
                } else {
                    Err(AppError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("attachment lease marker is not a file: {}", path.display()),
                    )))
                }
            }
            Err(error) => Err(AppError::Io(error)),
        }
    }

    /// 批量为目标会话保留已有 blob 的租约。先验证全部 SHA 与 blob，再写 marker；
    /// 部分写失败时只回滚本次原子新建的 marker，已有 marker 保持不变。
    pub fn retain_pending_batch(
        &self,
        session_id: &str,
        shas: impl IntoIterator<Item = String>,
    ) -> Result<Vec<String>, AppError> {
        self.retain_pending_batch_inner(session_id, shas, |_, _| Ok(()))
    }

    fn retain_pending_batch_inner<F>(
        &self,
        session_id: &str,
        shas: impl IntoIterator<Item = String>,
        mut before_acquire: F,
    ) -> Result<Vec<String>, AppError>
    where
        F: FnMut(usize, &str) -> Result<(), AppError>,
    {
        let session_id = validate_session_id(session_id)?;
        let shas = shas.into_iter().collect::<std::collections::BTreeSet<_>>();
        for sha in &shas {
            validate_sha(sha)?;
            if self.get(sha)?.is_none() {
                return Err(AppError::Config(crate::infra::i18n::tr(
                    "attachment.blobMissing",
                    &[("sha", sha)],
                )));
            }
        }

        let mut created: Vec<String> = Vec::new();
        for (index, sha) in shas.iter().enumerate() {
            let acquired = before_acquire(index, sha)
                .and_then(|()| self.acquire_pending_marker(session_id, sha));
            match acquired {
                Ok(true) => created.push(sha.clone()),
                Ok(false) => {}
                Err(error) => {
                    for created_sha in &created {
                        let _ = self.promote(session_id, created_sha);
                    }
                    return Err(error);
                }
            }
        }
        Ok(shas.into_iter().collect())
    }

    /// 发送成功：释放租约，字节留在 `blobs/` 里由 transcript 引用。
    ///
    /// 这就是「零拷贝提升」的全部内容 —— 不读、不写、不复制任何字节。
    pub fn promote(&self, session_id: &str, sha: &str) -> Result<(), AppError> {
        validate_sha(sha)?;
        let blob_path = self.blob_path(sha);
        if blob_path.exists() {
            touch_blob(&blob_path)?;
        }
        let path = self
            .session_pending_dir(validate_session_id(session_id)?)
            .join(sha);
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(AppError::Io(error));
            }
        }
        Ok(())
    }

    /// 列出某个 session 当前持有的全部租约。
    pub fn list_pending(&self, session_id: &str) -> Result<Vec<String>, AppError> {
        let dir = self.session_pending_dir(validate_session_id(session_id)?);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            if let Some(name) = entry.file_name().to_str() {
                if validate_sha(name).is_ok() {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Collect every active draft lease in one directory walk so a GC pass can make
    /// membership decisions in O(blobs + leases), rather than probing all leases per blob.
    pub fn collect_pending_blob_shas(&self) -> Result<HashSet<String>, AppError> {
        let pending_root = self.pending_dir();
        if !pending_root.exists() {
            return Ok(HashSet::new());
        }
        let mut shas = HashSet::new();
        for session_entry in std::fs::read_dir(&pending_root).map_err(AppError::Io)? {
            let session_entry = session_entry.map_err(AppError::Io)?;
            if !session_entry.file_type().map_err(AppError::Io)?.is_dir() {
                continue;
            }
            for lease in std::fs::read_dir(session_entry.path()).map_err(AppError::Io)? {
                let lease = lease.map_err(AppError::Io)?;
                if !lease.file_type().map_err(AppError::Io)?.is_file() {
                    continue;
                }
                if let Some(sha) = lease
                    .file_name()
                    .to_str()
                    .filter(|sha| validate_sha(sha).is_ok())
                {
                    shas.insert(sha.to_string());
                }
            }
        }
        Ok(shas)
    }

    /// 会话被删除：仅清理它的全部租约。孤儿 blob 统一交给带宽限窗的 sweep，
    /// 避免各个调用点对「能否删除」做不同判定。
    ///
    /// 与 `SessionManager::delete_session` 联动。
    pub fn clear_session(&self, session_id: &str) -> Result<BlobGcReport, AppError> {
        let session_id = validate_session_id(session_id)?;
        let mut report = BlobGcReport::default();
        for sha in self.list_pending(session_id)? {
            self.promote(session_id, &sha)?;
            report.leases_released += 1;
        }
        if let Err(error) = std::fs::remove_dir(self.session_pending_dir(session_id)) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(AppError::Io(error));
            }
        }
        Ok(report)
    }

    /// 回收超期租约。
    ///
    /// Final blob deletion happens in [`Self::sweep_orphan_blobs`] after the grace window.
    pub fn gc_pending(&self, ttl: Duration) -> Result<BlobGcReport, AppError> {
        let pending_root = self.pending_dir();
        if !pending_root.exists() {
            return Ok(BlobGcReport::default());
        }
        let now = SystemTime::now();
        let mut report = BlobGcReport::default();

        for session_entry in std::fs::read_dir(&pending_root).map_err(AppError::Io)? {
            let session_entry = session_entry.map_err(AppError::Io)?;
            if !session_entry.file_type().map_err(AppError::Io)?.is_dir() {
                continue;
            }
            let session_dir = session_entry.path();
            for lease in std::fs::read_dir(&session_dir).map_err(AppError::Io)? {
                let lease = lease.map_err(AppError::Io)?;
                let lease_path = lease.path();
                let Some(sha) = lease_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if validate_sha(sha).is_err() {
                    continue;
                }
                if !is_lease_expired(&lease_path, now, ttl) {
                    continue;
                }
                let _ = std::fs::remove_file(&lease_path);
                report.leases_released += 1;
            }
            let _ = std::fs::remove_dir(&session_dir);
        }
        Ok(report)
    }

    /// Apply the budget only to thumbnails. A sent-image blob is its transcript's sole
    /// copy, so it is never a cache entry and must not be evicted for capacity.
    pub fn evict_rebuildable_over_budget(&self, max_bytes: u64) -> Result<u64, AppError> {
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        let dir = self.thumbs_dir();
        if !dir.exists() {
            return Ok(0);
        }
        for entry in std::fs::read_dir(&dir).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            let metadata = entry.metadata().map_err(AppError::Io)?;
            if !metadata.is_file() {
                continue;
            }
            let Some(sha) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_sha(&sha).is_err() {
                continue;
            }
            let used = metadata
                .accessed()
                .or_else(|_| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            total = total.saturating_add(metadata.len());
            entries.push((used, metadata.len(), entry.path()));
        }
        if total <= max_bytes {
            return Ok(0);
        }
        // 最久没被用到的排在前面，先删它们。
        entries.sort_by_key(|(used, _, _)| *used);
        let mut freed = 0u64;
        for (_, len, path) in entries {
            if total.saturating_sub(freed) <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                freed = freed.saturating_add(len);
            }
        }
        if freed > 0 {
            tracing::debug!("attachments: evicted {freed} bytes of rebuildable attachment data");
        }
        Ok(freed)
    }

    /// Delete blobs with neither a transcript reference nor a live draft lease once they
    /// are older than `grace`. Each orphan thumbnail goes with its source blob.
    pub fn sweep_orphan_blobs(
        &self,
        live_shas: &HashSet<String>,
        grace: Duration,
    ) -> Result<BlobGcReport, AppError> {
        let mut report = BlobGcReport::default();
        let blobs = self.blobs_dir();
        if !blobs.exists() {
            return Ok(report);
        }
        let now = SystemTime::now();
        for entry in std::fs::read_dir(blobs).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            let metadata = entry.metadata().map_err(AppError::Io)?;
            if !metadata.is_file() {
                continue;
            }
            let Some(sha) = entry
                .file_name()
                .to_str()
                .filter(|sha| validate_sha(sha).is_ok())
                .map(str::to_string)
            else {
                continue;
            };
            if live_shas.contains(&sha) {
                report.blobs_retained += 1;
                continue;
            }
            let age = now
                .duration_since(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH))
                .unwrap_or_default();
            if age < grace {
                continue;
            }
            if std::fs::remove_file(entry.path()).is_ok() {
                let _ = std::fs::remove_file(self.thumb_path(&sha));
                report.blobs_deleted += 1;
            }
        }
        Ok(report)
    }
}

fn touch_blob(path: &Path) -> Result<(), AppError> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(AppError::Io)?
        .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()))
        .map_err(AppError::Io)
}

// ── 不变量 ────────────────────────────────────────────────────────────

/// sha 必须是 64 位小写十六进制。
///
/// 这既是格式校验也是**路径安全断言** —— 合法的 sha 里不可能出现 `/` 或 `..`，
/// 所以拼路径这件事不需要信任调用方。
fn validate_sha_value(sha: &str) -> Result<&str, AppError> {
    validate_sha(sha)?;
    Ok(sha)
}

fn validate_sha(sha: &str) -> Result<(), AppError> {
    if sha.len() == SHA256_HEX_LEN
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "attachments: invalid blob sha256: {sha:?}"
    )))
}

/// Validate an externally supplied blob identifier before a caller derives any behavior
/// from it. This is intentionally byte-free: callers that merely need to distinguish a
/// malformed ID from a missing blob must not load the entire attachment.
pub(crate) fn validate_blob_sha(sha: &str) -> Result<(), AppError> {
    validate_sha(sha)
}

/// session id 只允许出现能安全用作单层目录名的字符。
///
/// 本模块不把路径安全性寄托在调用方自觉：即使今天所有调用方都传后端可信的
/// `slot.session_id`，作为 `pub` 模块也必须自己守住这条不变量。
pub(crate) fn validate_session_id(session_id: &str) -> Result<&str, AppError> {
    let ok = !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(session_id)
    } else {
        Err(AppError::Config(format!(
            "attachments: invalid session id: {session_id:?}"
        )))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lease_expired(path: &Path, now: SystemTime, ttl: Duration) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    now.duration_since(modified)
        .map(|age| age >= ttl)
        .unwrap_or(false)
}

/// 把可疑文件改名保留现场，不静默删除。
fn isolate_corrupt(path: &Path, reason: &str) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let target = path.with_file_name(format!("{name}.corrupt-{reason}"));
    match std::fs::rename(path, &target) {
        Ok(()) => tracing::warn!(
            "attachments: isolated corrupt blob {} -> {} (reason={reason})",
            path.display(),
            target.display()
        ),
        Err(error) => tracing::warn!(
            "attachments: failed to isolate corrupt blob {}: {error}",
            path.display()
        ),
    }
}

// ── 校验 ──────────────────────────────────────────────────────────────

/// 图片字节的校验结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedImage {
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// 校验一份图片的 MIME、大小与魔术字节。
///
/// **不解析图片内容。** Rust 只需要确认「这看起来是声明的那种格式、且不超限」，
/// 真正的解码由 provider（发出去之后）与 Chromium（显示时）负责，两者都远比我们成熟。
/// 因此这里不需要任何图形库。
pub fn validate_image_bytes(bytes: &[u8], mime_type: &str) -> Result<(), String> {
    if !ALLOWED_IMAGE_MIME.contains(&mime_type) {
        return Err(crate::infra::i18n::tr(
            "attachment.imageMime",
            &[("mime", mime_type)],
        ));
    }
    if bytes.is_empty() {
        return Err(crate::infra::i18n::tr("attachment.imageEmpty", &[]));
    }
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(crate::infra::i18n::tr(
            "attachment.imageLarge",
            &[
                ("bytes", &bytes.len().to_string()),
                ("limit", &IMAGE_MAX_BYTES.to_string()),
            ],
        ));
    }
    if !matches_image_magic(bytes, mime_type) {
        return Err(crate::infra::i18n::tr(
            "attachment.imageMagic",
            &[("mime", mime_type)],
        ));
    }
    Ok(())
}

/// 校验一份文件附件（目前只支持 PDF）。
pub fn validate_file_bytes(bytes: &[u8], mime_type: &str) -> Result<(), String> {
    if !mime_type.eq_ignore_ascii_case("application/pdf") {
        return Err(crate::infra::i18n::tr(
            "attachment.fileMime",
            &[("mime", mime_type)],
        ));
    }
    if bytes.is_empty() {
        return Err(crate::infra::i18n::tr("attachment.fileEmpty", &[]));
    }
    if bytes.len() > FILE_MAX_BYTES {
        return Err(crate::infra::i18n::tr(
            "attachment.fileLarge",
            &[
                ("bytes", &bytes.len().to_string()),
                ("limit", &FILE_MAX_BYTES.to_string()),
            ],
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(crate::infra::i18n::tr("attachment.pdfMagic", &[]));
    }
    Ok(())
}

/// 零依赖的格式头部检查。
///
/// 只看每种格式开头那几个约定字节，够用来挡住「声明是 PNG 实际是别的东西」，
/// 并且不需要引入任何编解码库。
fn matches_image_magic(bytes: &[u8], mime_type: &str) -> bool {
    match mime_type {
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        // SVG 是文本，没有二进制魔术字节；确认它是 UTF-8 且含有 svg 根元素即可。
        // 内容安全不在这里做 —— SVG 只会被 Chromium 以 <img> 加载，
        // 那条路在规范层面处于 secure static mode，强制不执行脚本、不加载外部资源。
        "image/svg+xml" => std::str::from_utf8(bytes)
            .map(|text| text.to_ascii_lowercase().contains("<svg"))
            .unwrap_or(false),
        _ => false,
    }
}

/// 生成安全默认文件名：无扩展名时根据 MIME 补全缺省名，太长时截断。
pub fn safe_filename(filename: Option<&str>, mime_type: &str) -> String {
    const MAX_NAME_LEN: usize = 120;
    const HARD_TRUNC: usize = 100;

    let name = filename.unwrap_or("").trim();
    if name.is_empty() || name == "." || name == ".." {
        return default_name_for_mime(mime_type);
    }
    let cleaned = name
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return default_name_for_mime(mime_type);
    }
    if cleaned.len() > MAX_NAME_LEN {
        if let Some(dot) = cleaned.rfind('.') {
            let ext = &cleaned[dot..];
            let base = &cleaned[..dot];
            let truncated_base: String = base
                .chars()
                .take(HARD_TRUNC.saturating_sub(ext.len()))
                .collect();
            format!("{truncated_base}{ext}")
        } else {
            cleaned.chars().take(HARD_TRUNC).collect()
        }
    } else {
        cleaned.to_string()
    }
}

fn default_name_for_mime(mime_type: &str) -> String {
    match mime_type {
        "image/png" => "pasted-image.png".to_string(),
        "image/jpeg" => "pasted-image.jpg".to_string(),
        "image/gif" => "pasted-image.gif".to_string(),
        "image/webp" => "pasted-image.webp".to_string(),
        "image/svg+xml" => "pasted-image.svg".to_string(),
        "application/pdf" => "attached-file.pdf".to_string(),
        _ => format!(
            "attachment.{}",
            mime_type.split('/').next_back().unwrap_or("bin")
        ),
    }
}

#[cfg(test)]
mod tests;
