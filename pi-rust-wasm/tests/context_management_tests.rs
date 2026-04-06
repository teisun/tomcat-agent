//! 集成测试：TASK-17 上下文管理（大文件截断、多轮 Compaction、Session 重载、Context Overflow 重试）。
//! 黑盒测试，通过 pi_wasm 公共 API + 临时目录隔离。

mod common;

use async_trait::async_trait;
use pi_wasm::core::compaction::{
    compact_tool_results, force_drop_oldest, run_compaction_loop, truncate_tool_result_if_needed,
};
use pi_wasm::{
    build_context_from_state, init_context_state, wire, AgentLoop, AgentLoopConfig, AgentMessage,
    AppError, BashResult, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice,
    ContextConfig, ContextState, DefaultEventBus, DirEntry, EditFileResult, EditOperation,
    EventBus, EventContext, LlmProvider, PrimitiveExecutor, PrimitiveOperation, SessionManager,
    StreamEvent, ToolCallInfo, TurnEntry, WriteFileResult,
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

fn tool_call_stream(id: &str, name: &str, args: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(args.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
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

/// [Layer 0 端到端] AgentLoop 对超大 tool result 自动截断并发出 ToolResultTruncated 事件
///
/// 验证：read_file 返回 600K 字符内容时，ToolResultTruncated 事件被发布
/// 意义：TASK-17 Layer 0 单条 tool result 截断与事件通知——端到端集成
#[tokio::test]
async fn test_large_tool_result_triggers_truncation_event() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = info_span!("test_large_tool_result_triggers_truncation_event").entered();

    info!("Arrange: MockLlm 返回 read_file 工具调用，MockPrimitive 返回 600K 字符内容");
    let stream_tool = tool_call_stream("rf1", "read_file", r#"{"path":"/big.txt"}"#);
    let stream_text = text_stream("done reading");
    let llm = Arc::new(MockLlm::new(vec![stream_tool, stream_text]));
    let primitive = Arc::new(MockPrimitiveWithLargeFile { file_size: 600_000 });
    let event_bus = Arc::new(DefaultEventBus::new());

    let truncated_event: Arc<Mutex<Option<(String, usize, usize)>>> = Arc::new(Mutex::new(None));
    let ev_clone = Arc::clone(&truncated_event);
    event_bus.on(
        wire::WIRE_TOOL_RESULT_TRUNCATED,
        Box::new(move |ctx: EventContext| {
            let tool = ctx
                .payload
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let orig = ctx
                .payload
                .get("originalChars")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let trunc = ctx
                .payload
                .get("truncatedChars")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            *ev_clone.lock().unwrap() = Some((tool, orig, trunc));
            Ok(())
        }),
    );

    let config = AgentLoopConfig {
        model: "mock-model".to_string(),
        session_id: "sess-truncation".to_string(),
        max_attempts: 1,
        retry_base_delay_ms: 0,
        context_config: ContextConfig {
            single_tool_result_max_chars: 400_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let abort = Arc::new(AtomicBool::new(false));
    let mut agent = AgentLoop::new(llm, primitive, event_bus, config, abort);

    let ctx_state = ContextState {
        user_turns_list: Vec::new(),
        estimate_context_chars: 0,
        context_budget_chars: 1_000_000,
        context_budget_tokens: 250_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    agent.set_context_state(Some(ctx_state));

    let messages = vec![AgentMessage::User {
        text: "read a big file".to_string(),
    }];

    info!("Act: 调用 AgentLoop::run()");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.run(messages))
        .await
        .map_err(|_| "run() 超时 10s")??;

    info!(
        "Assert: ToolResultTruncated 事件已发布: {:?}",
        truncated_event.lock().unwrap()
    );
    let ev = truncated_event.lock().unwrap().take();
    assert!(ev.is_some(), "应发布 ToolResultTruncated 事件");
    let (tool, orig, trunc) = ev.unwrap();
    assert_eq!(tool, "read_file");
    assert_eq!(orig, 600_000);
    assert!(trunc < orig, "截断后字符数应小于原始");
    assert!(
        result.final_text.contains("done reading"),
        "截断后 AgentLoop 应继续并返回最终文本"
    );

    Ok(())
}

/// [Layer 1 + Layer 3 全链路] compact_tool_results 后仍超预算时 force_drop_oldest 兜底恢复
///
/// 验证：构造超预算 ContextState → Layer 1 压缩 → 仍超预算 → Layer 3 强制删除 → 回到预算内
/// 意义：TASK-17 四层防护协同——Layer 1 与 Layer 3 的端到端集成
#[test]
fn test_compaction_pipeline_layer1_then_layer3_recovers_budget() {
    common::setup_logging();
    let _span = info_span!("test_compaction_pipeline_layer1_then_layer3_recovers_budget").entered();

    info!("Arrange: 构造含 5 个 turn 的 ContextState，总字符远超 budget (tool results >20K)");
    let mut turns = Vec::new();
    for i in 0..5 {
        turns.push(TurnEntry::UserTurn {
            id: format!("turn_{}", i),
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

    let budget_chars = 30_000;
    let budget_tokens = budget_chars / 4;
    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: budget_chars,
        context_budget_tokens: budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    assert!(state.is_over_budget());

    info!("Act: Layer 1 compact_tool_results (keep_recent=1)");
    let reduced = compact_tool_results(&mut state, 1);
    info!(
        "Layer 1 reduced {} chars, still over? {}",
        reduced,
        state.is_over_budget()
    );
    assert!(reduced > 0, "Layer 1 应替换了部分 tool results");

    if state.is_over_budget() {
        info!("Act: Layer 3 force_drop_oldest");
        force_drop_oldest(&mut state);
    }

    info!("Assert: 最终 ContextState 在预算内");
    assert!(
        !state.is_over_budget(),
        "Layer 1 + Layer 3 后应回到预算内: estimate={}, budget={}",
        state.estimate_context_chars,
        state.context_budget_chars
    );
    assert!(!state.user_turns_list.is_empty(), "至少保留一个 turn");
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
        "auto_compaction_start",
        Box::new(move |_ctx: EventContext| {
            cs.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    event_bus.on(
        "auto_compaction_end",
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
                id: "turn_old".to_string(),
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
                id: "turn_recent".to_string(),
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
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
        "应触发 auto_compaction_start 事件"
    );
    assert!(
        compaction_ended.load(Ordering::SeqCst),
        "应触发 auto_compaction_end 事件"
    );

    let recovered_state = agent.take_context_state();
    assert!(
        recovered_state.is_some(),
        "AgentLoop 完成后仍应持有 context_state"
    );

    Ok(())
}

/// [大文件截断 Unicode 安全] 含中文的大工具结果截断后不会出现残缺 UTF-8
///
/// 验证：truncate_tool_result_if_needed 对多字节 UTF-8 内容截断后，结果是合法 UTF-8
/// 意义：TASK-17 Layer 0 Unicode 安全——跨语言场景的鲁棒性
#[test]
fn test_truncation_unicode_safety_integration() {
    common::setup_logging();
    let _span = info_span!("test_truncation_unicode_safety_integration").entered();

    info!("Arrange: 生成 200K 中文字符串（600K bytes）");
    let mut content = "你好世界\n".repeat(50_000);
    let original_len = content.len();

    info!("Act: 截断到 400K 字符限制");
    let info = truncate_tool_result_if_needed(&mut content, 400_000);
    assert!(info.is_some());

    info!("Assert: 截断后为合法 UTF-8 且长度合理");
    assert!(content.len() < original_len, "截断后应比原始短");
    let _ = content.chars().count(); // panics if invalid UTF-8
    assert!(
        content.ends_with("[truncated — original content exceeded limit]"),
        "应以截断后缀结尾"
    );
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
                id: "turn_1".to_string(),
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
                id: "turn_2".to_string(),
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
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

// ────────────────────── MockCompactionLlm ──────────────────────────────────

struct MockCompactionLlm {
    summaries: Mutex<VecDeque<Result<String, AppError>>>,
    captured_requests: Mutex<Vec<ChatRequest>>,
}

impl MockCompactionLlm {
    fn new(summaries: Vec<Result<String, AppError>>) -> Self {
        Self {
            summaries: Mutex::new(summaries.into()),
            captured_requests: Mutex::new(Vec::new()),
        }
    }

    fn captured_count(&self) -> usize {
        self.captured_requests.lock().unwrap().len()
    }

    fn captured_all_message_texts(&self) -> Vec<String> {
        self.captured_requests
            .lock()
            .unwrap()
            .iter()
            .flat_map(|req| {
                req.messages
                    .iter()
                    .filter_map(|m| m.text_content().map(|s| s.to_string()))
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for MockCompactionLlm {
    fn provider_name(&self) -> &str {
        "mock-compaction"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        self.captured_requests.lock().unwrap().push(req);
        let result = self
            .summaries
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(AppError::Llm("no more summaries".into())));
        let text = result?;
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(&text),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Err(AppError::Llm(
            "MockCompactionLlm: chat_stream not supported".into(),
        ))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

// ────────── Layer 1 深度验证测试 ──────────────────────────────────────────

const TEST_TS: &str = "2026-04-04T12:00:00Z";

fn make_turn_with_tool_result(user_text: &str, tool_content: &str) -> TurnEntry {
    TurnEntry::UserTurn {
        id: format!("turn_{}", user_text),
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

/// [Layer 1 深度] 占位符替换正确性：旧 turn 的 >20K tool result 被替换为占位符，保护区内 turn 保持原内容
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    let total: usize = state
        .user_turns_list
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();
    state.estimate_context_chars = total;
    state.context_budget_chars = total / 3;

    info!("Act: compact_tool_results with keep_recent=1");
    compact_tool_results(&mut state, 1);

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

/// [Layer 1 深度] Layer 1 V2 替换所有 >20K tool results in compactable zone
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };

    info!("Act: compact with m=1, only >20K in compactable zone get replaced");
    compact_tool_results(&mut state, 1);

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
        "first (>20K) should be replaced"
    );
    assert_eq!(
        get_tool_content(1),
        small,
        "second (<20K) should keep original"
    );
    assert_eq!(
        get_tool_content(2),
        PLACEHOLDER,
        "third (>20K) should be replaced"
    );
    assert_eq!(
        get_tool_content(3),
        big,
        "fourth (protected) should keep original"
    );
}

/// [Layer 1 深度] estimate 精确变化量（>20K 阈值才触发替换）
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };

    let reduced = compact_tool_results(&mut state, 1);

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

// ────────── Layer 2 深度验证测试 ──────────────────────────────────────────

fn make_big_turn(user_text: &str, size: usize) -> TurnEntry {
    TurnEntry::UserTurn {
        id: format!("big_{}", user_text),
        messages: vec![
            AgentMessage::User {
                text: user_text.to_string(),
            },
            AgentMessage::Assistant {
                text: "a".repeat(size),
                tool_calls: vec![],
            },
        ],
        timestamp: TEST_TS.to_string(),
    }
}

/// [Layer 2] 单批压缩：6 turns 超预算，LLM 返回短摘要，压缩后回预算
#[tokio::test]
async fn test_compaction_loop_single_batch() {
    common::setup_logging();
    let _span = info_span!("test_compaction_loop_single_batch").entered();

    let turns: Vec<TurnEntry> = (0..6)
        .map(|i| make_big_turn(&format!("q{}", i), 2_000))
        .collect();
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();
    let budget = total / 2;

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: budget,
        context_budget_tokens: budget / 4,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    assert!(state.is_over_budget());

    let short_summary = "## Goal\nUser asked 4 questions.".to_string();
    let llm = MockCompactionLlm::new(vec![Ok(short_summary)]);
    let config = ContextConfig {
        keep_recent_turns: 2,
        compaction_turns: 4,
        ..Default::default()
    };

    info!("Act: run_compaction_loop");
    let result = run_compaction_loop(&mut state, &llm, &config, std::path::Path::new("")).await;
    assert!(result.is_ok());

    info!("Assert: turns compressed, back within budget");
    assert_eq!(
        state.user_turns_list.len(),
        3,
        "4 old drained + 1 SummaryTurn inserted + 2 recent = 3"
    );
    assert!(
        matches!(&state.user_turns_list[0], TurnEntry::SummaryTurn { summary, .. } if summary.contains("Goal")),
        "first turn should be SummaryTurn"
    );
    assert!(
        !state.is_over_budget(),
        "should be within budget after compaction"
    );
    assert_eq!(llm.captured_count(), 1, "LLM chat called exactly once");
}

/// [Layer 2] 多批循环：10 turns 大幅超预算，需要 2+ 轮 LLM 调用
#[tokio::test]
async fn test_compaction_loop_multi_batch() {
    common::setup_logging();
    let _span = info_span!("test_compaction_loop_multi_batch").entered();

    let turns: Vec<TurnEntry> = (0..10)
        .map(|i| make_big_turn(&format!("q{}", i), 2_000))
        .collect();
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();
    let budget = total / 2;

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: budget,
        context_budget_tokens: budget / 4,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };

    let llm = MockCompactionLlm::new(vec![
        Ok("## Summary batch 1".into()),
        Ok("## Summary batch 2".into()),
        Ok("## Summary batch 3".into()),
    ]);
    let config = ContextConfig {
        keep_recent_turns: 2,
        compaction_turns: 3,
        ..Default::default()
    };

    info!("Act: run_compaction_loop with small batches");
    let result = run_compaction_loop(&mut state, &llm, &config, std::path::Path::new("")).await;
    assert!(result.is_ok());

    info!("Assert: multiple LLM calls were needed");
    assert!(
        llm.captured_count() >= 2,
        "should need multiple LLM calls, got {}",
        llm.captured_count()
    );
    assert!(
        !state.is_over_budget(),
        "should be within budget after multi-batch compaction: estimate={}, budget={}",
        state.estimate_context_chars,
        state.context_budget_chars
    );
}

/// [Layer 2] UPDATE 模式：batch 含 SummaryTurn 时使用 UPDATE prompt
#[tokio::test]
async fn test_compaction_loop_update_mode() {
    common::setup_logging();
    let _span = info_span!("test_compaction_loop_update_mode").entered();

    let turns = vec![
        TurnEntry::SummaryTurn {
            id: "sum_old".to_string(),
            summary: "old summary about coding".to_string(),
            timestamp: TEST_TS.to_string(),
        },
        make_big_turn("q1", 3_000),
        make_big_turn("q2", 3_000),
        make_big_turn("q3-recent", 500),
    ];
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: total / 3,
        context_budget_tokens: total / 12,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };

    let llm = MockCompactionLlm::new(vec![Ok("## Updated summary".into())]);
    let config = ContextConfig {
        keep_recent_turns: 1,
        compaction_turns: 3,
        ..Default::default()
    };

    info!("Act: run_compaction_loop with SummaryTurn in batch");
    let result = run_compaction_loop(&mut state, &llm, &config, std::path::Path::new("")).await;
    assert!(result.is_ok());

    info!("Assert: UPDATE prompt used (contains 'Existing summary')");
    let texts = llm.captured_all_message_texts();
    assert!(
        !texts.is_empty(),
        "should have captured at least one request"
    );
    let has_update_marker = texts.iter().any(|t| t.contains("Existing summary"));
    assert!(
        has_update_marker,
        "at least one message should contain UPDATE template marker 'Existing summary', got: {:?}",
        texts
            .iter()
            .map(|t| &t[..80.min(t.len())])
            .collect::<Vec<_>>()
    );
}

/// [Layer 2] LLM 报错时优雅降级：state 不变，函数返回 Ok
#[tokio::test]
async fn test_compaction_loop_llm_error_degrades() {
    common::setup_logging();
    let _span = info_span!("test_compaction_loop_llm_error_degrades").entered();

    let turns: Vec<TurnEntry> = (0..4)
        .map(|i| make_big_turn(&format!("q{}", i), 2_000))
        .collect();
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: total / 2,
        context_budget_tokens: total / 8,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    let original_len = state.user_turns_list.len();
    let original_estimate = state.estimate_context_chars;

    let llm = MockCompactionLlm::new(vec![Err(AppError::Llm("API error".into()))]);
    let config = ContextConfig {
        keep_recent_turns: 1,
        compaction_turns: 3,
        ..Default::default()
    };

    info!("Act: run_compaction_loop with LLM error");
    let result = run_compaction_loop(&mut state, &llm, &config, std::path::Path::new("")).await;

    info!("Assert: graceful degradation — state unchanged, Ok returned");
    assert!(result.is_ok(), "should return Ok even on LLM error");
    assert_eq!(
        state.user_turns_list.len(),
        original_len,
        "turns should be unchanged"
    );
    assert_eq!(
        state.estimate_context_chars, original_estimate,
        "estimate should be unchanged"
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
    let boundary = pi_wasm::core::session::transcript::TranscriptEntry::Compaction(
        pi_wasm::core::session::transcript::CompactionEntry {
            id: None,
            parent_id: None,
            timestamp: "2026-01-01T00:00:00.000Z".to_string(),
            summary: Some("Summary of everything before this point".to_string()),
            covered_start_id: None,
            covered_end_id: None,
            covered_count: Some(1),
            is_boundary: Some(true),
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
    let original = "important content ".repeat(2000);
    let mut state = ContextState {
        user_turns_list: vec![TurnEntry::UserTurn {
            id: "turn_persist".to_string(),
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
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    let config = ContextConfig::default();
    let results = layer0_persist_large_results(&mut state, &config, dir.path(), "sess_persist");
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

/// [V2 集成] cascade_params buffer 安全网在小窗口模型下正确触发
/// ratio < 0.70 不触发水位线，但 remaining < autocompact_buf 触发 buffer 安全网
#[test]
fn test_cascade_params_small_window_buffer_safety() {
    common::setup_logging();
    let _span = info_span!("test_cascade_params_small_window_buffer_safety").entered();

    use pi_wasm::core::compaction::determine_cascade_params;

    let mut state = ContextState {
        user_turns_list: vec![],
        estimate_context_chars: 0,
        context_budget_chars: 0,
        context_budget_tokens: 32000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    // ratio = 13000/32000 = 0.406, well below 0.70
    state.update_api_usage(13000, 0);

    let config = ContextConfig {
        context_window: 16384,
        max_output_tokens: 384,
        // input_budget = 16000, remaining = 16000 - 13000 = 3000
        // buffer_cap(5000) = min(5000, 16000*3/10=4800) = 4800
        // remaining(3000) < autocompact_buf(4800) → triggers
        autocompact_buffer_tokens: 5_000,
        warning_buffer_tokens: 20_000,
        ..Default::default()
    };
    let params = determine_cascade_params(&state, &config);
    assert!(
        params.should_cascade,
        "small window: ratio 0.40 but remaining(3000) < buffer(4800) should trigger"
    );
}

/// [Layer 2] 摘要比原文长时中断：不死循环，state 不变
#[tokio::test]
async fn test_compaction_loop_summary_too_long_breaks() {
    common::setup_logging();
    let _span = info_span!("test_compaction_loop_summary_too_long_breaks").entered();

    let turns: Vec<TurnEntry> = (0..4)
        .map(|i| make_big_turn(&format!("q{}", i), 500))
        .collect();
    let total: usize = turns
        .iter()
        .map(pi_wasm::core::session::estimate_turn_chars)
        .sum();

    let mut state = ContextState {
        user_turns_list: turns,
        estimate_context_chars: total,
        context_budget_chars: total / 2,
        context_budget_tokens: total / 8,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        compaction_summary: None,
        compaction_consecutive_failures: 0,
    };
    let original_len = state.user_turns_list.len();
    let original_estimate = state.estimate_context_chars;

    let way_too_long = "z".repeat(50_000);
    let llm = MockCompactionLlm::new(vec![Ok(way_too_long)]);
    let config = ContextConfig {
        keep_recent_turns: 1,
        compaction_turns: 3,
        ..Default::default()
    };

    info!("Act: run_compaction_loop with summary longer than original");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_compaction_loop(&mut state, &llm, &config, std::path::Path::new("")),
    )
    .await
    .expect("should not timeout (no infinite loop)");

    info!("Assert: breaks without modifying state");
    assert!(result.is_ok());
    assert_eq!(
        state.user_turns_list.len(),
        original_len,
        "turns should be unchanged when summary is too long"
    );
    assert_eq!(
        state.estimate_context_chars, original_estimate,
        "estimate should be unchanged"
    );
}
