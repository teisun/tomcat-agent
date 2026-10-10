use std::sync::Arc;

use serde_json::json;

use super::super::*;
use crate::core::agent_loop::types::SubagentType;
use crate::core::agent_loop::ConfigBackend;

struct DisplayPrimitive;

fn sample_diff() -> Vec<crate::core::tools::primitive::FileDiffLine> {
    vec![
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(1),
            skipped_lines: None,
            text: "hello".to_string(),
        },
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(2),
            skipped_lines: None,
            text: "world".to_string(),
        },
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(3),
            skipped_lines: None,
            text: "!".to_string(),
        },
    ]
}

#[async_trait::async_trait]
impl PrimitiveExecutor for DisplayPrimitive {
    async fn read(
        &self,
        _path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
        _line_numbers: bool,
        _hashline: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::ReadResult, AppError> {
        unreachable!()
    }

    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok("hello\nworld\n!\n".to_string())
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        unreachable!()
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: "~/workspace/demo.txt".to_string(),
            written: true,
            bytes_written: 12,
            diff_hint: None,
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
    }

    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<crate::core::tools::primitive::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        Ok(crate::core::tools::primitive::EditFileResult {
            path: path.to_string(),
            applied: true,
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _foreground_wait_ms: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }

    async fn hashline_edit(
        &self,
        path: &str,
        _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        Ok(crate::core::tools::primitive::EditFileResult {
            path: path.to_string(),
            applied: !path.ends_with("reject.txt"),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
    }

    async fn search_files(
        &self,
        _args: crate::core::tools::primitive::SearchFilesArgs,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::SearchFilesOutput, AppError> {
        unreachable!()
    }

    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

const CONFIG_RESPONSE_MESSAGE: &str = "已设置 llm.default_model = gpt-5.4";

struct DisplayConfigBackend;

#[async_trait::async_trait]
impl ConfigBackend for DisplayConfigBackend {
    async fn config_get(&self, _key: &str) -> Result<serde_json::Value, AppError> {
        unreachable!()
    }

    async fn config_set(&self, _key: &str, _value: &str) -> Result<serde_json::Value, AppError> {
        Ok(json!({
            "applied": true,
            "message": CONFIG_RESPONSE_MESSAGE
        }))
    }
}

#[tokio::test]
async fn native_write_display_baseline_registration_matrix() {
    use crate::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};
    use crate::core::tools::primitive::compute_line_hash;
    for name in ["write", "edit", "hashline_edit", "batch_edit"] {
        for decision in ["allow", "deny", "cancel"] {
            let temp = tempfile::tempdir().unwrap();
            let cwd = temp.path().canonicalize().unwrap();
            let path = cwd.join("a.txt");
            std::fs::write(&path, "original\n").unwrap();
            let transcript = cwd.join("s.jsonl");
            let tracker = crate::core::checkpoint::file_baselines::TurnFileBaselines::new(
                &transcript,
                "u",
                cwd.clone(),
            )
            .unwrap();
            let gate = DefaultPermissionGate::new(
                GateConfig {
                    agent_definition_dir: if decision == "deny" {
                        std::path::PathBuf::from("/nonexistent_pi_workspace")
                    } else {
                        cwd.clone()
                    },
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
            let confirm: Arc<dyn crate::core::UserConfirmationProvider> = if decision == "deny" {
                Arc::new(crate::core::DenyAllConfirmation)
            } else {
                Arc::new(crate::core::AllowAllConfirmation)
            };
            let primitive: Arc<dyn PrimitiveExecutor> =
                Arc::new(crate::core::DefaultPrimitiveExecutor::new(
                    crate::infra::PrimitiveConfig::default(),
                    confirm,
                    Arc::new(crate::infra::TracingAuditRecorder),
                    gate,
                ));
            let cancel = tokio_util::sync::CancellationToken::new();
            if decision == "cancel" {
                cancel.cancel();
            }
            let ctx = ToolExecCtx {
                primitive: &primitive,
                session_id: "baseline-test",
                tool_call_id: "call",
                config_backend: &None,
                package_install_backend: &None,
                bash_task_registry: &None,
                read_file_state: None,
                openai_files_runtime: None,
                web_fetch_runtime: None,
                web_search_runtime: None,
                todos_runtime: None,
                plan_runtime: None,
                skill_set: None,
                connector_registry: None,
                plugin_engine_config: None,
                subagent_type: SubagentType::User,
                expose_skills_to_reviewer: false,
                cancel: &cancel,
                event_emitter: None,
                completion_routes: None,
                file_baselines: Some(&tracker),
            };
            let edits = json!([{"old_content":"original","new_content":"new"}]);
            let args = match name {
                "write" => json!({"path":path,"content":"new\n","overwrite":true}),
                "edit" => json!({"path":path,"edits":edits}),
                "batch_edit" => json!({"files":[{"path":path,"edits":edits}]}),
                _ => {
                    json!({"path":path,"edits":[{"op":"replace","pos":format!("1#{}",compute_line_hash("original")),"lines":"new\n"}]})
                }
            };
            let manifest = crate::core::checkpoint::file_baselines::session_dir(&transcript)
                .join("u/baselines.jsonl");
            for iteration in 0..if decision == "allow" { 2 } else { 1 } {
                std::fs::write(&path, "original\n").unwrap();
                let mut display = None;
                let result = match name {
                    "write" => branches::write::handle_write(&ctx, &args, &mut display).await,
                    "hashline_edit" => {
                        branches::hashline_edit::handle_hashline_edit(&ctx, &args, &mut display)
                            .await
                    }
                    _ => branches::edit::handle_edit(&ctx, &args, &mut display).await,
                };
                if decision == "allow" {
                    assert!(result.is_ok(), "{name}: {result:?}");
                    assert!(display.is_some());
                    let rows = std::fs::read_to_string(&manifest).unwrap();
                    assert_eq!(rows.lines().count(), 1, "{name} iteration {iteration}");
                    let row: serde_json::Value = serde_json::from_str(rows.trim()).unwrap();
                    let backup = manifest
                        .parent()
                        .unwrap()
                        .join(row["backup"].as_str().unwrap());
                    assert_eq!(std::fs::read_to_string(backup).unwrap(), "original\n");
                } else {
                    assert!(
                        !manifest.exists(),
                        "{name}/{decision} must not publish baseline"
                    );
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
                }
            }
        }
    }
}

#[tokio::test]
async fn write_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "w1".into(),
        name: "write".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "content": "hello",
            "overwrite": false
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn edit_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit1".into(),
        name: "edit".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "old_content": "before",
            "new_content": "after"
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn hashline_edit_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "hedit1".into(),
        name: "hashline_edit".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "edits": [{
                "op": "replace",
                "pos": "1#ab",
                "end": "1#ab",
                "lines": "after"
            }]
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn rejected_hashline_edit_invalidates_its_read_stamp_for_a_refresh_read() {
    let dir = tempfile::tempdir().unwrap();
    let rejected_path = dir.path().join("reject.txt");
    std::fs::write(&rejected_path, "before\n").unwrap();
    let metadata = std::fs::metadata(&rejected_path).unwrap();
    let state = Arc::new(crate::core::tools::pipeline::read_state::ReadFileState::new());
    let normalized_path =
        crate::infra::platform::normalize_path(rejected_path.to_str().unwrap()).unwrap();
    state.put(
        normalized_path.clone(),
        crate::core::tools::pipeline::read_state::ReadStamp {
            mtime_ms: crate::core::tools::pipeline::read_state::metadata_mtime_ms(&metadata),
            size: metadata.len(),
            content_hash: crate::core::tools::pipeline::read_state::hash_content(b"before\n"),
            offset: None,
            limit: None,
            is_partial_view: false,
            render_mode: crate::core::tools::pipeline::read_state::ReadRenderMode::Plain,
            covered_lines: Some((1, 1)),
            reached_eof: true,
            tool_call_id: Some("prior-read".into()),
        },
    );
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "rejected-hashline-edit".into(),
        name: "hashline_edit".into(),
        arguments: json!({
            "path": rejected_path,
            "edits": [{
                "op": "replace",
                "pos": "1#ab",
                "end": "1#ab",
                "lines": "after"
            }]
        })
        .to_string(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        Some(&state),
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert!(outcome.model_text.contains(&crate::infra::i18n::tr_in(
        crate::infra::i18n::Locale::En,
        "toolHashline.denied",
        &[("path", "")],
    )));
    assert!(
        state.get(&normalized_path).is_none(),
        "被拒编辑后的下一次 read 必须能刷新，而不是命中 FILE_UNCHANGED"
    );
}

#[tokio::test]
async fn batch_edit_with_one_file_still_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit-batch-1".into(),
        name: "edit".into(),
        arguments: json!({
            "files": [{
                "path": "~/workspace/demo.txt",
                "old_content": "before",
                "new_content": "after"
            }]
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn batch_edit_with_multiple_files_uses_files_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit-batch-2".into(),
        name: "edit".into(),
        arguments: json!({
            "files": [
                {
                    "path": "~/workspace/demo-a.txt",
                    "old_content": "before",
                    "new_content": "after"
                },
                {
                    "path": "~/workspace/demo-b.txt",
                    "old_content": "before",
                    "new_content": "after"
                }
            ]
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    match outcome.display {
        Some(ToolDisplay::Files { files, .. }) => {
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].file, "~/workspace/demo-a.txt");
            assert_eq!(files[1].file, "~/workspace/demo-b.txt");
        }
        other => panic!("expected Files display, got {other:?}"),
    }
}

#[tokio::test]
async fn config_set_success_populates_text_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let backend: SharedConfigBackend = Arc::new(DisplayConfigBackend);
    let config_backend = Some(backend);
    let tc = ToolCallInfo {
        id: "cfg1".into(),
        name: "config_set".into(),
        arguments: json!({
            "key": "llm.default_model",
            "value": "gpt-5.4"
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &config_backend,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::Text {
            text: CONFIG_RESPONSE_MESSAGE.to_string(),
        })
    );
}
