use super::super::read_state::{hash_content, ReadFileState, ReadRenderMode, ReadStamp};

/// 一条「按请求窗口原样读到」的 stamp：分窗读覆盖请求区间，整读覆盖到文件末尾。
fn stamp(mtime: i64, size: u64, off: Option<u64>, lim: Option<u64>) -> ReadStamp {
    stamp_with_render_mode(mtime, size, off, lim, ReadRenderMode::Plain)
}

fn stamp_with_render_mode(
    mtime: i64,
    size: u64,
    off: Option<u64>,
    lim: Option<u64>,
    render_mode: ReadRenderMode,
) -> ReadStamp {
    let partial = off.is_some() || lim.is_some();
    let start = off.unwrap_or(1);
    let covered = match lim {
        Some(lim) => (start, start + lim - 1),
        None => (start, u64::MAX),
    };
    ReadStamp {
        mtime_ms: mtime,
        size,
        content_hash: 0,
        offset: off,
        limit: lim,
        is_partial_view: partial,
        render_mode,
        covered_lines: Some(covered),
        reached_eof: lim.is_none(),
        tool_call_id: None,
    }
}

/// 读到一半被截断的 stamp：只覆盖 `covered`，且没读到文件末尾。
fn truncated_stamp(mtime: i64, size: u64, covered: (u64, u64)) -> ReadStamp {
    ReadStamp {
        mtime_ms: mtime,
        size,
        content_hash: 0,
        offset: Some(covered.0),
        limit: None,
        is_partial_view: true,
        render_mode: ReadRenderMode::Plain,
        covered_lines: Some(covered),
        reached_eof: false,
        tool_call_id: None,
    }
}

#[test]
fn matches_request_dedup_hits_when_window_and_metadata_align() {
    let s = stamp(100, 1024, Some(1), Some(50));
    assert!(s.matches_request(100, 1024, Some(1), Some(50), ReadRenderMode::Plain));
}

#[test]
fn matches_request_misses_when_mtime_changes() {
    let s = stamp(100, 1024, Some(1), Some(50));
    assert!(!s.matches_request(101, 1024, Some(1), Some(50), ReadRenderMode::Plain));
}

#[test]
fn matches_request_misses_when_size_changes() {
    let s = stamp(100, 1024, None, None);
    assert!(!s.matches_request(100, 1025, None, None, ReadRenderMode::Plain));
}

#[test]
fn independent_subagent_read_states_do_not_share_dedup_stamps() {
    let first = ReadFileState::new();
    let second = ReadFileState::new();
    let path = std::path::PathBuf::from("/workspace/src/lib.rs");
    first.put(path.clone(), stamp(100, 1024, None, None));

    assert!(first.get(&path).is_some());
    assert!(
        second.get(&path).is_none(),
        "a separate reviewer must perform its own first read"
    );
}

#[test]
fn matches_request_misses_when_window_differs() {
    let s = stamp(100, 1024, Some(1), Some(50));
    assert!(!s.matches_request(100, 1024, Some(1), Some(60), ReadRenderMode::Plain));
    assert!(!s.matches_request(100, 1024, Some(2), Some(50), ReadRenderMode::Plain));
}

#[test]
fn matches_request_hits_when_earlier_read_contains_the_window() {
    // 读过 L1684-1733（50 行）之后再要 L1684-1703：内容就在上下文里，不该再读一遍。
    let s = stamp(100, 1024, Some(1684), Some(50));
    assert_eq!(
        s.covers(100, 1024, Some(1684), Some(20), ReadRenderMode::Plain),
        Some((1684, 1733))
    );
    // 整读且读到了末尾，任何子窗口都被覆盖。
    let full = stamp(100, 1024, None, None);
    assert!(full.matches_request(100, 1024, Some(1), Some(50), ReadRenderMode::Plain));
}

#[test]
fn matches_request_misses_on_partial_overlap_or_unbounded_request() {
    let s = stamp(100, 1024, Some(1684), Some(50));
    // 起点在已读区间之前：前半段没读过。
    assert!(!s.matches_request(100, 1024, Some(1600), Some(200), ReadRenderMode::Plain));
    // 终点越过已读区间：后半段没读过。
    assert!(!s.matches_request(100, 1024, Some(1700), Some(100), ReadRenderMode::Plain));
    // 上次没读到末尾，这次又不给上界：无从判断够不够。
    let truncated = truncated_stamp(100, 1024, (1, 2000));
    assert!(!truncated.matches_request(100, 1024, Some(1), None, ReadRenderMode::Plain));
    assert!(truncated.matches_request(100, 1024, Some(1), Some(2000), ReadRenderMode::Plain));
    // 非文本结果没有行区间，一律重读。
    let mut imageish = stamp(100, 1024, None, None);
    imageish.covered_lines = None;
    assert!(!imageish.matches_request(100, 1024, None, None, ReadRenderMode::Plain));
}

#[test]
fn matches_request_misses_when_render_mode_differs() {
    let line_numbers =
        stamp_with_render_mode(100, 1024, Some(1), Some(50), ReadRenderMode::LineNumbers);
    assert!(
        !line_numbers.matches_request(100, 1024, Some(1), Some(50), ReadRenderMode::Hashline),
        "line-number text cannot satisfy a hashline request"
    );

    let hashline = stamp_with_render_mode(100, 1024, Some(1), Some(50), ReadRenderMode::Hashline);
    assert!(
        !hashline.matches_request(100, 1024, Some(1), Some(50), ReadRenderMode::LineNumbers),
        "hashline text cannot satisfy a line-number request"
    );
}

#[test]
fn resolve_prefers_hashline_and_normalizes_ignored_line_numbers_flag() {
    assert_eq!(
        ReadRenderMode::resolve(true, true),
        ReadRenderMode::Hashline
    );
    assert_eq!(
        ReadRenderMode::resolve(false, true),
        ReadRenderMode::Hashline
    );

    let stamp = stamp_with_render_mode(100, 1024, Some(1), Some(50), ReadRenderMode::Hashline);
    assert!(
        stamp.matches_request(
            100,
            1024,
            Some(1),
            Some(50),
            ReadRenderMode::resolve(false, true)
        ),
        "two requests that both render as hashline must dedup"
    );
}

#[test]
fn invalidate_tool_call_only_drops_stamps_still_owned_by_that_call() {
    let state = ReadFileState::new();
    let evicted = std::path::PathBuf::from("/a");
    let reread = std::path::PathBuf::from("/b");
    let untouched = std::path::PathBuf::from("/c");

    let mut from_call1 = stamp(1, 1, None, None);
    from_call1.tool_call_id = Some("call-1".to_string());
    state.put(evicted.clone(), from_call1.clone());
    // 同一个文件后来又被 call-2 读过一次，那份内容还看得见，不该被 call-1 的清理波及。
    let mut from_call2 = stamp(1, 1, None, None);
    from_call2.tool_call_id = Some("call-2".to_string());
    state.put(reread.clone(), from_call2);
    state.put(untouched.clone(), stamp(1, 1, None, None));

    assert_eq!(state.invalidate_tool_call("call-1"), 1);
    assert!(state.get(&evicted).is_none());
    assert!(state.get(&reread).is_some());
    assert!(state.get(&untouched).is_some());
    assert_eq!(state.invalidate_tool_call("call-1"), 0);
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
