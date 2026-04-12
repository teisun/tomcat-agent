//! 集成测试：TASK-17 上下文管理（大文件截断、多轮 Compaction、Session 重载、Context Overflow 重试）。
//! 黑盒测试，通过 pi_wasm 公共 API + 临时目录隔离。

mod common;

use async_trait::async_trait;
use pi_wasm::core::compaction::compact_tool_results;
use pi_wasm::{
    build_context_from_state, compound_turn_id, init_context_state, AgentLoop, AgentLoopConfig,
    AgentMessage, AppError, BashResult, ChatMessage, ChatRequest, ChatResponse, ContextConfig,
    ContextState, DefaultEventBus, DirEntry, EditFileResult, EditOperation, EventBus, EventContext,
    LlmProvider, PrimitiveExecutor, PrimitiveOperation, SessionManager, StreamEvent, ToolCallInfo,
    TurnEntry, WriteFileResult,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, info_span};

// ────────────────────── Mock 实现 ──────────────────────────────────────────

struct MockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl MockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlm {
    fn provider_name(&self) -> &str {
        "mock"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let mut guard = self.streams.lock().unwrap();
        let events = guard
            .pop_front()
            .ok_or_else(|| AppError::Llm("MockLlm: no more streams".to_string()))?;
        drop(guard);
        Ok(Box::new(tokio_stream::iter(events)))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct MockPrimitiveWithLargeFile {
    file_size: usize,
}

#[async_trait]
impl PrimitiveExecutor for MockPrimitiveWithLargeFile {
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok("x".repeat(self.file_size))
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<BashResult, AppError> {
        Ok(BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

// ────────────────────── 辅助 ──────────────────────────────────────────────

fn text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn temp_sessions_dir(label: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    std::env::temp_dir().join(format!(
        "pi_ctx_test_{}_{}_{}",
        label,
        std::process::id(),
        ms
    ))
}

// ────────────────────── 测试用例 ──────────────────────────────────────────

/// [Layer 1 + Layer 3 全链路] compact_tool_results 后仍超 ratio 时 force_drop_oldest_to_target 兜底
#[test]
fn test_compaction_pipeline_layer1_then_layer3_recovers_budget() {
    common::setup_logging();
    let _span = info_span!("test_compaction_pipeline_layer1_then_layer3_recovers_budget").entered();

    let mut turns = Vec::new();
    for i in 0..5 {
        let sid = format!("turn_{}", i);
        let eid = sid.clone();
        turns.push(TurnEntry::UserTurn {
            id: compound_turn_id(&sid, &eid),
            start_id: sid,
            end_id: eid,
            messages: vec![
                AgentMessage::User {
                    text: format!("question {}", i),
                },
                AgentMessage::ToolResult {
                    tool_call_id: format!("tc_{}", i),
                    content: "x".repeat(25_000),
                    is_error: false,
                },
                AgentMessage::Assistant {
                    text: format!("answer {}", i),
                    tool_calls: vec![],
                },
            ],
            timestamp: TEST_TS.to_string(),
        });
    }
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let budget_chars = 80_000;
    let budget_tokens = budget_chars / 4;
    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);
    assert!(reduced > 0);

    if state.usage_ratio() >= 0.50 {
        pi_wasm::core::compaction::force_drop_oldest_to_target(&mut state);
    }

    assert!(state.usage_ratio() < 0.50);
    assert!(!state.user_turns_list.is_empty());
}

/// [Session 重载] 写入消息与 Compaction entry 后 init_context_state 正确重建
///
/// 验证：创建会话 → 写消息 → 写 compaction → 再写消息 → init_context_state →
///       turns 数量正确、SummaryTurn 内容正确、后续 UserTurn 正确
/// 意义：TASK-17 Transcript 持久化与重载——跨进程会话恢复端到端
#[test]
fn test_session_reload_with_compaction_entries() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_with_compaction_entries").entered();

    let dir = temp_sessions_dir("reload");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    info!("Arrange: 写入 user/assistant 消息 → compaction entry → 更多消息");
    mgr.append_message(serde_json::json!({"role":"user","content":"old question 1"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer 1"}))?;
    mgr.append_message(serde_json::json!({"role":"user","content":"old question 2"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer 2"}))?;

    mgr.append_compaction_with_range(
        "## Goal\nUser wants help with coding.\n## Progress\nAnswered 2 questions.",
        None,
        None,
        2,
    )?;

    mgr.append_message(serde_json::json!({"role":"user","content":"new question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new answer"}))?;

    info!("Act: init_context_state 从 transcript 重建 ContextState");
    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system prompt")?;

    info!("Assert: 验证 turns 数量与内容");
    // Before compaction: 2 UserTurns (old Q1+A1, old Q2+A2)
    // Compaction entry → SummaryTurn
    // After compaction: 1 UserTurn (new Q + A)
    // Total expected: old turns + SummaryTurn + new turn = depends on grouping
    // Actually: entries order is msg, msg, msg, msg, compaction, msg, msg
    // The init_context_state groups: UserTurn(q1,a1), UserTurn(q2,a2), then compaction flushes → SummaryTurn, then UserTurn(new q, new a)
    assert!(
        state.user_turns_list.len() >= 3,
        "should have at least 3 groups: 2 old turns + summary + 1 new turn, got {}",
        state.user_turns_list.len()
    );

    let has_summary = state
        .user_turns_list
        .iter()
        .any(|t| matches!(t, TurnEntry::SummaryTurn { summary, .. } if summary.contains("Goal")));
    assert!(
        has_summary,
        "应含 SummaryTurn 且内容包含 compaction summary"
    );

    let has_new_turn = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("new question")))
        } else {
            false
        }
    });
    assert!(has_new_turn, "应含 compaction 之后的 new question UserTurn");

    let msgs = build_context_from_state(&state);
    assert!(msgs.len() >= 3, "展平后消息数应 >= 3");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [Context Overflow 重试] 首次 LLM 调用返回 context overflow 错误 → 触发 Compaction → 重试成功
///
/// 验证：AgentLoop 在 context overflow 时触发 AutoCompactionStart/End 事件，重试后返回成功文本
/// 意义：TASK-17 ContextOverflow 自动恢复路径——AgentLoop 端到端集成
#[tokio::test]
async fn test_context_overflow_triggers_compaction_and_retries(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_context_overflow_triggers_compaction_and_retries").entered();

    info!("Arrange: MockLlm 首次返回 context overflow 错误，第二次返回成功文本");
    let stream_err = vec![Err(AppError::Llm(
        "context length exceeded: 500000 tokens".to_string(),
    ))];
    let stream_ok = text_stream("recovered after compaction");
    let llm = Arc::new(MockLlm::new(vec![stream_err, stream_ok]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 100 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let compaction_started = Arc::new(AtomicBool::new(false));
    let compaction_ended = Arc::new(AtomicBool::new(false));
    let cs = Arc::clone(&compaction_started);
    let ce = Arc::clone(&compaction_ended);
    event_bus.on(
        "context_overflow_trim_start",
        Box::new(move |_ctx: EventContext| {
            cs.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    event_bus.on(
        "context_overflow_trim_end",
        Box::new(move |_ctx: EventContext| {
            ce.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );

    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-ctx-overflow".to_string(),
        max_attempts: 3,
        retry_base_delay_ms: 0,
        context_config: ContextConfig {
            keep_recent_turns: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let ctx_state = ContextState {
        user_turns_list: vec![
            TurnEntry::UserTurn {
                id: compound_turn_id("turn_old", "turn_old"),
                start_id: "turn_old".to_string(),
                end_id: "turn_old".to_string(),
                messages: vec![
                    AgentMessage::User {
                        text: "old question".to_string(),
                    },
                    AgentMessage::ToolResult {
                        tool_call_id: "tc1".to_string(),
                        content: "x".repeat(50_000),
                        is_error: false,
                    },
                ],
                timestamp: TEST_TS.to_string(),
            },
            TurnEntry::UserTurn {
                id: compound_turn_id("turn_recent", "turn_recent"),
                start_id: "turn_recent".to_string(),
                end_id: "turn_recent".to_string(),
                messages: vec![AgentMessage::User {
                    text: "recent question".to_string(),
                }],
                timestamp: TEST_TS.to_string(),
            },
        ],
        estimate_context_chars: 60_000,
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    agent.set_context_state(Some(ctx_state));

    let messages = vec![AgentMessage::User {
        text: "trigger overflow".to_string(),
    }];

    info!("Act: 调用 AgentLoop::run()，期望 context overflow → compaction → retry → 成功");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")??;

    info!("Assert: 最终成功返回，compaction 事件已触发");
    assert!(
        result.final_text.contains("recovered"),
        "Context overflow 重试后应返回成功文本，实际: {:?}",
        result.final_text
    );
    assert!(
        compaction_started.load(Ordering::SeqCst),
        "应触发 context_overflow_trim_start 事件"
    );
    assert!(
        compaction_ended.load(Ordering::SeqCst),
        "应触发 context_overflow_trim_end 事件"
    );

    let recovered_state = agent.take_context_state();
    assert!(
        recovered_state.is_some(),
        "AgentLoop 完成后仍应持有 context_state"
    );

    Ok(())
}

/// [build_context_from_state 端到端] SummaryTurn + UserTurn 混合展平后消息顺序正确
///
/// 验证：SummaryTurn 转为 CompactionSummary，UserTurn 展平为原始消息，顺序保持
/// 意义：TASK-17 上下文重建——build_context_from_state 正确性的端到端验证
#[test]
fn test_build_context_preserves_order_with_mixed_turns() {
    common::setup_logging();
    let _span = info_span!("test_build_context_preserves_order_with_mixed_turns").entered();

    let state = ContextState {
        user_turns_list: vec![
            TurnEntry::SummaryTurn {
                id: "sum_1".to_string(),
                summary: "## Goal\nBuild a web app".to_string(),
                timestamp: TEST_TS.to_string(),
            },
            TurnEntry::UserTurn {
                id: compound_turn_id("turn_1_u", "turn_1_tr"),
                start_id: "turn_1_u".to_string(),
                end_id: "turn_1_tr".to_string(),
                messages: vec![
                    AgentMessage::User {
                        text: "add auth".to_string(),
                    },
                    AgentMessage::Assistant {
                        text: "I'll add JWT auth".to_string(),
                        tool_calls: vec![ToolCallInfo {
                            id: "tc1".to_string(),
                            name: "write_file".to_string(),
                            arguments: r#"{"path":"auth.rs"}"#.to_string(),
                        }],
                    },
                    AgentMessage::ToolResult {
                        tool_call_id: "tc1".to_string(),
                        content: "file written".to_string(),
                        is_error: false,
                    },
                ],
                timestamp: TEST_TS.to_string(),
            },
            TurnEntry::UserTurn {
                id: compound_turn_id("turn_2", "turn_2"),
                start_id: "turn_2".to_string(),
                end_id: "turn_2".to_string(),
                messages: vec![AgentMessage::User {
                    text: "run tests".to_string(),
                }],
                timestamp: TEST_TS.to_string(),
            },
        ],
        estimate_context_chars: 500,
        context_budget_chars: 10_000,
        context_budget_tokens: 2_500,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let msgs = build_context_from_state(&state);

    assert_eq!(msgs.len(), 5, "应展平为 5 条消息");
    assert!(
        matches!(&msgs[0], AgentMessage::CompactionSummary { summary } if summary.contains("Goal"))
    );
    assert!(matches!(&msgs[1], AgentMessage::User { text } if text == "add auth"));
    assert!(matches!(&msgs[2], AgentMessage::Assistant { .. }));
    assert!(matches!(&msgs[3], AgentMessage::ToolResult { .. }));
    assert!(matches!(&msgs[4], AgentMessage::User { text } if text == "run tests"));
}

// ────────── Layer 1 深度验证测试 ──────────────────────────────────────────

const TEST_TS: &str = "2026-04-04T12:00:00Z";

fn make_turn_with_tool_result(user_text: &str, tool_content: &str) -> TurnEntry {
    let sid = format!("turn_{}", user_text);
    let eid = sid.clone();
    TurnEntry::UserTurn {
        id: compound_turn_id(&sid, &eid),
        start_id: sid,
        end_id: eid,
        messages: vec![
            AgentMessage::User {
                text: user_text.to_string(),
            },
            AgentMessage::ToolResult {
                tool_call_id: "tc".to_string(),
                content: tool_content.to_string(),
                is_error: false,
            },
            AgentMessage::Assistant {
                text: "ok".to_string(),
                tool_calls: vec![],
            },
        ],
        timestamp: TEST_TS.to_string(),
    }
}

const PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

/// [Layer 1 深度] 占位符替换正确性：旧 turn 的超大 tool result 被替换为占位符，保护区内 turn 保持原内容
#[test]
fn test_compact_tool_results_replaces_with_placeholder() {
    common::setup_logging();
    let _span = info_span!("test_compact_tool_results_replaces_with_placeholder").entered();

    let big = "x".repeat(25_000);
    let mut state = ContextState {
        user_turns_list: vec![
            make_turn_with_tool_result("q1", &big),
            make_turn_with_tool_result("q2", &big),
            make_turn_with_tool_result("q3-recent", &big),
        ],
        estimate_context_chars: 0,
        context_budget_chars: 0,
        context_budget_tokens: 0,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    let total: usize = state
        .user_turns_list
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();
    state.estimate_context_chars = total;
    state.context_budget_chars = total / 3;

    info!("Act: compact_tool_results with keep_recent=1");
    compact_tool_results(&mut state, &ContextConfig::default(), 1);

    info!("Assert: old turns replaced, recent preserved");
    for (i, turn) in state.user_turns_list.iter().enumerate() {
        if let TurnEntry::UserTurn { messages, .. } = turn {
            for msg in messages {
                if let AgentMessage::ToolResult { content, .. } = msg {
                    if i < 2 {
                        assert_eq!(
                            content, PLACEHOLDER,
                            "turn {} tool result should be placeholder",
                            i
                        );
                    } else {
                        assert_eq!(
                            content, &big,
                            "turn {} (protected recent) should keep original",
                            i
                        );
                    }
                }
            }
        }
    }
}

/// [Layer 1 深度] compactable zone 内超过占位符阈值的 tool results 均被替换
#[test]
fn test_compact_tool_results_replaces_all_large_in_compactable_zone() {
    common::setup_logging();
    let _span =
        info_span!("test_compact_tool_results_replaces_all_large_in_compactable_zone").entered();

    let big = "x".repeat(25_000);
    let small = "x".repeat(5_000);
    let turns = vec![
        make_turn_with_tool_result("q1", &big),
        make_turn_with_tool_result("q2", &small),
        make_turn_with_tool_result("q3", &big),
        make_turn_with_tool_result("q4-recent", &big),
    ];
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: total,
        context_budget_tokens: total / 4,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    info!("Act: compact with m=1, only >threshold in compactable zone get replaced");
    compact_tool_results(&mut state, &ContextConfig::default(), 1);

    let get_tool_content = |idx: usize| -> String {
        if let TurnEntry::UserTurn { messages, .. } = &state.user_turns_list[idx] {
            messages
                .iter()
                .find_map(|m| {
                    if let AgentMessage::ToolResult { content, .. } = m {
                        Some(content.clone())
                    } else {
                        None
                    }
                })
                .unwrap()
        } else {
            panic!("not a UserTurn");
        }
    };
    assert_eq!(
        get_tool_content(0),
        PLACEHOLDER,
        "first (above threshold) should be replaced"
    );
    assert_eq!(
        get_tool_content(1),
        small,
        "second (below threshold) should keep original"
    );
    assert_eq!(
        get_tool_content(2),
        PLACEHOLDER,
        "third (above threshold) should be replaced"
    );
    assert_eq!(
        get_tool_content(3),
        big,
        "fourth (protected) should keep original"
    );
}

/// [Layer 1 深度] estimate 精确变化量（仅超过占位符阈值时触发替换）
#[test]
fn test_compact_tool_results_estimate_precise() {
    common::setup_logging();
    let _span = info_span!("test_compact_tool_results_estimate_precise").entered();

    let content_len = 25_000;
    let big = "y".repeat(content_len);
    let turns = vec![
        make_turn_with_tool_result("q1", &big),
        make_turn_with_tool_result("q2-recent", &big),
    ];
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: 1,
        context_budget_tokens: 0,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };

    let reduced = compact_tool_results(&mut state, &ContextConfig::default(), 1);

    let expected_reduced = content_len - PLACEHOLDER.len();
    assert_eq!(
        reduced, expected_reduced,
        "reduced should be exactly original_len - placeholder_len"
    );
    assert_eq!(
        state.estimate_context_chars,
        total - expected_reduced,
        "estimate should be total - reduced"
    );
}

/// [V2 集成] Session 重载识别 compact boundary 无重复
#[test]
fn test_session_reload_with_boundary() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_with_boundary").entered();

    let dir = temp_sessions_dir("boundary");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"old question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"old answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();
    let boundary = pi_wasm::core::session::transcript::TranscriptEntry::BranchSummary(
        pi_wasm::core::session::transcript::BranchSummaryEntry {
            id: None,
            parent_id: None,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Some("Summary of everything before this point".to_string()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(1),
            is_boundary: Some(true),
            preheat_compaction_id: None,
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
        },
    );
    pi_wasm::core::session::transcript::append_entry(&path, &boundary)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"new question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"new answer"}))?;

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system")?;

    let has_old = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("old")))
        } else {
            false
        }
    });
    assert!(!has_old, "turns before boundary should be discarded");

    let has_summary = state.user_turns_list.iter().any(|t| {
        matches!(t, TurnEntry::SummaryTurn { summary, .. } if summary.contains("Summary of everything"))
    });
    assert!(has_summary, "boundary summary should be present");

    let has_new = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("new")))
        } else {
            false
        }
    });
    assert!(has_new, "turns after boundary should be present");
    assert_eq!(state.user_turns_list.len(), 2, "summary + 1 new turn");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [V2 集成] Layer 0 大文件落盘可读回
#[test]
fn test_layer0_persist_and_readback() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_layer0_persist_and_readback").entered();

    use pi_wasm::core::compaction::layer0_persist_large_results;

    let dir = tempfile::tempdir()?;
    let original = "important content ".repeat(4000);
    let mut state = ContextState {
        user_turns_list: vec![TurnEntry::UserTurn {
            id: compound_turn_id("turn_persist", "turn_persist"),
            start_id: "turn_persist".to_string(),
            end_id: "turn_persist".to_string(),
            messages: vec![AgentMessage::ToolResult {
                tool_call_id: "tc_persist".into(),
                content: original.clone(),
                is_error: false,
            }],
            timestamp: TEST_TS.to_string(),
        }],
        estimate_context_chars: original.len(),
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        preheat: pi_wasm::core::compaction::preheat::Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    let config = ContextConfig::default();
    let (results, _) =
        layer0_persist_large_results(&mut state, &config, dir.path(), "sess_persist");
    assert_eq!(results.len(), 1);

    let readback = std::fs::read_to_string(&results[0].persisted_path)?;
    assert_eq!(
        readback, original,
        "persisted content should match original"
    );

    if let TurnEntry::UserTurn { messages, .. } = &state.user_turns_list[0] {
        if let AgentMessage::ToolResult { content, .. } = &messages[0] {
            assert!(content.starts_with("[Tool result persisted:"));
            assert!(content.contains("Preview:"));
        }
    }

    assert!(
        state.estimate_context_chars < original.len(),
        "estimate should decrease after persistence"
    );

    Ok(())
}

/// [TASK-20 集成] Session 重载：is_boundary=false 被跳过、is_boundary=true 生效
#[test]
fn test_session_reload_boundary_false_skipped() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_boundary_false_skipped").entered();

    let dir = temp_sessions_dir("boundary_false");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"first question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"first answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();

    let msg_ids: Vec<String> = pi_wasm::core::session::transcript::read_entries_tail(&path, 500)?
        .into_iter()
        .filter_map(|e| {
            if let pi_wasm::core::session::transcript::TranscriptEntry::Message(me) = e {
                me.id
            } else {
                None
            }
        })
        .collect();
    assert!(msg_ids.len() >= 2, "expect user+assistant message ids");
    let covered_start = msg_ids[0].clone();
    let covered_end = msg_ids[1].clone();
    let compact_id = compound_turn_id(&covered_start, &covered_end);

    let preheat_entry = pi_wasm::core::session::transcript::TranscriptEntry::BranchSummary(
        pi_wasm::core::session::transcript::BranchSummaryEntry {
            id: Some(compact_id.clone()),
            parent_id: None,
            timestamp: "2026-01-01T00:00:01.000Z".to_string(),
            summary: Some("Preheat summary (should be ignored)".to_string()),
            covered_start_id: Some(covered_start),
            covered_end_id: Some(covered_end),
            covered_count: Some(1),
            is_boundary: Some(false),
            preheat_compaction_id: Some(compact_id),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
        },
    );
    pi_wasm::core::session::transcript::append_entry(&path, &preheat_entry)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"second question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"second answer"}))?;

    let cfg = ContextConfig::default();
    let state = init_context_state(&mgr, &cfg, "system")?;

    let has_preheat_summary = state.user_turns_list.iter().any(|t| {
        matches!(t, TurnEntry::SummaryTurn { summary, .. } if summary.contains("Preheat summary"))
    });
    assert!(
        !has_preheat_summary,
        "is_boundary=false entry should be skipped during reload"
    );

    let has_first = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("first")))
        } else {
            false
        }
    });
    assert!(has_first, "original turns should still be present");

    let has_second = state.user_turns_list.iter().any(|t| {
        if let TurnEntry::UserTurn { messages, .. } = t {
            messages
                .iter()
                .any(|m| matches!(m, AgentMessage::User { text } if text.contains("second")))
        } else {
            false
        }
    });
    assert!(has_second, "turns after preheat entry should be present");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// [TASK-20] 重载后未应用 preheat：`restore_completed` + `poll_result` 可取回摘要
#[test]
fn test_session_reload_pending_preheat_restore() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = info_span!("test_session_reload_pending_preheat_restore").entered();

    let dir = temp_sessions_dir("preheat_restore");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mgr = SessionManager::new(dir.clone());
    let key = mgr.current_session_key();
    mgr.create_session(key, None)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"first question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"first answer"}))?;

    let path = mgr.current_transcript_path()?.unwrap();
    let msg_ids: Vec<String> = pi_wasm::core::session::transcript::read_entries_tail(&path, 500)?
        .into_iter()
        .filter_map(|e| {
            if let pi_wasm::core::session::transcript::TranscriptEntry::Message(me) = e {
                me.id
            } else {
                None
            }
        })
        .collect();
    assert!(msg_ids.len() >= 2);
    let covered_start = msg_ids[0].clone();
    let covered_end = msg_ids[1].clone();
    let compact_id = compound_turn_id(&covered_start, &covered_end);

    let preheat_entry = pi_wasm::core::session::transcript::TranscriptEntry::BranchSummary(
        pi_wasm::core::session::transcript::BranchSummaryEntry {
            id: Some(compact_id.clone()),
            parent_id: None,
            timestamp: "2026-01-01T00:00:01.000Z".to_string(),
            summary: Some("Restored preheat summary body".to_string()),
            covered_start_id: Some(covered_start),
            covered_end_id: Some(covered_end),
            covered_count: Some(1),
            is_boundary: Some(false),
            preheat_compaction_id: Some(compact_id.clone()),
            estimated_covered_tokens_before: None,
            estimated_summary_tokens: None,
            estimated_tokens_saved: None,
        },
    );
    pi_wasm::core::session::transcript::append_entry(&path, &preheat_entry)?;

    mgr.append_message(serde_json::json!({"role":"user","content":"second question"}))?;
    mgr.append_message(serde_json::json!({"role":"assistant","content":"second answer"}))?;

    let cfg = ContextConfig::default();
    let mut state = init_context_state(&mgr, &cfg, "system")?;

    assert!(
        state.preheat.is_finished(),
        "reload should leave CachedCompleted preheat"
    );
    use pi_wasm::core::compaction::preheat::PreheatOutcome;
    match state.preheat.poll_result() {
        PreheatOutcome::Completed(r) => {
            assert!(
                r.summary_text.contains("Restored preheat summary"),
                "poll should return disk preheat summary"
            );
            assert_eq!(
                r.transcript_compaction_entry_id.as_deref(),
                Some(compact_id.as_str())
            );
        }
        o => panic!("expected Completed, got {:?}", o),
    }
    assert!(state.preheat.is_idle());

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
