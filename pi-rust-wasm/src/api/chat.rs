//! CLI 对话模式：主循环、流式渲染、多轮上下文、工具调用、Markdown 高亮。

use std::io::{self, Write as IoWrite};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::infra::error::AppError;
use crate::infra::event_bus::EventContext;
use crate::infra::{
    wire, AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder,
    TracingAuditRecorder,
};
use crate::{
    convert_to_llm_format, resolve_extra_roots_paths,
    resolve_sessions_dir, resolve_workspace_dir, AgentLoop, AgentLoopConfig, AppConfig,
    ChatMessage, DefaultPrimitiveExecutor, DefaultToolRegistry, LlmProvider, OpenAiProvider,
    PrimitiveExecutor, SessionEntry, SessionManager, Tool, ToolExecutor, ToolRegistry,
};
use crate::core::compaction::run_compaction_cascade_v2;
use crate::core::session::manager::{
    build_context_from_state, init_context_state, TurnEntry,
};

use super::render::MarkdownRenderer;
use tracing::debug;

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
    let mut context_state =
        init_context_state(&ctx.session, context_config, &system_text)?;

    loop {
        let input = match rl.readline("u> ") {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("\n再见！");
                break;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                continue;
            }
            Err(e) => {
                eprintln!("输入错误: {}", e);
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

        // Pre-flight cascade V2: ratio watermark-driven compaction
        if context_state.is_over_budget() {
            let tp = ctx
                .session
                .current_transcript_path()
                .ok()
                .flatten()
                .unwrap_or_default();
            let work_dir_str = ctx.workspace_dir.to_string_lossy();
            run_compaction_cascade_v2(
                &mut context_state,
                &*ctx.llm,
                context_config,
                &tp,
                &ctx.workspace_dir,
                ctx.session.current_session_key(),
            )
            .await;
            let _ = work_dir_str;
        }

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

        debug!(
            "[tool_debug] ContextState turns={} estimate={} budget={} messages={}",
            context_state.user_turns_list.len(),
            context_state.estimate_context_chars,
            context_state.context_budget_chars,
            messages.len()
        );

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
        print!("\npi.{}> ", ctx.config.agent.id);
        io::stdout().flush().map_err(AppError::Io)?;

        let run_result = agent_loop.run(messages).await;
        ctx.event_bus.off(listener_id);
        match run_result {
            Ok(result) => {
                if let Some(remaining) = renderer.lock().flush() {
                    print!("{}", remaining);
                    io::stdout().flush().map_err(AppError::Io)?;
                }

                // Take back ContextState
                context_state = agent_loop.take_context_state().unwrap_or_else(|| {
                    init_context_state(&ctx.session, context_config, &system_text)
                        .unwrap_or(crate::core::ContextState {
                            user_turns_list: Vec::new(),
                            estimate_context_chars: system_text.len(),
                            context_budget_chars: crate::infra::config::compute_context_budget_chars(
                                context_config,
                            ),
                            context_budget_tokens: context_config.context_window.saturating_sub(context_config.max_output_tokens),
                            last_api_usage: None,
                            post_usage_appended_chars: 0,
                            compaction_consecutive_failures: 0,
                        })
                });

                // Pack current turn and append to context state
                let current_turn = TurnEntry::UserTurn {
                    messages: result.new_messages.clone(),
                };
                context_state.on_new_user_turn(current_turn);

                // Write to transcript
                let chat_msgs = convert_to_llm_format(&result.new_messages);
                for msg in &chat_msgs {
                    ctx.session.append_message(serde_json::to_value(msg)?)?;
                }
            }
            Err(e) => {
                if let Some(remaining) = renderer.lock().flush() {
                    print!("{}", remaining);
                    let _ = io::stdout().flush();
                }

                // Take back context state even on error
                context_state = agent_loop.take_context_state().unwrap_or_else(|| {
                    init_context_state(&ctx.session, context_config, &system_text)
                        .unwrap_or(crate::core::ContextState {
                            user_turns_list: Vec::new(),
                            estimate_context_chars: system_text.len(),
                            context_budget_chars: crate::infra::config::compute_context_budget_chars(
                                context_config,
                            ),
                            context_budget_tokens: context_config.context_window.saturating_sub(context_config.max_output_tokens),
                            last_api_usage: None,
                            post_usage_appended_chars: 0,
                            compaction_consecutive_failures: 0,
                        })
                });

                let is_fatal = is_fatal_error(&e);
                eprintln!("\n[错误] {}", e);
                if is_fatal {
                    eprintln!("(致命错误，退出对话)");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionEntry;

    #[test]
    fn build_tool_definitions_is_non_empty() {
        let defs = build_tool_definitions();
        assert!(defs.len() >= 4);
        for d in &defs {
            assert!(d["function"]["name"].is_string());
        }
    }

    #[test]
    fn build_tool_definitions_contains_all_primitives() {
        let defs = build_tool_definitions();
        let names: Vec<String> = defs
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(String::from))
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"edit_file".to_string()));
        assert!(names.contains(&"execute_bash".to_string()));
        assert!(names.contains(&"list_dir".to_string()));
    }

    #[test]
    fn convert_to_llm_format_assistant_with_tool_calls() {
        use crate::{convert_to_llm_format, AgentMessage, ToolCallInfo};
        let tcs = vec![ToolCallInfo {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"/tmp/x"}"#.into(),
        }];
        let messages = vec![AgentMessage::Assistant {
            text: "thinking...".into(),
            tool_calls: tcs,
        }];
        let out = convert_to_llm_format(&messages);
        assert_eq!(out.len(), 1);
        assert!(out[0].tool_calls.is_some());
        let tc_val = out[0].tool_calls.as_ref().unwrap();
        assert_eq!(tc_val.len(), 1);
        assert_eq!(tc_val[0]["function"]["name"], "read_file");
    }

    #[test]
    fn convert_to_llm_format_assistant_tool_calls_null_content_when_empty() {
        use crate::{convert_to_llm_format, AgentMessage, ToolCallInfo};
        let tcs = vec![ToolCallInfo {
            id: "call_2".into(),
            name: "list_dir".into(),
            arguments: r#"{"path":"."}"#.into(),
        }];
        let messages = vec![AgentMessage::Assistant {
            text: String::new(),
            tool_calls: tcs,
        }];
        let out = convert_to_llm_format(&messages);
        assert_eq!(out.len(), 1);
        assert!(out[0].content.is_none());
        assert!(out[0].tool_calls.is_some());
    }

    #[test]
    fn effective_model_uses_session_override() {
        let entry = SessionEntry {
            session_id: "s1".into(),
            updated_at: 0,
            session_file: None,
            cwd: None,
            thinking_level: None,
            model_override: Some("gpt-4o".to_string()),
            input_tokens: None,
            output_tokens: None,
            compaction_count: None,
        };
        let config = AppConfig::default();
        let model = entry
            .model_override
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&config.llm.default_model);
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn effective_model_uses_global_when_no_override() {
        let entry = SessionEntry {
            session_id: "s2".into(),
            updated_at: 0,
            session_file: None,
            cwd: None,
            thinking_level: None,
            model_override: None,
            input_tokens: None,
            output_tokens: None,
            compaction_count: None,
        };
        let config = AppConfig::default();
        let model = entry
            .model_override
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&config.llm.default_model);
        assert_eq!(model, config.llm.default_model);
    }

    #[test]
    fn ensure_session_creates_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        assert!(mgr.get_session(key).unwrap().is_none());

        if mgr.get_session(key).unwrap().is_none() {
            mgr.create_session(key, None).unwrap();
        }
        assert!(mgr.get_session(key).unwrap().is_some());
    }
}
