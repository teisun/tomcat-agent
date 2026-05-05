//! # PR-RF（T2-c）`tool_exec` 上的 `read` dedup / staleness 端到端焦小测
//!
//! 覆盖 `openspec/specs/architecture/tools/read.md` §3.2「重复 read 阻断」与
//! §4.x「外部修改触发重读」共 5 个场景，复用 `DefaultPrimitiveExecutor` + 真实
//! `tempdir` fs（**不**使用 mock），确保「stamp 写入 → 元数据比对 → 短路 stub」
//! 这条链路在生产 PrimitiveExecutor 上闭环可工作。
//!
//! 用例清单：
//! 1. `tool_exec_read_second_call_returns_unchanged_stub`：同窗口二次 read → stub。
//! 2. `tool_exec_read_after_mtime_bump_refetches`：mtime 更新 → 重新读，**不** stub。
//! 3. `tool_exec_read_partial_then_full_does_not_dedup`：分窗 vs 全文件互不命中。
//! 4. `tool_exec_read_different_window_does_not_dedup`：不同 (offset, limit) 互不命中。
//! 5. `tool_exec_read_state_clear_resets_dedup`：`clear()` 后再次 read 不再命中 stub。

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::core::agent_loop::tool_exec::execute_tool;
use crate::core::agent_loop::ToolCallInfo;
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::core::tools::read_state::{ReadFileState, FILE_UNCHANGED_STUB};
use crate::core::AllowAllConfirmation;
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};

fn make_gate(definition: &std::path::Path) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

fn make_executor(dir: &std::path::Path) -> Arc<dyn PrimitiveExecutor> {
    Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(dir),
    ))
}

fn make_tc(args_json: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "tc-1".into(),
        name: "read".into(),
        arguments: args_json.into(),
    }
}

/// 把文件 mtime 强制 +2s（最小可观测粒度），确保 dedup 廉价指纹明显失配。
/// 写入新内容 + 显式 set_modified，避免 fs 缓存或同秒 mtime 假命中。
fn bump_mtime(path: &std::path::Path) {
    std::fs::write(path, b"changed-content\n").expect("rewrite file");
    let new_t = SystemTime::now() + Duration::from_secs(2);
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.set_modified(new_t);
    }
}

#[tokio::test]
async fn tool_exec_read_second_call_returns_unchanged_stub() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("a.txt");
    std::fs::write(&f, b"hello\nworld\n").unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    );
    let tc = make_tc(&args);

    let (first, err1, _) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!err1, "first read must succeed");
    assert!(
        first.contains("hello"),
        "first read should return content, got {:?}",
        first
    );
    assert_eq!(state.len(), 1, "first read should populate stamp");

    let (second, err2, _) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!err2, "second read should not flag is_error");
    assert_eq!(
        second, FILE_UNCHANGED_STUB,
        "second identical read must return the FILE_UNCHANGED stub verbatim"
    );
}

#[tokio::test]
async fn tool_exec_read_after_mtime_bump_refetches() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("b.txt");
    std::fs::write(&f, b"original\n").unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    );
    let tc = make_tc(&args);

    let _ = execute_tool(&primitive, &None, Some(&state), &tc).await;
    bump_mtime(&f);

    let (out, err, _) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!err, "post-bump read should succeed");
    assert!(
        out.contains("changed-content") && !out.contains("File unchanged"),
        "mtime change must invalidate dedup; got {:?}",
        out
    );
}

#[tokio::test]
async fn tool_exec_read_partial_then_full_does_not_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("c.txt");
    let body: String = (1..=50).map(|i| format!("line{}\n", i)).collect();
    std::fs::write(&f, body.as_bytes()).unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    // partial：offset=10 limit=5
    let partial_tc = make_tc(&format!(
        r#"{{"path":{:?},"offset":10,"limit":5,"line_numbers":false}}"#,
        f.to_string_lossy()
    ));
    let _ = execute_tool(&primitive, &None, Some(&state), &partial_tc).await;

    // full：no offset / limit
    let full_tc = make_tc(&format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    ));
    let (out, err, _) = execute_tool(&primitive, &None, Some(&state), &full_tc).await;

    assert!(!err);
    assert!(
        out.starts_with("line1\n") && !out.contains("File unchanged"),
        "full read after partial must NOT short-circuit, got prefix: {:?}",
        out.chars().take(40).collect::<String>()
    );
}

#[tokio::test]
async fn tool_exec_read_different_window_does_not_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("d.txt");
    let body: String = (1..=20).map(|i| format!("L{:02}\n", i)).collect();
    std::fs::write(&f, body.as_bytes()).unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let tc1 = make_tc(&format!(
        r#"{{"path":{:?},"offset":1,"limit":5,"line_numbers":false}}"#,
        f.to_string_lossy()
    ));
    let tc2 = make_tc(&format!(
        r#"{{"path":{:?},"offset":6,"limit":5,"line_numbers":false}}"#,
        f.to_string_lossy()
    ));

    let _ = execute_tool(&primitive, &None, Some(&state), &tc1).await;
    let (out, err, _) = execute_tool(&primitive, &None, Some(&state), &tc2).await;
    assert!(!err);
    assert!(
        out.starts_with("L06\n") && !out.contains("File unchanged"),
        "different (offset,limit) windows must not dedup, got: {:?}",
        out
    );
}

#[tokio::test]
async fn tool_exec_read_state_clear_resets_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("e.txt");
    std::fs::write(&f, b"keep\n").unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(&format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    ));

    let _ = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert_eq!(state.len(), 1);

    state.clear();
    assert_eq!(state.len(), 0, "session-end cleanup must drop all stamps");

    let (out, err, _) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!err);
    assert!(
        !out.contains("File unchanged"),
        "after clear, dedup must miss; got {:?}",
        out
    );
    assert_eq!(
        state.len(),
        1,
        "successful re-read should re-populate stamp"
    );
}

// ─── PR-RJ T3-c：read 命中 image / pdf → follow_up_parts 注入下一条 user 消息 ──

#[tokio::test]
async fn tool_exec_image_result_injects_into_next_user_message_parts() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let png_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/llm_multimodal/sample_image.png");
    let png = dir_path.join("dog.png");
    std::fs::copy(&png_src, &png).unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    let tc = make_tc(&format!(r#"{{"path":{:?}}}"#, png.to_string_lossy()));

    let (msg, is_error, follow_ups) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!is_error, "image read should succeed");
    assert!(
        msg.contains("Image saved as next user input"),
        "tool message should be a placeholder, got: {:?}",
        msg
    );
    assert_eq!(
        follow_ups.len(),
        1,
        "exactly one InputImage part expected for next user message"
    );
    match &follow_ups[0] {
        crate::core::llm::ChatMessageContentPart::InputImage {
            mime_type, data, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("image/png"));
            assert!(data.as_ref().map(|s| !s.is_empty()).unwrap_or(false));
        }
        other => panic!("expected InputImage variant, got {:?}", other),
    }
}

#[tokio::test]
async fn tool_exec_pdf_result_injects_into_next_user_message_parts() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let pdf_b64 = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/llm_multimodal/sample_pdf_b64.txt"),
    )
    .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf_b64.trim().as_bytes())
        .unwrap();
    let pdf = dir_path.join("notes.pdf");
    std::fs::write(&pdf, &bytes).unwrap();

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    let tc = make_tc(&format!(r#"{{"path":{:?}}}"#, pdf.to_string_lossy()));

    let (msg, is_error, follow_ups) = execute_tool(&primitive, &None, Some(&state), &tc).await;
    assert!(!is_error, "pdf read should succeed");
    assert!(
        msg.contains("PDF attached as next user input"),
        "tool message should be a placeholder, got: {:?}",
        msg
    );
    assert_eq!(follow_ups.len(), 1, "exactly one InputFile part expected");
    match &follow_ups[0] {
        crate::core::llm::ChatMessageContentPart::InputFile {
            filename,
            mime_type,
            data,
            ..
        } => {
            assert_eq!(filename.as_deref(), Some("notes.pdf"));
            assert_eq!(mime_type.as_deref(), Some("application/pdf"));
            assert!(data.as_ref().map(|s| !s.is_empty()).unwrap_or(false));
        }
        other => panic!("expected InputFile variant, got {:?}", other),
    }
}
