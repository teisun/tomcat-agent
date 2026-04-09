//! CLI 对话模式：主循环、流式渲染、多轮上下文、工具调用、Markdown 高亮。

use std::io::{self, Write as IoWrite};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::compaction::apply::check_before_request;
use crate::core::compaction::preheat::Preheat;
use crate::core::session::manager::{build_context_from_state, init_context_state, TurnEntry};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventContext;
use crate::infra::{
    wire, AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder,
    TracingAuditRecorder,
};
use crate::{
    convert_to_llm_format, resolve_extra_roots_paths, resolve_sessions_dir, resolve_workspace_dir,
    AgentLoop, AgentLoopConfig, AppConfig, ChatMessage, DefaultPrimitiveExecutor,
    DefaultToolRegistry, LlmProvider, OpenAiProvider, PrimitiveExecutor, SessionEntry,
    SessionManager, Tool, ToolExecutor, ToolRegistry,
};

use super::render::MarkdownRenderer;

#[cfg(test)]
mod tests;

// ─── ChatContext ──────────────────────────────────────────────────────────────

pub struct ChatContext {
    pub session: SessionManager,
    pub llm: Arc<dyn LlmProvider>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    pub cancelled: Arc<AtomicBool>,
    /// Agent 默认工作目录，用于 system prompt 和路径白名单默认值。
    pub workspace_dir: std::path::PathBuf,
}

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);

        let workspace_dir = resolve_workspace_dir(&config)?;
        std::fs::create_dir_all(&workspace_dir).map_err(AppError::Io)?;

        let llm: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::new(&config.llm)?);

        let audit: Arc<dyn AuditRecorder> = match AuditStore::open_if_enabled(&config)? {
            Some(store) => Arc::new(FileAuditRecorder::new(Arc::new(store))),
            None => Arc::new(TracingAuditRecorder),
        };
        let extra_roots = resolve_extra_roots_paths(&config)?;
        let confirmation = Arc::new(CliConfirmation);
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(
            DefaultPrimitiveExecutor::new(
                config.primitive.clone(),
                confirmation,
                audit.clone(),
                workspace_dir.clone(),
            )
            .with_extra_roots(extra_roots),
        );

        let tool_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(tool_executor, audit));

        let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
        let cancelled = Arc::new(AtomicBool::new(false));

        Ok(Self {
            session,
            llm,
            config,
            primitive,
            tool_registry,
            event_bus,
            cancelled,
            workspace_dir,
        })
    }

    fn effective_model(&self, entry: Option<&SessionEntry>) -> String {
        entry
            .and_then(|e| e.model_override.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.config.llm.default_model)
            .to_string()
    }
}

// ─── CLI UserConfirmationProvider ─────────────────────────────────────────────

use crate::core::confirmation::UserConfirmationProvider;
use crate::core::primitives::PrimitiveOperation;

pub struct CliConfirmation;

#[async_trait::async_trait]
impl UserConfirmationProvider for CliConfirmation {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        println!("\n--- 操作确认 ---");
        let source_label = if plugin_id == "__agent__" {
            "host".to_string()
        } else {
            plugin_id.to_string()
        };
        println!("类型: {:?}  来源: {}", operation, source_label);
        if !preview.is_empty() {
            let lines: Vec<&str> = preview.lines().collect();
            let display = if lines.len() > 20 {
                format!(
                    "{}\n  ... ({} 行已省略)",
                    lines[..20].join("\n"),
                    lines.len() - 20
                )
            } else {
                preview.to_string()
            };
            println!("预览:\n{}", display);
        }
        print!("是否执行？[y/N] ");
        io::stdout().flush().map_err(AppError::Io)?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).map_err(AppError::Io)?;
        let answer = line.trim().to_lowercase();
        Ok(answer == "y" || answer == "yes")
    }
}

// ─── NoopToolExecutor ─────────────────────────────────────────────────────────

struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        Err(AppError::Tool(format!(
            "对话模式下不支持插件工具执行: {}",
            tool.name
        )))
    }
}

// ─── Tool definitions for LLM ─────────────────────────────────────────────────

fn build_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "读取文件内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" }
                    },
                    "required": ["path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "写入文件内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "content": { "type": "string", "description": "文件内容" },
                        "overwrite": { "type": "boolean", "description": "是否覆盖" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "编辑文件（基于内容匹配替换）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "old_content": { "type": "string", "description": "被替换的原内容" },
                        "new_content": { "type": "string", "description": "替换后的新内容" }
                    },
                    "required": ["path", "old_content", "new_content"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "execute_bash",
                "description": "执行 bash 命令",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的命令" },
                        "cwd": { "type": "string", "description": "工作目录（可选）" }
                    },
                    "required": ["command"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "列出目录内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "目录路径" }
                    },
                    "required": ["path"]
                }
            }
        }),
    ]
}

// ─── Main chat loop ───────────────────────────────────────────────────────────

pub async fn chat_loop(ctx: &ChatContext, resume: bool) -> Result<(), AppError> {
    ensure_session(ctx)?;

    let entry = ctx.session.get_session(ctx.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());

    if resume {
        println!("恢复会话: {}", ctx.session.current_session_key());
    }
    println!("pi 对话模式 (模型: {})", model);
    println!("输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。\n");

    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))?;

    // ContextState: 在 loop 外一次性初始化，跨迭代复用
    let context_config = &ctx.config.context;
    let workspace_str = ctx.workspace_dir.to_string_lossy();
    let system_text = crate::core::system_prompt::build_system_prompt(&workspace_str);
    let mut context_state = init_context_state(&ctx.session, context_config, &system_text)?;

    loop {
        let input = match rl.readline("u> ") {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("\n再见！");
                context_state.preheat.abort();
                break;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                continue;
            }
            Err(e) => {
                eprintln!("输入错误: {}", e);
                context_state.preheat.abort();
                break;
            }
        };

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(&input);

        ctx.cancelled.store(false, Ordering::SeqCst);

        let entry = ctx.session.get_session(ctx.session.current_session_key())?;
        let model = ctx.effective_model(entry.as_ref());

        // Update context estimate for the new user input
        context_state.on_message_appended(input.len());

        // Timing ②: restore pending preheat + apply boundary before request
        context_state.preheat.try_restart_if_pending(
            context_state.usage_ratio(),
            &context_state.user_turns_list,
            &context_state.transcript_path,
            ctx.llm.clone(),
            context_config,
            ctx.event_bus.clone(),
        );
        check_before_request(&mut context_state, &*ctx.event_bus).await;

        // Build messages from ContextState
        let mut messages = build_context_from_state(&context_state);
        messages.insert(
            0,
            crate::core::AgentMessage::System {
                text: system_text.clone(),
            },
        );
        messages.push(crate::core::AgentMessage::User {
            text: input.clone(),
        });

        // Append user message to transcript
        let user_msg = ChatMessage::user(&input);
        ctx.session
            .append_message(serde_json::to_value(&user_msg)?)?;

        let renderer = Arc::new(parking_lot::Mutex::new(MarkdownRenderer::new()));
        let config = AgentLoopConfig {
            max_attempts: 3,
            max_tool_rounds: usize::MAX,
            retry_base_delay_ms: 300,
            model: model.clone(),
            session_id: ctx.session.current_session_key().to_string(),
            tool_definitions: build_tool_definitions(),
            context_config: context_config.clone(),
            work_dir: ctx.workspace_dir.to_string_lossy().to_string(),
        };
        let mut agent_loop = AgentLoop::new(
            ctx.llm.clone(),
            ctx.primitive.clone(),
            ctx.event_bus.clone(),
            config,
            ctx.cancelled.clone(),
        );
        agent_loop.set_context_state(Some(context_state));

        let renderer_clone = Arc::clone(&renderer);
        let listener_id = ctx.event_bus.on(
            wire::WIRE_MESSAGE_UPDATE,
            Box::new(move |evt: EventContext| {
                if let Some(delta) = evt
                    .payload
                    .get("assistantMessageEvent")
                    .and_then(|e| e.get("delta"))
                    .and_then(|d| d.as_str())
                {
                    renderer_clone.lock().push(delta);
                    while let Some(chunk) = renderer_clone.lock().take_ready() {
                        print!("{}", chunk);
                        let _ = io::stdout().flush();
                    }
                }
                Ok(())
            }),
        );
        let metrics_listener_id = ctx.event_bus.on(
            wire::WIRE_CONTEXT_METRICS_UPDATE,
            Box::new(move |evt: EventContext| {
                let tokens = evt.payload.get("inputTokensUsed")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                let ratio = evt.payload.get("contextUtilizationRatio")
                    .and_then(|v| v.as_f64()).unwrap_or(0.0);
                let compactions = evt.payload.get("compactionCount")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                let saved = evt.payload.get("compactionTokensFreed")
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                let persisted = evt.payload.get("totalToolResultBytesPersisted")
                    .and_then(|v| v.as_u64()).unwrap_or(0);

                let ratio_pct = (ratio * 100.0).min(99999.0);
                let persisted_display = if persisted >= 1024 {
                    format!("{:.1} KB", persisted as f64 / 1024.0)
                } else {
                    format!("{} B", persisted)
                };
                let preheat_in_progress = evt
                    .payload
                    .get("preheatInProgress")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let preheat_result_pending = evt
                    .payload
                    .get("preheatResultPending")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let (zh_suffix, en_suffix) = if preheat_in_progress {
                    (" | 预热中…", " | Preheating…")
                } else if preheat_result_pending {
                    (" | 摘要待应用", " | Summary pending apply")
                } else {
                    ("", "")
                };
                // 行尾必须换行，否则 stderr 与 stdout 流式正文可能在同一视觉行粘连（如「0 B你给」）。
                eprint!(
                    "\n\x1b[90m[ctx] {} 令牌 | {:.1}% 占用 | 压缩 x{} | 已节省 {} 令牌 | 已持久化 {}{}\x1b[0m\n",
                    tokens, ratio_pct, compactions, saved, persisted_display, zh_suffix
                );
                eprint!(
                    "\x1b[90m[ctx] {} tok | {:.1}% | compact x{} | saved {} tok | persisted {}{}\x1b[0m\n",
                    tokens, ratio_pct, compactions, saved, persisted_display, en_suffix
                );
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        // L1: auto_compaction_start / auto_compaction_end
        let l1_start_id = ctx.event_bus.on(
            wire::WIRE_AUTO_COMPACTION_START,
            Box::new(|_ctx: EventContext| {
                eprint!("\n\x1b[90m[ctx] 后台压缩已启动…\x1b[0m\n");
                eprint!("\x1b[90m[ctx] Background compaction started…\x1b[0m\n");
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        let l1_end_id = ctx.event_bus.on(
            wire::WIRE_AUTO_COMPACTION_END,
            Box::new(|evt: EventContext| {
                let before = evt
                    .payload
                    .get("estimatedCoveredTokensBefore")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let summ = evt
                    .payload
                    .get("estimatedSummaryTokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let saved = evt
                    .payload
                    .get("estimatedTokensSaved")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                eprint!(
                    "\n\x1b[90m[ctx] 压缩摘要就绪（待应用）| 覆盖区 ~{} 令牌 → 摘要 ~{} 令牌（估省 {} 令牌）\x1b[0m\n",
                    before, summ, saved
                );
                eprint!(
                    "\x1b[90m[ctx] Summary generated (pending apply) | covered ~{} tok → summary ~{} tok (saved ~{} tok)\x1b[0m\n",
                    before, summ, saved
                );
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        // L1 exhausted: compaction_error
        let l1_err_id = ctx.event_bus.on(
            wire::WIRE_COMPACTION_ERROR,
            Box::new(|evt: EventContext| {
                let source = evt
                    .payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let err_raw = evt
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let err_display = if err_raw.chars().count() > 200 {
                    let t: String = err_raw.chars().take(200).collect();
                    format!("{}…", t)
                } else {
                    err_raw.to_string()
                };
                if source == "apply" {
                    eprint!(
                        "\n\x1b[33m[ctx] 摘要应用失败：{}\x1b[0m\n",
                        err_display
                    );
                    eprint!(
                        "\x1b[33m[ctx] Summary application failed: {}\x1b[0m\n",
                        err_display
                    );
                    let _ = io::stderr().flush();
                    return Ok(());
                }
                let exhausted = evt
                    .payload
                    .get("exhaustedAfterRetries")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let attempts = evt
                    .payload
                    .get("attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if exhausted && source == "preheat" {
                    eprint!(
                        "\n\x1b[33m[ctx] 预热失败（已重试 {} 次）：{}\x1b[0m\n",
                        attempts, err_display
                    );
                    eprint!(
                        "\x1b[33m[ctx] Preheat failed after {} attempt(s): {}\x1b[0m\n",
                        attempts, err_display
                    );
                } else if source == "preheat" {
                    eprint!(
                        "\n\x1b[33m[ctx] 上下文压缩暂时失败，将在下次发送消息时自动重试：{}\x1b[0m\n",
                        err_display
                    );
                    eprint!(
                        "\x1b[33m[ctx] Context compaction temporarily failed; will retry on your next message: {}\x1b[0m\n",
                        err_display
                    );
                }
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        // L2: boundary_switched
        let l2_id = ctx.event_bus.on(
            wire::WIRE_BOUNDARY_SWITCHED,
            Box::new(|evt: EventContext| {
                let saved = evt
                    .payload
                    .get("estimatedTokensFreed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                eprint!(
                    "\n\x1b[90m[ctx] 上下文已压缩重置，约节省 {} 令牌\x1b[0m\n",
                    saved
                );
                eprint!(
                    "\x1b[90m[ctx] Context compacted; saved ~{} tok\x1b[0m\n",
                    saved
                );
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        // L3: context_overflow_trim_*
        let l3_start_id = ctx.event_bus.on(
            wire::WIRE_CONTEXT_OVERFLOW_TRIM_START,
            Box::new(|_ctx: EventContext| {
                eprint!("\n\x1b[33m[ctx] 上下文溢出，正在截断旧消息…\x1b[0m\n");
                eprint!("\x1b[33m[ctx] Context overflow; trimming older messages…\x1b[0m\n");
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        let l3_end_id = ctx.event_bus.on(
            wire::WIRE_CONTEXT_OVERFLOW_TRIM_END,
            Box::new(|evt: EventContext| {
                let saved = evt
                    .payload
                    .get("estimatedTokensFreed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let turns = evt
                    .payload
                    .get("turnsRemoved")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                eprint!(
                    "\n\x1b[90m[ctx] 截断完成（删 {} 轮，估省 {} 令牌），正在重试\x1b[0m\n",
                    turns, saved
                );
                eprint!(
                    "\x1b[90m[ctx] Trim done ({} turns removed, ~{} tok saved); retrying\x1b[0m\n",
                    turns, saved
                );
                let _ = io::stderr().flush();
                Ok(())
            }),
        );
        let l0_id = ctx.event_bus.on(
            wire::WIRE_LAYER0_CONTEXT_RELEASE,
            Box::new(|evt: EventContext| {
                let p = evt
                    .payload
                    .get("persistTokensFreed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let ph = evt
                    .payload
                    .get("placeholderTokensFreed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                eprint!(
                    "\n\x1b[90m[ctx] L0：大文件落盘释放 ~{} 令牌 | 历史工具结果释放 ~{} 令牌\x1b[0m\n",
                    p, ph
                );
                eprint!(
                    "\x1b[90m[ctx] L0: large file persist release ~{} tok | historical tool result release ~{} tok\x1b[0m\n",
                    p, ph
                );
                let _ = io::stderr().flush();
                Ok(())
            }),
        );

        print!("\npi.{}> ", ctx.config.agent.id);
        io::stdout().flush().map_err(AppError::Io)?;

        let run_result = agent_loop.run(messages).await;
        ctx.event_bus.off(listener_id);
        ctx.event_bus.off(metrics_listener_id);
        ctx.event_bus.off(l1_start_id);
        ctx.event_bus.off(l1_end_id);
        ctx.event_bus.off(l1_err_id);
        ctx.event_bus.off(l2_id);
        ctx.event_bus.off(l3_start_id);
        ctx.event_bus.off(l3_end_id);
        ctx.event_bus.off(l0_id);
        match run_result {
            Ok(result) => {
                if let Some(remaining) = renderer.lock().flush() {
                    print!("{}", remaining);
                    io::stdout().flush().map_err(AppError::Io)?;
                }

                // Take back ContextState
                context_state = agent_loop.take_context_state().unwrap_or_else(|| {
                    init_context_state(&ctx.session, context_config, &system_text).unwrap_or(
                        crate::core::ContextState {
                            user_turns_list: Vec::new(),
                            estimate_context_chars: system_text.len(),
                            context_budget_chars:
                                crate::infra::config::compute_context_budget_chars(context_config),
                            context_budget_tokens: context_config
                                .context_window
                                .saturating_sub(context_config.max_output_tokens),
                            last_api_usage: None,
                            post_usage_appended_chars: 0,
                            transcript_path: ctx
                                .session
                                .current_transcript_path()
                                .ok()
                                .flatten()
                                .unwrap_or_default(),
                            preheat: Preheat::new(),
                            session_obs: Default::default(),
                            live: Default::default(),
                        },
                    )
                });

                // Pack current turn and append to context state
                let current_turn = TurnEntry::UserTurn {
                    id: crate::core::session::manager::generate_entry_id(),
                    messages: result.new_messages.clone(),
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                };
                context_state.on_new_user_turn(current_turn);

                // Write to transcript
                let chat_msgs = convert_to_llm_format(&result.new_messages);
                for msg in &chat_msgs {
                    ctx.session.append_message(serde_json::to_value(msg)?)?;
                }

                ctx.session.persist_context_observability(&context_state)?;
            }
            Err(e) => {
                if let Some(remaining) = renderer.lock().flush() {
                    print!("{}", remaining);
                    let _ = io::stdout().flush();
                }

                // Take back context state even on error
                context_state = agent_loop.take_context_state().unwrap_or_else(|| {
                    init_context_state(&ctx.session, context_config, &system_text).unwrap_or(
                        crate::core::ContextState {
                            user_turns_list: Vec::new(),
                            estimate_context_chars: system_text.len(),
                            context_budget_chars:
                                crate::infra::config::compute_context_budget_chars(context_config),
                            context_budget_tokens: context_config
                                .context_window
                                .saturating_sub(context_config.max_output_tokens),
                            last_api_usage: None,
                            post_usage_appended_chars: 0,
                            transcript_path: ctx
                                .session
                                .current_transcript_path()
                                .ok()
                                .flatten()
                                .unwrap_or_default(),
                            preheat: Preheat::new(),
                            session_obs: Default::default(),
                            live: Default::default(),
                        },
                    )
                });

                let _ = ctx.session.persist_context_observability(&context_state);

                let is_fatal = is_fatal_error(&e);
                eprintln!("\n[错误] {}", e);
                if is_fatal {
                    eprintln!("(致命错误，退出对话)");
                    context_state.preheat.abort();
                    return Err(e);
                }
                eprintln!("(可重试，请继续输入)\n");
                continue;
            }
        }

        println!();
    }

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// 判断错误是否致命（配置缺失等不可恢复场景）；API/网络错误为非致命。
fn is_fatal_error(e: &AppError) -> bool {
    matches!(e, AppError::Config(_))
}

fn ensure_session(ctx: &ChatContext) -> Result<(), AppError> {
    let key = ctx.session.current_session_key();
    if ctx.session.get_session(key)?.is_none() {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        ctx.session.create_session(key, cwd)?;
    }
    Ok(())
}
