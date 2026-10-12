use super::*;
use serial_test::serial;

async fn file_response(buffer: &SharedWriterBuffer, id: &str) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(frame) = read_ndjson_lines(buffer)
                .into_iter()
                .find(|v| v["id"] == id)
            {
                return frame;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

fn write_files_plan_fixture(path: &std::path::Path, plan_id: &str, timeout: u64) {
    use crate::core::plan_runtime::file_store::{
        write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
        PLAN_FILE_SCHEMA_VERSION,
    };
    write_plan(
        path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: plan_id.into(),
                goal: "files accumulate".into(),
                state: PlanFileState::Planning,
                session_key: None,
                session_id: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                schema_version: PLAN_FILE_SCHEMA_VERSION,
                todos: vec![TodoItem {
                    id: "work".into(),
                    content: "implement".into(),
                    status: TodoStatus::Pending,
                    evidence: vec![],
                    kind: Default::default(),
                }],
                code_review_pass: false,
                code_review_rounds: 0,
                code_review_open_findings: vec![],
                code_review_disputed_findings: vec![],
                code_review_handoff: false,
                code_review_handoff_acknowledged: false,
                code_review_residual_findings: vec![],
                unknown: serde_yaml::Mapping::new(),
            },
            body: "## Plan\nimplement".into(),
        },
        timeout,
    )
    .unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn session_files_chat_only_rewind_then_real_build_recaptures_old_owner() {
    use crate::api::serve::test_support::{
        build_initialized_state_with_provider, serve_test_config, DeterministicMockLlm,
    };
    use crate::core::checkpoint::{file_baselines, session_files};
    use crate::core::llm::StreamEvent;
    use std::sync::Arc;
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    for with_keep in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let a = root.join("a.txt");
        let c = root.join("c.txt");
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
        let provider = Arc::new(DeterministicMockLlm::new(vec![
            call("read-a", "read", serde_json::json!({"path":a})),
            call(
                "rewrite-a",
                "write",
                serde_json::json!({"path":a,"content":"v3","overwrite":true}),
            ),
            call(
                "write-c",
                "write",
                serde_json::json!({"path":c,"content":"new"}),
            ),
            vec![Ok(StreamEvent::FinishReason {
                reason: "stop".into(),
            })],
        ]));
        let mut cfg = serve_test_config(&root, "http://127.0.0.1:1");
        cfg.workspace.workspace_roots = vec![root.to_string_lossy().into_owned()];
        cfg.primitive.auto_confirm = true;
        let (state, buffer, _temp, slot) =
            build_initialized_state_with_provider(temp, cfg, provider).await;
        let session = &slot.ctx.session_runtime.session;
        let transcript = session.transcript_path(&slot.session_id);
        let cwd = file_baselines::session_cwd(&transcript).unwrap();
        let publish = |owner: &str, path: &std::path::Path, value: &str| {
            let tracker =
                file_baselines::TurnFileBaselines::new(&transcript, owner, cwd.clone()).unwrap();
            let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
            pending.commit();
        };
        std::fs::write(&a, "v0").unwrap();
        let u1 = session
            .append_message(serde_json::json!({"role":"user","content":"first edits"}))
            .unwrap();
        publish(&u1, &a, "v1");
        let anchor = session
            .append_message(serde_json::json!({"role":"assistant","content":"first done"}))
            .unwrap();
        let u2 = session
            .append_message(serde_json::json!({"role":"user","content":"continue"}))
            .unwrap();
        publish(&u2, &a, "v2");
        publish(&u2, &root.join("b.txt"), "b");
        let source = if with_keep {
            session_files::keep(&transcript, &slot.session_id, &u1)
                .unwrap()
                .source_turn_id
        } else {
            u1.clone()
        };
        // The production transcript-only checkpoint finalization operation keeps disk and backups.
        session
            .mark_messages_after_anchor_superseded(&anchor)
            .unwrap();
        crate::api::serve::commands::rehydrate_slot_context_state(&slot).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "v2");
        assert!(file_baselines::session_dir(&transcript).join(&u2).exists());
        let plan = root.join("old-owner.plan.md");
        write_files_plan_fixture(
            &plan,
            "old-owner",
            slot.ctx.session_runtime.plan_runtime.lock_timeout_ms(),
        );
        handle_command(
            state.clone(),
            ServeCommand::SetPlanMode {
                id: Some("reactivated-build".into()),
                session_id: Some(slot.session_id.clone()),
                action: SetPlanModeAction::Build,
                plan_id: Some(plan.to_string_lossy().into_owned()),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            file_response(&buffer, "reactivated-build").await["success"],
            true
        );
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if session_files::list(&transcript, &slot.session_id)
                    .is_ok_and(|v| v.files.iter().any(|f| f.path == c.to_string_lossy()))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real native write must publish c before interruption");
        crate::api::serve::control::handle_control_or_interrupt(
            state.clone(),
            ServeCommand::Interrupt {
                id: Some("stop-reactivated".into()),
                session_id: Some(slot.session_id.clone()),
            },
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while slot.is_busy() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let view = session_files::list(&transcript, &slot.session_id).unwrap();
        assert_eq!(view.source_turn_id.as_deref(), Some(source.as_str()));
        assert_eq!(view.files.len(), if with_keep { 2 } else { 3 });
        assert_eq!(
            session_files::baseline(&transcript, &slot.session_id, &source, a.to_str().unwrap())
                .unwrap()
                .text,
            if with_keep { "v2" } else { "v0" }
        );
        let rows =
            file_baselines::read_rows(&file_baselines::session_dir(&transcript).join(&u1)).unwrap();
        assert!(
            rows.iter().any(|row| row.path == c),
            "Build must really reuse the earlier owner"
        );
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn session_files_build_resume_continue_never_reset() {
    use crate::core::checkpoint::file_baselines::{session_cwd, session_dir, TurnFileBaselines};
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let transcript = session.transcript_path(&slot.session_id);
    let first = session
        .append_message(serde_json::json!({"role":"user","content":"implement"}))
        .unwrap();
    let mut owner = first.clone();
    let mut paths = Vec::new();
    for (index, name) in ["a", "b", "c", "d"].iter().enumerate() {
        if index == 1 {
            owner = session
                .append_message(serde_json::json!({"role":"user","content":"continue"}))
                .unwrap();
        } else {
            let plan_id = if index == 3 { "files-p2" } else { "files-p1" };
            let path = temp.path().join(format!("{plan_id}.plan.md"));
            if index != 2 {
                write_files_plan_fixture(
                    &path,
                    plan_id,
                    slot.ctx.session_runtime.plan_runtime.lock_timeout_ms(),
                );
            }
            let id = format!("build-{index}");
            handle_command(
                state.clone(),
                ServeCommand::SetPlanMode {
                    id: Some(id.clone()),
                    session_id: Some(slot.session_id.clone()),
                    action: SetPlanModeAction::Build,
                    plan_id: Some(path.to_string_lossy().into_owned()),
                },
            )
            .await
            .unwrap();
            let response = file_response(&buffer, &id).await;
            assert_eq!(response["success"], true, "{response}");
        }
        let path = temp
            .path()
            .canonicalize()
            .unwrap()
            .join(format!("{name}.txt"));
        let tracker =
            TurnFileBaselines::new(&transcript, &owner, session_cwd(&transcript).unwrap()).unwrap();
        let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
        std::fs::write(&path, name).unwrap();
        pending.commit();
        std::fs::File::open(
            session_dir(&transcript)
                .join(&owner)
                .join("baselines.jsonl"),
        )
        .unwrap()
        .set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + index as u64),
        )
        .unwrap();
        paths.push(path.to_string_lossy().into_owned());
        if index != 1 {
            crate::api::serve::control::handle_control_or_interrupt(
                state.clone(),
                ServeCommand::Interrupt {
                    id: Some(format!("stop-{index}")),
                    session_id: Some(slot.session_id.clone()),
                },
            )
            .await
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                while slot.is_busy() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            // The empty model may finish before Interrupt reaches the loop; use the same
            // idempotent lifecycle transition to arrange the Pending/Resume fixture.
            slot.ctx
                .session_runtime
                .plan_runtime
                .park_executing_plan()
                .unwrap();
        }
    }
    handle_command(
        state,
        ServeCommand::GetSessionFiles {
            id: Some("accumulated".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let response = file_response(&buffer, "accumulated").await;
    assert_eq!(
        response["payload"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        paths
    );
    assert_eq!(response["payload"]["sourceTurnId"], first);
}

#[tokio::test]
#[serial(env_lock)]
async fn session_file_rpc_busy_read_restore_and_chat_preservation() {
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let owner = session
        .append_message(serde_json::json!({"role":"user","content":"edit"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let path = temp.path().canonicalize().unwrap().join("target.txt");
    std::fs::write(&path, "before").unwrap();
    let cwd = crate::core::checkpoint::file_baselines::session_cwd(&transcript).unwrap();
    let tracker =
        crate::core::checkpoint::file_baselines::TurnFileBaselines::new(&transcript, &owner, cwd)
            .unwrap();
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    std::fs::write(&path, "after").unwrap();
    pending.commit();
    let before = std::fs::read(&transcript).unwrap();
    slot.mark_busy();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("list".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let response = file_response(&buffer, "list").await;
    assert_eq!(response["success"], true);
    assert_eq!(response["payload"]["sourceTurnId"], owner);
    let paths = vec![path.to_string_lossy().into_owned()];
    let command = |id: &str| ServeCommand::RestoreSessionFiles {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
        source_turn_id: owner.clone(),
        paths: paths.clone(),
    };
    handle_command(state.clone(), command("busy"))
        .await
        .unwrap();
    assert_eq!(file_response(&buffer, "busy").await["error"], "busy");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
    slot.mark_idle();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFileBaseline {
            id: Some("base".into()),
            session_id: slot.session_id.clone(),
            source_turn_id: owner.clone(),
            path: paths[0].clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        file_response(&buffer, "base").await["payload"]["text"],
        "before"
    );
    handle_command(state, command("restore")).await.unwrap();
    let response = file_response(&buffer, "restore").await;
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .all(|v| v["type"] != "agent_start"));
}

#[tokio::test]
#[serial(env_lock)]
async fn session_file_real_history_rewrite_reselects_source() {
    use crate::core::checkpoint::file_baselines::{session_cwd, session_dir, TurnFileBaselines};
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let transcript = session.transcript_path(&slot.session_id);
    let root = session_dir(&transcript);
    let publish = |owner: &str, name: &str, tick: u64| {
        let path = temp.path().canonicalize().unwrap().join(name);
        let tracker =
            TurnFileBaselines::new(&transcript, owner, session_cwd(&transcript).unwrap()).unwrap();
        let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
        std::fs::write(&path, "changed").unwrap();
        pending.commit();
        std::fs::File::open(root.join(owner).join("baselines.jsonl"))
            .unwrap()
            .set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick),
            )
            .unwrap();
        path.to_string_lossy().into_owned()
    };
    let u1 = session
        .append_message(serde_json::json!({"role":"user","content":"change a"}))
        .unwrap();
    let a = publish(&u1, "a.txt", 1);
    let anchor = session
        .append_message(serde_json::json!({"role":"assistant","content":"a changed"}))
        .unwrap();
    let u2 = session
        .append_message(serde_json::json!({"role":"user","content":"change b"}))
        .unwrap();
    let b = publish(&u2, "b.txt", 2);
    session
        .append_message(
            serde_json::json!({"role":"assistant","content":"partial b result","turn_failed":true}),
        )
        .unwrap();

    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("before-retry".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let before = file_response(&buffer, "before-retry").await;
    assert_eq!(before["payload"]["sourceTurnId"], u1);
    assert_eq!(before["payload"]["files"].as_array().unwrap().len(), 2);

    // Copy-forward archives chat, not the still-modified files' backups.
    let retry = session.copy_user_message_forward(&u2).unwrap();
    assert!(root.join(&u2).exists());
    assert!(root.join(&u1).exists());
    assert!(!root.join(&retry).join("baselines.jsonl").exists());
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("after-retry".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-retry").await;
    assert_eq!(after["payload"]["sourceTurnId"], u1);
    assert_eq!(after["payload"]["files"].as_array().unwrap().len(), 2);
    assert_eq!(after["payload"]["files"][0]["path"], a);

    session
        .append_message(serde_json::json!({"role":"assistant","content":"retried without edits"}))
        .unwrap();
    let u3 = session
        .append_message(serde_json::json!({"role":"user","content":"change c"}))
        .unwrap();
    let c = publish(&u3, "c.txt", 3);
    session
        .append_message(serde_json::json!({"role":"assistant","content":"c changed"}))
        .unwrap();
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("before-checkpoint".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let before = file_response(&buffer, "before-checkpoint").await;
    assert_eq!(before["payload"]["sourceTurnId"], u1);
    assert_eq!(before["payload"]["files"].as_array().unwrap().len(), 3);

    // These are the real checkpoint finalization operations, not handmade superseded JSON.
    session
        .mark_messages_after_anchor_superseded(&anchor)
        .unwrap();
    assert!(root.join(&u3).exists());
    assert!(root.join(&u1).exists());
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFiles {
            id: Some("after-checkpoint".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-checkpoint").await;
    assert_eq!(after["payload"]["sourceTurnId"], u1);
    assert_eq!(after["payload"]["files"].as_array().unwrap().len(), 3);
    assert!(after["payload"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == c));

    // Editing the first input must discard its directory and all remaining live suffix turns.
    session
        .rewind_user_message(
            &u1,
            serde_json::json!({"role":"user","content":"edited request"}),
        )
        .unwrap();
    assert!(!root.join(&u1).exists());
    handle_command(
        state,
        ServeCommand::GetSessionFiles {
            id: Some("after-edit".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    let after = file_response(&buffer, "after-edit").await;
    assert_eq!(after["payload"]["sourceTurnId"], serde_json::Value::Null);
    assert_eq!(after["payload"]["files"], serde_json::json!([]));
    // History rewrites, not Files queries, changed history; none of these metadata reads wrote files.
    assert_eq!(std::fs::read_to_string(a).unwrap(), "changed");
    assert_eq!(std::fs::read_to_string(b).unwrap(), "changed");
    assert_eq!(std::fs::read_to_string(c).unwrap(), "changed");
    assert!(read_ndjson_lines(&buffer)
        .iter()
        .all(|v| v["type"] != "agent_start"));
}

#[tokio::test]
#[serial(env_lock)]
async fn session_files_keep_rpc_busy_then_accepts() {
    use crate::core::checkpoint::file_baselines::{session_cwd, TurnFileBaselines};
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let session = &slot.ctx.session_runtime.session;
    let owner = session
        .append_message(serde_json::json!({"role":"user","content":"edit"}))
        .unwrap();
    let transcript = session.transcript_path(&slot.session_id);
    let path = temp.path().canonicalize().unwrap().join("kept.txt");
    let tracker =
        TurnFileBaselines::new(&transcript, &owner, session_cwd(&transcript).unwrap()).unwrap();
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    std::fs::write(&path, "accepted").unwrap();
    pending.commit();
    let request = |id: &str| ServeCommand::KeepSessionFiles {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
        source_turn_id: owner.clone(),
    };
    slot.mark_busy();
    handle_command(state.clone(), request("busy-keep"))
        .await
        .unwrap();
    assert_eq!(file_response(&buffer, "busy-keep").await["error"], "busy");
    slot.mark_idle();
    let before = std::fs::read(&transcript).unwrap();
    handle_command(state.clone(), request("keep"))
        .await
        .unwrap();
    let kept = file_response(&buffer, "keep").await;
    assert_eq!(kept["success"], true, "{kept}");
    assert!(kept["payload"]["sourceTurnId"]
        .as_str()
        .unwrap()
        .starts_with("keep-"));
    handle_command(
        state,
        ServeCommand::GetSessionFiles {
            id: Some("kept-files".into()),
            session_id: slot.session_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        file_response(&buffer, "kept-files").await["payload"]["files"],
        serde_json::json!([])
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "accepted");
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
}

#[tokio::test]
#[serial(env_lock)]
async fn session_files_rpc_commit_accepts_then_next_edit_uses_committed_baseline() {
    use crate::core::checkpoint::file_baselines::TurnFileBaselines;
    let _key = EnvGuard::set(TEST_API_KEY_ENV, "stub");
    let (state, buffer, temp, slot) = build_initialized_state_with_streams(vec![]).await;
    let root = temp.path().canonicalize().unwrap();
    let session = &slot.ctx.session_runtime.session;
    let transcript = session.transcript_path(&slot.session_id);
    let mut lines = std::fs::read_to_string(&transcript)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header["cwd"] = serde_json::json!(root);
    lines[0] = header.to_string();
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-q"]);
    let path = root.join("a.txt");
    std::fs::write(&path, "v0").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "initial"]);
    let owner = session
        .append_message(serde_json::json!({"role":"user","content":"edit a"}))
        .unwrap();
    let tracker = TurnFileBaselines::new(&transcript, &owner, root.clone()).unwrap();
    let pending = tracker.prepare(path.to_str().unwrap()).unwrap();
    std::fs::write(&path, "v1").unwrap();
    pending.commit();
    let query = |id: &str| ServeCommand::GetSessionFiles {
        id: Some(id.into()),
        session_id: slot.session_id.clone(),
    };
    handle_command(state.clone(), query("before-commit"))
        .await
        .unwrap();
    assert_eq!(
        file_response(&buffer, "before-commit").await["payload"]["files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "accepted"]);
    handle_command(state.clone(), query("after-commit"))
        .await
        .unwrap();
    assert_eq!(
        file_response(&buffer, "after-commit").await["payload"]["files"],
        serde_json::json!([])
    );
    std::fs::write(&path, "v2").unwrap();
    handle_command(state.clone(), query("after-edit"))
        .await
        .unwrap();
    let edited = file_response(&buffer, "after-edit").await;
    assert_eq!(edited["payload"]["sourceTurnId"], owner);
    assert_eq!(edited["payload"]["files"].as_array().unwrap().len(), 1);
    handle_command(
        state.clone(),
        ServeCommand::GetSessionFileBaseline {
            id: Some("committed-baseline".into()),
            session_id: slot.session_id.clone(),
            source_turn_id: owner.clone(),
            path: path.to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        file_response(&buffer, "committed-baseline").await["payload"]["text"],
        "v1"
    );
    handle_command(
        state,
        ServeCommand::RestoreSessionFiles {
            id: Some("restore-committed".into()),
            session_id: slot.session_id.clone(),
            source_turn_id: owner,
            paths: vec![path.to_string_lossy().into_owned()],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        file_response(&buffer, "restore-committed").await["success"],
        true
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "v1");
}

#[test]
fn session_file_schema_and_command_ids_are_exported() {
    let dts = serve_dts();
    for name in [
        "SessionFilesResponse",
        "SessionFileBaselineResponse",
        "SessionFilesRestoreResponse",
        "SessionFilesKeepResponse",
    ] {
        assert!(dts.contains(name));
    }
    let command: ServeCommand = serde_json::from_value(
        serde_json::json!({"type":"get_session_files","id":"f","sessionId":"s"}),
    )
    .unwrap();
    assert_eq!(command.command_id(), Some("f"));
    assert_eq!(command.session_id(), Some("s"));
    assert_eq!(command.wire_type(), "get_session_files");
    let keep: ServeCommand = serde_json::from_value(serde_json::json!({
        "type":"keep_session_files","id":"k","sessionId":"s","sourceTurnId":"scope"
    }))
    .unwrap();
    assert_eq!(keep.command_id(), Some("k"));
    assert_eq!(keep.session_id(), Some("s"));
    assert_eq!(keep.wire_type(), "keep_session_files");
}
