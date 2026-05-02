use super::super::truncation::floor_char_boundary;
use super::super::{
    compact_tool_results, force_drop_oldest_to_target, is_context_overflow_error,
    layer0_persist_large_results,
};
use super::mocks::*;
use crate::core::compaction::preheat::{Preheat, PreheatOutcome};
use crate::core::llm::{
    ChatMessage, ChatMessageRole, ChatRequest, ChatResponse, LlmProvider, StreamEvent,
};
use crate::core::session::transcript::{
    append_entry, read_entries_tail, write_header, MessageEntry, SessionHeader, TranscriptEntry,
};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::{DefaultEventBus, EventBus};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;

#[test]
fn preheat_restore_pending_result_keeps_non_idle_until_consumed() {
    let mut p = Preheat::new();
    assert!(p.is_idle());
    p.restore_pending_result(dummy_compaction_result());
    assert!(!p.is_idle());
    assert!(p.is_finished());
}

#[test]
fn preheat_warmup_active_vs_result_pending() {
    let mut p = Preheat::new();
    assert!(!p.is_warmup_task_active());
    assert!(!p.preheat_result_pending());
    p.restore_completed(dummy_compaction_result());
    assert!(!p.is_warmup_task_active());
    assert!(p.preheat_result_pending());
}

#[test]
fn floor_char_boundary_ascii() {
    let s = "hello world";
    assert_eq!(floor_char_boundary(s, 5), 5);
    assert_eq!(floor_char_boundary(s, 100), s.len());
    assert_eq!(floor_char_boundary(s, 0), 0);
}

#[test]
fn floor_char_boundary_multibyte() {
    let s = "你好世界"; // 4 chars, 12 bytes
    assert_eq!(floor_char_boundary(s, 3), 3);
    assert_eq!(floor_char_boundary(s, 4), 3);
    assert_eq!(floor_char_boundary(s, 5), 3);
    assert_eq!(floor_char_boundary(s, 6), 6);
}

#[test]
fn compact_tool_results_reduces_budget() {
    let mut state = make_state(11_000, 5_000, 1_250);
    // Turn 1: [user, large tool result]  Turn 2: [user] — m=1 protects turn 2
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(25_000)),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert!(reduced > 0);
}

#[test]
fn compact_tool_results_protects_recent() {
    let tool_content = "x".repeat(25_000);
    let mut state = make_state(25_000, 5_000, 1_250);
    // Only one turn (one user message), m=1 → everything protected
    state.messages = vec![user_msg("q"), tool_msg("c1", &tool_content)];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn compact_tool_results_skips_small() {
    let mut state = make_state(5_000, 3_000, 750);
    // Small tool result (1000 < 10_000 threshold) → not replaced
    state.messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(1_000)),
        user_msg("q2"),
    ];
    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert_eq!(reduced, 0);
}

#[test]
fn force_drop_oldest_to_target_below_half() {
    let mut state = make_state(4000, 4000, 1000);
    state.messages = vec![
        user_msg(&"x".repeat(2000)),
        user_msg(&"y".repeat(1000)),
        user_msg(&"z".repeat(500)),
    ];
    force_drop_oldest_to_target(&mut state);
    assert!(state.usage_ratio() < 0.50);
}

#[test]
fn is_context_overflow_error_matches() {
    assert!(is_context_overflow_error(
        "context length exceeded: 500000 tokens"
    ));
    assert!(is_context_overflow_error(
        "maximum context token limit reached"
    ));
    assert!(!is_context_overflow_error("API error 429: rate limit"));
}

#[test]
fn context_state_on_message_appended() {
    let mut state = make_state(100, 1000, 250);
    state.on_message_appended(500);
    assert_eq!(state.estimate_context_chars, 600);
    assert_eq!(state.post_usage_appended_chars, 500);
    assert!(!state.is_over_budget());
    state.on_message_appended(500);
    assert!(state.is_over_budget());
}

#[test]
fn context_state_messages_push() {
    let mut state = make_state(0, 1000, 250);
    // on_message_appended is called when a message arrives; messages are pushed after
    state.on_message_appended(5);
    state.messages.push(user_msg("hello"));
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.estimate_context_chars, 5);
}

// ---------------------------------------------------------------------------
// T2-P0-002 Phase D —— 重试退避 + transcript 失败留痕
//
// 设计：
//   - `AlwaysFailingProvider` 让 `generate_summary` 每次都返回 Err，触发 retry loop 走完 3 次。
//   - `tokio::test(start_paused = true)` 让 `tokio::time::sleep` 走虚拟时钟，CI 不被墙钟拖慢。
//   - 用 `tokio::time::Instant::now()` 在 await_result 前后取虚拟 elapsed，
//     断言累计退避 ≈ 500ms + 1000ms = 1500ms（attempt=3 不再 sleep，否则会 >= 3500ms）。
//   - 失败留痕用 `read_entries_tail` 检查 transcript 末尾是否有 `summary == None` + `error/attempts`
//     的 BranchSummary 行；同时承接 #T-040 超大消息场景（详见 plan §6.C 决议段）。
// ---------------------------------------------------------------------------

struct AlwaysFailingProvider {
    calls: Arc<AtomicUsize>,
}

impl AlwaysFailingProvider {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait]
impl LlmProvider for AlwaysFailingProvider {
    fn provider_name(&self) -> &str {
        "always_failing_mock"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Llm("simulated provider failure".to_string()))
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        Err(AppError::Llm(
            "stream not used in preheat retry tests".to_string(),
        ))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

fn message_with_id(role: ChatMessageRole, id: &str, text: &str) -> ChatMessage {
    let mut m = match role {
        ChatMessageRole::User => ChatMessage::user(text),
        ChatMessageRole::Assistant => ChatMessage::assistant(text),
        _ => ChatMessage::user(text),
    };
    m.msg_id = Some(id.to_string());
    m.timestamp = Some("2026-04-26T00:00:00Z".to_string());
    m
}

#[tokio::test(start_paused = true)]
async fn preheat_retries_with_exponential_backoff() {
    let (provider, calls) = AlwaysFailingProvider::new();
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let cfg = ContextConfig::default();

    let messages = vec![
        message_with_id(ChatMessageRole::User, "u1", "hi"),
        message_with_id(ChatMessageRole::Assistant, "a1", "hello"),
    ];
    let dummy_path = std::path::PathBuf::new(); // 空路径 → spawn 内 transcript 写入跳过

    let mut preheat = Preheat::new();
    let started = tokio::time::Instant::now();
    let did_start = preheat.try_start(
        0.95,
        &messages,
        &dummy_path,
        provider,
        &cfg,
        event_bus.clone(),
    );
    assert!(
        did_start,
        "Preheat 在 ratio=0.95 + 非空 messages 时必须启动"
    );

    let outcome = preheat.await_result(Duration::from_secs(60)).await;
    assert!(
        matches!(outcome, PreheatOutcome::Exhausted),
        "三次 Err 后应 transition 到 ExhaustedPending（PreheatOutcome::Exhausted）",
    );

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "MAX_PREHEAT_RETRIES = 3，AlwaysFailingProvider 必须被调用 3 次",
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1500),
        "退避总耗时应 >= 500ms + 1000ms = 1500ms（attempt=3 失败后不睡），实际 {elapsed:?}",
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "总等待不应包含第 3 次 sleep 或额外延时，实际 {elapsed:?}",
    );
}

#[tokio::test(start_paused = true)]
async fn preheat_exhausted_writes_failure_entry_to_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failure_trail.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            version: Some(3),
            id: "sid_d".to_string(),
            timestamp: "2026-04-26T00:00:00.000Z".to_string(),
            cwd: None,
        },
    )
    .unwrap();

    // 写一条 type=message + id=msg_end 作为 covered_end_id 锚点，
    // insert_entry_after_message_id 才能在该锚点之后插入失败留痕；
    // 否则按 §5.7.4 退化为 append_entry，仍能落盘但用例语义弱化。
    let anchor_msg = TranscriptEntry::Message(MessageEntry {
        id: Some("msg_end".to_string()),
        parent_id: None,
        timestamp: "2026-04-26T00:00:01.000Z".to_string(),
        message: serde_json::json!({"role": "assistant", "content": "hello"}),
    });
    append_entry(&path, &anchor_msg).unwrap();

    let (provider, _) = AlwaysFailingProvider::new();
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let cfg = ContextConfig::default();

    let messages = vec![
        message_with_id(ChatMessageRole::User, "msg_start", "ping"),
        message_with_id(ChatMessageRole::Assistant, "msg_end", "pong"),
    ];

    let mut preheat = Preheat::new();
    let did_start = preheat.try_start(0.95, &messages, &path, provider, &cfg, event_bus.clone());
    assert!(did_start);
    let outcome = preheat.await_result(Duration::from_secs(60)).await;
    assert!(matches!(outcome, PreheatOutcome::Exhausted));

    let entries = read_entries_tail(&path, 16).unwrap();
    let failure_entry = entries
        .iter()
        .rev()
        .find_map(|e| match e {
            TranscriptEntry::BranchSummary(b) if b.summary.is_none() => Some(b.clone()),
            _ => None,
        })
        .expect("3 次失败后 transcript 应出现 summary == None 的 BranchSummary 失败留痕");

    assert_eq!(
        failure_entry.attempts,
        Some(3),
        "attempts 必须等于 MAX_PREHEAT_RETRIES = 3（与计划 §6.D 接口约束一致）",
    );
    assert!(
        failure_entry
            .error
            .as_deref()
            .map(|s| s.contains("simulated provider failure"))
            .unwrap_or(false),
        "error 必须保留最末次 LLM 错误描述，实际 {:?}",
        failure_entry.error,
    );
    assert_eq!(
        failure_entry.is_boundary,
        Some(false),
        "失败行同样标 is_boundary=false，避免 reload 时清空 prefix",
    );
    assert_eq!(
        failure_entry.covered_end_id.as_deref(),
        Some("msg_end"),
        "失败行需要保留 covered 范围，便于运行期定位故障窗口",
    );
}

#[test]
fn layer0_persist_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    // Layer 0 persists tool results from the last turn (after the last user message)
    state.messages = vec![
        user_msg("question"),
        tool_msg_with_id("tc_1_msg", "tc_1", &big_content),
    ];
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "test_session");
    assert_eq!(results.len(), 1);
    assert!(std::path::Path::new(&results[0].persisted_path).exists());
    assert!(state.estimate_context_chars < 60_000);
    // Check the tool message content was replaced
    let tool = state
        .messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .unwrap();
    assert!(tool
        .text_content()
        .unwrap_or("")
        .starts_with("[Tool result persisted:"));
}
