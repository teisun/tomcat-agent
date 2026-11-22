use super::super::truncation::floor_char_boundary;
use super::super::{
    compact_tool_results, force_drop_oldest_after_confirmed_overflow, layer0_persist_large_results,
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
use crate::infra::error::{is_context_overflow_text, AppError};
use crate::infra::event_bus::{DefaultEventBus, EventBus};
use crate::infra::{wire, EventContext};
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
    let mut messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(25_000)),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert!(reduced > 0);
}

#[test]
fn compact_tool_results_protects_recent() {
    let tool_content = "x".repeat(25_000);
    let mut state = make_state(25_000, 5_000, 1_250);
    // Only one turn (one user message), m=1 → everything protected
    let mut messages = vec![user_msg("q"), tool_msg("c1", &tool_content)];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert_eq!(reduced, 0);
}

#[test]
fn compact_tool_results_skips_small() {
    let mut state = make_state(5_000, 3_000, 750);
    // Small tool result (1000 < 10_000 threshold) → not replaced
    let mut messages = vec![
        user_msg("q"),
        tool_msg("c1", &"x".repeat(1_000)),
        user_msg("q2"),
    ];
    let config = ContextConfig {
        keep_recent_turns: 1,
        ..Default::default()
    };
    let history_end = messages.len();
    let reduced = compact_tool_results(&mut state, &mut messages, &config, history_end).chars_freed;
    assert_eq!(reduced, 0);
}

#[test]
fn confirmed_overflow_drops_a_turn_even_when_local_estimate_is_low() {
    let mut state = make_state(4000, 4000, 1000);
    let mut messages = vec![
        user_msg(&"x".repeat(2000)),
        user_msg(&"y".repeat(1000)),
        user_msg(&"z".repeat(500)),
    ];
    state.context_budget_tokens = 100_000;
    let mut history_end = messages.len();
    let (turns_removed, _) =
        force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut history_end);
    assert_eq!(turns_removed, 1);
    assert_eq!(messages.len(), 2);
    assert!(state.usage_ratio() < 0.50);
}

#[test]
fn confirmed_overflow_never_empties_the_last_turn() {
    let mut state = make_state(4000, 4000, 1000);
    let mut messages = vec![user_msg(&"x".repeat(3000))];

    let mut history_end = messages.len();
    let (turns_removed, chars_removed) =
        force_drop_oldest_after_confirmed_overflow(&mut state, &mut messages, &mut history_end);

    assert_eq!(turns_removed, 0);
    assert_eq!(chars_removed, 0);
    assert_eq!(messages.len(), 1);
}

#[test]
fn is_context_overflow_text_matches() {
    assert!(is_context_overflow_text(
        "context length exceeded: 500000 tokens"
    ));
    assert!(is_context_overflow_text(
        "maximum context token limit reached"
    ));
    assert!(!is_context_overflow_text("API error 429: rate limit"));
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
// Preheat failure diagnostics and retry ownership
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

// Exercise the real checkpoint while the failed task still owns its result.
#[tokio::test(start_paused = true)]
async fn preheat_regression_collects_failure_below_apply_waterline() {
    let config = ContextConfig::default();
    let mut state = make_state(2_400, 4_000, 1_000);
    let mut messages = vec![user_msg_with_id("q", "question")];
    let (provider, _) = AlwaysFailingProvider::new();
    let emitter = Arc::new(crate::infra::ScopedEventEmitter::new(
        Arc::new(DefaultEventBus::new()),
        "checkpoint",
    ));
    assert!(state.preheat.try_start(
        0.60,
        &messages,
        std::path::Path::new(""),
        None,
        Arc::new(provider),
        None,
        None,
        &config,
        emitter.clone(),
        None,
    ));
    assert!(state.preheat.is_warmup_task_active());
    tokio::task::yield_now().await;
    assert!(!state.preheat.is_warmup_task_active());
    let read_state = crate::core::tools::pipeline::read_state::ReadFileState::default();
    let env = crate::core::compaction::apply::BoundaryEnv {
        config: &config,
        work_dir: std::path::Path::new(""),
        session_id: "checkpoint",
        read_file_state: &read_state,
    };
    let mut start = messages.len();
    assert!(!crate::core::compaction::apply::check_before_request(
        &mut state,
        &mut messages,
        &emitter,
        &env,
        &mut start,
    ));
    assert!(
        state.preheat.is_exhausted_pending(),
        "a finished failure is collected even at 60%"
    );

    let (provider, calls) = AlwaysFailingProvider::new();
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    for (ratio, batch) in [
        (0.45, messages.as_slice()),
        (0.60, &[][..]),
        (0.60, messages.as_slice()),
    ] {
        let started = state.preheat.try_restart_if_pending(
            ratio,
            batch,
            std::path::Path::new(""),
            None,
            provider.clone(),
            None,
            None,
            &config,
            emitter.clone(),
            None,
        );
        assert_eq!(started, ratio >= 0.50 && !batch.is_empty());
        assert_eq!(state.preheat.is_exhausted_pending(), !started);
    }
    assert!(!state.preheat.try_restart_if_pending(
        0.80,
        &messages,
        std::path::Path::new(""),
        None,
        provider,
        None,
        None,
        &config,
        emitter,
        None,
    ));
    tokio::task::yield_now().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the running replacement is not duplicated"
    );
}

struct PendingSummary {
    entered: Arc<tokio::sync::Notify>,
    exited: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl LlmProvider for PendingSummary {
    fn provider_name(&self) -> &str {
        "pending-summary"
    }

    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, AppError> {
        struct OnDrop<'a>(&'a tokio::sync::Notify);
        impl Drop for OnDrop<'_> {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        let _exit = OnDrop(&self.exited);
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn chat_stream(
        &self,
        _: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        unreachable!("fake collect delegates to chat")
    }

    fn count_tokens(&self, _: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[tokio::test(start_paused = true)]
async fn preheat_regression_timeout_reaps_provider_before_foreground_can_start() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let exited = Arc::new(tokio::sync::Notify::new());
    let mut preheat = Preheat::new();
    assert!(preheat.try_start(
        1.2,
        &[user_msg_with_id("q", "question")],
        std::path::Path::new(""),
        None,
        Arc::new(PendingSummary {
            entered: entered.clone(),
            exited: exited.clone()
        }),
        None,
        None,
        &ContextConfig::default(),
        Arc::new(crate::infra::ScopedEventEmitter::new(
            Arc::new(DefaultEventBus::new()),
            "handoff"
        )),
        None,
    ));
    entered.notified().await;
    let outcome = preheat.await_result(Duration::from_secs(30)).await;
    assert!(!matches!(outcome, PreheatOutcome::Completed(_)));
    // Never yield here: returning from the wait admits the foreground request immediately.
    assert!(
        futures_util::FutureExt::now_or_never(exited.notified()).is_some(),
        "foreground is admitted while the background provider still runs"
    );
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
async fn preheat_failure_does_not_retry_outside_provider() {
    let (provider, calls) = AlwaysFailingProvider::new();
    let provider: Arc<dyn LlmProvider> = Arc::new(provider);
    let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let captured = Arc::new(std::sync::Mutex::new(None::<EventContext>));
    let captured_cb = Arc::clone(&captured);
    event_bus.on(
        wire::WIRE_COMPACTION_ERROR,
        Box::new(move |ctx: EventContext| {
            *captured_cb.lock().unwrap() = Some(ctx);
            Ok(())
        }),
    );
    let event_emitter = Arc::new(crate::infra::ScopedEventEmitter::new(
        Arc::clone(&event_bus),
        "sid-preheat-test",
    ));
    let cfg = ContextConfig::default();

    let messages = vec![
        message_with_id(ChatMessageRole::User, "u1", "hi"),
        message_with_id(ChatMessageRole::Assistant, "a1", "hello"),
    ];
    let dummy_path = std::path::PathBuf::new(); // 空路径 → spawn 内 transcript 写入跳过

    let mut preheat = Preheat::new();
    let did_start = preheat.try_start(
        0.95,
        &messages,
        &dummy_path,
        None,
        provider,
        None,
        None,
        &cfg,
        Arc::clone(&event_emitter),
        None,
    );
    assert!(
        did_start,
        "Preheat 在 ratio=0.95 + 非空 messages 时必须启动"
    );

    let outcome = preheat.await_result(Duration::from_secs(60)).await;
    assert!(matches!(outcome, PreheatOutcome::Exhausted));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let ctx = captured
        .lock()
        .unwrap()
        .clone()
        .expect("耗尽后应发出 compaction_error");
    assert_eq!(ctx.payload["attempts"], 1);
    assert_eq!(ctx.session_id.as_deref(), Some("sid-preheat-test"));
    assert_eq!(
        ctx.payload.get("sessionId").and_then(|v| v.as_str()),
        Some("sid-preheat-test")
    );
    assert_eq!(
        ctx.payload.get("source").and_then(|v| v.as_str()),
        Some("preheat")
    );
}

#[tokio::test(start_paused = true)]
async fn preheat_exhausted_keeps_its_unfulfilled_marker_and_emits_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failure_trail.jsonl");
    write_header(
        &path,
        &SessionHeader {
            r#type: "session".to_string(),
            origin: None,
            version: Some(3),
            id: "sid_d".to_string(),
            timestamp: "2026-04-26T00:00:00.000Z".to_string(),
            cwd: None,
            project_root: None,
        },
    )
    .unwrap();

    // The marker is appended at start, while `msg_end` is still the transcript tail.
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
    let event_emitter = Arc::new(crate::infra::ScopedEventEmitter::new(
        Arc::clone(&event_bus),
        "sid-preheat-test",
    ));
    let cfg = ContextConfig::default();

    let messages = vec![
        message_with_id(ChatMessageRole::User, "msg_start", "ping"),
        message_with_id(ChatMessageRole::Assistant, "msg_end", "pong"),
    ];

    let mut preheat = Preheat::new();
    let did_start = preheat.try_start(
        0.95,
        &messages,
        &path,
        None,
        provider,
        None,
        None,
        &cfg,
        Arc::clone(&event_emitter),
        None,
    );
    assert!(did_start);
    let outcome = preheat.await_result(Duration::from_secs(60)).await;
    assert!(matches!(outcome, PreheatOutcome::Exhausted));

    let entries = read_entries_tail(&path, 16).unwrap();
    let marker = entries
        .iter()
        .rev()
        .find_map(|e| match e {
            TranscriptEntry::BranchSummary(b) if b.is_boundary == Some(true) => Some(b.clone()),
            _ => None,
        })
        .expect("preheat start must leave its empty marker even if generation fails");

    assert_eq!(
        marker.summary,
        None,
        "failure diagnostics are emitted as CompactionError; the transcript marker stays unfulfilled"
    );
    assert_eq!(
        marker.covered_end_id.as_deref(),
        Some("msg_end"),
        "marker keeps the covered range for later diagnostics",
    );
}

#[test]
fn layer0_persist_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = make_state(60_000, 100_000, 25_000);
    let big_content = "x".repeat(60_000);
    // Layer 0 persists tool results from the last turn (after the last user message)
    let mut messages = vec![
        user_msg("question"),
        tool_msg_with_id("tc_1_msg", "tc_1", &big_content),
    ];
    let config = ContextConfig::default();
    let history_end = messages.len();
    let (results, _) = layer0_persist_large_results(
        &mut state,
        &mut messages,
        &config,
        dir.path(),
        "test_session",
        history_end,
    );
    assert_eq!(results.len(), 1);
    assert!(std::path::Path::new(&results[0].persisted_path).exists());
    assert!(state.estimate_context_chars < 60_000);
    // Check the tool message content was replaced
    let tool = messages
        .iter()
        .find(|m| m.role == ChatMessageRole::Tool)
        .unwrap();
    assert!(tool
        .text_content()
        .unwrap_or("")
        .starts_with("[Tool result persisted:"));
}
