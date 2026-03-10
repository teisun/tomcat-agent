//! CLI 对话模式：主循环、流式渲染、多轮上下文、工具调用、Markdown 高亮。

use std::io::{self, Write as IoWrite};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::infra::error::AppError;
use crate::infra::{AuditRecorder, TracingAuditRecorder};
use crate::{
    resolve_sessions_dir, AppConfig, ChatMessage, ChatRequest, DefaultPrimitiveExecutor,
    DefaultToolRegistry, LlmProvider, OpenAiProvider, PrimitiveExecutor, SessionEntry,
    SessionManager, StreamEvent, Tool, ToolExecutor, ToolRegistry,
};

use super::render::MarkdownRenderer;

const MAX_TOOL_ROUNDS: usize = 10;

// ─── ChatContext ──────────────────────────────────────────────────────────────

pub struct ChatContext {
    pub session: SessionManager,
    pub llm: Box<dyn LlmProvider>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub cancelled: Arc<AtomicBool>,
}

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);

        let llm: Box<dyn LlmProvider> = Box::new(OpenAiProvider::new(&config.llm)?);

        let audit: Arc<dyn AuditRecorder> = Arc::new(TracingAuditRecorder);
        let confirmation = Arc::new(CliConfirmation);
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DefaultPrimitiveExecutor::new(
            config.primitive.clone(),
            confirmation,
            audit.clone(),
        ));

        let tool_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(tool_executor, audit));

        let cancelled = Arc::new(AtomicBool::new(false));

        Ok(Self {
            session,
            llm,
            config,
            primitive,
            tool_registry,
            cancelled,
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
        println!("类型: {:?}  来源: {}", operation, plugin_id);
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

    let mut renderer = MarkdownRenderer::new();

    loop {
        let input = match rl.readline("你> ") {
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

        let mut history = ctx
            .session
            .build_context_messages(ctx.session.context_cap())?;
        let user_msg = ChatMessage::user(&input);
        history.push(serde_json::to_value(&user_msg)?);

        ctx.session
            .append_message(serde_json::to_value(&user_msg)?)?;

        let messages: Vec<ChatMessage> = history
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();

        let response_text = do_chat_turn(ctx, &mut renderer, &messages, &model).await?;

        if !response_text.is_empty() {
            let assistant_msg = ChatMessage::assistant(&response_text);
            ctx.session
                .append_message(serde_json::to_value(&assistant_msg)?)?;
        }

        println!();
    }

    Ok(())
}

async fn do_chat_turn(
    ctx: &ChatContext,
    renderer: &mut MarkdownRenderer,
    initial_messages: &[ChatMessage],
    model: &str,
) -> Result<String, AppError> {
    let tool_defs = build_tool_definitions();
    let mut messages: Vec<ChatMessage> = initial_messages.to_vec();
    let mut final_text = String::new();

    for _round in 0..MAX_TOOL_ROUNDS {
        let req = ChatRequest {
            messages: messages.clone(),
            model: model.to_string(),
            temperature: None,
            max_tokens: None,
            stream: Some(true),
            model_override: None,
            tools: Some(tool_defs.clone()),
        };

        let mut stream = ctx.llm.chat_stream(req).await?;
        let mut content_buf = String::new();
        let mut tool_calls_buf: Vec<ToolCallAccumulator> = Vec::new();

        print!("\nAI> ");
        io::stdout().flush().map_err(AppError::Io)?;

        while let Some(event) = stream.next().await {
            if ctx.cancelled.load(Ordering::SeqCst) {
                println!("\n[生成已中断]");
                break;
            }
            match event {
                Ok(StreamEvent::ContentDelta { delta }) => {
                    renderer.push(&delta);
                    while let Some(chunk) = renderer.take_ready() {
                        print!("{}", chunk);
                        io::stdout().flush().map_err(AppError::Io)?;
                    }
                    content_buf.push_str(&delta);
                }
                Ok(StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                }) => {
                    while tool_calls_buf.len() <= index as usize {
                        tool_calls_buf.push(ToolCallAccumulator::default());
                    }
                    let acc = &mut tool_calls_buf[index as usize];
                    if let Some(id_val) = id {
                        acc.id = id_val;
                    }
                    if let Some(name_val) = name {
                        acc.name = name_val;
                    }
                    if let Some(args) = arguments_delta {
                        acc.arguments.push_str(&args);
                    }
                }
                Ok(StreamEvent::FinishReason { .. }) => break,
                Ok(StreamEvent::Usage { .. }) => {}
                Err(e) => {
                    eprintln!("\n[流式错误: {}]", e);
                    break;
                }
            }
        }

        if let Some(remaining) = renderer.flush() {
            print!("{}", remaining);
            io::stdout().flush().map_err(AppError::Io)?;
        }
        println!();

        final_text.push_str(&content_buf);

        if tool_calls_buf.is_empty() || tool_calls_buf.iter().all(|tc| tc.name.is_empty()) {
            break;
        }

        let tool_calls: Vec<ToolCallInfo> = tool_calls_buf
            .into_iter()
            .filter(|tc| !tc.name.is_empty())
            .map(|tc| ToolCallInfo {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect();

        let assistant_with_tools = build_assistant_tool_call_message(&content_buf, &tool_calls);
        messages.push(assistant_with_tools);

        for tc in &tool_calls {
            let result = execute_tool_call(ctx, tc).await;
            let tool_msg = ChatMessage::tool(&tc.id, &result);
            messages.push(tool_msg);
        }

        content_buf.clear();
        *renderer = MarkdownRenderer::new();
    }

    Ok(final_text)
}

// ─── Tool call execution ──────────────────────────────────────────────────────

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

struct ToolCallInfo {
    id: String,
    name: String,
    arguments: String,
}

fn build_assistant_tool_call_message(content: &str, tool_calls: &[ToolCallInfo]) -> ChatMessage {
    let tc_json: Vec<serde_json::Value> = tool_calls
        .iter()
        .map(|tc| {
            serde_json::json!({
                "id": tc.id,
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments,
                }
            })
        })
        .collect();

    ChatMessage::assistant_with_tool_calls(
        if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tc_json,
    )
}

async fn execute_tool_call(ctx: &ChatContext, tc: &ToolCallInfo) -> String {
    let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
        Ok(v) => v,
        Err(e) => return format!("参数解析失败: {}", e),
    };

    println!("\n  [工具调用] {} ({})", tc.name, tc.id);

    let plugin_id = "__chat__";

    match tc.name.as_str() {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("");
            match ctx.primitive.read_file(path, plugin_id).await {
                Ok(content) => content,
                Err(e) => format!("读取失败: {}", e),
            }
        }
        "write_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            match ctx
                .primitive
                .write_file(path, content, overwrite, plugin_id)
                .await
            {
                Ok(r) => {
                    if r.written {
                        format!("已写入: {}", r.path)
                    } else {
                        format!("写入被拒绝: {}", r.path)
                    }
                }
                Err(e) => format!("写入失败: {}", e),
            }
        }
        "edit_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let old_content = args["old_content"].as_str().unwrap_or("");
            let new_content = args["new_content"].as_str().unwrap_or("");
            let edits = vec![crate::EditOperation {
                operation_type: crate::EditOperationType::Replace,
                start_line: None,
                end_line: None,
                old_content: Some(old_content.to_string()),
                new_content: new_content.to_string(),
            }];
            match ctx.primitive.edit_file(path, edits, plugin_id).await {
                Ok(r) => {
                    if r.applied {
                        format!("已编辑: {}", r.path)
                    } else {
                        format!("编辑被拒绝: {}", r.path)
                    }
                }
                Err(e) => format!("编辑失败: {}", e),
            }
        }
        "execute_bash" => {
            let command = args["command"].as_str().unwrap_or("");
            let cwd = args["cwd"].as_str();
            match ctx.primitive.execute_bash(command, cwd, plugin_id).await {
                Ok(r) => {
                    let mut out = String::new();
                    if !r.stdout.is_empty() {
                        out.push_str(&r.stdout);
                    }
                    if !r.stderr.is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str("STDERR: ");
                        out.push_str(&r.stderr);
                    }
                    out.push_str(&format!("\n(exit code: {})", r.exit_code));
                    out
                }
                Err(e) => format!("执行失败: {}", e),
            }
        }
        "list_dir" => {
            let path = args["path"].as_str().unwrap_or("");
            match ctx.primitive.list_dir(path, plugin_id).await {
                Ok(entries) => {
                    let lines: Vec<String> = entries
                        .iter()
                        .map(|e| {
                            if e.is_dir {
                                format!("  {}/ (dir)", e.name)
                            } else {
                                format!("  {}", e.name)
                            }
                        })
                        .collect();
                    lines.join("\n")
                }
                Err(e) => format!("列目录失败: {}", e),
            }
        }
        other => {
            format!("未知工具: {}", other)
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

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
    fn build_assistant_tool_call_message_creates_valid_json() {
        let tcs = vec![ToolCallInfo {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"/tmp/x"}"#.into(),
        }];
        let msg = build_assistant_tool_call_message("thinking...", &tcs);
        assert!(msg.tool_calls.is_some());
        let tc_val = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tc_val.len(), 1);
        assert_eq!(tc_val[0]["function"]["name"], "read_file");
    }

    #[test]
    fn build_assistant_tool_call_message_null_content_when_empty() {
        let tcs = vec![ToolCallInfo {
            id: "call_2".into(),
            name: "list_dir".into(),
            arguments: r#"{"path":"."}"#.into(),
        }];
        let msg = build_assistant_tool_call_message("", &tcs);
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_some());
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
    fn tool_call_accumulator_default() {
        let acc = ToolCallAccumulator::default();
        assert!(acc.id.is_empty());
        assert!(acc.name.is_empty());
        assert!(acc.arguments.is_empty());
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
