use super::super::read_state::{hash_content, ReadFileState, ReadStamp};

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
    state.put(std::path::PathBuf::from("/b"), stamp(2, 2, Some(1), Some(10)));
    assert_eq!(state.len(), 2);
    state.clear();
    assert_eq!(state.len(), 0);
}

#[test]
fn hash_content_is_deterministic_and_distinct() {
    assert_eq!(hash_content(b"hello"), hash_content(b"hello"));
    assert_ne!(hash_content(b"hello"), hash_content(b"world"));
}
