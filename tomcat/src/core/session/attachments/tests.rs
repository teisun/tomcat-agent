//! AttachmentBlobStore 单元测试。
//!
//! 覆盖 §6「Rust 单元（blob store）」的全部条目：内容寻址去重、原子写无残留、
//! sha 自校验、单张上限边界、TTL GC 三分支、delete_session 联动、session id 不变量、
//! 以及零依赖魔术字节校验的正反例。

use std::time::Duration;

use tempfile::TempDir;

use super::{
    safe_filename, validate_file_bytes, validate_image_bytes, AttachmentBlobStore,
    ORPHAN_BLOB_GRACE, PENDING_BLOB_TTL,
};
use crate::core::llm::{FILE_MAX_BYTES, IMAGE_MAX_BYTES};

fn setup() -> (TempDir, AttachmentBlobStore) {
    let tmp = TempDir::new().unwrap();
    let store = AttachmentBlobStore::new(tmp.path());
    (tmp, store)
}

fn minimal_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, //
        0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT
        0x08, 0xD7, 0x63, 0x60, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
        0xAE, 0x42, 0x60, 0x82,
    ]
}

/// 把租约的 mtime 往回拨，模拟它已经放置了很久。
fn age_lease(store: &AttachmentBlobStore, session_id: &str, sha: &str, age: Duration) {
    let path = store.root().join("pending").join(session_id).join(sha);
    let when = std::time::SystemTime::now() - age;
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(when).unwrap();
}

/// 把 blob 的 mtime 往回拨，模拟没有被任何活跃操作碰过的孤儿。
fn age_blob(store: &AttachmentBlobStore, sha: &str, age: Duration) {
    let path = store.blobs_dir().join(sha);
    let when = std::time::SystemTime::now() - age;
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(when).unwrap();
}

fn count_blobs(store: &AttachmentBlobStore) -> usize {
    let dir = store.blobs_dir();
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir).unwrap().count()
}

// ── 内容寻址与去重 ────────────────────────────────────────────────────

#[test]
fn put_is_content_addressed_and_deduplicates() {
    let (_tmp, store) = setup();
    let bytes = minimal_png();

    let first = store.put(&bytes).unwrap();
    let second = store.put(&bytes).unwrap();

    assert_eq!(first, second, "同一份字节必须得到同一个 sha");
    assert_eq!(count_blobs(&store), 1, "同一份字节只能落一个 blob 文件");
}

#[test]
fn put_does_not_rewrite_existing_blob() {
    // 这是「发送时零拷贝提升」的基础：已存在的字节不会被重写一遍。
    let (_tmp, store) = setup();
    let bytes = minimal_png();
    let sha = store.put(&bytes).unwrap();
    let path = store.blobs_dir().join(&sha);
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::thread::sleep(Duration::from_millis(20));
    store.put(&bytes).unwrap();

    let after = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert!(after > before, "重复 put 必须刷新孤儿宽限窗");
}

#[test]
fn different_bytes_get_different_blobs() {
    let (_tmp, store) = setup();
    let a = store.put(b"alpha").unwrap();
    let b = store.put(b"beta").unwrap();
    assert_ne!(a, b);
    assert_eq!(count_blobs(&store), 2);
}

#[test]
fn get_round_trips_bytes() {
    let (_tmp, store) = setup();
    let bytes = minimal_png();
    let sha = store.put(&bytes).unwrap();
    assert_eq!(store.get(&sha).unwrap(), Some(bytes));
}

#[test]
fn get_missing_blob_returns_none() {
    let (_tmp, store) = setup();
    let absent = "0".repeat(64);
    assert_eq!(store.get(&absent).unwrap(), None);
    assert!(!store.exists(&absent));
}

// ── 原子写 ────────────────────────────────────────────────────────────

#[test]
fn put_leaves_no_partial_file_behind() {
    // 原子写的可观察保证：blobs/ 里只有最终文件，没有临时残留。
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();

    let names: Vec<String> = std::fs::read_dir(store.blobs_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(names, vec![sha], "不得留下任何临时/半截文件");
}

// ── sha 自校验 ────────────────────────────────────────────────────────

#[test]
fn get_detects_and_isolates_tampered_blob() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();

    // 外部改动字节：内容不再与文件名的哈希一致。
    std::fs::write(store.blobs_dir().join(&sha), b"tampered").unwrap();

    assert_eq!(
        store.get(&sha).unwrap(),
        None,
        "内容与文件名 sha 不符必须被检出，而不是把坏字节交出去"
    );
    assert!(
        !store.blobs_dir().join(&sha).exists(),
        "坏 blob 必须被移走，避免下次再被读到"
    );
    let isolated: Vec<String> = std::fs::read_dir(store.blobs_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        isolated.iter().any(|n| n.contains(".corrupt-")),
        "应保留现场为 .corrupt-*，实际为 {isolated:?}"
    );
}

#[test]
fn malformed_sha_is_rejected() {
    let (_tmp, store) = setup();
    // 长度不对、含非十六进制字符、含路径成分，全部必须被拒。
    for bad in [
        "",
        "abc",
        &"z".repeat(64),
        &"A".repeat(64), // 大写不接受，保持单一规范形式
        "../../etc/passwd",
        &format!("{}/x", "a".repeat(62)),
    ] {
        assert!(store.get(bad).is_err(), "{bad:?} 应被拒绝");
        assert!(!store.exists(bad), "{bad:?} 不应被认为存在");
    }
}

// ── 单张上限边界 ──────────────────────────────────────────────────────

#[test]
fn image_size_limit_is_inclusive_at_the_boundary() {
    let mut at_limit = minimal_png();
    at_limit.resize(IMAGE_MAX_BYTES, 0);
    assert!(
        validate_image_bytes(&at_limit, "image/png").is_ok(),
        "恰好等于上限必须接受"
    );

    let mut over_limit = at_limit;
    over_limit.push(0);
    let error = validate_image_bytes(&over_limit, "image/png").unwrap_err();
    assert!(error.contains("too large"), "实际错误：{error}");
}

#[test]
fn file_size_limit_is_inclusive_at_the_boundary() {
    let mut at_limit = b"%PDF-1.7\n".to_vec();
    at_limit.resize(FILE_MAX_BYTES, 0);
    assert!(validate_file_bytes(&at_limit, "application/pdf").is_ok());

    let mut over_limit = at_limit;
    over_limit.push(0);
    assert!(validate_file_bytes(&over_limit, "application/pdf").is_err());
}

#[test]
fn empty_payload_is_rejected() {
    assert!(validate_image_bytes(&[], "image/png").is_err());
    assert!(validate_file_bytes(&[], "application/pdf").is_err());
}

// ── 魔术字节校验（替代原先靠解码做的格式校验）──────────────────────────

#[test]
fn magic_bytes_accept_each_supported_format() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("image/png", minimal_png()),
        ("image/jpeg", vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00]),
        ("image/gif", b"GIF89a\x01\x00".to_vec()),
        ("image/webp", b"RIFF\x24\x00\x00\x00WEBPVP8 ".to_vec()),
        (
            "image/svg+xml",
            br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec(),
        ),
    ];
    for (mime, bytes) in cases {
        assert!(
            validate_image_bytes(&bytes, mime).is_ok(),
            "{mime} 的合法头部被误拒"
        );
    }
}

#[test]
fn magic_bytes_reject_mislabelled_payload() {
    // 声明 PNG 实际是 JPEG：必须被挡下来。
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00];
    let error = validate_image_bytes(&jpeg, "image/png").unwrap_err();
    assert!(error.contains("magic byte"), "实际错误：{error}");

    // 完全不是图片的垃圾字节。
    assert!(validate_image_bytes(b"not an image at all", "image/png").is_err());
    // 声明 PDF 实际不是。
    assert!(validate_file_bytes(b"just text", "application/pdf").is_err());
}

#[test]
fn unsupported_mime_types_are_rejected() {
    assert!(validate_image_bytes(&minimal_png(), "image/tiff").is_err());
    assert!(validate_image_bytes(&minimal_png(), "text/html").is_err());
    assert!(validate_file_bytes(b"%PDF-1.7", "application/zip").is_err());
}

#[test]
fn svg_with_inline_style_is_accepted() {
    // 回归保护：设计工具导出的 SVG 几乎必然带 style=，旧实现的文本黑名单会误杀它们。
    // 新实现只确认它是 UTF-8 且有 svg 根元素，安全性交给 Chromium 的 secure static mode。
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
        <style>.a{fill:red}</style>
        <rect class="a" style="fill:blue" fill="url(#grad)" width="16" height="16"/>
    </svg>"#;
    assert!(
        validate_image_bytes(svg, "image/svg+xml").is_ok(),
        "带 style= / <style> / url(#grad) 的正常 SVG 必须被接受"
    );
}

// ── 租约与提升 ────────────────────────────────────────────────────────

#[test]
fn mark_and_list_pending() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &sha).unwrap();
    assert_eq!(store.list_pending("s1").unwrap(), vec![sha]);
    assert!(
        store.list_pending("s2").unwrap().is_empty(),
        "会话之间必须隔离"
    );
}

#[test]
fn promote_releases_lease_and_refreshes_blob_grace_window() {
    // 提升仍不搬字节，但必须刷新 mtime，以免并发 sweep 将刚发出的唯一副本误判为孤儿。
    let (_tmp, store) = setup();
    let bytes = minimal_png();
    let sha = store.put(&bytes).unwrap();
    store.mark_pending("s1", &sha).unwrap();

    let path = store.blobs_dir().join(&sha);
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::thread::sleep(Duration::from_millis(20));
    store.promote("s1", &sha).unwrap();

    assert!(store.list_pending("s1").unwrap().is_empty(), "租约应已释放");
    assert!(store.exists(&sha), "字节必须保留给 transcript 引用");
    assert!(
        std::fs::metadata(&path).unwrap().modified().unwrap() > before,
        "提升必须刷新 blob 的宽限窗"
    );
    assert_eq!(store.get(&sha).unwrap(), Some(bytes));
}

#[test]
fn clear_session_reports_lease_and_directory_errors() {
    let (_tmp, store) = setup();
    let sha = store.put(b"shared").unwrap();
    let directory = store.root().join("pending/session");
    let marker = directory.join(&sha);
    std::fs::create_dir_all(&marker).unwrap();
    assert!(store.clear_session("session").is_err());
    assert!(marker.is_dir());
    assert_eq!(store.get(&sha).unwrap(), Some(b"shared".to_vec()));
    std::fs::remove_dir(&marker).unwrap();
    assert_eq!(store.clear_session("session").unwrap().leases_released, 0);
    assert_eq!(store.clear_session("session").unwrap().leases_released, 0);
    store.promote("session", &sha).unwrap(); // An absent lease remains idempotent.
    store.mark_pending("session", &sha).unwrap();
    assert_eq!(store.clear_session("session").unwrap().leases_released, 1);
    std::fs::create_dir_all(directory.join("not-a-lease")).unwrap();
    assert!(store.clear_session("session").is_err());
}

#[test]
fn touch_pending_renews_an_expiring_lease() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &sha).unwrap();
    age_lease(&store, "s1", &sha, Duration::from_secs(60));

    store.touch_pending("s1", &sha).unwrap();

    let report = store.gc_pending(Duration::from_secs(30)).unwrap();
    assert_eq!(report.leases_released, 0, "续期后不该被 GC 判为超期");
    assert!(store.exists(&sha));
}

#[test]
fn touch_pending_ignores_missing_blob() {
    // 草稿里的引用可能已经指不到字节了；续期不应因此报错或凭空造出租约。
    let (_tmp, store) = setup();
    let absent = "1".repeat(64);
    store.touch_pending("s1", &absent).unwrap();
    assert!(store.list_pending("s1").unwrap().is_empty());
}

#[test]
fn retain_pending_batch_deduplicates_and_is_idempotent() {
    let (_tmp, store) = setup();
    let blob_sha = store.put(b"original rendition").unwrap();
    let provider_sha = store.put(b"provider rendition").unwrap();
    store.mark_pending("source", &blob_sha).unwrap();
    store.mark_pending("target", &blob_sha).unwrap();
    let existing_path = store.root().join("pending/target").join(&blob_sha);
    let existing_mtime = std::fs::metadata(&existing_path)
        .unwrap()
        .modified()
        .unwrap();

    let retained = store
        .retain_pending_batch(
            "target",
            vec![blob_sha.clone(), provider_sha.clone(), blob_sha.clone()],
        )
        .unwrap();
    let mut expected = vec![blob_sha.clone(), provider_sha.clone()];
    expected.sort();
    assert_eq!(retained, expected);
    assert_eq!(store.list_pending("target").unwrap(), expected);
    assert_eq!(
        std::fs::metadata(existing_path)
            .unwrap()
            .modified()
            .unwrap(),
        existing_mtime,
        "幂等 retain 不得重写已有 marker"
    );
    assert_eq!(
        store.list_pending("source").unwrap(),
        vec![blob_sha],
        "目标 retain 不得改动源租约"
    );
}

#[test]
fn retain_pending_batch_validates_every_sha_before_writing() {
    let (_tmp, store) = setup();
    let valid = store.put(b"valid blob").unwrap();
    let missing = "1".repeat(64);
    assert!(store
        .retain_pending_batch("target", vec![valid.clone(), missing])
        .is_err());
    assert!(store.list_pending("target").unwrap().is_empty());

    assert!(store
        .retain_pending_batch("target", vec![valid, "not-a-sha".to_string()])
        .is_err());
    assert!(store.list_pending("target").unwrap().is_empty());
}

#[test]
fn retain_pending_batch_rolls_back_only_markers_created_by_that_call() {
    let (_tmp, store) = setup();
    let mut shas = vec![
        store.put(b"first").unwrap(),
        store.put(b"second").unwrap(),
        store.put(b"third").unwrap(),
    ];
    shas.sort();
    store.mark_pending("target", &shas[0]).unwrap();

    let error = store
        .retain_pending_batch_inner("target", shas.clone(), |index, _| {
            if index == 2 {
                Err(crate::AppError::Config(
                    "injected lease failure".to_string(),
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected lease failure"));
    assert_eq!(
        store.list_pending("target").unwrap(),
        vec![shas[0].clone()],
        "已有 marker 必须保留，本次在失败前创建的 marker 必须回滚"
    );
}

// ── 租约释放与标记清扫 ─────────────────────────────────────────────────

#[test]
fn gc_releases_expired_lease_without_deleting_blob() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &sha).unwrap();
    age_lease(&store, "s1", &sha, Duration::from_secs(3600));

    let report = store.gc_pending(Duration::from_secs(60)).unwrap();

    assert_eq!(report.leases_released, 1);
    assert!(store.exists(&sha), "实际删除只允许由标记清扫负责");
}

#[test]
fn sweep_keeps_expired_but_transcript_referenced_blob() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
    let mut live = std::collections::HashSet::new();
    live.insert(sha.clone());
    let report = store.sweep_orphan_blobs(&live, ORPHAN_BLOB_GRACE).unwrap();

    assert_eq!(report.blobs_deleted, 0);
    assert_eq!(report.blobs_retained, 1);
    assert!(store.exists(&sha), "被 transcript 引用的字节必须保留");
}

#[test]
fn gc_leaves_fresh_lease_alone() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &sha).unwrap();

    let report = store.gc_pending(PENDING_BLOB_TTL).unwrap();

    assert_eq!(report, Default::default(), "未超期的租约不应被动");
    assert!(store.exists(&sha));
}

#[test]
fn sweep_keeps_blob_still_live_from_another_session_lease() {
    // 在用名单已经合并所有 pending 租约；sweep 不需要逐会话判断。
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
    let mut live = std::collections::HashSet::new();
    live.insert(sha.clone());
    let report = store.sweep_orphan_blobs(&live, ORPHAN_BLOB_GRACE).unwrap();

    assert_eq!(report.blobs_deleted, 0);
    assert_eq!(report.blobs_retained, 1);
    assert!(store.exists(&sha), "还有别的会话租着，不能回收");
}

#[test]
fn sweep_obeys_grace_window_then_deletes_orphan_and_thumbnail() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.put_thumbnail(&sha, b"thumbnail").unwrap();
    age_blob(
        &store,
        &sha,
        ORPHAN_BLOB_GRACE.saturating_sub(Duration::from_secs(60)),
    );

    let early = store
        .sweep_orphan_blobs(&std::collections::HashSet::new(), ORPHAN_BLOB_GRACE)
        .unwrap();
    assert_eq!(early.blobs_deleted, 0, "59 分钟的孤儿仍在宽限期");
    assert!(store.exists(&sha));

    age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
    let late = store
        .sweep_orphan_blobs(&std::collections::HashSet::new(), ORPHAN_BLOB_GRACE)
        .unwrap();
    assert_eq!(late.blobs_deleted, 1, "61 分钟的孤儿应被回收");
    assert!(!store.exists(&sha));
    assert!(!store.has_thumbnail(&sha));
}

#[test]
fn gc_on_empty_store_is_a_no_op() {
    let (_tmp, store) = setup();
    assert_eq!(
        store.gc_pending(PENDING_BLOB_TTL).unwrap(),
        Default::default()
    );
}

// ── delete_session 联动 ───────────────────────────────────────────────

#[test]
fn clear_session_releases_leases_and_defers_blob_collection_to_sweep() {
    let (_tmp, store) = setup();
    let kept = store.put(b"referenced-by-transcript").unwrap();
    let dropped = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &kept).unwrap();
    store.mark_pending("s1", &dropped).unwrap();

    let report = store.clear_session("s1").unwrap();

    assert_eq!(report.leases_released, 2);
    assert!(store.exists(&kept), "删除会话不会猜测全局引用关系");
    assert!(store.exists(&dropped), "实际删除留给带完整在用名单的 sweep");
    assert!(store.list_pending("s1").unwrap().is_empty());
}

#[test]
fn clear_session_does_not_touch_other_sessions() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    store.mark_pending("s1", &sha).unwrap();
    store.mark_pending("s2", &sha).unwrap();

    store.clear_session("s1").unwrap();

    assert_eq!(store.list_pending("s2").unwrap(), vec![sha.clone()]);
    assert!(store.exists(&sha), "另一个会话还租着，字节不能被回收");
}

// ── session id 不变量 ─────────────────────────────────────────────────

#[test]
fn invalid_session_ids_are_rejected_inside_the_module() {
    // 不把路径安全性寄托在调用方：即使今天所有调用方都传可信 id，模块也要自己守住。
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    for bad in [
        "",
        "../escape",
        "a/b",
        "with space",
        "dot.dot",
        &"x".repeat(129),
    ] {
        assert!(
            store.mark_pending(bad, &sha).is_err(),
            "session id {bad:?} 应被拒绝"
        );
        assert!(store.list_pending(bad).is_err());
        assert!(store.promote(bad, &sha).is_err());
    }
}

#[test]
fn valid_session_ids_are_accepted() {
    let (_tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    for good in ["s1", "abc-123", "under_score", "A1b2C3"] {
        store.mark_pending(good, &sha).unwrap();
        assert_eq!(store.list_pending(good).unwrap(), vec![sha.clone()]);
    }
}

// ── 路径暴露 ──────────────────────────────────────────────────────────

#[test]
fn exposes_roots_for_webview_resource_configuration() {
    // 宿主需要这些路径来配置 localResourceRoots；它自己算不出来。
    let (tmp, store) = setup();
    assert_eq!(store.root(), tmp.path().join("attachments"));
    assert!(store.blobs_dir().starts_with(store.root()));
}

// ── 文件名安全 ────────────────────────────────────────────────────────

#[test]
fn safe_filename_strips_path_components_and_fills_defaults() {
    assert_eq!(
        safe_filename(Some("../../etc/passwd"), "image/png"),
        "passwd"
    );
    assert_eq!(safe_filename(Some("a\\b\\c.png"), "image/png"), "c.png");
    assert_eq!(safe_filename(None, "image/png"), "pasted-image.png");
    assert_eq!(
        safe_filename(Some("  "), "image/svg+xml"),
        "pasted-image.svg"
    );
    assert_eq!(
        safe_filename(Some(".."), "application/pdf"),
        "attached-file.pdf"
    );

    let long = format!("{}.png", "n".repeat(300));
    let truncated = safe_filename(Some(&long), "image/png");
    assert!(truncated.len() <= 120);
    assert!(truncated.ends_with(".png"), "截断时必须保住扩展名");
}

// ── 可重建字节的 LRU 淘汰 ──────────────────────────────────────────────
//
// 这组用例守的是「淘汰绝不能碰未发送内容」这条底线。判据是「能不能重建」，
// 而不是「在哪个目录」—— 目录已经合并成一个了。

#[test]
fn eviction_never_touches_unsent_bytes() {
    let (_tmp, store) = setup();
    let sha = store.put(&vec![7u8; 4096]).unwrap();
    store.mark_pending("sid_owner", &sha).unwrap();

    // 预算为 0：只要允许动，就一定会被删。
    let freed = store.evict_rebuildable_over_budget(0).unwrap();

    assert_eq!(freed, 0);
    assert!(
        store.exists(&sha),
        "还持有租约的字节是未发送内容的唯一副本，预算再紧也不能删"
    );
}

#[test]
fn eviction_never_touches_sent_history_blob() {
    let (_tmp, store) = setup();
    let sha = store.put(&vec![9u8; 8192]).unwrap();

    let freed = store.evict_rebuildable_over_budget(0).unwrap();

    assert_eq!(freed, 0);
    assert!(
        store.exists(&sha),
        "transcript 只保存哈希；CAS blob 是历史图片的唯一副本，不可淘汰"
    );
}

#[test]
fn eviction_leaves_unreferenced_garbage_to_the_collector() {
    // 没租约又没人引用 = 垃圾。它该由 GC 无条件删掉，
    // 而不是「等到超预算才删」—— 混进淘汰逻辑只会让两条路径互相掩盖。
    let (_tmp, store) = setup();
    let sha = store.put(&vec![3u8; 4096]).unwrap();

    let freed = store.evict_rebuildable_over_budget(0).unwrap();
    assert_eq!(freed, 0);

    store.gc_pending(Duration::ZERO).unwrap();
    assert!(store.exists(&sha));
}

#[test]
fn eviction_drops_thumbnails_before_anything_else_when_over_budget() {
    let (_tmp, store) = setup();
    let source = store.put(&vec![1u8; 1024]).unwrap();
    store.mark_pending("sid_owner", &source).unwrap();
    store.put_thumbnail(&source, &vec![2u8; 2048]).unwrap();

    let freed = store.evict_rebuildable_over_budget(0).unwrap();

    assert!(freed >= 2048);
    assert!(
        !store.has_thumbnail(&source),
        "缩略图可以重新生成，先淘汰它"
    );
    assert!(store.exists(&source), "源字节仍在租约里，必须留下");
}

#[test]
fn eviction_is_a_no_op_within_budget() {
    let (_tmp, store) = setup();
    let sha = store.put(&vec![5u8; 1024]).unwrap();

    let freed = store.evict_rebuildable_over_budget(1024 * 1024).unwrap();

    assert_eq!(freed, 0);
    assert!(store.exists(&sha));
}

#[test]
fn put_does_not_create_a_lease() {
    // 写入内容寻址 blob 本身不代表草稿归属，租约必须由调用方显式创建。
    let (_tmp, store) = setup();
    let sha = store.put(b"history image bytes").unwrap();

    assert!(store.exists(&sha));
    assert!(store.list_pending("sid_any").unwrap().is_empty());
}

#[test]
fn duplicate_put_reuses_an_existing_blob_and_refreshes_grace_window() {
    let (_tmp, store) = setup();
    let bytes = b"same image in two places";
    let put_sha = store.put(bytes).unwrap();
    let path = store.blobs_dir().join(&put_sha);
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::thread::sleep(Duration::from_millis(20));
    let materialized_sha = store.put(bytes).unwrap();

    assert_eq!(materialized_sha, put_sha);
    assert!(
        std::fs::metadata(&path).unwrap().modified().unwrap() > before,
        "命中已有字节时也必须刷新孤儿宽限窗"
    );
}
