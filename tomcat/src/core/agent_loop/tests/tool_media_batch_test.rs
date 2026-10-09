//! Media follow-ups must be appended after the entire provider tool-call batch.
//!
//! OpenAI permits a user message only after every call in the preceding assistant
//! tool batch has a corresponding tool result. This test uses the production
//! read-image path plus a persisted session so it catches both in-memory and
//! transcript-chain regressions.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, AgentRunOutcome};
use crate::core::llm::{
    ChatMessage, ChatMessageContent, ChatMessageContentPart, ChatMessageRole, StreamEvent,
};
use crate::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};
use crate::core::session::{assert_active_tool_result_integrity, SessionManager};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::infra::{DefaultEventBus, PrimitiveConfig, TracingAuditRecorder};
use crate::AllowAllConfirmation;

use super::mocks::{test_binding, RecordingStreamLlmProvider};

fn file_primitive(root: &std::path::Path) -> Arc<dyn PrimitiveExecutor> {
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: root.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        gate,
    ))
}

#[tokio::test]
async fn file_baseline_dispatcher_rebuilds_for_keep_changes_only() {
    use crate::core::checkpoint::{file_baselines, session_files};
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "original").unwrap();
    let mut streams = Vec::new();
    for (index, content) in ["first", "second", "third", "fourth"]
        .into_iter()
        .enumerate()
    {
        for (name, args) in [
            ("read", serde_json::json!({"path":path})),
            (
                "write",
                serde_json::json!({"path":path,"content":content,"overwrite":true}),
            ),
        ] {
            streams.push(vec![
                Ok(StreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some(format!("{name}-{index}")),
                    name: Some(name.into()),
                    arguments_delta: Some(args.to_string()),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "tool_calls".into(),
                }),
            ]);
        }
        streams.push(vec![Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        })]);
    }
    let (provider, _) = RecordingStreamLlmProvider::new(streams);
    let sessions = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(sessions.path().into());
    manager
        .create_session(
            manager.current_session_key(),
            Some(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    let owner = manager
        .append_message(serde_json::json!({"role":"user","content":"continue editing"}))
        .unwrap();
    let mut user = ChatMessage::user("continue editing");
    user.msg_id = Some(owner.clone());
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        file_primitive(&root),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            message_append_sink: Some(Arc::new(manager.clone())),
            read_file_state: Arc::new(ReadFileState::new()),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_session_manager(manager.clone());
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    assert!(matches!(
        agent.run(vec![user.clone()]).await,
        AgentRunOutcome::Completed(_)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    let initial = agent.file_baselines.clone().unwrap();
    let kept = session_files::keep(&transcript, "s", &owner)
        .unwrap()
        .source_turn_id;
    std::fs::write(&path, "manual after Keep").unwrap();
    assert!(session_files::list(&transcript, "s")
        .unwrap()
        .files
        .is_empty());
    assert!(matches!(
        agent.run(vec![user.clone()]).await,
        AgentRunOutcome::Completed(_)
    ));
    let after_keep = agent.file_baselines.clone().unwrap();
    assert!(
        !Arc::ptr_eq(&initial, &after_keep),
        "same user id still needs a new Keep cycle"
    );
    assert_eq!(after_keep.message_id, owner);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    assert_eq!(
        session_files::baseline(&transcript, "s", &kept, path.to_str().unwrap())
            .unwrap()
            .text,
        "manual after Keep"
    );
    assert!(matches!(
        agent.run(vec![user.clone()]).await,
        AgentRunOutcome::Completed(_)
    ));
    assert!(
        Arc::ptr_eq(&after_keep, agent.file_baselines.as_ref().unwrap()),
        "unchanged cycle reuses the tracker"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "third");
    file_baselines::discard_keeps_after(&transcript, 0);
    assert!(matches!(
        agent.run(vec![user]).await,
        AgentRunOutcome::Completed(_)
    ));
    assert!(
        !Arc::ptr_eq(&after_keep, agent.file_baselines.as_ref().unwrap()),
        "removing Keep also invalidates the tracker"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fourth");
    assert_eq!(
        session_files::baseline(&transcript, "s", &owner, path.to_str().unwrap())
            .unwrap()
            .text,
        "original"
    );
    assert_eq!(
        file_baselines::read_rows(&file_baselines::session_dir(&transcript).join(&owner))
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn genuine_image_only_user_keeps_file_ownership() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let path = root.join("written.txt");
    let (provider, _) = RecordingStreamLlmProvider::new(vec![
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("write".into()),
                name: Some("write".into()),
                arguments_delta: Some(
                    serde_json::json!({"path":path,"content":"created"}).to_string(),
                ),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".into(),
            }),
        ],
        vec![Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        })],
    ]);
    let sessions = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(sessions.path().into());
    manager
        .create_session(
            manager.current_session_key(),
            Some(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    let mut user =
        ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::image_file_id("user-image").unwrap()
        ]);
    let owner = manager
        .append_message(serde_json::to_value(&user).unwrap())
        .unwrap();
    user.msg_id = Some(owner.clone());
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        file_primitive(&root),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            message_append_sink: Some(Arc::new(manager.clone())),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_session_manager(manager.clone());
    assert!(matches!(
        agent.run(vec![user]).await,
        AgentRunOutcome::Completed(_)
    ));
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    let files = crate::core::checkpoint::session_files::list(&transcript, "image-user").unwrap();
    assert_eq!(files.source_turn_id.as_deref(), Some(owner.as_str()));
    assert_eq!(files.files.len(), 1);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "created");
}

#[tokio::test]
async fn tool_media_follow_up_does_not_take_file_ownership() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let image = root_path.join("photo.png");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/llm_multimodal/sample_image.png"),
        &image,
    )
    .unwrap();
    // Keep the PNG valid while crossing the former 1 MiB upload preference.
    let mut image_bytes = std::fs::read(&image).unwrap();
    image_bytes.resize(1024 * 1024 + 17, 0);
    std::fs::write(&image, &image_bytes).unwrap();
    let before = root_path.join("before.txt");
    let after = root_path.join("after.txt");
    let call = |id: &str, name: &str, args: serde_json::Value| {
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some(id.into()),
                name: Some(name.into()),
                arguments_delta: Some(args.to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".into(),
            }),
        ]
    };
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        call(
            "before",
            "write",
            serde_json::json!({"path":before,"content":"before"}),
        ),
        call("image", "read", serde_json::json!({"path":image})),
        call(
            "after",
            "write",
            serde_json::json!({"path":after,"content":"after"}),
        ),
        vec![Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        })],
    ]);
    let sessions = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(sessions.path().to_path_buf());
    manager
        .create_session(
            manager.current_session_key(),
            Some(root_path.to_string_lossy().into_owned()),
        )
        .unwrap();
    let owner = manager
        .append_message(serde_json::json!({"role":"user","content":"write, inspect, write"}))
        .unwrap();
    let mut user = ChatMessage::user("write, inspect, write");
    user.msg_id = Some(owner.clone());
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        file_primitive(&root_path),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "media-ownership".into(),
            message_append_sink: Some(Arc::new(manager.clone())),
            read_file_state: Arc::new(ReadFileState::new()),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_session_manager(manager.clone());
    let outcome = agent.run(vec![user]).await;
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read_to_string(&before).unwrap(), "before");
    assert_eq!(std::fs::read_to_string(&after).unwrap(), "after");
    let transcript = manager.current_transcript_path().unwrap().unwrap();
    let dirs = std::fs::read_dir(crate::core::checkpoint::file_baselines::session_dir(
        &transcript,
    ))
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect::<Vec<_>>();
    assert_eq!(
        dirs,
        vec![owner.clone()],
        "media must not create a file owner"
    );
    let files =
        crate::core::checkpoint::session_files::list(&transcript, "media-ownership").unwrap();
    assert_eq!(files.source_turn_id.as_deref(), Some(owner.as_str()));
    assert_eq!(files.files.len(), 2);
    let requests = requests.0.lock().unwrap();
    let media = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool && m.tool_call_id.as_deref() == Some("image"))
        .expect("image belongs to its tool result");
    assert!(
        matches!(&media.content, Some(ChatMessageContent::Parts(parts)) if parts.iter().any(|p| matches!(p,ChatMessageContentPart::InputImage { .. })))
    );
    assert_eq!(
        requests
            .last()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.role == ChatMessageRole::User)
            .count(),
        1
    );
    let persisted = std::fs::read_to_string(&transcript).unwrap();
    let persisted_media = persisted
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["message"]["tool_call_id"] == "image")
        .collect::<Vec<_>>();
    assert_eq!(persisted_media.len(), 1);
    assert_eq!(persisted_media[0]["message"]["role"], "tool");
    assert!(!persisted.contains("image_b64"));
    let part = &persisted_media[0]["message"]["content"][1];
    assert_eq!(part["type"], "input_image_ref");
    let sha = part["blob_sha"].as_str().unwrap();
    assert_eq!(
        manager.attachment_store().get(sha).unwrap().unwrap(),
        std::fs::read(&image).unwrap()
    );
    let restarted = SessionManager::new(sessions.path().to_path_buf());
    let restored = crate::core::session::init_context_state(
        &restarted,
        &crate::infra::ContextConfig::default(),
        "sys",
    )
    .unwrap();
    let restored_tool = restored
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("image"))
        .unwrap();
    assert!(
        matches!(&restored_tool.content,Some(ChatMessageContent::Parts(parts)) if parts.iter().any(|p| matches!(p,ChatMessageContentPart::InputImage { .. })))
    );
}

#[tokio::test]
async fn tool_owned_media_subagent_uses_shared_blob_repository_without_session_manager() {
    use crate::core::agent_loop::SubagentType;
    use crate::core::session::subagent_transcript::open_subagent_transcript;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let image = root.join("photo.png");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/llm_multimodal/sample_image.png"),
        &image,
    )
    .unwrap();
    let sink = open_subagent_transcript(
        root.to_str().unwrap(),
        "child",
        SubagentType::Explorer,
        "mock",
        "parent",
    )
    .unwrap();
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![
        vec![
            Ok(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("image".into()),
                name: Some("read".into()),
                arguments_delta: Some(serde_json::json!({"path":image}).to_string()),
            }),
            Ok(StreamEvent::FinishReason {
                reason: "tool_calls".into(),
            }),
        ],
        vec![Ok(StreamEvent::FinishReason {
            reason: "stop".into(),
        })],
    ]);
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        file_primitive(&root),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "child".into(),
            agent_trail_dir: root.to_string_lossy().into_owned(),
            message_append_sink: Some(sink),
            ..Default::default()
        },
        CancellationToken::new(),
    );
    assert!(matches!(
        agent.run(vec![ChatMessage::user("inspect")]).await,
        AgentRunOutcome::Completed(_)
    ));
    let disk = std::fs::read_to_string(root.join("subagent-sessions/child.jsonl")).unwrap();
    assert!(!disk.contains("image_b64"));
    let tool = disk
        .lines()
        .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .find(|v| v["message"]["role"] == "tool")
        .unwrap();
    let sha = tool["message"]["content"][1]["blob_sha"].as_str().unwrap();
    let store = crate::core::session::attachments::AttachmentBlobStore::new(&root.join("sessions"));
    assert_eq!(
        store.get(sha).unwrap().unwrap(),
        std::fs::read(image).unwrap()
    );
    assert!(requests
        .0
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .messages
        .iter()
        .any(|m| m.role == ChatMessageRole::Tool
            && matches!(&m.content, Some(ChatMessageContent::Parts(_)))));
}

#[tokio::test]
async fn image_stays_with_its_tool_in_a_complete_batch() {
    let root = tempfile::tempdir().expect("temporary workspace");
    let image_path = root.path().join("evidence.png");
    let text_path = root.path().join("evidence.txt");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/llm_multimodal/sample_image.png"),
        &image_path,
    )
    .expect("copy PNG fixture");
    std::fs::write(&text_path, "second tool result\n").expect("write text fixture");

    let path_arguments =
        |path: &std::path::Path| serde_json::json!({ "path": path.to_string_lossy() }).to_string();
    let tool_batch = vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("read-image".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(path_arguments(&image_path)),
        }),
        Ok(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("read-text".to_string()),
            name: Some("read".to_string()),
            arguments_delta: Some(path_arguments(&text_path)),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ];
    let final_reply = vec![
        Ok(StreamEvent::ContentDelta {
            delta: "batch complete".to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ];
    let (provider, requests) = RecordingStreamLlmProvider::new(vec![tool_batch, final_reply]);

    let session_root = tempfile::tempdir().expect("temporary session root");
    let manager = SessionManager::new(session_root.path().to_path_buf());
    manager
        .create_session(manager.current_session_key(), None)
        .expect("create session");
    let mut agent = AgentLoop::new(
        test_binding(Arc::new(provider), "gpt-4"),
        file_primitive(root.path()),
        Arc::new(DefaultEventBus::new()),
        AgentLoopConfig {
            session_id: "media-batch".to_string(),
            message_append_sink: Some(Arc::new(manager.clone())),
            read_file_state: Arc::new(ReadFileState::new()),
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .with_session_manager(manager.clone());

    let outcome = agent.run(vec![ChatMessage::user("read both files")]).await;
    assert!(
        matches!(outcome, AgentRunOutcome::Completed(_)),
        "image follow-up must not break the tool-call chain: {outcome:?}"
    );

    let requests = requests.0.lock().expect("recorded requests");
    assert_eq!(
        requests.len(),
        2,
        "the batch must lead to a second model turn"
    );
    let second_turn = &requests[1].messages;
    let tool_indexes = second_turn
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == ChatMessageRole::Tool).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(tool_indexes.len(), 2, "both tool results must be present");
    let image_follow_up_index = second_turn
        .iter()
        .enumerate()
        .find_map(|(index, message)| {
            matches!(
                &message.content,
                Some(ChatMessageContent::Parts(parts))
                    if parts.iter().any(|part| matches!(part, ChatMessageContentPart::InputImage { .. }))
            )
            .then_some(index)
        })
        .expect("second turn must include the PNG as an InputImage");
    assert_eq!(image_follow_up_index, tool_indexes[0]);
    assert_eq!(
        second_turn[image_follow_up_index].role,
        ChatMessageRole::Tool
    );
    assert_eq!(
        second_turn[image_follow_up_index].tool_call_id.as_deref(),
        Some("read-image")
    );
    assert_eq!(
        second_turn
            .iter()
            .filter(|m| m.role == ChatMessageRole::User)
            .count(),
        1
    );
    assert_active_tool_result_integrity(&manager.get_entries(32).expect("read transcript"))
        .expect("persisted transcript must contain one result for every tool call");
}
