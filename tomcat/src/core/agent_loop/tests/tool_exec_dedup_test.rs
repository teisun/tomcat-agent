//! # PR-RF（T2-c）`tool_exec` 上的 `read` dedup / staleness 端到端焦小测
//!
//! 覆盖 `docs/architecture/tools/read.md` §3.2「重复 read 阻断」与
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

use crate::core::agent_loop::tool_exec::{execute_tool, execute_tool_with_openai_files};
use crate::core::agent_loop::ToolCallInfo;
use crate::core::llm::openai_files::{CacheEntry, FilePurpose, OpenAiFilesClient, OpenAiFilesRuntime};
use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::pipeline::read_state::{ReadFileState, FILE_UNCHANGED_STUB};
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::core::AllowAllConfirmation;
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};
use sha2::{Digest, Sha256};

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

fn write_fake_pdf(path: &std::path::Path, total_bytes: usize) {
    let mut bytes = Vec::with_capacity(total_bytes.max(16));
    bytes.extend_from_slice(b"%PDF-1.7\n");
    if total_bytes > bytes.len() {
        bytes.resize(total_bytes, b'0');
    }
    std::fs::write(path, bytes).unwrap();
}

fn sha256_file(path: &std::path::Path) -> [u8; 32] {
    let raw = std::fs::read(path).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(raw);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    out
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

    let (first, err1, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(!err1, "first read must succeed");
    assert!(
        first.contains("hello"),
        "first read should return content, got {:?}",
        first
    );
    assert_eq!(state.len(), 1, "first read should populate stamp");

    let (second, err2, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
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

    let _ = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    bump_mtime(&f);

    let (out, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
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
    let _ = execute_tool(&primitive, &None, &None, Some(&state), &partial_tc).await;

    // full：no offset / limit
    let full_tc = make_tc(&format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    ));
    let (out, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &full_tc).await;

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

    let _ = execute_tool(&primitive, &None, &None, Some(&state), &tc1).await;
    let (out, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc2).await;
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

    let _ = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert_eq!(state.len(), 1);

    state.clear();
    assert_eq!(state.len(), 0, "session-end cleanup must drop all stamps");

    let (out, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
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

    let (msg, is_error, follow_ups) =
        execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
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

    let (msg, is_error, follow_ups) =
        execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
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

#[tokio::test]
async fn tool_exec_pdf_oversize_without_files_runtime_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let pdf = dir_path.join("oversize.pdf");
    write_fake_pdf(&pdf, 11 * 1024 * 1024);

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    let tc = make_tc(&format!(r#"{{"path":{:?}}}"#, pdf.to_string_lossy()));

    let (msg, is_error, follow_ups) =
        execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "without files runtime, oversize pdf must fail by policy");
    assert!(
        msg.contains("requires OpenAI Files upload"),
        "should guide to Files upload path, got: {:?}",
        msg
    );
    assert!(follow_ups.is_empty());
}

#[tokio::test]
async fn tool_exec_pdf_oversize_uses_cached_file_id_when_runtime_available() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let pdf = dir_path.join("oversize.pdf");
    write_fake_pdf(&pdf, 11 * 1024 * 1024);
    let meta = std::fs::metadata(&pdf).unwrap();
    let mtime_ms = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
    let size = meta.len();
    let sha256 = sha256_file(&pdf);

    let runtime = Arc::new(OpenAiFilesRuntime::new(
        OpenAiFilesClient::new_for_test(
            reqwest::Client::new(),
            "http://127.0.0.1:9".to_string(),
            "stub".to_string(),
            0,
            86_400,
        ),
        dir_path.join("openai-files-registry.json"),
    ));
    runtime.cache.by_path.insert(
        std::fs::canonicalize(&pdf).unwrap_or(pdf.clone()),
        CacheEntry {
            mtime_ms,
            size,
            sha256,
            file_id: "file-cached-pdf".to_string(),
            purpose: FilePurpose::UserData,
            mime: "application/pdf".to_string(),
            uploaded_at: SystemTime::now(),
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            bytes: Some(size),
            created_at: Some(1_700_000_000),
        },
    );

    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    let tc = make_tc(&format!(r#"{{"path":{:?}}}"#, pdf.to_string_lossy()));

    let (msg, is_error, follow_ups) = execute_tool_with_openai_files(
        &primitive,
        &None,
        &None,
        Some(&state),
        Some(&runtime),
        &tc,
    )
    .await;
    assert!(
        !is_error,
        "with runtime + cached file_id, oversize should succeed; msg={:?}",
        msg
    );
    assert!(msg.contains("PDF attached as next user input"));
    assert_eq!(follow_ups.len(), 1);
    match &follow_ups[0] {
        crate::core::llm::ChatMessageContentPart::InputFile {
            file_id,
            data,
            filename,
            ..
        } => {
            assert_eq!(file_id.as_deref(), Some("file-cached-pdf"));
            assert!(data.is_none(), "upload path should use file_id not inline base64");
            assert_eq!(filename.as_deref(), Some("oversize.pdf"));
        }
        other => panic!("expected InputFile(file_id), got {:?}", other),
    }
}

// ─── T2-P0-017 PR-D（T1）edit 工具：staleness + unknown-tool 焦小测 ──

fn make_edit_tc(args_json: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "edit-1".into(),
        name: "edit".into(),
        arguments: args_json.into(),
    }
}

/// T2-P0-016 PR-C：edit / hashline_edit 现在要求 `read` 已落 stamp。
/// 测试用例若不关心 staleness 行为，使用此 helper 在调 edit 前先走一次真实 `read`。
async fn prime_read_stamp(
    primitive: &Arc<dyn PrimitiveExecutor>,
    state: &Arc<ReadFileState>,
    path: &std::path::Path,
) {
    let args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        path.to_string_lossy()
    );
    let tc = make_tc(&args);
    let (_, err, _) = execute_tool(primitive, &None, &None, Some(state), &tc).await;
    assert!(!err, "prime_read_stamp 必须成功落 stamp");
}

#[tokio::test]
async fn edit_legacy_edit_file_returns_unknown_tool_error() {
    // PR-命名：旧 `edit_file` 必须按未知工具回错（不重定向、无别名）。
    let dir = tempfile::tempdir().unwrap();
    let primitive = make_executor(dir.path());
    let state = Arc::new(ReadFileState::new());
    let tc = ToolCallInfo {
        id: "legacy-edit-1".into(),
        name: "edit_file".into(),
        arguments: r#"{"path":"/tmp/x","old_content":"a","new_content":"b"}"#.into(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "legacy edit_file 必须按未知工具回错");
    assert!(
        msg.contains("edit_file") || msg.to_lowercase().contains("unknown") || msg.contains("未知"),
        "错误文案应提示未知工具：{}",
        msg
    );
}

#[tokio::test]
async fn edit_rejected_when_read_stamp_stale() {
    // 先 read 落 stamp → 外部改文件（mtime+size 都变）→ edit 必须被 Stale 拦截。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("stale.txt");
    std::fs::write(&f, b"hello\nworld\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    // 先 read 让 ReadFileState 落 stamp。
    let read_args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    );
    let read_tc = make_tc(&read_args);
    let (_, err1, _) = execute_tool(&primitive, &None, &None, Some(&state), &read_tc).await;
    assert!(!err1);
    assert_eq!(state.len(), 1);

    // 外部修改文件（同时变 mtime 和 size）。
    bump_mtime(&f);

    // 此时 edit 必须被 Stale 拦截（在 primitive 调用之前）。
    let edit_args = format!(
        r#"{{"path":{:?},"old_content":"hello","new_content":"hi"}}"#,
        f.to_string_lossy()
    );
    let edit_tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &edit_tc).await;
    assert!(is_error, "stamp 不一致必须返回 is_error");
    assert!(msg.contains("Stale"), "错误文案应含 Stale：{}", msg);
}

#[tokio::test]
async fn edit_no_prior_read_rejects_after_t2_p0_016() {
    // T2-P0-016 PR-C / edit.md §10.2：与 write 同 PR 锁定后，edit / hashline_edit
    // 在无 prior read 时 **强拒** NoPriorRead，磁盘字节级未变。
    // （历史用例 `edit_no_prior_read_does_not_block_phase1` 已被本断言反转替换。）
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("noread.txt");
    std::fs::write(&f, b"foo\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let edit_args = format!(
        r#"{{"path":{:?},"old_content":"foo","new_content":"bar"}}"#,
        f.to_string_lossy()
    );
    let edit_tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &edit_tc).await;
    assert!(is_error, "NoPriorRead 必须 is_error=true：{}", msg);
    assert!(
        msg.contains("NoPriorRead"),
        "错误文案应含 NoPriorRead：{}",
        msg
    );
    assert_eq!(
        std::fs::read(&f).unwrap(),
        b"foo\n",
        "NoPriorRead 拦截后磁盘必须未变"
    );
}

#[tokio::test]
async fn edit_rejects_ipynb_before_touching_disk() {
    // PR-H：`.ipynb` 直接拒；磁盘不应被读 / 改；无 .bak。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("notebook.ipynb");
    std::fs::write(&f, b"{\"cells\":[]}\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let edit_args = format!(
        r#"{{"path":{:?},"old_content":"cells","new_content":"DONE"}}"#,
        f.to_string_lossy()
    );
    let edit_tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &edit_tc).await;
    assert!(is_error);
    assert!(
        msg.contains("Notebook"),
        "ipynb 应当返回 Notebook 错误：{}",
        msg
    );
    // 磁盘字节级未变 + 无 .bak
    assert_eq!(std::fs::read(&f).unwrap(), b"{\"cells\":[]}\n");
    assert!(!dir_path.join("notebook.bak").exists());
}

#[tokio::test]
async fn edit_error_codes_normalized() {
    // 单测覆盖 PR-H E5 错误码集合：NotFound / Ambiguous / Overlap / Stale / Notebook。
    // BinaryFile / Io / NoPriorRead 在其它用例覆盖。
    // T2-P0-016 PR-C：每次重写 fixture 后必须 prime read，让 stamp 与新文件 mtime/size 对齐，
    // 否则 NoPriorRead / Stale 会把后续断言提前击穿。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("e.txt");
    std::fs::write(&f, "x\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    // NotFound
    prime_read_stamp(&primitive, &state, &f).await;
    let tc = make_edit_tc(&format!(
        r#"{{"path":{:?},"old_content":"missing","new_content":"y"}}"#,
        f.to_string_lossy()
    ));
    let (msg, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(err);
    assert!(msg.contains("NotFound"), "期望 NotFound：{}", msg);

    // Ambiguous
    bump_mtime(&f);
    std::fs::write(&f, "x\nx\n").unwrap();
    prime_read_stamp(&primitive, &state, &f).await;
    let tc = make_edit_tc(&format!(
        r#"{{"path":{:?},"old_content":"x","new_content":"y"}}"#,
        f.to_string_lossy()
    ));
    let (msg, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(err);
    assert!(msg.contains("Ambiguous"), "期望 Ambiguous：{}", msg);

    // Overlap
    bump_mtime(&f);
    std::fs::write(&f, "abcdef\n").unwrap();
    prime_read_stamp(&primitive, &state, &f).await;
    let tc = make_edit_tc(&format!(
        r#"{{"path":{:?},"edits":[{{"old_content":"abcd","new_content":"X"}},{{"old_content":"cde","new_content":"Y"}}]}}"#,
        f.to_string_lossy()
    ));
    let (msg, err, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(err);
    assert!(msg.contains("Overlap"), "期望 Overlap：{}", msg);
}

// ─── T2-P0-017 Phase3 / PR-M：hashline_edit + read 闭环 ─────────────────────

#[tokio::test]
async fn hashline_edit_replace_matches_read_hashline() {
    use crate::core::tools::primitive::compute_line_hash;
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("h.txt");
    let body = "alpha\nbeta\ngamma\ndelta\n";
    std::fs::write(&f, body).unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    prime_read_stamp(&primitive, &state, &f).await;

    // 取第 2 行（beta）的 2 字符 hash
    let beta_hash = compute_line_hash("beta", 2);
    let edit_args = format!(
        r#"{{"path":{:?},"edits":[{{"op":"replace","pos":"2#{}","lines":"BETA\n"}}]}}"#,
        f.to_string_lossy(),
        beta_hash
    );
    let tc = ToolCallInfo {
        id: "hl-1".into(),
        name: "hashline_edit".into(),
        arguments: edit_args,
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(!is_error, "hashline_edit 应当成功：{}", msg);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "alpha\nBETA\ngamma\ndelta\n",
        "第 2 行被替换为 BETA"
    );
}

#[tokio::test]
async fn hashline_edit_rejects_hash_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("hm.txt");
    std::fs::write(&f, "a\nb\nc\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    prime_read_stamp(&primitive, &state, &f).await;

    // 故意给一个错的哈希（XX 一定与真实哈希字符不同）
    let edit_args = format!(
        r#"{{"path":{:?},"edits":[{{"op":"replace","pos":"2#XX","lines":"B\n"}}]}}"#,
        f.to_string_lossy()
    );
    let tc = ToolCallInfo {
        id: "hl-2".into(),
        name: "hashline_edit".into(),
        arguments: edit_args,
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "哈希不一致必须拒绝");
    assert!(
        msg.contains("HashMismatch"),
        "错误文案应含 HashMismatch：{}",
        msg
    );
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "a\nb\nc\n",
        "拒绝时磁盘必须未变"
    );
}

// ─── T2-P0-017 Phase3 / T3-K：secrets 扫描 ──────────────────────────────────

#[tokio::test]
async fn edit_secrets_pass_when_no_hit() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("s.rs");
    std::fs::write(&f, "fn main() { println!(\"hello\"); }\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    prime_read_stamp(&primitive, &state, &f).await;
    let edit_args = format!(
        r#"{{"path":{:?},"old_content":"hello","new_content":"world"}}"#,
        f.to_string_lossy()
    );
    let tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(!is_error, "普通代码不应当走 confirm：{}", msg);
}

#[tokio::test]
async fn edit_secrets_hit_proceeds_with_allow_all_confirmation() {
    // 默认 mock confirmation = AllowAll → 命中后 confirm 通过；磁盘被改。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("k.rs");
    std::fs::write(&f, "let key = \"OLD_KEY\";\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    prime_read_stamp(&primitive, &state, &f).await;
    // 把 OLD_KEY 改成 OpenAI 风格 key
    let edit_args = format!(
        r#"{{"path":{:?},"old_content":"OLD_KEY","new_content":"sk-ABCDEFGHIJKLMNOPQRSTUV"}}"#,
        f.to_string_lossy()
    );
    let tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(
        !is_error,
        "AllowAll confirmation 下 secrets 命中应当放行：{}",
        msg
    );
    let after = std::fs::read_to_string(&f).unwrap();
    assert!(
        after.contains("sk-ABCDEFGHIJKLMNOPQRSTUV"),
        "磁盘应已写入新 key"
    );
}

#[tokio::test]
async fn edit_oneof_shape_b_edits_array_is_parsed() {
    // 形状 B（edits[]）端到端走通：替换两段 + 第二段 replace_all。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("multi.txt");
    std::fs::write(&f, "use std::io;\nTODO\nbody\nTODO\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());
    prime_read_stamp(&primitive, &state, &f).await;

    let edit_args = format!(
        r#"{{"path":{:?},"edits":[{{"old_content":"use std::io;","new_content":"use std::io::{{self, Write}};"}},{{"old_content":"TODO","new_content":"DONE","replace_all":true}}]}}"#,
        f.to_string_lossy()
    );
    let edit_tc = make_edit_tc(&edit_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &edit_tc).await;
    assert!(!is_error, "形状 B 应当成功：{}", msg);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "use std::io::{self, Write};\nDONE\nbody\nDONE\n"
    );
}

// ─── T2-P0-016 PR-命名 / PR-C：write 工具门禁焦小测 ────────────────────────

fn make_write_tc(args_json: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "write-1".into(),
        name: "write".into(),
        arguments: args_json.into(),
    }
}

#[tokio::test]
async fn tool_exec_legacy_write_file_returns_unknown_tool_error() {
    // PR-命名：旧 `write_file` 必须按未知工具回错（不重定向、无别名；与 read_file / edit_file 一致）。
    let dir = tempfile::tempdir().unwrap();
    let primitive = make_executor(dir.path());
    let state = Arc::new(ReadFileState::new());
    let tc = ToolCallInfo {
        id: "legacy-write-1".into(),
        name: "write_file".into(),
        arguments: r#"{"path":"/tmp/x","content":"hi"}"#.into(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "legacy write_file 必须按未知工具回错");
    assert!(
        msg.contains("write_file")
            || msg.to_lowercase().contains("unknown")
            || msg.contains("未知"),
        "错误文案应提示未知工具：{}",
        msg
    );
}

#[tokio::test]
async fn write_existing_path_without_overwrite_rejected_with_exists() {
    // PR-C：路径已存在 + overwrite=false → 编排层早退 `Exists` is_error: true，磁盘字节级未变。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("e.txt");
    std::fs::write(&f, b"original\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let args = format!(r#"{{"path":{:?},"content":"new"}}"#, f.to_string_lossy());
    let tc = make_write_tc(&args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "Exists 必须 is_error=true：{}", msg);
    assert!(msg.contains("Exists"), "错误文案应含 Exists：{}", msg);
    assert_eq!(
        std::fs::read(&f).unwrap(),
        b"original\n",
        "Exists 路径磁盘必须未变"
    );
}

#[tokio::test]
async fn write_overwrite_without_prior_read_rejected_with_no_prior_read() {
    // PR-C：已存在 + overwrite=true 但本会话从未 read 过 → 编排层早退 `NoPriorRead`。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("blind.txt");
    std::fs::write(&f, b"original\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let args = format!(
        r#"{{"path":{:?},"content":"new","overwrite":true}}"#,
        f.to_string_lossy()
    );
    let tc = make_write_tc(&args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &tc).await;
    assert!(is_error, "NoPriorRead 必须 is_error=true：{}", msg);
    assert!(
        msg.contains("NoPriorRead"),
        "错误文案应含 NoPriorRead：{}",
        msg
    );
    assert_eq!(
        std::fs::read(&f).unwrap(),
        b"original\n",
        "NoPriorRead 路径磁盘必须未变"
    );
}

#[tokio::test]
async fn write_overwrite_after_external_change_rejected_with_stale() {
    // PR-C：已存在 + overwrite=true + 已 read 过，但外部改了文件 → 编排层早退 `Stale`。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("stale.txt");
    std::fs::write(&f, b"hello\nworld\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    let read_args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    );
    let read_tc = make_tc(&read_args);
    let (_, err1, _) = execute_tool(&primitive, &None, &None, Some(&state), &read_tc).await;
    assert!(!err1);
    assert_eq!(state.len(), 1);

    bump_mtime(&f);

    let write_args = format!(
        r#"{{"path":{:?},"content":"replaced\n","overwrite":true}}"#,
        f.to_string_lossy()
    );
    let write_tc = make_write_tc(&write_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &write_tc).await;
    assert!(is_error, "Stale 必须 is_error=true：{}", msg);
    assert!(msg.contains("Stale"), "错误文案应含 Stale：{}", msg);
    assert_ne!(
        std::fs::read(&f).unwrap(),
        b"replaced\n",
        "Stale 拦截后磁盘不应被覆盖为模型预期内容"
    );
}

#[tokio::test]
async fn write_success_invalidates_read_stamp() {
    // PR-C：write 成功后必须 invalidate stamp，避免下一轮 read 误返 FILE_UNCHANGED。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let f = dir_path.join("inv.txt");
    std::fs::write(&f, b"initial\n").unwrap();
    let primitive = make_executor(&dir_path);
    let state = Arc::new(ReadFileState::new());

    // 先 read（落 stamp）
    let read_args = format!(
        r#"{{"path":{:?},"line_numbers":false}}"#,
        f.to_string_lossy()
    );
    let read_tc = make_tc(&read_args);
    let (_, err1, _) = execute_tool(&primitive, &None, &None, Some(&state), &read_tc).await;
    assert!(!err1);
    assert_eq!(state.len(), 1, "read 后应落一个 stamp");

    // 再 write（覆盖；本会话内 stamp 还指向旧 mtime/size，但因测试中 read+write 紧邻，
    // 同一秒内 metadata mtime 可能未变 → 这条用例只关心成功后是否 invalidate；
    // 为绕过同秒 mtime+size 都未变的廉价判定，先 bump_mtime 一次让 stamp 与新内容“看起来一致”，
    // 重新读一次刷 stamp，再覆盖写）。
    bump_mtime(&f);
    let (_, err_re, _) = execute_tool(&primitive, &None, &None, Some(&state), &read_tc).await;
    assert!(!err_re, "重新 read 必须成功");

    let write_args = format!(
        r#"{{"path":{:?},"content":"after\n","overwrite":true}}"#,
        f.to_string_lossy()
    );
    let write_tc = make_write_tc(&write_args);
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, Some(&state), &write_tc).await;
    assert!(!is_error, "覆盖写应成功：{}", msg);

    // 关键断言 1：write 后 stamp 已被 invalidate（HashMap key 应被移除）。
    assert_eq!(
        state.len(),
        0,
        "write 成功后必须 invalidate read stamp，剩余 stamp 数应为 0"
    );

    // 关键断言 2：再次 read 必须返回真实新内容，而非 FILE_UNCHANGED stub。
    let (after_msg, err3, _) = execute_tool(&primitive, &None, &None, Some(&state), &read_tc).await;
    assert!(!err3);
    assert_ne!(
        after_msg, FILE_UNCHANGED_STUB,
        "write 后再 read 不能撒谎成 FILE_UNCHANGED"
    );
    assert!(
        after_msg.contains("after"),
        "再次 read 应包含新内容：{}",
        after_msg
    );
}
