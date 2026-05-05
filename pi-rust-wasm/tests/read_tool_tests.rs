//! 集成测试：`read` 工具（PR-RA / RB / RF / RJ / RM）。
//!
//! 黑盒覆盖 `openspec/specs/architecture/tools/read.md`：
//!
//! - §2.1 / §2.2：分页（offset/limit）窗口语义；
//! - §2.3：二进制 → 结构化 hint（不读全文、不污染 LLM 上下文）；
//! - §3.1：`line_numbers` 默认 cat -n 渲染；`hashline` 优先且互斥；
//! - §4.1 / §4.2 T3-b：图片 / PDF 路由到 `ReadResult::Image|Pdf`，
//!   metadata 阶段尺寸校验；返回的 `path` 能直接喂给
//!   `ChatMessageContentPart::image_b64` / `file_b64` helper（PR-RJ-0
//!   重构后签名 `(mime, &Path)`），完成多模态注入下一条 user message。
//!
//! 全部用例满足 [INTEGRATION_TEST_SPEC.md](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)
//! §9.0 强制门禁：入口调用 `common::setup_logging()`，每个用例独立 `info_span!`，
//! 在 Arrange / Act / Assert 三阶段各至少落一条 `tracing::info!`。

mod common;

use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use pi_wasm::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use pi_wasm::{
    AllowAllConfirmation, ChatMessageContentPart, DefaultPrimitiveExecutor, PrimitiveConfig,
    PrimitiveExecutor, ReadResult, TracingAuditRecorder,
};
use tempfile::TempDir;
use tracing::{info, info_span, Instrument};

const FIXTURE_PNG: &str = "tests/fixtures/llm_multimodal/sample_image.png";
const FIXTURE_PDF_B64: &str = "tests/fixtures/llm_multimodal/sample_pdf_b64.txt";

fn make_gate(definition: &Path) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: true,
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

fn unwrap_text(result: ReadResult) -> String {
    match result {
        ReadResult::Text(t) => t.content,
        other => panic!("expected ReadResult::Text, got {:?}", other),
    }
}

/// 仓库根目录绝对路径（来自 `CARGO_MANIFEST_DIR`），用于解析 fixture 路径。
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

// ─── 1. 文本路径：分页 / 行号 ─────────────────────────────────────────────

#[tokio::test]
async fn read_text_offset_limit_window_with_line_numbers() {
    common::setup_logging();
    async {
        info!(stage = "arrange", "writing 20-line fixture file");
        let dir: TempDir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let f = dir_path.join("offsets.txt");
        let body: String = (1..=20).map(|n| format!("L{:02}\n", n)).collect();
        std::fs::write(&f, &body).expect("write fixture");
        let exec = make_executor(&dir_path);

        info!(
            stage = "act",
            offset = 15u64,
            limit = 3u64,
            "invoking read window"
        );
        let result = exec
            .read(&f.to_string_lossy(), Some(15), Some(3), true, false, "p1")
            .await
            .expect("read window must succeed");

        info!(
            stage = "assert",
            "checking absolute line numbers + window content"
        );
        let text = unwrap_text(result);
        assert!(
            text.starts_with("    15\tL15\n    16\tL16\n    17\tL17\n"),
            "absolute line numbers + window mismatch, got: {:?}",
            text
        );
    }
    .instrument(info_span!(
        "read_text_offset_limit_window_with_line_numbers"
    ))
    .await;
}

// ─── 2. 二进制结构化提示（未知扩展，含 NUL）─────────────────────────────

#[tokio::test]
async fn read_binary_returns_structured_hint() {
    common::setup_logging();
    async {
        info!(
            stage = "arrange",
            "writing fixture with NUL bytes (unknown ext)"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let f = dir_path.join("blob.bin");
        // 12 bytes，第 4 字节 NUL，触发二进制检测；不命中 png/jpeg/pdf 等 magic。
        let bytes: [u8; 12] = [
            0xDE, 0xAD, 0xBE, 0x00, 0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04,
        ];
        std::fs::write(&f, bytes).expect("write fixture");
        let exec = make_executor(&dir_path);

        info!(stage = "act", "invoking read on binary blob");
        let err = exec
            .read(&f.to_string_lossy(), None, None, true, false, "p1")
            .await
            .expect_err("non-multimodal binary must return structured AppError");

        info!(stage = "assert", err = %err, "verifying structured hint shape");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("binary") || msg.to_lowercase().contains("non-utf-8"),
            "expected binary hint, got: {}",
            msg
        );
        // First-byte hex preview (case-insensitive).
        assert!(
            msg.to_lowercase().contains("0xde"),
            "expected `0xDE` first-byte preview, got: {}",
            msg
        );
    }
    .instrument(info_span!("read_binary_returns_structured_hint"))
    .await;
}

// ─── 3. Hashline 渲染（PR-RM）─────────────────────────────────────────────

#[tokio::test]
async fn read_hashline_renders_two_char_hash_prefix() {
    common::setup_logging();
    async {
        info!(stage = "arrange", "writing 3-line fixture for hashline");
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let f = dir_path.join("hash.txt");
        std::fs::write(&f, "alpha\nbeta\ngamma\n").expect("write fixture");
        let exec = make_executor(&dir_path);

        info!(
            stage = "act",
            hashline = true,
            "invoking read with hashline=true"
        );
        let text = unwrap_text(
            exec.read(&f.to_string_lossy(), None, None, true, true, "p1")
                .await
                .expect("hashline read must succeed"),
        );

        info!(
            stage = "assert",
            "verifying `{{:>6}}#XX:{{content}}` per-line shape"
        );
        for (idx, expected_body) in ["alpha", "beta", "gamma"].iter().enumerate() {
            let line_no = (idx + 1) as u64;
            let prefix = format!("{:>6}#", line_no);
            let occurrence = text
                .lines()
                .find(|l| l.starts_with(&prefix) && l.ends_with(&format!(":{}", expected_body)))
                .unwrap_or_else(|| {
                    panic!(
                        "expected hashline for `{}` (line {}), full output: {:?}",
                        expected_body, line_no, text
                    )
                });
            // 形如 "     1#AB:alpha"：6 字符行号 + '#' + 2 字符 hash + ':' + 内容。
            assert_eq!(
                occurrence.len(),
                6 + 1 + 2 + 1 + expected_body.len(),
                "hashline length mismatch for line {}: {:?}",
                line_no,
                occurrence
            );
        }
    }
    .instrument(info_span!("read_hashline_renders_two_char_hash_prefix"))
    .await;
}

// ─── 4. 图片路由 → ReadResult::Image，并能注入 ChatMessageContentPart ───

#[tokio::test]
async fn read_png_routes_to_image_and_can_build_input_image_part() {
    common::setup_logging();
    async {
        info!(stage = "arrange", "copying PNG fixture into tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let png_src = repo_root().join(FIXTURE_PNG);
        let png = dir_path.join("dog.png");
        std::fs::copy(&png_src, &png).expect("copy png fixture");
        let exec = make_executor(&dir_path);

        info!(stage = "act", "invoking read on PNG");
        let result = exec
            .read(&png.to_string_lossy(), None, None, true, false, "p1")
            .await
            .expect("png read must succeed");

        info!(
            stage = "assert",
            "verifying Image variant + downstream part construction"
        );
        let bin = match result {
            ReadResult::Image(b) => b,
            other => panic!("expected ReadResult::Image, got {:?}", other),
        };
        assert_eq!(bin.mime, "image/png");
        assert_eq!(bin.path, png);
        assert_eq!(bin.filename, "dog.png");
        assert!(bin.original_size > 0);

        // T3-c：把图片 part 注入下一条 user message 时调用的同一 helper。
        let part = ChatMessageContentPart::image_b64(bin.mime.clone(), &bin.path)
            .expect("image_b64 must accept the path returned by read");
        let json = serde_json::to_value(&part).expect("serde json");
        assert_eq!(json["type"], "input_image");
        assert_eq!(json["mime_type"], "image/png");
        assert!(json["image_b64"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false));
    }
    .instrument(info_span!(
        "read_png_routes_to_image_and_can_build_input_image_part"
    ))
    .await;
}

// ─── 5. PDF 路由 → ReadResult::Pdf，并能注入 InputFile part ────────────

#[tokio::test]
async fn read_pdf_routes_to_pdf_and_can_build_input_file_part() {
    common::setup_logging();
    async {
        info!(
            stage = "arrange",
            "decoding base64 PDF fixture into tempdir"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let pdf_b64 = std::fs::read_to_string(repo_root().join(FIXTURE_PDF_B64))
            .expect("read pdf b64 fixture");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pdf_b64.trim().as_bytes())
            .expect("decode pdf base64");
        let pdf = dir_path.join("notes.pdf");
        std::fs::write(&pdf, &bytes).expect("write pdf fixture");
        let exec = make_executor(&dir_path);

        info!(stage = "act", "invoking read on PDF");
        let result = exec
            .read(&pdf.to_string_lossy(), None, None, true, false, "p1")
            .await
            .expect("pdf read must succeed");

        info!(
            stage = "assert",
            "verifying Pdf variant + downstream part construction"
        );
        let bin = match result {
            ReadResult::Pdf(b) => b,
            other => panic!("expected ReadResult::Pdf, got {:?}", other),
        };
        assert_eq!(bin.mime, "application/pdf");
        assert_eq!(bin.filename, "notes.pdf");
        assert_eq!(bin.original_size, bytes.len() as u64);

        let part =
            ChatMessageContentPart::file_b64(bin.filename.clone(), bin.mime.clone(), &bin.path)
                .expect("file_b64 must accept the path returned by read");
        let json = serde_json::to_value(&part).expect("serde json");
        assert_eq!(json["type"], "input_file");
        assert_eq!(json["filename"], "notes.pdf");
        assert!(json["file_b64"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false));
    }
    .instrument(info_span!(
        "read_pdf_routes_to_pdf_and_can_build_input_file_part"
    ))
    .await;
}

// ─── 6. 异常边界：oversize 图片在 metadata 阶段拒绝 ─────────────────────

#[tokio::test]
async fn read_oversize_image_rejected_before_loading_bytes() {
    common::setup_logging();
    async {
        info!(
            stage = "arrange",
            "writing 5 MiB PNG-magic blob (>IMAGE_MAX_BYTES)"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().canonicalize().expect("canonicalize");
        let f = dir_path.join("big.png");
        let png_magic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let mut bytes = Vec::with_capacity(5 * 1024 * 1024 + png_magic.len());
        bytes.extend_from_slice(&png_magic);
        bytes.resize(5 * 1024 * 1024 + png_magic.len(), 0);
        std::fs::write(&f, &bytes).expect("write oversize png");
        let exec = make_executor(&dir_path);

        info!(stage = "act", "invoking read on oversize PNG");
        let err = exec
            .read(&f.to_string_lossy(), None, None, true, false, "p1")
            .await
            .expect_err("oversize image must be rejected at metadata stage");

        info!(stage = "assert", err = %err, "verifying error mentions size limit");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("image")
                || msg.to_lowercase().contains("size")
                || msg.contains("超"),
            "expected size-related rejection, got: {}",
            msg
        );
    }
    .instrument(info_span!(
        "read_oversize_image_rejected_before_loading_bytes"
    ))
    .await;
}
