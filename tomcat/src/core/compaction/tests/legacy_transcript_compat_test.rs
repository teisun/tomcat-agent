//! T2-P0-002 Phase D 兼容性单测 —— `BranchSummaryEntry` 新增 `error` / `attempts`
//! 字段后旧 transcript JSONL 仍能正常反序列化、reload；新行成功路径不会写出多余字段。
//!
//! 设计：
//! - 旧行（无 `error` / `attempts` 字段）必须反序列化为 `None / None`，避免历史 session
//!   reload 直接 panic（`session::manager::context::fold_entries_to_messages` 会消费）。
//! - 成功 BranchSummary 序列化时凭 `#[serde(skip_serializing_if = "Option::is_none")]`
//!   保持紧凑，否则会污染 JSONL 行长度且让本计划之外的 entries 也带空字段。
//! - 失败 BranchSummary 序列化必须出现 `error` / `attempts`，方便运行期定位故障窗口。

use crate::core::session::transcript::{BranchSummaryEntry, TranscriptEntry};

#[test]
fn legacy_branch_summary_entry_without_error_attempts_deserializes() {
    // 模拟 T2-P0-002 之前生成的 BranchSummary JSONL 行（缺 error / attempts 两个字段）
    let legacy_json = r#"{"type":"branch_summary","id":"old_id","timestamp":"2025-01-01T00:00:00.000Z","summary":"legacy summary","coveredStartId":"a","coveredEndId":"b","coveredCount":2,"isBoundary":false,"preheatCompactionId":"old_id"}"#;
    let entry: TranscriptEntry =
        serde_json::from_str(legacy_json).expect("legacy 行必须能反序列化为 TranscriptEntry");
    let bs = match entry {
        TranscriptEntry::BranchSummary(b) => b,
        other => panic!("expected BranchSummary, got {other:?}"),
    };
    assert!(
        bs.error.is_none(),
        "旧 transcript 行没有 error 字段时 serde 必须默认为 None，实际 {:?}",
        bs.error,
    );
    assert!(
        bs.attempts.is_none(),
        "旧 transcript 行没有 attempts 字段时 serde 必须默认为 None，实际 {:?}",
        bs.attempts,
    );
    assert_eq!(bs.summary.as_deref(), Some("legacy summary"));
    assert_eq!(bs.is_boundary, Some(false));
    assert_eq!(bs.covered_count, Some(2));
}

#[test]
fn successful_branch_summary_serializes_without_error_or_attempts_fields() {
    let entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("ok_id".to_string()),
        parent_id: None,
        timestamp: "2026-04-26T00:00:00.000Z".to_string(),
        summary: Some("structured 9-section summary".to_string()),
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(5),
        is_boundary: Some(false),
        preheat_compaction_id: Some("ok_id".to_string()),
        estimated_covered_tokens_before: Some(10_000),
        estimated_summary_tokens: Some(2_000),
        estimated_tokens_saved: Some(8_000),
        error: None,
        attempts: None,
    });
    let json = serde_json::to_string(&entry).unwrap();
    assert!(
        !json.contains("\"error\""),
        "成功路径不应写出 error 字段，否则 JSONL 行会膨胀（`skip_serializing_if = Option::is_none`）",
    );
    assert!(
        !json.contains("\"attempts\""),
        "成功路径不应写出 attempts 字段，否则 JSONL 行会膨胀",
    );
    assert!(
        json.contains("\"summary\":\"structured 9-section summary\""),
        "成功路径必须保留 summary 字段：{json}",
    );
}

#[test]
fn failure_branch_summary_serializes_with_error_attempts_and_round_trips() {
    let entry = TranscriptEntry::BranchSummary(BranchSummaryEntry {
        id: Some("fail_id".to_string()),
        parent_id: None,
        timestamp: "2026-04-26T00:00:01.000Z".to_string(),
        summary: None,
        covered_start_id: Some("a".to_string()),
        covered_end_id: Some("b".to_string()),
        covered_count: Some(3),
        is_boundary: Some(false),
        preheat_compaction_id: Some("fail_id".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        error: Some("context_length_exceeded".to_string()),
        attempts: Some(3),
    });
    let json = serde_json::to_string(&entry).unwrap();
    assert!(
        json.contains("\"error\":\"context_length_exceeded\""),
        "失败路径必须写出 error 字段：{json}",
    );
    assert!(
        json.contains("\"attempts\":3"),
        "失败路径必须写出 attempts=3：{json}",
    );
    assert!(
        !json.contains("\"summary\""),
        "失败路径 summary == None，凭 skip_serializing_if 不应出现 summary 字段（reload 凭此跳过假摘要重建）：{json}",
    );

    // round-trip：反序列化回来应保留 error/attempts
    let back: TranscriptEntry = serde_json::from_str(&json).unwrap();
    let back_bs = match back {
        TranscriptEntry::BranchSummary(b) => b,
        other => panic!("expected BranchSummary after round-trip, got {other:?}"),
    };
    assert_eq!(back_bs.error.as_deref(), Some("context_length_exceeded"));
    assert_eq!(back_bs.attempts, Some(3));
    assert!(back_bs.summary.is_none());
}
