use std::path::Path;

use super::super::file_store::{
    write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
};
use super::super::review::resolve_internal_tools;
use super::super::verify::{
    build_summary_from_outcome, build_verify_prompt, normalize_for_gate, parse_verify_block,
    verifier_allowed_tools_with_policy, verifier_system_prompt_text, VerifyCheck, VerifySummary,
    VERIFIER_ALLOWED_TOOLS, VERIFIER_MAX_TURNS,
};
use super::super::PlanRuntime;
use crate::core::agent_registry::SubagentOutcomeLabel;
use crate::core::llm::ChatMessage;
use crate::core::tools::contract::catalog::BUILTIN_TOOL_CATALOG;
use crate::{AgentRunOutcome, AgentRunResult};

#[test]
fn parse_verify_block_happy_path() {
    let text = r#"
noise
<verify>
checks:
  - name: unit
    command: cargo test -p tomcat verify
    result: pass
    output_excerpt: ok
verdict: pass
summary: all good
</verify>
"#;
    let summary = parse_verify_block(text).unwrap();
    assert_eq!(summary.verdict, "pass");
    assert_eq!(summary.summary, "all good");
    assert_eq!(summary.checks.len(), 1);
    assert_eq!(summary.checks[0].result, "pass");
}

#[test]
fn parse_verify_block_picks_last_block() {
    let text = r#"
<verify>
checks: []
verdict: partial
summary: old
</verify>
<verify>
checks: []
verdict: fail
summary: new
</verify>
"#;
    let summary = parse_verify_block(text).unwrap();
    assert_eq!(summary.verdict, "fail");
    assert_eq!(summary.summary, "new");
}

#[test]
fn parse_verify_block_rejects_unknown_verdict() {
    let text = r#"
<verify>
checks: []
verdict: maybe
summary: nope
</verify>
"#;
    assert!(parse_verify_block(text).is_none());
}

#[test]
fn parse_verify_block_preserves_multibyte_summary_without_panic() {
    let summary_text = "验".repeat(250);
    let text = format!(
        "<verify>\nchecks:\n  - name: unit\n    command: cargo test -p tomcat verify\n    result: pass\n    output_excerpt: ok\nverdict: pass\nsummary: {summary_text}\n</verify>"
    );
    let summary = parse_verify_block(&text).unwrap();
    assert_eq!(summary.summary, summary_text);
    assert_eq!(summary.summary.chars().count(), 250);
}

#[test]
fn parse_verify_block_preserves_multibyte_output_excerpt_without_panic() {
    let excerpt = "证".repeat(180);
    let text = format!(
        "<verify>\nchecks:\n  - name: unit\n    command: cargo test -p tomcat verify\n    result: pass\n    output_excerpt: {excerpt}\nverdict: pass\nsummary: short summary\n</verify>"
    );
    let summary = parse_verify_block(&text).unwrap();
    assert_eq!(summary.checks.len(), 1);
    assert_eq!(summary.checks[0].output_excerpt, excerpt);
    assert_eq!(summary.checks[0].output_excerpt.chars().count(), 180);
}

#[test]
fn verify_summary_round_trips_to_json() {
    let summary = VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit".into(),
            command: "cargo test".into(),
            result: "pass".into(),
            output_excerpt: "ok".into(),
        }],
        verdict: "pass".into(),
        summary: "looks good".into(),
        verifier_turns_used: 2,
        verifier_turns_limit: 64,
        verifier_stop_reason: "completed".into(),
        child_session_id: "child-1".into(),
    };
    let json = summary.to_json();
    assert_eq!(json["verdict"], "pass");
    assert_eq!(json["summary"], "looks good");
    assert_eq!(json["checks"][0]["command"], "cargo test");
    assert_eq!(json["verifier_turns_limit"], 64);
}

#[test]
fn normalize_for_gate_demotes_empty_command_pass_and_partializes_key_checks() {
    let mut summary = VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit test".into(),
            command: String::new(),
            result: "pass".into(),
            output_excerpt: "ok".into(),
        }],
        verdict: "pass".into(),
        summary: "claimed success".into(),
        ..Default::default()
    };

    let warnings = normalize_for_gate(&mut summary);

    assert_eq!(summary.checks[0].result, "skip");
    assert_eq!(summary.verdict, "partial");
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].contains("command 为空"));
    assert!(warnings[1].contains("降级为 partial"));
}

#[test]
fn build_summary_from_outcome_marks_turn_budget_cutoff_as_aborted() {
    let verify_block = r#"<verify>
checks:
  - name: smoke
    command: cargo test -p tomcat verifier
    result: pass
    output_excerpt: ok
verdict: pass
summary: model claimed success
</verify>"#;
    let mut new_messages = Vec::new();
    for idx in 0..63 {
        new_messages.push(ChatMessage::assistant(format!("turn {idx}")));
    }
    new_messages.push(ChatMessage::assistant_with_tool_calls(
        Some(verify_block),
        vec![serde_json::json!({
            "id": "call_64",
            "type": "function",
            "function": { "name": "bash", "arguments": "{}" }
        })],
    ));
    new_messages.push(ChatMessage::tool("call_64", "ok"));

    let (summary, label) = build_summary_from_outcome(
        "test",
        "child-1",
        VERIFIER_MAX_TURNS,
        AgentRunOutcome::Completed(AgentRunResult {
            final_text: verify_block.to_string(),
            new_messages,
        }),
    );

    assert_eq!(summary.verdict, "aborted");
    assert_eq!(summary.verifier_stop_reason, "max_turns");
    assert_eq!(summary.verifier_turns_used, VERIFIER_MAX_TURNS);
    assert!(summary.summary.contains("runtime override"));
    assert_eq!(label, SubagentOutcomeLabel::Failed);
}

#[test]
fn build_summary_from_outcome_keeps_long_multibyte_summary_when_budget_note_is_appended() {
    let body = "测".repeat(250);
    let verify_block = format!(
        "<verify>\nchecks:\n  - name: smoke\n    command: cargo test -p tomcat verifier\n    result: pass\n    output_excerpt: ok\nverdict: pass\nsummary: {body}\n</verify>"
    );
    let mut new_messages = Vec::new();
    for idx in 0..63 {
        new_messages.push(ChatMessage::assistant(format!("turn {idx}")));
    }
    new_messages.push(ChatMessage::assistant_with_tool_calls(
        Some(&verify_block),
        vec![serde_json::json!({
            "id": "call_64",
            "type": "function",
            "function": { "name": "bash", "arguments": "{}" }
        })],
    ));
    new_messages.push(ChatMessage::tool("call_64", "ok"));

    let (summary, label) = build_summary_from_outcome(
        "test",
        "child-3",
        VERIFIER_MAX_TURNS,
        AgentRunOutcome::Completed(AgentRunResult {
            final_text: verify_block,
            new_messages,
        }),
    );

    assert_eq!(summary.verdict, "aborted");
    assert_eq!(summary.verifier_stop_reason, "max_turns");
    assert!(summary.summary.contains(&body));
    assert!(summary.summary.contains("runtime override"));
    assert_eq!(label, SubagentOutcomeLabel::Failed);
}

#[test]
fn build_summary_from_outcome_keeps_pass_when_limit_is_used_exactly_and_normally() {
    let verify_block = r#"<verify>
checks:
  - name: smoke
    command: cargo test -p tomcat verifier
    result: pass
    output_excerpt: ok
verdict: pass
summary: model finished on the last turn
</verify>"#;
    let mut new_messages = Vec::new();
    for idx in 0..63 {
        new_messages.push(ChatMessage::assistant(format!("turn {idx}")));
    }
    new_messages.push(ChatMessage::assistant(verify_block));

    let (summary, label) = build_summary_from_outcome(
        "test",
        "child-2",
        VERIFIER_MAX_TURNS,
        AgentRunOutcome::Completed(AgentRunResult {
            final_text: verify_block.to_string(),
            new_messages,
        }),
    );

    assert_eq!(summary.verdict, "pass");
    assert_eq!(summary.verifier_stop_reason, "completed");
    assert_eq!(summary.verifier_turns_used, VERIFIER_MAX_TURNS);
    assert_eq!(summary.summary, "model finished on the last turn");
    assert_eq!(label, SubagentOutcomeLabel::Completed);
}

#[test]
fn verifier_system_prompt_contains_contract() {
    let prompt = verifier_system_prompt_text();
    assert!(prompt.contains("<verify>"));
    assert!(prompt.contains("pass|fail|partial|aborted"));
    assert!(prompt.contains("read, search_files, list_dir, bash"));
    assert!(prompt.contains("P0-P6"));
    assert!(prompt.contains("AGENTS.md"));
    assert!(prompt.contains("CLAUDE.md"));
    assert!(prompt.contains("label that check or summary as inferred"));
    assert!(prompt.contains("adversarial probe"));
}

#[test]
fn build_verify_prompt_mentions_discovery_order_and_inferred_rules() {
    let prompt = build_verify_prompt(
        "plan_demo",
        "## Goal\nship it\n",
        Path::new("/tmp/plan_demo.plan.md"),
        Some(Path::new("/tmp/workspace")),
    );
    assert!(prompt.contains("P0 plan body / brief / user note"));
    assert!(prompt.contains("P1 injected system context"));
    assert!(prompt.contains("P4 AGENTS.md or CLAUDE.md fallback"));
    assert!(prompt.contains("label the check or summary as inferred"));
    assert!(prompt.contains("Include at least one adversarial probe"));
}

#[test]
fn verifier_not_in_catalog() {
    for entry in BUILTIN_TOOL_CATALOG.iter() {
        assert_ne!(entry.name, "verifier");
        assert_ne!(entry.name, "verify");
    }
}

#[test]
fn verifier_allowed_tools_do_not_include_write_paths() {
    let tools = resolve_internal_tools(VERIFIER_ALLOWED_TOOLS);
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("read"));
    assert!(names.contains("search_files"));
    assert!(names.contains("list_dir"));
    assert!(names.contains("bash"));
    assert!(!names.contains("create_plan"));
    assert!(!names.contains("update_plan"));
    assert!(!names.contains("write"));
    assert!(!names.contains("edit"));
}

#[test]
fn verifier_can_expose_load_skill_when_config_enabled() {
    let tools = resolve_internal_tools(&verifier_allowed_tools_with_policy(true));
    let names: std::collections::BTreeSet<String> = tools
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("load_skill"));
}

#[test]
fn verifier_max_turns_default_is_64() {
    assert_eq!(VERIFIER_MAX_TURNS, 64);
}

#[test]
fn verify_prompt_uses_active_external_plan_path() {
    let workspace = tempfile::tempdir().unwrap();
    let external_dir = workspace.path().join("external");
    std::fs::create_dir_all(&external_dir).unwrap();
    let external_path = external_dir.join("custom.plan.md");
    write_plan(
        &external_path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: "external_path_plan".into(),
                goal: "goal".into(),
                state: PlanFileState::Planning,
                session_key: Some("sess".into()),
                session_id: Some("uuid".into()),
                created_at: "2026-05-24T00:00:00Z".into(),
                schema_version: 1,
                todos: vec![TodoItem {
                    id: "t1".into(),
                    content: "ship".into(),
                    status: TodoStatus::Pending,
                }],
                unknown: Default::default(),
            },
            body: "## Goal\nexternal\n".into(),
        },
        1000,
    )
    .unwrap();

    let runtime = PlanRuntime::new("sess");
    runtime
        .build_plan(&external_path.to_string_lossy(), Some("uuid-path".into()))
        .unwrap();
    let resolved = runtime.resolved_plan_path("external_path_plan").unwrap();
    let resolved_display = crate::infra::platform::format_home_path(&resolved);
    let prompt = build_verify_prompt("external_path_plan", "body", &resolved, None);
    assert_eq!(
        resolved,
        crate::normalize_path(&external_path.to_string_lossy()).unwrap()
    );
    assert!(prompt.contains(&resolved_display));
}
