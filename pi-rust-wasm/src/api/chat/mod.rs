//! CLI 对话模式：主循环、流式渲染、多轮上下文、工具调用、Markdown 高亮。

use std::io::{self, Write as IoWrite};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{build_context_from_state, init_context_state};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventContext;
use crate::infra::{
    wire, AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder,
    TracingAuditRecorder,
};
use crate::{
    resolve_extra_roots_paths, resolve_sessions_dir, resolve_workspace_dir, AgentLoop,
    AgentLoopConfig, AppConfig, DefaultPrimitiveExecutor, DefaultToolRegistry, LlmProvider,
    OpenAiProvider, PrimitiveExecutor, SessionEntry, SessionManager, Tool, ToolExecutor,
    ToolRegistry,
};

use super::render::MarkdownRenderer;

#[cfg(test)]
mod tests;

mod session_stderr_listeners;

// ─── ChatContext ──────────────────────────────────────────────────────────────

pub struct ChatContext {
    pub session: SessionManager,
    pub llm: Arc<dyn LlmProvider>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    /// 当前回合用户中断令牌。ctrlc handler 会 `lock().cancel()`；
    /// `chat_loop` 在每次 readline 读到非空输入后**重建**它（`CancellationToken`
    /// 一旦 cancel 不可逆），保证新回合不会被上一回合的中断信号污染。
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    /// 上一次 Ctrl+C 按下的时刻；ctrlc handler 判双击用。
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
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
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));

        Ok(Self {
            session,
            llm,
            config,
            primitive,
            tool_registry,
            event_bus,
            cancel_token,
            last_interrupt_at,
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
    let session_stderr_ids =
        session_stderr_listeners::register_chat_session_stderr_listeners(&*ctx.event_bus);

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

        // 读到新输入后重建 CancellationToken。
        // 关键约束：token 一旦 cancel 不可逆——如果用户 Ctrl+C 落在 prompt 处，
        // 旧 token 已被 cancel；这里替换成新 token，新回合才能正常运行。
        // 必须在 `agent_loop.run` 之前完成。
        let turn_token = {
            let mut guard = ctx.cancel_token.lock();
            *guard = CancellationToken::new();
            guard.clone()
        };

        let entry = ctx.session.get_session(ctx.session.current_session_key())?;
        let model = ctx.effective_model(entry.as_ref());

        // Update context estimate for the new user input
        context_state.on_message_appended(input.len());
        info!(
            target: "pi_wasm_chat_diag",
            phase = "chat_after_user_append",
            ratio = context_state.usage_ratio(),
            compaction_count = context_state.session_obs.compaction_count,
            turns = context_state.turn_count()
        );

        // Timing ②: restore pending preheat + apply boundary before request
        context_state.preheat.try_restart_if_pending(
            context_state.usage_ratio(),
            &context_state.messages,
            &context_state.transcript_path,
            ctx.llm.clone(),
            context_config,
            ctx.event_bus.clone(),
        );
        check_before_request(&mut context_state, &*ctx.event_bus).await;
        info!(
            target: "pi_wasm_chat_diag",
            phase = "chat_after_timing2_check",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = false,
            ratio = context_state.usage_ratio(),
            compaction_count = context_state.session_obs.compaction_count
        );

        // Build messages from ContextState
        let mut messages = build_context_from_state(&context_state);
        messages.insert(0, ChatMessage::system(&system_text));
        messages.push(ChatMessage::user(&input));

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
            turn_token,
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

        print!("\npi.{}> ", ctx.config.agent.id);
        io::stdout().flush().map_err(AppError::Io)?;

        info!(
            target: "pi_wasm_chat_diag",
            phase = "chat_before_agent_run",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = true
        );
        let outcome = agent_loop.run(messages).await;
        ctx.event_bus.off(listener_id);

        // T-004 / T-017：`Completed` 与 `Interrupted` 走**同一条**持久化路径——
        // partial assistant（content_buf 截短处）+ 已完成的 tool_result 都已被
        // `AgentLoop::run` 装进 `AgentRunResult.new_messages`，这里只需 append +
        // observability，不区分成功与中断。
        let (maybe_result, was_interrupted, maybe_error) = match outcome {
            AgentRunOutcome::Completed(r) => (Some(r), false, None),
            AgentRunOutcome::Interrupted(r) => (Some(r), true, None),
            AgentRunOutcome::Failed(e) => (None, false, Some(e)),
        };

        if let Some(remaining) = renderer.lock().flush() {
            print!("{}", remaining);
            let _ = io::stdout().flush();
        }

        context_state = agent_loop.take_context_state().unwrap_or_else(|| {
            init_context_state(&ctx.session, context_config, &system_text).unwrap_or(
                crate::core::ContextState {
                    messages: Vec::new(),
                    estimate_context_chars: system_text.len(),
                    context_budget_chars: crate::infra::config::compute_context_budget_chars(
                        context_config,
                    ),
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

        if let Some(result) = maybe_result {
            for msg in result.new_messages {
                let row_id = ctx.session.append_message(serde_json::to_value(&msg)?)?;
                let mut cm = msg;
                cm.msg_id = Some(row_id);
                context_state.messages.push(cm);
            }
            ctx.session.persist_context_observability(&context_state)?;

            if was_interrupted {
                eprintln!("\n^C 已中断（partial 已保存）");
            }
        } else if let Some(e) = maybe_error {
            let _ = ctx.session.persist_context_observability(&context_state);

            let is_fatal = is_fatal_error(&e);
            eprintln!("\n[错误] {}", e);
            if is_fatal {
                eprintln!("(致命错误，退出对话)");
                context_state.preheat.abort();
                session_stderr_listeners::unregister_chat_session_stderr_listeners(
                    &*ctx.event_bus,
                    &session_stderr_ids,
                );
                return Err(e);
            }
            eprintln!("(可重试，请继续输入)\n");
            continue;
        }

        println!();
    }

    session_stderr_listeners::unregister_chat_session_stderr_listeners(
        &*ctx.event_bus,
        &session_stderr_ids,
    );
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
