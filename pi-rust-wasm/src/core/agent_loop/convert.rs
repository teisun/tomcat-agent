use tracing::info;

use crate::core::compaction::is_context_overflow_error;
use crate::infra::error::AppError;

use super::types::LoopError;

fn err_snippet(s: &str) -> String {
    s.chars().take(200).collect()
}

pub(super) fn classify_error(err: &AppError) -> LoopError {
    let s = err.to_string();
    let snippet = err_snippet(&s);
    if s.contains("401") {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "fatal_401",
            snippet = %snippet
        );
        return LoopError::Fatal(s);
    }
    // HTTP 400 + context_length_exceeded 等：须为 Retryable，Attempt loop 才能走 L3 截断。
    if is_context_overflow_error(&s) {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_context_overflow",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    if s.contains("400") {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "fatal_400_generic",
            snippet = %snippet
        );
        return LoopError::Fatal(s);
    }
    if s.contains("429")
        || s.contains("500")
        || s.contains("502")
        || s.contains("503")
        || s.contains("请求失败")
        || s.contains("超时")
    {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_rate_or_server",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    if s.contains("context") && (s.contains("length") || s.contains("token")) {
        info!(
            target: "pi_wasm_chat_diag",
            phase = "classify_error",
            branch = "retryable_context_heuristic",
            snippet = %snippet
        );
        return LoopError::Retryable(s);
    }
    info!(
        target: "pi_wasm_chat_diag",
        phase = "classify_error",
        branch = "fatal_default",
        snippet = %snippet
    );
    LoopError::Fatal(s)
}
