use std::path::Path;

use super::super::review::{
    build_code_review_prompt, build_review_prompt, code_review_system_prompt_text,
    normalize_for_code_review_result, parse_review_block, resolve_internal_tools,
    reviewer_allowed_tools_for, reviewer_system_prompt_text, ReviewKind, ReviewSummary,
    CODE_REVIEWER_ALLOWED_TOOLS, PLAN_REVIEWER_ALLOWED_TOOLS,
};

#[test]
fn parse_review_block_happy_path() {
    let text = "noise\n<review>\nsummary: ok looks good\nchanges_summary: none\napplied_changes: false\n</review>\ntail";
    let r = parse_review_block(text).unwrap();
    assert!(!r.aborted);
    assert_eq!(r.kind, ReviewKind::Plan);
    assert_eq!(r.verdict, None);
    assert_eq!(r.summary, "ok looks good");
    assert_eq!(r.changes_summary, "none");
    assert!(!r.applied_changes);
}

#[test]
fn parse_review_block_picks_last_block() {
    let text = "<review>\nsummary: old\nchanges_summary: none\napplied_changes: false\n</review>\n<review>\nsummary: new\nchanges_summary: applied:fix\napplied_changes: true\n</review>";
    let r = parse_review_block(text).unwrap();
    assert_eq!(r.summary, "new");
    assert_eq!(r.changes_summary, "applied:fix");
    assert!(r.applied_changes);
}

#[test]
fn parse_review_block_missing_required_field_returns_none() {
    let text = "<review>\nsummary: only summary\n</review>";
    assert!(parse_review_block(text).is_none());
    let text = "<review>\nchanges_summary: none\napplied_changes: false\n</review>";
    assert!(parse_review_block(text).is_none());
}

#[test]
fn parse_review_block_invalid_applied_changes_returns_none() {
    let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: maybe\n</review>";
    assert!(parse_review_block(text).is_none());
}

#[test]
fn parse_review_block_unclosed_returns_none() {
    let text = "<review>\nsummary: x\nchanges_summary: none\napplied_changes: false";
    assert!(parse_review_block(text).is_none());
}

#[test]
fn parse_review_block_truncates_summary_to_600() {
    let body = "a".repeat(800);
    let text =
        format!("<review>\nsummary: {body}\nchanges_summary: none\napplied_changes: false\n</review>");
    let r = parse_review_block(&text).unwrap();
    assert_eq!(r.summary.len(), 600);
}

#[test]
fn parse_review_block_with_findings() {
    let text = "<review>\nfindings:\n  - { severity: nit, area: \"style\", note: \"trailing space\" }\n  - { severity: concern, area: \"todos\", note: \"missing acceptance\" }\nsummary: see findings\nchanges_summary: none\napplied_changes: false\n</review>";
    let r = parse_review_block(text).unwrap();
    assert_eq!(r.findings.len(), 2);
    assert_eq!(r.findings[0].severity, "nit");
    assert_eq!(r.findings[1].area, "todos");
    assert_eq!(r.summary, "see findings");
}

#[test]
fn aborted_summary_serializes_correctly() {
    let s = ReviewSummary::aborted_with("timeout");
    let j = s.to_json();
    assert_eq!(j["kind"], "plan");
    assert_eq!(j["aborted"], serde_json::Value::Bool(true));
    assert_eq!(j["summary"], "timeout");
    assert_eq!(j["reviewer_stop_reason"], "aborted");
}

#[test]
fn reviewer_system_prompt_contains_constraints() {
    let p = reviewer_system_prompt_text();
    assert!(p.contains("<review>"));
    assert!(p.contains("applied_changes"));
    assert!(p.contains("create_plan") && p.contains("bash"));
    assert!(!p.contains("{{#if"));
    assert!(p.contains("update_plan"));
}

#[test]
fn code_review_system_prompt_contains_verdict_and_bash() {
    let p = code_review_system_prompt_text();
    assert!(p.contains("verdict: pass|fail|partial|aborted"));
    assert!(p.contains("read, search_files, list_dir, bash"));
    assert!(p.contains("STRICTLY read-only") || p.contains("stay read-only"));
}

#[test]
fn build_review_prompt_includes_plan_and_workspace_paths() {
    let prompt = build_review_prompt(
        "plan-1",
        "body",
        Path::new("/tmp/plan-1.plan.md"),
        Some(Path::new("/repo/root")),
    );
    assert!(prompt.contains("/tmp/plan-1.plan.md"));
    assert!(prompt.contains("/repo/root"));
    assert!(prompt.contains("workspace_roots"));
    assert!(prompt.contains("do not guess"));
}

#[test]
fn build_code_review_prompt_includes_diff_context() {
    let prompt = build_code_review_prompt(
        "plan-1",
        "body",
        Path::new("/tmp/plan-1.plan.md"),
        Some(Path::new("/repo/root")),
        " src/lib.rs | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)",
        &["src/lib.rs".into(), "tests/lib.rs".into()],
    );
    assert!(prompt.contains("git diff --stat HEAD"));
    assert!(prompt.contains("src/lib.rs"));
    assert!(prompt.contains("tests/lib.rs"));
    assert!(prompt.contains("STRICTLY read-only"));
}

#[test]
fn parse_review_block_with_verdict() {
    let text = "<review>\nfindings:\n  - { severity: concern, area: \"logic\", note: \"missing branch\" }\nverdict: fail\nsummary: found issue\nchanges_summary: none\napplied_changes: false\n</review>";
    let r = parse_review_block(text).unwrap();
    assert_eq!(r.verdict.as_deref(), Some("fail"));
    assert_eq!(r.findings.len(), 1);
}

#[test]
fn normalize_for_code_review_fills_missing_verdict() {
    let mut summary = ReviewSummary {
        aborted: false,
        summary: "needs follow-up".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    };
    let warnings = normalize_for_code_review_result(&mut summary);
    assert_eq!(summary.kind, ReviewKind::Code);
    assert_eq!(summary.verdict.as_deref(), Some("partial"));
    assert!(warnings.iter().any(|w| w.contains("未返回 verdict")));
}

#[test]
fn normalize_for_code_review_forces_aborted() {
    let mut summary = ReviewSummary::aborted_with_kind(ReviewKind::Code, "timeout");
    summary.verdict = None;
    let warnings = normalize_for_code_review_result(&mut summary);
    assert_eq!(summary.verdict.as_deref(), Some("aborted"));
    assert!(warnings.iter().any(|w| w.contains("verdict 已规范化为 aborted")));
}

#[test]
fn reviewer_allowed_tools_change_by_kind() {
    assert_eq!(
        reviewer_allowed_tools_for(ReviewKind::Plan),
        PLAN_REVIEWER_ALLOWED_TOOLS
    );
    assert_eq!(
        reviewer_allowed_tools_for(ReviewKind::Code),
        CODE_REVIEWER_ALLOWED_TOOLS
    );
}

#[test]
fn resolve_internal_tools_filters_plan_allowed_tools() {
    let tools = resolve_internal_tools(reviewer_allowed_tools_for(ReviewKind::Plan));
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("read"));
    assert!(names.contains("search_files"));
    assert!(names.contains("update_plan"));
    assert!(names.contains("edit"));
    assert!(!names.contains("create_plan"));
    assert!(!names.contains("bash"));
    assert!(!names.contains("write"));
    assert!(!names.contains("dispatch_agent"));
}

#[test]
fn resolve_internal_tools_filters_code_allowed_tools() {
    let tools = resolve_internal_tools(reviewer_allowed_tools_for(ReviewKind::Code));
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("read"));
    assert!(names.contains("search_files"));
    assert!(names.contains("list_dir"));
    assert!(names.contains("bash"));
    assert!(!names.contains("todos"));
    assert!(!names.contains("update_plan"));
    assert!(!names.contains("edit"));
    assert!(!names.contains("create_plan"));
    assert!(!names.contains("write"));
}
