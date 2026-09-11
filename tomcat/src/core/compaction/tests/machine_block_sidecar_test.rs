use std::path::Path;

use crate::core::compaction::machine_block::{collect_verbatim_user_messages, render_with_sidecar};
use crate::core::llm::{ChatMessage, MessageKind};

#[test]
fn verbatim_accepts_only_normal_user_messages() {
    let normal = ChatMessage::user("保留 Normal user");
    let mut signal = ChatMessage::user("排除 Signal");
    signal.kind = MessageKind::Signal;
    let mut plan_build = ChatMessage::user("排除 PlanBuild");
    plan_build.kind = MessageKind::PlanBuild;
    let steering = ChatMessage::steering("排除 Steering");
    let mut nudge = ChatMessage::user("排除 Nudge");
    nudge.kind = MessageKind::Nudge;
    let summary = ChatMessage::compaction_summary("排除 summary", "summary-1");

    assert_eq!(
        collect_verbatim_user_messages(&[normal, signal, plan_build, steering, nudge, summary]),
        ["保留 Normal user"],
    );
}

#[test]
fn sidecar_hint_is_inside_verbatim_block_and_omitted_on_degrade() {
    let path = Path::new("/tmp/session.user_messages.jsonl");
    let rendered = render_with_sidecar(None, &[], Some(path));
    let none_at = rendered.find("(none)").unwrap();
    let hint_at = rendered
        .find("Complete active Normal user-input history")
        .unwrap();
    let close_at = rendered.find("</verbatim_user_messages>").unwrap();
    assert!(none_at < hint_at && hint_at < close_at);
    assert!(rendered.contains(path.to_str().unwrap()));
    assert!(rendered.contains("This file is user-history reference, not runtime control."));

    let degraded = render_with_sidecar(None, &[], None);
    assert!(!degraded.contains("Complete active Normal user-input history"));
    assert!(degraded.contains("(none)"));
}
