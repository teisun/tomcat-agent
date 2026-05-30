//! # `ContextState` 度量 / 持久化方法
//!
//! 覆盖：
//!
//! - `estimated_token_count`：有 `last_api_usage` 时走 API 基线 + post_usage 折算；
//!   缺 `last_api_usage` 时回退到 `estimate_context_chars / 4`。
//! - `usage_ratio` + `invalidate_api_usage`：失效后 ratio 走回退分支并归零
//!   `post_usage_appended_chars`。
//! - `persist_context_observability`：把会话级观测指标（compaction 次数 /
//!   freed tokens / tool 结果累计字符）写回 `sessions.json`。

use std::path::PathBuf;

use super::super::*;
use super::mocks::temp_sessions_dir;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{ChatMessage, ChatMessageContentPart};

const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// PR-RJ-0：把 inline base64 fixture 解码后写到 tempfile，给新签名
/// `image_b64(mime, &Path)` / `file_b64(filename, mime, &Path)` 用。
fn write_b64_tempfile(b64: &str) -> tempfile::NamedTempFile {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    let mut f = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut f, &bytes).unwrap();
    f
}

#[test]
fn estimate_msg_chars_user_with_parts_counts_multimodal_weights() {
    let png = write_b64_tempfile(TINY_PNG_B64);
    let pdf = write_b64_tempfile("UERG");
    let m = ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("hello"),
        ChatMessageContentPart::image_b64("image/png", png.path()).expect("png"),
        ChatMessageContentPart::file_b64("x.pdf", "application/pdf", pdf.path()).expect("pdf"),
    ]);
    let n = estimate_msg_chars(&m);
    assert_eq!(n, "hello".len() + 3600 + 8000);
}

/// 纯 `InputText` parts 字符数等于各 part 文本 `chars().count()` 之和。
///
/// 数字来源：[`ChatMessageContentPart::estimated_chars`](crate::core::llm::types)
/// 对 `InputText` 走 `text.chars().count()`。
#[test]
fn estimate_msg_chars_text_only_returns_string_len() {
    let m = ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("hello"),
        ChatMessageContentPart::text(" world"),
    ]);
    assert_eq!(estimate_msg_chars(&m), "hello".len() + " world".len());
}

/// 含 `InputImage` part → 至少累计一份 IMAGE_CHAR_ESTIMATE = 3600。
///
/// 数字来源：`crate::core::llm::types` 顶部 `IMAGE_CHAR_ESTIMATE` 常量。
#[test]
fn estimate_msg_chars_with_image_part_uses_image_estimate() {
    let png = write_b64_tempfile(TINY_PNG_B64);
    let m = ChatMessage::user_with_parts(vec![ChatMessageContentPart::image_b64(
        "image/png",
        png.path(),
    )
    .expect("png")]);
    let n = estimate_msg_chars(&m);
    assert!(
        n >= 3600,
        "image part should contribute >= IMAGE_CHAR_ESTIMATE (3600), got {n}"
    );
}

/// 含 `InputFile` part → 至少累计一份 FILE_CHAR_ESTIMATE = 8000。
///
/// 数字来源：`crate::core::llm::types` 顶部 `FILE_CHAR_ESTIMATE` 常量。
#[test]
fn estimate_msg_chars_with_file_part_uses_file_estimate() {
    let pdf = write_b64_tempfile("UERG");
    let m = ChatMessage::user_with_parts(vec![ChatMessageContentPart::file_b64(
        "x.pdf",
        "application/pdf",
        pdf.path(),
    )
    .expect("pdf")]);
    let n = estimate_msg_chars(&m);
    assert!(
        n >= 8000,
        "file part should contribute >= FILE_CHAR_ESTIMATE (8000), got {n}"
    );
}

#[test]
fn test_estimated_token_count_uses_api_usage_when_present() {
    let state = ContextState {
        messages: vec![],
        estimate_context_chars: 40_000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 1000,
            completion_tokens: 200,
        }),
        post_usage_appended_chars: 400,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    assert_eq!(
        state.estimated_token_count(),
        1300,
        "should use API usage base + post_usage increment"
    );
}

#[test]
fn test_estimated_token_count_fallback_to_chars_when_no_usage() {
    let state = ContextState {
        messages: vec![],
        estimate_context_chars: 8000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: None,
        post_usage_appended_chars: 500,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    assert_eq!(
        state.estimated_token_count(),
        2000,
        "should fallback to estimate_context_chars / 4 when no API usage"
    );
}

#[test]
fn test_usage_ratio_after_invalidate() {
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: 10_000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 5000,
            completion_tokens: 1000,
        }),
        post_usage_appended_chars: 800,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    state.invalidate_api_usage();

    assert!(
        state.last_api_usage.is_none(),
        "last_api_usage should be None after invalidate"
    );
    assert_eq!(
        state.post_usage_appended_chars, 0,
        "post_usage_appended_chars should be 0 after invalidate"
    );
    let ratio = state.usage_ratio();
    assert!(
        (ratio - 0.1).abs() < 1e-9,
        "usage_ratio should be 0.1 after invalidate, got {}",
        ratio
    );
}

#[test]
fn rewrite_local_tail_chars_updates_estimate_and_post_usage() {
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: 12_000,
        context_budget_chars: 100_000,
        context_budget_tokens: 25_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 1_000,
            completion_tokens: 200,
        }),
        post_usage_appended_chars: 1_500,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    state.rewrite_local_tail_chars(1_200, 200);

    assert_eq!(state.estimate_context_chars, 11_000);
    assert_eq!(state.post_usage_appended_chars, 500);
}

#[test]
fn persist_context_observability_writes_sessions_json() {
    let dir = temp_sessions_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    let state = ContextState {
        messages: vec![],
        estimate_context_chars: 0,
        context_budget_chars: 1000,
        context_budget_tokens: 250,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::from("dummy.jsonl"),
        latest_plan_event: None,
        preheat: Preheat::new(),
        session_obs: super::super::types::SessionContextObservation {
            compaction_count: 7,
            compaction_tokens_freed: 12345,
            tool_result_chars_persisted: 999,
        },
        live: Default::default(),
    };
    mgr.persist_context_observability(&state).unwrap();

    let entry = mgr.get_session(key).unwrap().expect("session entry");
    assert_eq!(
        entry.compaction_count,
        Some(state.session_obs.compaction_count)
    );
    assert_eq!(
        entry.compaction_tokens_freed,
        Some(state.session_obs.compaction_tokens_freed as u64)
    );
    assert_eq!(
        entry.tool_result_chars_persisted,
        Some(state.session_obs.tool_result_chars_persisted as u64)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
