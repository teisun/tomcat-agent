use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_sessions_dir() -> PathBuf {
    let c = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    std::env::temp_dir().join(format!("pi_wasm_mgr_{}_{}_{}", std::process::id(), ms, c))
}

#[test]
fn create_session_and_list() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    let entry = mgr.create_session(key, Some("/tmp".to_string())).unwrap();
    assert!(!entry.session_id.is_empty());
    assert!(entry.updated_at > 0);
    let list = mgr.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_store_empty_when_no_file() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let store = mgr.load_store().unwrap();
    assert!(store.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_then_get_entries() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let entries = mgr.get_entries(10).unwrap();
    assert!(entries.is_empty());
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let entries2 = mgr.get_entries(10).unwrap();
    assert_eq!(entries2.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_removes_from_store() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    assert_eq!(mgr.list_sessions().unwrap().len(), 1);
    mgr.delete_session(key).unwrap();
    assert!(mgr.list_sessions().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_session_returns_none_for_unknown_key() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let opt = mgr.get_session("unknown:key").unwrap();
    assert!(opt.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn from_sessions_dir_with_absolute_path() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path_str = dir.to_string_lossy();
    let mgr = SessionManager::from_sessions_dir(path_str.as_ref()).unwrap();
    assert!(mgr.store_path().ends_with("sessions.json"));
    assert!(mgr.load_store().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcript_path_format() {
    let dir = temp_sessions_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let p = mgr.transcript_path("sid_123");
    assert!(p.ends_with("sid_123.jsonl"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_session_returns_some_after_create() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    let created = mgr.create_session(key, None).unwrap();
    let opt = mgr.get_session(key).unwrap();
    assert!(opt.is_some());
    let entry = opt.unwrap();
    assert_eq!(entry.session_id, created.session_id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_session_header_after_create() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let header = mgr.read_session_header().unwrap();
    assert!(header.is_some());
    assert!(!header.unwrap().id.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_session_header_none_when_no_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let header = mgr.read_session_header().unwrap();
    assert!(header.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_entry_with_session_returns_option() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let opt = mgr.get_entry("unknown_id").unwrap();
    assert!(opt.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_children_with_session_returns_vec() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let children = mgr.get_children("parent", 5).unwrap();
    assert!(children.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_leaf_entry_with_session_returns_last() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"hi"}))
        .unwrap();
    let leaf = mgr.get_leaf_entry().unwrap();
    assert!(leaf.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_branch_with_session_returns_vec() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let branch = mgr.get_branch("any_leaf").unwrap();
    assert!(branch.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_session_modifies_store() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let before = mgr.get_session(key).unwrap().unwrap().updated_at;
    mgr.update_session(key, |e| {
        e.cwd = Some("/updated".to_string());
    })
    .unwrap();
    let after = mgr.get_session(key).unwrap().unwrap();
    assert!(after.updated_at >= before);
    assert_eq!(after.cwd.as_deref(), Some("/updated"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_thinking_level_change_succeeds() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let r = mgr.append_thinking_level_change("full");
    assert!(r.is_ok());
    let entries = mgr.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_model_change_succeeds() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    let r = mgr.append_model_change(Some("openai"), Some("gpt-4"));
    assert!(r.is_ok());
    let entries = mgr.get_entries(10).unwrap();
    assert_eq!(entries.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_empty_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system prompt").unwrap();
    assert!(state.user_turns_list.is_empty());
    assert_eq!(state.estimate_context_chars, "system prompt".len());
    assert_eq!(state.context_budget_chars, 1_088_000);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_with_messages() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q2"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a2"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert_eq!(state.user_turns_list.len(), 2);
    assert!(state.estimate_context_chars > 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_with_compaction_entry() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_compaction(Some("summary of old turns")).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q_after"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a_after"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert_eq!(state.user_turns_list.len(), 2);
    if let TurnEntry::SummaryTurn { summary, .. } = &state.user_turns_list[0] {
        assert_eq!(summary, "summary of old turns");
    } else {
        panic!("first turn should be SummaryTurn");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_context_from_state_flattens_turns() {
    let state = ContextState {
        user_turns_list: vec![
            TurnEntry::SummaryTurn {
                id: "sum_1".to_string(),
                summary: "summary".to_string(),
                timestamp: "2026-04-04T12:00:00Z".to_string(),
            },
            TurnEntry::UserTurn {
                id: "turn_1".to_string(),
                messages: vec![
                    AgentMessage::User {
                        text: "hello".to_string(),
                    },
                    AgentMessage::Assistant {
                        text: "world".to_string(),
                        tool_calls: vec![],
                    },
                ],
                timestamp: "2026-04-04T12:00:00Z".to_string(),
            },
        ],
        estimate_context_chars: 100,
        context_budget_chars: 1000,
        context_budget_tokens: 250,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: crate::core::compaction::preheat::Preheat::new(),
    };
    let msgs = build_context_from_state(&state);
    assert_eq!(msgs.len(), 3);
    assert!(matches!(&msgs[0], AgentMessage::CompactionSummary { .. }));
    assert!(matches!(&msgs[1], AgentMessage::User { .. }));
    assert!(matches!(&msgs[2], AgentMessage::Assistant { .. }));
}

#[test]
fn init_context_state_no_session() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();
    assert!(state.user_turns_list.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_boundary_discards_prior() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"old q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old a1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"old q2"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old a2"}))
        .unwrap();

    let path = mgr.current_transcript_path().unwrap().unwrap();
    let boundary_entry = TranscriptEntry::Compaction(CompactionEntry {
        id: None,
        parent_id: None,
        timestamp: "2026-01-01T00:00:00.000Z".to_string(),
        summary: Some("boundary summary".to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: Some(2),
        is_boundary: Some(true),
    });
    super::super::transcript::append_entry(&path, &boundary_entry).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"new q"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new a"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    assert_eq!(state.user_turns_list.len(), 2, "boundary + 1 new turn");

    let has_boundary_summary = state.user_turns_list.iter().any(
        |t| matches!(t, TurnEntry::SummaryTurn { summary, .. } if summary == "boundary summary"),
    );
    assert!(has_boundary_summary, "should contain boundary summary");

    let has_old = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("old")))
        } else {
            false
        }
    });
    assert!(!has_old, "old turns before boundary should be discarded");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_context_state_non_boundary_compaction_preserves_prior() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    mgr.append_message(serde_json::json!({"role":"user","content":"q1"}))
        .unwrap();
    mgr.append_message(serde_json::json!({"role":"assistant","content":"a1"}))
        .unwrap();
    mgr.append_compaction(Some("non-boundary summary")).unwrap();
    mgr.append_message(serde_json::json!({"role":"user","content":"q2"}))
        .unwrap();

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "sys").unwrap();

    assert!(
        state.user_turns_list.len() >= 3,
        "should preserve pre-compaction turn + summary + post turn"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ────────── compute_fold_start 纯函数测试 ──────────────────────────

fn make_user_msg_entry(ts: &str) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        id: None,
        parent_id: None,
        timestamp: ts.to_string(),
        message: serde_json::json!({"role":"user","content":"q"}),
    })
}

fn make_assistant_msg_entry(ts: &str) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        id: None,
        parent_id: None,
        timestamp: ts.to_string(),
        message: serde_json::json!({"role":"assistant","content":"a"}),
    })
}

fn make_boundary_entry(ts: &str, summary: &str) -> TranscriptEntry {
    use super::super::transcript::CompactionEntry;
    TranscriptEntry::Compaction(CompactionEntry {
        id: None,
        parent_id: None,
        timestamp: ts.to_string(),
        summary: Some(summary.to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: None,
        is_boundary: Some(true),
    })
}

#[test]
fn fold_start_skips_old_entries() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let old = "2026-04-03T10:00:00Z";
    let new = "2026-04-04T10:00:00Z";

    let mut entries = Vec::new();
    for _ in 0..50 {
        entries.push(make_user_msg_entry(old));
        entries.push(make_assistant_msg_entry(old));
    }
    for _ in 0..15 {
        entries.push(make_user_msg_entry(new));
        entries.push(make_assistant_msg_entry(new));
    }

    let start = compute_fold_start(&entries, today, 10);
    assert!(
        start >= 100,
        "should skip old entries, got fold_start={}",
        start
    );
}

#[test]
fn fold_start_includes_backfill() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let old = "2026-04-03T10:00:00Z";
    let new = "2026-04-04T10:00:00Z";

    let mut entries = Vec::new();
    for _ in 0..20 {
        entries.push(make_user_msg_entry(old));
        entries.push(make_assistant_msg_entry(old));
    }
    for _ in 0..3 {
        entries.push(make_user_msg_entry(new));
        entries.push(make_assistant_msg_entry(new));
    }

    let start = compute_fold_start(&entries, today, 10);
    let user_msgs_from_start = entries[start..]
        .iter()
        .filter(|e| is_user_message(e))
        .count();
    assert!(
        user_msgs_from_start >= 10,
        "should include backfill, user_msgs_from_start={}",
        user_msgs_from_start
    );
}

#[test]
fn fold_start_respects_boundary() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let old = "2026-04-03T10:00:00Z";
    let new = "2026-04-04T10:00:00Z";

    let mut entries = Vec::new();
    for _ in 0..25 {
        entries.push(make_user_msg_entry(old));
    }
    let boundary_idx = entries.len();
    entries.push(make_boundary_entry(old, "boundary summary"));
    for _ in 0..12 {
        entries.push(make_user_msg_entry(new));
    }

    let start = compute_fold_start(&entries, today, 10);
    assert_eq!(start, boundary_idx, "should start from boundary");
}

#[test]
fn fold_start_all_old_no_today() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let old = "2026-04-03T10:00:00Z";

    let mut entries = Vec::new();
    for _ in 0..30 {
        entries.push(make_user_msg_entry(old));
        entries.push(make_assistant_msg_entry(old));
    }

    let start = compute_fold_start(&entries, today, 10);
    let user_msgs = entries[start..]
        .iter()
        .filter(|e| is_user_message(e))
        .count();
    assert!(
        user_msgs >= 10,
        "should backfill at least 10 user msgs, got {}",
        user_msgs
    );
}

#[test]
fn fold_start_empty() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let entries: Vec<TranscriptEntry> = vec![];
    assert_eq!(compute_fold_start(&entries, today, 10), 0);
}

// ────────── filter_turns_by_day 纯函数测试 ──────────────────────────

fn make_test_turn(ts: &str) -> TurnEntry {
    TurnEntry::UserTurn {
        id: format!("test_{}", ts),
        messages: vec![AgentMessage::User {
            text: "q".to_string(),
        }],
        timestamp: ts.to_string(),
    }
}

#[test]
fn filter_enough_today_no_backfill() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let mut turns = Vec::new();
    for _ in 0..5 {
        turns.push(make_test_turn("2026-04-03T10:00:00Z"));
    }
    for _ in 0..12 {
        turns.push(make_test_turn("2026-04-04T10:00:00Z"));
    }

    let selected = filter_turns_by_day(turns, today, 10);
    assert_eq!(selected.len(), 12, "today has 12 >= 10, no backfill needed");
    assert!(selected
        .iter()
        .all(|t| parse_date(t.timestamp()) == Some(today)));
}

#[test]
fn filter_backfill_to_10() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let mut turns = Vec::new();
    for _ in 0..12 {
        turns.push(make_test_turn("2026-04-03T10:00:00Z"));
    }
    for _ in 0..3 {
        turns.push(make_test_turn("2026-04-04T10:00:00Z"));
    }

    let selected = filter_turns_by_day(turns, today, 10);
    assert_eq!(selected.len(), 10, "3 today + 7 backfill = 10");

    let today_count = selected
        .iter()
        .filter(|t| parse_date(t.timestamp()) == Some(today))
        .count();
    assert_eq!(today_count, 3);
}

#[test]
fn filter_cross_midnight() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let turns: Vec<_> = (0..15)
        .map(|_| make_test_turn("2026-04-03T23:00:00Z"))
        .collect();

    let selected = filter_turns_by_day(turns, today, 10);
    assert_eq!(
        selected.len(),
        10,
        "no today turns, backfill 10 from yesterday"
    );
}

#[test]
fn filter_all_today_gt_10() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let turns: Vec<_> = (0..15)
        .map(|_| make_test_turn("2026-04-04T10:00:00Z"))
        .collect();

    let selected = filter_turns_by_day(turns, today, 10);
    assert_eq!(
        selected.len(),
        15,
        "all today turns should be kept without truncation"
    );
}

#[test]
fn filter_empty() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let selected = filter_turns_by_day(vec![], today, 10);
    assert!(selected.is_empty());
}

#[test]
fn try_append_returns_err_on_violation() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();
    mgr.try_append_message(serde_json::json!({ "role": "user", "content": "hi" }))
        .unwrap();
    let result = mgr.try_append_message(serde_json::json!({
        "role": "tool",
        "tool_call_id": "c1",
        "content": "ok"
    }));
    assert!(result.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_generates_unique_ids() {
    let id1 = generate_entry_id();
    let id2 = generate_entry_id();
    let id3 = generate_entry_id();
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}
