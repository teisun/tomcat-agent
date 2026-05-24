//! # `CliTurnRenderer`：单订阅者终端渲染器
//!
//! 架构定稿：`docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.3 / §4.2.4。
//!
//! 目标——把同一回合内交错到达的事件按以下排版固化输出：
//!
//! ```text
//! [thinking] ……多行 dim 灰字……
//!
//! [tool] read  path=src/main.rs
//! [tool] read  ✓ 238 lines (0.3s)
//!
//! ……Markdown 正文……
//! ```
//!
//! ## 设计要点
//!
//! - **总线一条 + 终端单订阅者**：`message_update` / `tool_execution_*` 都在本结构内
//!   做格式化，避免多个监听器各自 `print!` 造成乱序。
//! - **kind 分流**：`message_update.assistantMessageEvent.kind` 来自 P3，
//!   `content_delta` 走 `MarkdownRenderer`，`thinking_delta` 走折叠/展开逻辑。
//! - **show_thinking** 用 `AtomicBool`：让 `/thinking` 命令（P4）能跨线程切换；
//!   `false` 时全回合只打一行 `[thinking] …`，`true` 时流式打 dim 增量。
//! - **打印通道**：正文 stdout（沿用 `MarkdownRenderer.flush` 路径），thinking 默认
//!   stdout（`print_to_stderr=true` 切到 stderr 作为 prompt 打架逃生阀），tool 始终
//!   stderr（与现有 ctx/search_tools 装饰一致）。
//! - **状态机**：`last_kind` 跟踪 *上一帧打印通道*，仅在通道切换或 `[tool]` 装饰前
//!   补 `\n`，避免出现「正文中间夹一段 thinking 没换行」的情况。
//!
//! ## 测试入口
//!
//! 见同目录的 `cli_turn_renderer_test.rs`：以 `Sink` 替换 stdout/stderr，覆盖
//! folded vs expanded、tool start/end、kind 切换换行等。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde_json::Value;

use crate::api::render::MarkdownRenderer;
use crate::infra::config::ToolCliVerbosity;
use crate::infra::event_bus::{EventContext, EventListenerId};
use crate::infra::events::ToolDisplay;
use crate::infra::{wire, EventBus};

/// 上一次打印通道类型，决定是否需要补换行；`Tool` 始终 stderr，`Thinking` /
/// `Content` 默认 stdout。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastKind {
    None,
    /// 正文（stdout，由 MarkdownRenderer 输出）。
    Content,
    /// thinking 增量（stdout 或 stderr，按 `print_to_stderr`）。
    Thinking,
    /// `[tool] start` 装饰行已打印。
    ToolStart,
}

/// 渲染器输出抽象，便于单测把 stdout / stderr 替换为 `Vec<u8>` 收集。
pub trait CliWriter: Send + Sync {
    /// 写一段已经包含 ANSI 的内容到 stdout 通道。
    fn write_stdout(&self, s: &str);
    /// 写一段已经包含 ANSI 的内容到 stderr 通道。
    fn write_stderr(&self, s: &str);
}

/// 真·终端实现。
pub struct StdCliWriter;

impl CliWriter for StdCliWriter {
    fn write_stdout(&self, s: &str) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut h = stdout.lock();
        let _ = h.write_all(s.as_bytes());
        let _ = h.flush();
    }
    fn write_stderr(&self, s: &str) {
        use std::io::Write;
        let stderr = std::io::stderr();
        let mut h = stderr.lock();
        let _ = h.write_all(s.as_bytes());
        let _ = h.flush();
    }
}

/// `CliTurnRenderer` 自身可被多个 listener 闭包共享，因此用内部 `Mutex` 保护可变态。
pub struct CliTurnRenderer {
    md: Arc<Mutex<MarkdownRenderer>>,
    show_thinking: Arc<AtomicBool>,
    print_to_stderr: bool,
    tool_cli_verbosity: ToolCliVerbosity,
    state: Mutex<RendererState>,
    writer: Arc<dyn CliWriter>,
    /// `tool_execution_start` 收到时记录起始时间，end 时算 elapsed。
    tool_starts: Mutex<std::collections::HashMap<String, Instant>>,
}

#[derive(Debug)]
struct RendererState {
    last_kind: LastKind,
    /// 当前回合 thinking 通道是否已经打印过 prefix。
    /// 折叠模式下：仅在第一次出现 thinking 时打 `[thinking] …`，后续 delta 全部丢弃。
    /// 展开模式下：第一次打前缀，后续追加 delta。
    thinking_prefix_printed: bool,
    /// 折叠模式下，是否已经打过「省略号一行」（保证整回合只打一次）。
    folded_one_liner_printed: bool,
}

impl CliTurnRenderer {
    /// 业务路径用此构造：包装 stdout/stderr 真·writer。
    /// `print_to_stderr` 由调用方读 `config.llm.thinking.print_to_stderr` 后传入；
    /// 见 chat/mod.rs（架构 §3.1 / 计划 §1 已决策「print_to_stderr」）。
    pub fn new(
        md: Arc<Mutex<MarkdownRenderer>>,
        show_thinking: Arc<AtomicBool>,
        print_to_stderr: bool,
        tool_cli_verbosity: ToolCliVerbosity,
    ) -> Arc<Self> {
        Self::with_writer(
            md,
            show_thinking,
            Arc::new(StdCliWriter),
            print_to_stderr,
            tool_cli_verbosity,
        )
    }

    /// 测试 / 高级路径：可注入自定义 writer 与 `print_to_stderr` 开关。
    pub fn with_writer(
        md: Arc<Mutex<MarkdownRenderer>>,
        show_thinking: Arc<AtomicBool>,
        writer: Arc<dyn CliWriter>,
        print_to_stderr: bool,
        tool_cli_verbosity: ToolCliVerbosity,
    ) -> Arc<Self> {
        Arc::new(Self {
            md,
            show_thinking,
            print_to_stderr,
            tool_cli_verbosity,
            state: Mutex::new(RendererState {
                last_kind: LastKind::None,
                thinking_prefix_printed: false,
                folded_one_liner_printed: false,
            }),
            writer,
            tool_starts: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// 把所有 markdown 残余冲走（chat_loop 在 run 结束后调用）。
    pub fn flush_markdown(&self) {
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
    }

    /// 单测可读：当前 thinking 是否处于展开态。
    #[cfg(test)]
    pub(crate) fn is_show_thinking(&self) -> bool {
        self.show_thinking.load(Ordering::Acquire)
    }

    /// 处理 `message_update` 事件（含 thinking_delta / content_delta 分流）。
    pub fn on_message_update(&self, payload: &Value) {
        let event = match payload.get("assistantMessageEvent") {
            Some(e) => e,
            None => return,
        };
        let kind = event.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "thinking_delta" => {
                let delta = event.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if delta.is_empty() {
                    return;
                }
                self.handle_thinking_delta(delta);
            }
            // 默认（缺 kind）按 content_delta 处理，向后兼容老订阅者。
            "" | "content_delta" => {
                let delta = event.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if delta.is_empty() {
                    return;
                }
                self.handle_content_delta(delta);
            }
            _ => {
                // 未知 kind：先忽略，避免脏渲染；后续 P5/P+ 引入新 kind 时再扩 match。
            }
        }
    }

    fn handle_thinking_delta(&self, delta: &str) {
        let show = self.show_thinking.load(Ordering::Acquire);
        let mut st = self.state.lock();
        // 思考通道与正文/工具切换时，先冲走 markdown 残余 + 换行，避免「正文 ... [thinking]」黏一行。
        if st.last_kind == LastKind::Content {
            drop(st); // 释放锁，避免与 md.lock 死锁顺序冲突
            if let Some(remaining) = self.md.lock().flush() {
                self.writer.write_stdout(&remaining);
            }
            st = self.state.lock();
            self.write_thinking("\n");
        }
        if show {
            // 展开态：第一次打前缀，后续 delta 直接追加（仍夹在 dim+gray 区）。
            if !st.thinking_prefix_printed {
                self.write_thinking("\x1b[2m\x1b[90m[thinking]\x1b[0m ");
                st.thinking_prefix_printed = true;
            }
            self.write_thinking(&format!("\x1b[2m\x1b[90m{}\x1b[0m", delta));
        } else {
            // 折叠态：整回合只打一行 `[thinking] …`，避免淹没正文。
            if !st.folded_one_liner_printed {
                self.write_thinking("\x1b[2m\x1b[90m[thinking] …\x1b[0m\n");
                st.folded_one_liner_printed = true;
            }
        }
        st.last_kind = LastKind::Thinking;
    }

    fn handle_content_delta(&self, delta: &str) {
        let mut st = self.state.lock();
        // 通道切换：上一行如果是 thinking 或 tool start，需要 \n 隔开正文。
        if matches!(st.last_kind, LastKind::Thinking | LastKind::ToolStart) {
            self.writer.write_stdout("\n");
        }
        st.last_kind = LastKind::Content;
        drop(st);
        let mut renderer = self.md.lock();
        renderer.push(delta);
        while let Some(chunk) = renderer.take_ready() {
            self.writer.write_stdout(&chunk);
        }
    }

    /// 处理 `tool_execution_start` 事件。
    pub fn on_tool_start(&self, payload: &Value) {
        let tool_name = payload
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let call_id = payload
            .get("toolCallId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args = payload.get("args").cloned().unwrap_or(Value::Null);
        self.tool_starts
            .lock()
            .insert(call_id.to_string(), Instant::now());
        if self.tool_cli_verbosity != ToolCliVerbosity::Full {
            return;
        }
        let summary = one_line_summary(tool_name, &args);
        // 切到 stderr 装饰区前，先把 markdown 残余冲掉以保证排序。
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
        let mut st = self.state.lock();
        // tool 装饰前总是补一个换行，让它独占行（与 thinking 区块分隔）。
        if st.last_kind != LastKind::None {
            self.writer.write_stderr("\n");
        }
        let line = format!("\x1b[90m[tool] {}  {}\x1b[0m\n", tool_name, summary);
        self.writer.write_stderr(&line);
        st.last_kind = LastKind::ToolStart;
    }

    /// P1（bash background monitor）：处理 `tool_execution_update` 事件。
    /// 当前只渲染 `task_output(block=true)` 的等待倒计时，每条 update 把
    /// `partial_result.phase` / `wakeReason` / `remainingMs` 拼成一行 dim 灰字
    /// 写到 stderr，**不**改 `last_kind`，避免影响后续正文分隔。
    pub fn on_tool_update(&self, payload: &Value) {
        if self.tool_cli_verbosity != ToolCliVerbosity::Full {
            return;
        }
        let tool_name = payload
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let partial = payload
            .get("partialResult")
            .cloned()
            .unwrap_or(Value::Null);
        let phase = partial
            .get("phase")
            .and_then(|v| v.as_str())
            .unwrap_or("update");
        let task_id = partial
            .get("taskId")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let remaining = partial
            .get("remainingMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let timeout = partial
            .get("timeoutMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let line = format!(
            "\x1b[90m[tool] {name} … {phase}  task={task} remaining={remaining}/{timeout}ms\x1b[0m\n",
            name = tool_name,
            phase = phase,
            task = task_id,
            remaining = remaining,
            timeout = timeout,
        );
        self.writer.write_stderr(&line);
    }

    /// 处理 `tool_execution_end` 事件。
    pub fn on_tool_end(&self, payload: &Value) {
        let tool_name = payload
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let call_id = payload
            .get("toolCallId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_error = payload
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let result = payload.get("result").cloned().unwrap_or(Value::Null);
        let display = payload.get("display").and_then(parse_tool_display);
        if self.tool_cli_verbosity == ToolCliVerbosity::Off {
            self.tool_starts.lock().remove(call_id);
            return;
        }
        let elapsed = self
            .tool_starts
            .lock()
            .remove(call_id)
            .map(|t| t.elapsed())
            .map(|d| {
                if d.as_millis() < 1000 {
                    format!("{}ms", d.as_millis())
                } else {
                    format!("{:.1}s", d.as_secs_f64())
                }
            })
            .unwrap_or_else(|| "?".to_string());
        let summary = result_summary_for_tool(&result, display.as_ref(), is_error);
        let (icon, color) = if is_error {
            ("✗", "\x1b[31m")
        } else {
            ("✓", "\x1b[32m")
        };
        let line = format!(
            "{color}[tool] {name}  {icon} {summary} ({elapsed})\x1b[0m\n",
            color = color,
            name = tool_name,
            icon = icon,
            summary = summary,
            elapsed = elapsed
        );
        self.writer.write_stderr(&line);
        // 失败时把错误摘要再扩 N 行，便于快速看到原因。
        if is_error && self.tool_cli_verbosity == ToolCliVerbosity::Full {
            for line in error_extra_lines(&result, 3) {
                self.writer
                    .write_stderr(&format!("\x1b[31m       {}\x1b[0m\n", line));
            }
        }
        // tool end 之后并不严格切换通道——下一帧若是 content，会自然换行。
        // 这里不改 last_kind，保持「tool 区已打印过」的状态信息以维持下次切换的换行。
    }

    /// 写到 thinking 通道（默认 stdout，可切 stderr 作为 prompt 逃生阀）。
    fn write_thinking(&self, s: &str) {
        if self.print_to_stderr {
            self.writer.write_stderr(s);
        } else {
            self.writer.write_stdout(s);
        }
    }

    /// 注册到 EventBus，返回需要在回合结束时反注册的 listener id 集合。
    pub fn register(self: &Arc<Self>, bus: &dyn EventBus) -> CliTurnRendererListenerIds {
        let me = Arc::clone(self);
        let msg = bus.on(
            wire::WIRE_MESSAGE_UPDATE,
            Box::new(move |evt: EventContext| {
                me.on_message_update(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_start = bus.on(
            wire::WIRE_TOOL_EXECUTION_START,
            Box::new(move |evt: EventContext| {
                me.on_tool_start(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_update = bus.on(
            wire::WIRE_TOOL_EXECUTION_UPDATE,
            Box::new(move |evt: EventContext| {
                me.on_tool_update(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_end = bus.on(
            wire::WIRE_TOOL_EXECUTION_END,
            Box::new(move |evt: EventContext| {
                me.on_tool_end(&evt.payload);
                Ok(())
            }),
        );
        CliTurnRendererListenerIds {
            msg,
            tool_start,
            tool_update,
            tool_end,
        }
    }

    pub fn unregister(bus: &dyn EventBus, ids: &CliTurnRendererListenerIds) {
        bus.off(ids.msg);
        bus.off(ids.tool_start);
        bus.off(ids.tool_update);
        bus.off(ids.tool_end);
    }
}

/// 由 [`CliTurnRenderer::register`] 返回的 listener id 句柄，回合结束时反注册。
pub struct CliTurnRendererListenerIds {
    pub msg: EventListenerId,
    pub tool_start: EventListenerId,
    pub tool_update: EventListenerId,
    pub tool_end: EventListenerId,
}

/// 工具调用单行摘要（`[tool] {name}  {summary}` 中间那段）。
///
/// 内置 read/write/edit/bash 用更友好的字段；其他工具退回 `arg=value` 串联。
/// bash 完整展示 command+argv（不截断）；其余工具最长 120 char。
pub fn one_line_summary(tool_name: &str, args: &Value) -> String {
    const DEFAULT_MAX_CHARS: usize = 120;
    let summary = match tool_name {
        "read" | "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut out = format!("path={}", path);
            if let Some(off) = args.get("offset").and_then(|v| v.as_i64()) {
                out.push_str(&format!(" offset={}", off));
            }
            if let Some(lim) = args.get("limit").and_then(|v| v.as_i64()) {
                out.push_str(&format!(" limit={}", lim));
            }
            out
        }
        "write" | "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("path={} (overwrite)", path)
        }
        "edit" | "edit_file" | "str_replace" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("path={} (replace)", path)
        }
        "bash" | "shell" | "execute_command" => {
            format!("command={}", shell_command_preview(args))
        }
        _ => {
            // 通用回退：JSON 化后压成一行
            args.to_string().replace('\n', " ")
        }
    };
    if matches!(tool_name, "bash" | "shell" | "execute_command") {
        summary
    } else {
        truncate_chars(&summary, DEFAULT_MAX_CHARS)
    }
}

/// bash 工具摘要：完整展示 command + argv，不截断、不只取脚本首行。
fn shell_command_preview(args: &Value) -> String {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let argv: Vec<&str> = args
        .get("args")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if !argv.is_empty() {
        let joined_argv = argv.join(" ");
        if command.is_empty() {
            bash_command_for_terminal(&joined_argv)
        } else {
            bash_command_for_terminal(&format!("{command} {joined_argv}"))
        }
    } else if command.is_empty() {
        String::new()
    } else {
        bash_command_for_terminal(command)
    }
}

/// 把多行脚本压成单行展示，但保留全部非空内容（不截断字符）。
fn bash_command_for_terminal(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        text.trim().to_string()
    } else {
        lines.join(" ")
    }
}

/// 解析 `tool_execution_end.result`：plan 工具等常把 JSON 对象序列化成字符串落盘。
fn parse_tool_result_value(result: &Value) -> Value {
    if let Some(s) = result.as_str() {
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return v;
        }
    }
    result.clone()
}

fn parse_tool_display(value: &Value) -> Option<ToolDisplay> {
    serde_json::from_value(value.clone()).ok()
}

/// 把 `~/.tomcat/...` 展开成绝对路径，便于终端识别为可点击 file link。
fn expand_path_for_terminal(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir()
            .map(|h| h.display().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    path.to_string()
}

fn display_summary(display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::File { file } => expand_path_for_terminal(file),
        ToolDisplay::Plan { plan } => expand_path_for_terminal(plan),
        ToolDisplay::Text { text } => text.trim().to_string(),
    }
}

/// 工具结果摘要（`✓/✗ {summary}`），尽量挑可读字段；失败时塞 error 文案。
pub fn result_summary(result: &Value, is_error: bool) -> String {
    result_summary_for_tool(result, None, is_error)
}

/// 工具结果摘要：优先使用结构化 `display`，否则退化到通用字段。
pub fn result_summary_for_tool(
    result: &Value,
    display: Option<&ToolDisplay>,
    is_error: bool,
) -> String {
    const DEFAULT_MAX_CHARS: usize = 80;
    const PATH_MAX_CHARS: usize = 512;
    if is_error {
        if let Some(s) = result.as_str() {
            let msg = s.trim();
            if !msg.is_empty() {
                return truncate_chars(msg, PATH_MAX_CHARS);
            }
        }
        let parsed = parse_tool_result_value(result);
        let msg = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
            .unwrap_or("failed");
        return truncate_chars(msg, PATH_MAX_CHARS);
    }

    if let Some(display) = display {
        let summary = display_summary(display);
        if !summary.is_empty() {
            let max_chars =
                if matches!(display, ToolDisplay::File { .. } | ToolDisplay::Plan { .. }) {
                    PATH_MAX_CHARS
                } else {
                    DEFAULT_MAX_CHARS
                };
            return truncate_chars(&summary, max_chars);
        }
    }

    let parsed = parse_tool_result_value(result);
    if let Some(lines) = parsed.get("lines").and_then(|v| v.as_u64()) {
        return format!("{} lines", lines);
    }
    if let Some(bytes) = parsed.get("bytes").and_then(|v| v.as_u64()) {
        return format!("{} bytes", bytes);
    }
    if let Some(s) = parsed
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
    {
        return truncate_chars(s, DEFAULT_MAX_CHARS);
    }
    "ok".to_string()
}

/// 失败时附加最多 `n` 行的错误细节（取 `stderr` / `error` 字段的前 N 行）。
pub fn error_extra_lines(result: &Value, n: usize) -> Vec<String> {
    const ERROR_LINE_MAX_CHARS: usize = 512;
    let raw = result
        .get("stderr")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("error").and_then(|v| v.as_str()))
        .or_else(|| result.as_str())
        .unwrap_or("");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .map(|s| truncate_chars(s, ERROR_LINE_MAX_CHARS))
        .collect()
}

fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests;
