use super::*;
use crate::core::permission::{PathRule, PathRuleMode};
use crate::core::project_instructions::InstructionKind;
use serial_test::serial;
use std::sync::Arc;

async fn wait_for_line(
    buffer: &SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let frames = read_ndjson_lines(buffer);
        if frames.iter().any(&predicate) {
            return frames;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "response timed out: {frames:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn instruction_catalog_resolution_snapshot_deny_and_steer() {
    let _api = install_test_api_key();
    let temp = tempfile::tempdir().unwrap();
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let (state, buffer, _temp, slot) = build_initialized_state_with_config(temp, cfg).await;
    let root = &slot.ctx.scope_services.resource_root;
    let commands = root.join(".cursor/commands");
    std::fs::create_dir_all(&commands).unwrap();
    let file = commands.join("review.md");
    std::fs::write(&file, "---\ndescription: Review changes\n---\nSNAPSHOT_OLD").unwrap();
    let id = "command:.cursor/commands/review.md";
    handle_command(
        Arc::clone(&state),
        ServeCommand::GetInstructionCatalog {
            id: Some("catalog".into()),
            session_id: Some(slot.session_id.clone()),
        },
    )
    .await
    .unwrap();
    let frames = wait_for_line(&buffer, |f| f["id"] == "catalog").await;
    let reply = frames.iter().find(|f| f["id"] == "catalog").unwrap();
    assert_eq!(reply["payload"]["items"][0]["id"], id);
    let params = ServeMessageParams {
        segments: vec![
            ServeContentSegment::Instruction {
                resource_id: id.into(),
                kind: InstructionKind::Command,
                label: "forged label".into(),
            },
            ServeContentSegment::Text {
                text: " intent".into(),
            },
            ServeContentSegment::Instruction {
                resource_id: id.into(),
                kind: InstructionKind::Command,
                label: "/review".into(),
            },
        ],
        ..Default::default()
    };
    let (archival, input) = build_turn_messages(&slot, String::new(), &params).unwrap();
    assert_eq!(
        serde_json::to_value(&archival).unwrap(),
        serde_json::to_value(&input).unwrap()
    );
    let old = serde_json::to_string(&archival).unwrap();
    assert_eq!(old.matches("SNAPSHOT_OLD").count(), 2);
    assert!(!old.contains("forged label"));
    std::fs::write(&file, "SNAPSHOT_NEW").unwrap();
    assert!(serde_json::to_string(
        &build_turn_messages(&slot, String::new(), &params)
            .unwrap()
            .0
    )
    .unwrap()
    .contains("SNAPSHOT_NEW"));
    assert!(!old.contains("SNAPSHOT_NEW"));
    slot.ctx.global_services.gate.grant_path_rule(PathRule {
        path: file.to_string_lossy().into(),
        mode: PathRuleMode::Deny,
    });
    assert!(build_turn_messages(&slot, String::new(), &params).is_err());
    let before = slot
        .ctx
        .session_runtime
        .session
        .get_entries(256)
        .unwrap()
        .len();
    for (request, command) in [("reject-prompt", false), ("reject-steer", true)] {
        let frame = if command {
            ServeCommand::Steer {
                id: Some(request.into()),
                session_id: Some(slot.session_id.clone()),
                text: "/review".into(),
                params: params.clone(),
            }
        } else {
            ServeCommand::Prompt {
                id: Some(request.into()),
                session_id: Some(slot.session_id.clone()),
                text: "/review".into(),
                params: params.clone(),
            }
        };
        handle_command(Arc::clone(&state), frame).await.unwrap();
        let frames = wait_for_line(&buffer, |f| f["id"] == request).await;
        assert_eq!(
            frames.iter().find(|f| f["id"] == request).unwrap()["success"],
            false
        );
    }
    assert_eq!(
        slot.ctx
            .session_runtime
            .session
            .get_entries(256)
            .unwrap()
            .len(),
        before
    );
    cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Close)
        .await
        .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn project_rules_refresh_without_tool_grants() {
    let _api = install_test_api_key();
    let temp = tempfile::tempdir().unwrap();
    let cfg = serve_test_config(temp.path(), "http://127.0.0.1:1");
    let (state, _, _temp, slot) = build_initialized_state_with_config(temp, cfg).await;
    let path = slot
        .ctx
        .scope_services
        .resource_root
        .join(".agents/rules/team.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "---\nalwaysApply: true\ndescription: team\n---\nRULE_ONE",
    )
    .unwrap();
    let mut snapshot = crate::api::chat::build_prompt_snapshot(&slot.ctx, 10000).await;
    assert!(snapshot.system_text().contains("RULE_ONE"));
    std::fs::write(&path, "---\nalwaysApply: true\n---\nRULE_TWO").unwrap();
    crate::api::chat::refresh_prompt_snapshot(&slot.ctx, 10000, &mut snapshot).await;
    assert!(snapshot.system_text().contains("RULE_TWO"));
    assert!(!snapshot.system_text().contains("RULE_ONE"));
    slot.ctx.global_services.gate.grant_path_rule(PathRule {
        path: path.to_string_lossy().into(),
        mode: PathRuleMode::Deny,
    });
    crate::api::chat::refresh_prompt_snapshot(&slot.ctx, 10000, &mut snapshot).await;
    assert!(!snapshot.system_text().contains("RULE_TWO"));
    cleanup_session_slot(&state, &slot, super::super::SlotCleanup::Close)
        .await
        .unwrap();
}
