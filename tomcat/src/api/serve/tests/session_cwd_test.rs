//! Session cwd must reach both model context and the real tool execution path.
use super::*;
use crate::core::llm::StreamEvent;
use serial_test::serial;
use std::sync::{atomic::Ordering, Arc};

fn call(id: &str, name: &str, args: serde_json::Value) -> Vec<Result<StreamEvent, AppError>> {
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
}

#[tokio::test]
#[serial(env_lock)]
async fn session_cwd_reaches_model_and_tools() {
    let _key = install_test_api_key();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(project.path())
        .status()
        .unwrap()
        .success());
    let process_cwd = std::env::current_dir().unwrap();
    let mut cfg = serve_test_config(data.path(), "http://127.0.0.1:1");
    cfg.primitive.auto_confirm = true;
    let runtime = runtime::ServeRuntime::new(
        cfg.clone(),
        write_test_config(&cfg),
        build_shared_model_prefs(&cfg).unwrap(),
        true,
    )
    .unwrap();
    let mut cases = Vec::new();
    for label in ["A", "B"] {
        let cwd = project.path().join(label);
        std::fs::create_dir_all(cwd.join("nested")).unwrap();
        let cwd = crate::infra::platform::canonicalize(cwd).unwrap();
        std::fs::write(cwd.join("value.txt"), format!("before ONLY_{label}\n")).unwrap();
        let (provider, requests) = RecordingMockLlm::new(vec![
            call(
                "read",
                "read",
                serde_json::json!({"paths":[{"path":"value.txt"}]}),
            ),
            call(
                "edit",
                "edit",
                serde_json::json!({"path":"value.txt","edits":[{"old_content":"before","new_content":"after"}]}),
            ),
            call(
                "write",
                "write",
                serde_json::json!({"path":"created.txt","content":label}),
            ),
            call(
                "bash",
                "bash",
                serde_json::json!({"command":"pwd; printf bash > bash-marker", "cwd":""}),
            ),
            call(
                "relative-bash",
                "bash",
                serde_json::json!({"command":"pwd; printf child > child-marker", "cwd":"nested"}),
            ),
            call(
                "search",
                "search_files",
                serde_json::json!({"pattern":"ONLY_","output_mode":"content"}),
            ),
            call("list", "list_dir", serde_json::json!({"path":"."})),
            vec![
                Ok(StreamEvent::ContentDelta {
                    delta: "done".into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ],
        ]);
        let (writer, output) = spawn_buffered_writer(&cfg.serve);
        let connection = ServeState::connect(runtime.clone(), writer);
        connection.initialized.store(true, Ordering::SeqCst);
        let mut slot = create_session_slot(
            connection.clone(),
            NewSessionParams {
                cwd: Some(cwd.to_string_lossy().into()),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
        // Replace only the provider. Context construction, tools, gates and disk writes are real.
        let ctx = Arc::get_mut(&mut Arc::get_mut(&mut slot).unwrap().ctx).unwrap();
        ctx.global_services.llm_resolver = Arc::new(FixedResolver::new(
            Arc::new(provider),
            "gpt-5.4",
            ctx.global_services.model_catalog.snapshot(),
        ));
        *slot.controller.lock() = Some(connection.connection_id);
        runtime.registry.insert(slot.clone()).unwrap();
        register_slot_hooks(&connection, &slot);
        connection.set_active_session(&slot.session_id).unwrap();
        cases.push((label, cwd, connection, slot, output, requests));
    }
    assert_eq!(
        cases[0].3.ctx.session_runtime.session.current_session_key(),
        cases[1].3.ctx.session_runtime.session.current_session_key()
    );
    for (label, cwd, connection, slot, output, requests) in &cases {
        handle_command(
            connection.clone(),
            ServeCommand::Prompt {
                id: Some(format!("prompt-{label}")),
                session_id: Some(slot.session_id.clone()),
                text: "Exercise the workspace tools".into(),
                params: Default::default(),
            },
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !read_ndjson_lines(output)
                .iter()
                .any(|f| f["type"] == "agent_idle")
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(cwd.join("value.txt")).unwrap(),
            format!("after ONLY_{label}\n")
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("created.txt")).unwrap(),
            *label
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("bash-marker")).unwrap(),
            "bash"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("nested/child-marker")).unwrap(),
            "child"
        );
        let requests = requests.0.lock();
        assert_eq!(requests.len(), 8);
        let system = requests[0]
            .messages
            .iter()
            .filter(|m| m.role == crate::core::llm::ChatMessageRole::System)
            .filter_map(|m| m.first_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            system.contains(&format!("(agent_workspace_dir): {}", cwd.display())),
            "wrong workspace context: {system}"
        );
        assert!(system.contains("working directory of the current session"));
        assert!(!system.contains("when tomcat chat was launched"));
        let tools = requests
            .last()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.role == crate::core::llm::ChatMessageRole::Tool)
            .filter_map(|m| m.first_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(tools.contains(&format!("ONLY_{label}")));
        assert!(!tools.contains(if *label == "A" { "ONLY_B" } else { "ONLY_A" }));
        assert!(
            tools.contains(cwd.to_str().unwrap()),
            "pwd must report the session directory: {tools}"
        );
        assert_eq!(slot.cwd.as_deref(), cwd.to_str());
    }
    assert_eq!(
        std::env::current_dir().unwrap(),
        process_cwd,
        "never change process cwd for a session"
    );
    for (_, _, connection, slot, _, _) in cases {
        cleanup_session_slot(&connection, &slot, SlotCleanup::Close)
            .await
            .unwrap();
    }
}
