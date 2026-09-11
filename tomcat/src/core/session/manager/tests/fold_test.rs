//! # 当日折叠（fold）/ 跨日 backfill 纯函数测试
//!
//! 覆盖 `compute_fold_start` / `filter_turns_by_day` 两个工具函数：
//!
//! - `fold_start_*`：跳过历史条目、在不足 backfill 时回填、boundary
//!   截断优先级、全部历史无今日、空集合等 5 个等价类。
//! - `filter_*`：今日已够 / 今日不够回补到 10 / 跨午夜全是昨天 /
//!   今日 >10 不截断 / 空输入 5 个等价类。

use super::super::*;
use crate::core::llm::ChatMessage;

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
    TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: None,
        parent_id: None,
        timestamp: ts.to_string(),
        summary: Some(summary.to_string()),
        covered_start_id: None,
        covered_end_id: None,
        covered_count: None,
        is_boundary: Some(true),
        preheat_compaction_id: None,
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    })
}

fn make_marker_entry(id: &str, ts: &str) -> TranscriptEntry {
    TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some(id.to_string()),
        parent_id: None,
        timestamp: ts.to_string(),
        summary: None,
        covered_start_id: Some("u1".to_string()),
        covered_end_id: Some("a1".to_string()),
        covered_count: Some(2),
        is_boundary: Some(true),
        preheat_compaction_id: Some(id.to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: None,
        attempts: None,
    })
}

fn make_summary_body(id: &str, summary: &str, ts: &str) -> TranscriptEntry {
    TranscriptEntry::BranchSummaryText(BranchSummaryTextEntry {
        id: Some(format!("{id}:text")),
        parent_id: Some(id.to_string()),
        timestamp: ts.to_string(),
        for_id: id.to_string(),
        summary: summary.to_string(),
    })
}

fn message_entry(id: &str, role: &str, content: &str, ts: &str) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        id: Some(id.to_string()),
        parent_id: None,
        timestamp: ts.to_string(),
        message: serde_json::json!({"role":role,"content":content}),
    })
}

#[test]
fn unfulfilled_marker_is_not_a_boundary_and_cannot_drop_messages() {
    let today = "2026-04-04T10:00:00Z";
    let entries = vec![
        message_entry("u1", "user", "original user input", today),
        message_entry("a1", "assistant", "original answer", today),
        make_marker_entry("marker-1", today),
        message_entry("u2", "user", "live tail", today),
    ];

    let folded = fold_entries_to_messages(&entries, 0);
    assert_eq!(
        folded
            .messages
            .iter()
            .filter_map(ChatMessage::text_content)
            .collect::<Vec<_>>(),
        ["original user input", "original answer", "live tail"],
    );
    let fold_start = compute_fold_start(
        &entries,
        chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap(),
        1,
    );
    assert_eq!(
        fold_start, 0,
        "a marker without a body is not a resume boundary"
    );
}

#[test]
fn tail_appended_summary_body_applies_at_its_earlier_marker_without_dropping_live_tail() {
    let ts = "2026-04-04T10:00:00Z";
    let entries = vec![
        message_entry("u1", "user", "old input", ts),
        message_entry("a1", "assistant", "old answer", ts),
        make_marker_entry("marker-1", ts),
        message_entry("u2", "user", "new live input", ts),
        make_summary_body("marker-1", "summary of old input", ts),
    ];

    let folded = fold_entries_to_messages(&entries, 0);
    assert_eq!(
        folded
            .messages
            .iter()
            .filter_map(ChatMessage::text_content)
            .collect::<Vec<_>>(),
        ["summary of old input", "new live input"],
    );
    assert_eq!(
        folded.messages[0].msg_id.as_deref(),
        Some("marker-1"),
        "the synthetic context message belongs to the marker, not the tail payload"
    );
}

#[test]
fn latest_completed_marker_wins_for_chained_compactions() {
    let ts = "2026-04-04T10:00:00Z";
    let entries = vec![
        message_entry("u1", "user", "old input", ts),
        make_marker_entry("marker-1", ts),
        make_summary_body("marker-1", "first summary", ts),
        message_entry("u2", "user", "more work", ts),
        make_marker_entry("marker-2", ts),
        make_summary_body("marker-2", "second summary", ts),
    ];

    let folded = fold_entries_to_messages(&entries, 0);
    assert_eq!(
        folded
            .messages
            .iter()
            .filter_map(ChatMessage::text_content)
            .collect::<Vec<_>>(),
        ["second summary"],
    );
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

fn make_test_msg(ts: &str) -> ChatMessage {
    let mut m = ChatMessage::user("q");
    m.timestamp = Some(ts.to_string());
    m
}

#[test]
fn filter_enough_today_no_backfill() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let mut messages = Vec::new();
    for _ in 0..5 {
        messages.push(make_test_msg("2026-04-03T10:00:00Z"));
    }
    for _ in 0..12 {
        messages.push(make_test_msg("2026-04-04T10:00:00Z"));
    }

    let selected = filter_turns_by_day(messages, today, 10);
    assert_eq!(selected.len(), 12, "today has 12 >= 10, no backfill needed");
    assert!(selected
        .iter()
        .all(|m| parse_date(m.timestamp.as_deref().unwrap_or("")) == Some(today)));
}

#[test]
fn filter_backfill_to_10() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let mut messages = Vec::new();
    for _ in 0..12 {
        messages.push(make_test_msg("2026-04-03T10:00:00Z"));
    }
    for _ in 0..3 {
        messages.push(make_test_msg("2026-04-04T10:00:00Z"));
    }

    let selected = filter_turns_by_day(messages, today, 10);
    assert_eq!(selected.len(), 10, "3 today + 7 backfill = 10");

    let today_count = selected
        .iter()
        .filter(|m| parse_date(m.timestamp.as_deref().unwrap_or("")) == Some(today))
        .count();
    assert_eq!(today_count, 3);
}

#[test]
fn filter_cross_midnight() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let messages: Vec<_> = (0..15)
        .map(|_| make_test_msg("2026-04-03T23:00:00Z"))
        .collect();

    let selected = filter_turns_by_day(messages, today, 10);
    assert_eq!(
        selected.len(),
        10,
        "no today turns, backfill 10 from yesterday"
    );
}

#[test]
fn filter_all_today_gt_10() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
    let messages: Vec<_> = (0..15)
        .map(|_| make_test_msg("2026-04-04T10:00:00Z"))
        .collect();

    let selected = filter_turns_by_day(messages, today, 10);
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
