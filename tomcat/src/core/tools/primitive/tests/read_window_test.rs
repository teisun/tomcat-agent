//! # PR-RB（T1）`read` 工具：分页 + 流式 + 二进制提示 + 25 MiB 上限
//!
//! 覆盖 `docs/architecture/tools/read.md` §2.1 / §2.2 / §2.3 / §2.5
//! 决策表所要求的 6 个 T1 单测：
//!
//! 1. `read_offset_limit_returns_window`：跳过 `offset-1` 行，返回 `limit` 行；
//! 2. `read_offset_beyond_eof_returns_empty`：`offset > total_lines` 返回空文本（不报错）；
//! 3. `read_limit_truncates_with_resume_hint`：截断时附 `offset=<next>, limit=<same>`；
//! 4. `read_binary_returns_structured_hint`：含 `\x00` → 结构化 hint + first-byte hex；
//! 5. `read_no_offset_large_file_rejected_with_hint`：无 offset/limit 且 > 上限 → 拒绝并提示；
//! 6. `read_with_offset_bypasses_max_bytes_check`：传 offset/limit 时绕过 metadata 上限。
//!
//! **fixture 规则**：所有测试都在 `tempfile::tempdir()` 里建临时文件，避免污染
//! 仓库；大文件用 `with_read_max_bytes(64)` 把上限调小，**不**真的生成 25 MiB
//! 文件（详见 `read.md` §2.5「不二刀切」原则与本测试 §6 用例）。

use crate::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use crate::core::tools::primitive::{
    DefaultPrimitiveExecutor, PrimitiveExecutor, ReadResult, ReadTextResult,
};
use crate::core::AllowAllConfirmation;
use crate::infra::error::AppError;
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};
use std::path::Path;
use std::sync::Arc;

fn make_gate(definition: &Path) -> Arc<dyn PermissionGate> {
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

fn make_executor(dir: &Path) -> DefaultPrimitiveExecutor {
    DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(dir),
    )
}

/// PR-RJ T3-a：把 `ReadResult::Text(_)` 摊出 `(content, ReadTextResult)`，
/// 非 Text variant 直接 panic（本测试文件目前只覆盖文本路径）。
fn unwrap_text(result: ReadResult) -> (String, ReadTextResult) {
    match result {
        ReadResult::Text(t) => (t.content.clone(), t),
        other => panic!("expected ReadResult::Text, got {:?}", other),
    }
}

#[tokio::test]
async fn read_offset_limit_returns_window() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("ten.txt");
    let body: String = (1..=10).map(|n| format!("line{}\n", n)).collect();
    std::fs::write(&f, &body).unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), Some(3), Some(2), false, false, "p1")
        .await
        .unwrap();
    let (out, meta) = unwrap_text(result);
    assert_eq!(meta.start_line, 3);
    assert!(meta.truncated, "limit=2 over 10 lines must truncate");
    assert_eq!(meta.remaining_lines, 6);
    assert!(
        out.starts_with("line3\nline4\n"),
        "expected window of lines 3-4 (no numbers), got: {:?}",
        out
    );
    assert!(
        out.contains("[") && out.contains("more lines truncated") && out.contains("offset=5"),
        "expected resume hint with next offset=5, got: {:?}",
        out
    );
    assert!(
        out.contains("limit=2"),
        "resume hint should preserve limit=2, got: {:?}",
        out
    );
}

#[tokio::test]
async fn read_offset_beyond_eof_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("short.txt");
    std::fs::write(&f, "a\nb\nc\n").unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), Some(99), Some(10), true, false, "p1")
        .await
        .unwrap();
    let (out, meta) = unwrap_text(result);
    assert!(
        out.is_empty(),
        "offset > total_lines must yield empty body, got: {:?}",
        out
    );
    assert_eq!(meta.num_lines, 0);
    assert!(!meta.truncated);
}

#[tokio::test]
async fn read_limit_truncates_with_resume_hint() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("two_hundred.txt");
    let body: String = (1..=200).map(|n| format!("L{:03}\n", n)).collect();
    std::fs::write(&f, &body).unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), Some(1), Some(50), false, false, "p1")
        .await
        .unwrap();
    let (out, _) = unwrap_text(result);

    assert!(out.starts_with("L001\n"), "should start at line 1");
    let hint_line = out.lines().last().unwrap_or("");
    assert!(
        hint_line.contains("offset=51") && hint_line.contains("limit=50"),
        "resume hint must say `offset=51, limit=50`, got: {:?}",
        hint_line
    );
    assert!(
        hint_line.contains("150 more lines"),
        "resume hint must report remaining count = 150, got: {:?}",
        hint_line
    );
}

#[tokio::test]
async fn read_binary_returns_structured_hint() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("blob.bin");
    std::fs::write(&f, [0x89, 0x50, 0x4E, 0x47, 0x00, 0x01, 0x02]).unwrap();

    let exec = make_executor(&dir_path);
    let err = exec
        .read(&f.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        matches!(err, AppError::Primitive(_)),
        "binary path must yield AppError::Primitive, got {:?}",
        err
    );
    assert!(
        msg.contains("File is binary or non-UTF-8"),
        "structured hint should lead with English diagnostic; got: {:?}",
        msg
    );
    assert!(
        msg.contains("0x89"),
        "first-byte hex (0x89, suggests PNG) must appear in hint; got: {:?}",
        msg
    );
    assert!(
        msg.contains("multimodal"),
        "hint should foreshadow T3 multimodal path; got: {:?}",
        msg
    );
}

#[tokio::test]
async fn read_no_offset_large_file_rejected_with_hint() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("big.txt");
    std::fs::write(&f, vec![b'a'; 256]).unwrap();

    let exec = make_executor(&dir_path).with_read_max_bytes(64);
    let err = exec
        .read(&f.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(matches!(err, AppError::Primitive(_)));
    assert!(
        msg.contains("File is large") && msg.contains("offset") && msg.contains("limit"),
        "rejection must instruct caller to pass offset/limit; got: {:?}",
        msg
    );
}

#[tokio::test]
async fn read_with_offset_bypasses_max_bytes_check() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("big_log.txt");
    let body: String = (1..=200).map(|n| format!("entry-{}\n", n)).collect();
    std::fs::write(&f, &body).unwrap();
    assert!(std::fs::metadata(&f).unwrap().len() > 64);

    let exec = make_executor(&dir_path).with_read_max_bytes(64);

    let result = exec
        .read(&f.to_string_lossy(), Some(10), Some(2), false, false, "p1")
        .await
        .expect("offset/limit must bypass max_bytes metadata gate");
    let (out, _) = unwrap_text(result);
    assert!(out.starts_with("entry-10\nentry-11\n"), "got: {:?}", out);
}

#[tokio::test]
async fn read_applies_post_output_budget_guard_with_resume_hint() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("budget.txt");
    let body: String = (1..=40)
        .map(|n| format!("L{:03}:{}\n", n, "a".repeat(4090)))
        .collect();
    std::fs::write(&f, &body).unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), Some(1), Some(40), false, false, "p1")
        .await
        .unwrap();
    let (out, meta) = unwrap_text(result);

    assert!(
        meta.truncated,
        "post-read budget must truncate oversized window"
    );
    assert_eq!(meta.num_lines, 32, "4096-byte rows should stop at line 32");
    assert_eq!(
        meta.remaining_lines, 0,
        "budget guard should stop early instead of scanning full remaining window"
    );
    assert!(
        out.contains("L032:") && !out.contains("L033:"),
        "expected output to stop at line 32, got tail: {:?}",
        out.lines().rev().take(3).collect::<Vec<_>>()
    );
    assert!(
        out.contains("offset=33") && out.contains("limit=40"),
        "resume hint should point at next unread line, got: {:?}",
        out.lines().last()
    );
}

#[tokio::test]
async fn read_first_returned_line_over_budget_returns_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("long-first-line.txt");
    std::fs::write(&f, format!("{}\nsecond\n", "x".repeat(128 * 1024 + 1))).unwrap();

    let exec = make_executor(&dir_path);
    let err = exec
        .read(&f.to_string_lossy(), Some(1), Some(2), false, false, "p1")
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(matches!(err, AppError::Primitive(_)));
    assert!(
        msg.contains("first returned line") && msg.contains("offset") && msg.contains("limit"),
        "error should explain how to shrink the window, got: {}",
        msg
    );
    assert!(
        msg.contains("128KiB"),
        "error should mention the 128KiB post-read budget, got: {}",
        msg
    );
}

// ─── PR-RF（T2-a）行号渲染 ─────────────────────────────────────────────────

#[test]
fn format_with_line_numbers_basic_starts_at_one() {
    use super::super::executor::format_with_line_numbers;
    let out = format_with_line_numbers(1, "a\nb\nc\n");
    assert_eq!(out, "     1\ta\n     2\tb\n     3\tc\n");
}

#[test]
fn format_with_line_numbers_respects_offset_origin() {
    use super::super::executor::format_with_line_numbers;
    let out = format_with_line_numbers(98, "x\ny\nz\n");
    assert_eq!(out, "    98\tx\n    99\ty\n   100\tz\n");
}

#[test]
fn format_with_line_numbers_handles_empty_and_no_trailing_newline() {
    use super::super::executor::format_with_line_numbers;
    assert_eq!(format_with_line_numbers(1, ""), "");
    let out = format_with_line_numbers(1, "tail");
    assert_eq!(out, "     1\ttail");
}

#[tokio::test]
async fn read_default_renders_cat_n_line_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("nums.txt");
    std::fs::write(&f, "alpha\nbeta\ngamma\n").unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap();
    let (out, _) = unwrap_text(result);
    assert!(out.contains("     1\talpha\n"), "got: {:?}", out);
    assert!(out.contains("     2\tbeta\n"), "got: {:?}", out);
    assert!(out.contains("     3\tgamma\n"), "got: {:?}", out);
}

#[tokio::test]
async fn read_offset_window_uses_absolute_line_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("offsets.txt");
    let body: String = (1..=20).map(|n| format!("L{:02}\n", n)).collect();
    std::fs::write(&f, body).unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), Some(15), Some(3), true, false, "p1")
        .await
        .unwrap();
    let (out, meta) = unwrap_text(result);
    assert_eq!(meta.start_line, 15);
    assert!(
        out.starts_with("    15\tL15\n    16\tL16\n    17\tL17\n"),
        "got: {:?}",
        out
    );
}

// ─── PR-RJ T3-b：image / PDF / fallback 路由 ───────────────────────────────

#[tokio::test]
async fn read_routes_png_to_image_variant() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let png_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/llm_multimodal/sample_image.png");
    let png = dir_path.join("dog.png");
    std::fs::copy(&png_src, &png).unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&png.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap();
    match result {
        ReadResult::Image(b) => {
            assert_eq!(b.mime, "image/png");
            assert_eq!(b.path, png);
            assert!(b.original_size > 0);
            assert_eq!(b.filename, "dog.png");
        }
        other => panic!("expected ReadResult::Image, got {:?}", other),
    }
}

#[tokio::test]
async fn read_routes_pdf_to_pdf_variant() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
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

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&pdf.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap();
    match result {
        ReadResult::Pdf(b) => {
            assert_eq!(b.mime, "application/pdf");
            assert_eq!(b.filename, "notes.pdf");
            assert_eq!(b.original_size, bytes.len() as u64);
        }
        other => panic!("expected ReadResult::Pdf, got {:?}", other),
    }
}

#[tokio::test]
async fn read_unknown_extension_falls_back_to_text() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("notes.md");
    std::fs::write(&f, "# heading\nbody\n").unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(&f.to_string_lossy(), None, None, false, false, "p1")
        .await
        .unwrap();
    let (out, _) = unwrap_text(result);
    assert!(
        out.contains("# heading"),
        "fallback should produce text, got {:?}",
        out
    );
}

#[tokio::test]
async fn read_oversize_image_rejected_at_metadata_stage() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("big.png");
    // PNG magic + 5 MiB padding（>IMAGE_MAX_BYTES=4.5 MiB）；不会被 read_to_end，
    // 只要 metadata 阶段拦下即可。
    let png_magic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let mut bytes = Vec::with_capacity(5 * 1024 * 1024 + png_magic.len());
    bytes.extend_from_slice(&png_magic);
    bytes.resize(5 * 1024 * 1024 + png_magic.len(), 0);
    std::fs::write(&f, &bytes).unwrap();

    let exec = make_executor(&dir_path);
    let err = exec
        .read(&f.to_string_lossy(), None, None, true, false, "p1")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, AppError::Primitive(_)));
    assert!(
        msg.contains("IMAGE_MAX_BYTES"),
        "expected IMAGE_MAX_BYTES diagnostic, got: {}",
        msg
    );
}

// ─── PR-RM T3 hashline ────────────────────────────────────────────────────

#[test]
fn hashline_format_matches_pi_agent_rust_layout() {
    use super::super::executor::{compute_line_hash, format_with_hashlines};

    // 1) compute_line_hash 单行：纯字母行 → seed=0；纯标点行 → seed=line_no。
    let h_alpha = compute_line_hash("fn foo() {}", 12);
    assert_eq!(h_alpha.len(), 2, "tag must be 2 chars, got {:?}", h_alpha);
    assert!(
        h_alpha
            .chars()
            .all(|c| b"ZPMQVRWSNKTXJBYH".contains(&(c as u8))),
        "tag chars must come from the dict, got {:?}",
        h_alpha
    );
    // 2) 缩进不影响 hash（去空白后才算）。
    let h_indent = compute_line_hash("    fn foo() {}", 12);
    assert_eq!(
        h_alpha, h_indent,
        "hash should ignore whitespace, indented variant differs: {:?} vs {:?}",
        h_alpha, h_indent
    );
    // 3) 纯标点行用行号做 seed → 不同行号产生不同 hash。
    let h_punct_a = compute_line_hash("---", 1);
    let h_punct_b = compute_line_hash("---", 2);
    assert_ne!(
        h_punct_a, h_punct_b,
        "punct-only rows must hash differently per line_no"
    );

    // 4) format_with_hashlines 渲染：{:>6}#{2-char}:{原行}（含 \n）。
    let body = "alpha\nbeta\n";
    let out = format_with_hashlines(10, body);
    let mut lines = out.lines();
    let l1 = lines.next().unwrap();
    let l2 = lines.next().unwrap();
    assert!(l1.starts_with("    10#"), "line 1 prefix wrong: {:?}", l1);
    assert!(
        l1.ends_with(":alpha"),
        "line 1 should end with `:alpha`, got {:?}",
        l1
    );
    assert!(l2.starts_with("    11#"), "line 2 prefix wrong: {:?}", l2);
    assert!(
        l2.ends_with(":beta"),
        "line 2 should end with `:beta`, got {:?}",
        l2
    );
}

#[tokio::test]
async fn read_with_hashline_renders_hash_prefixed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().canonicalize().unwrap();
    let f = dir_path.join("h.txt");
    std::fs::write(&f, "alpha\nbeta\n").unwrap();

    let exec = make_executor(&dir_path);
    let result = exec
        .read(
            &f.to_string_lossy(),
            None,
            None,
            true, /*ignored*/
            true,
            "p1",
        )
        .await
        .unwrap();
    let (out, _) = unwrap_text(result);
    // hashline 优先：输出应是 `{:>6}#{2}:{line}`，不含 cat-n 的 `\t` 前缀。
    let first = out.lines().next().unwrap();
    assert!(
        first.starts_with("     1#"),
        "expected hashline format, got: {:?}",
        first
    );
    assert!(
        first.ends_with(":alpha"),
        "first row should end with :alpha, got: {:?}",
        first
    );
    assert!(
        !out.contains("\t"),
        "hashline output must NOT include cat-n tabs"
    );
}
