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
//! - **thinking_display** 用 `AtomicU8`：让 `/thinking` 命令（P4）能跨线程切换；
//!   `minimal` 打一行占位，`summary` 仅显示 summary，`full` 显示 summary + raw。
//! - **打印通道**：正文 stdout（沿用 `MarkdownRenderer.flush` 路径），thinking 默认
//!   stdout（`print_to_stderr=true` 切到 stderr 作为 prompt 打架逃生阀），tool 始终
//!   stderr（与现有 ctx/search_tools 装饰一致）。
//! - **状态机**：`last_kind` 跟踪 *上一帧打印通道*，仅在通道切换或 `[tool]` 装饰前
//!   补 `\n`，避免出现「正文中间夹一段 thinking 没换行」的情况。
//!
//! ## 测试入口
//!
//! 见父目录 `tests/cli_turn_renderer_test.rs`：以 `Sink` 替换 stdout/stderr，覆盖
//! folded vs expanded、tool start/end、kind 切换换行等。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde_json::Value;

use crate::api::render::MarkdownRenderer;
use crate::core::llm::ThinkingSource;
use crate::infra::config::{ThinkingDisplay, ToolCliVerbosity};
use crate::infra::event_bus::{EventContext, EventListenerId};
use crate::infra::events::ToolDisplay;
use crate::infra::{wire, EventBus};

fn format_countdown_ms(ms: u64) -> String {
    let total_secs = ms.saturating_add(999) / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{total_secs}s")
    }
}

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
    /// stderr 是否连接到真实交互终端（TTY）。默认 false，测试 writer 可按需覆盖。
    fn stderr_is_terminal(&self) -> bool {
        false
    }
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
    fn stderr_is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    }
}

/// `CliTurnRenderer` 自身可被多个 listener 闭包共享，因此用内部 `Mutex` 保护可变态。
pub struct CliTurnRenderer {
    md: Arc<Mutex<MarkdownRenderer>>,
    session_id: Option<String>,
    thinking_display: Arc<AtomicU8>,
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
    /// 不区分 summary/raw：当前 assistant message 第一次出现可见 thinking 时打前缀，
    /// 后续 delta 都在同一条 thinking 行追加。
    thinking_prefix_printed: bool,
    /// `task_output(block=true)` 倒计时在真实终端走单行原地刷新；该标记表示当前
    /// stderr 上存在一条待覆盖的内联倒计时行，切换到其它输出前需先清掉。
    inline_tool_update_active: bool,
}

impl CliTurnRenderer {
    /// 业务路径用此构造：包装 stdout/stderr 真·writer。
    /// `print_to_stderr` 由调用方读 `config.llm.thinking.print_to_stderr` 后传入；
    /// 见 chat/mod.rs（架构 §3.1 / 计划 §1 已决策「print_to_stderr」）。
    pub fn new(
        md: Arc<Mutex<MarkdownRenderer>>,
        thinking_display: Arc<AtomicU8>,
        session_id: Option<String>,
        print_to_stderr: bool,
        tool_cli_verbosity: ToolCliVerbosity,
    ) -> Arc<Self> {
        Self::with_writer(
            md,
            thinking_display,
            session_id,
            Arc::new(StdCliWriter),
            print_to_stderr,
            tool_cli_verbosity,
        )
    }

    /// 测试 / 高级路径：可注入自定义 writer 与 `print_to_stderr` 开关。
    pub fn with_writer(
        md: Arc<Mutex<MarkdownRenderer>>,
        thinking_display: Arc<AtomicU8>,
        session_id: Option<String>,
        writer: Arc<dyn CliWriter>,
        print_to_stderr: bool,
        tool_cli_verbosity: ToolCliVerbosity,
    ) -> Arc<Self> {
        Arc::new(Self {
            md,
            session_id,
            thinking_display,
            print_to_stderr,
            tool_cli_verbosity,
            state: Mutex::new(RendererState {
                last_kind: LastKind::None,
                thinking_prefix_printed: false,
                inline_tool_update_active: false,
            }),
            writer,
            tool_starts: Mutex::new(std::collections::HashMap::new()),
        })
    }

    fn accepts(&self, event_session_id: Option<&str>) -> bool {
        match self.session_id.as_deref() {
            None => true,
            Some(bound_session_id) => match event_session_id {
                None => true,
                Some(event_session_id) => event_session_id == bound_session_id,
            },
        }
    }

    /// 把所有 markdown 残余冲走（chat_loop 在 run 结束后调用）。
    pub fn flush_markdown(&self) {
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
    }

    /// 单测可读：当前 thinking 显示档位。
    #[cfg(test)]
    pub(crate) fn thinking_display(&self) -> ThinkingDisplay {
        ThinkingDisplay::from_u8(self.thinking_display.load(Ordering::Acquire))
    }

    /// 新 assistant message 开始时重置 thinking 前缀状态；若上一条消息恰好停在
    /// thinking 行末尾，则先补一个换行，避免下一条消息的 `[thinking]` 接在上一条后面。
    pub fn on_message_start(&self) {
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
        if st.last_kind == LastKind::Thinking {
            self.write_thinking("\n");
        }
        st.thinking_prefix_printed = false;
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
                let source = match parse_thinking_source(event) {
                    Some(source) => source,
                    None => return,
                };
                self.handle_thinking_delta(delta, source);
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

    pub fn on_llm_error(&self, payload: &Value) {
        let message = payload
            .get("errorMessage")
            .or_else(|| payload.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return;
        }
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
        if st.last_kind != LastKind::None {
            self.writer.write_stderr("\n");
        }
        self.writer
            .write_stderr(&format!("\x1b[31m[llm] {message}\x1b[0m\n"));
        st.last_kind = LastKind::ToolStart;
    }

    pub fn on_llm_notice(&self, payload: &Value) {
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return;
        }
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
        if st.last_kind != LastKind::None {
            self.writer.write_stderr("\n");
        }
        self.writer
            .write_stderr(&format!("\x1b[2m\x1b[90m[llm] {message}\x1b[0m\n"));
        st.last_kind = LastKind::ToolStart;
    }

    fn handle_thinking_delta(&self, delta: &str, source: ThinkingSource) {
        let display = ThinkingDisplay::from_u8(self.thinking_display.load(Ordering::Acquire));
        if matches!(display, ThinkingDisplay::Minimal) {
            let mut st = self.state.lock();
            if st.inline_tool_update_active {
                self.writer.write_stderr("\r\x1b[2K");
                self.write_thinking("\n");
                st.inline_tool_update_active = false;
            }
            if st.last_kind == LastKind::Content {
                drop(st); // 释放锁，避免与 md.lock 死锁顺序冲突
                if let Some(remaining) = self.md.lock().flush() {
                    self.writer.write_stdout(&remaining);
                }
                st = self.state.lock();
                self.write_thinking("\n");
            }
            if !st.thinking_prefix_printed {
                self.write_thinking("\x1b[2m\x1b[90m[thinking] ...\x1b[0m");
                st.thinking_prefix_printed = true;
                st.last_kind = LastKind::Thinking;
            }
            return;
        }
        if source == ThinkingSource::Summary && !display.shows_summary() {
            return;
        }
        if source == ThinkingSource::Raw && !display.shows_raw() {
            return;
        }
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            self.write_thinking("\n");
            st.inline_tool_update_active = false;
        }
        // 思考通道与正文/工具切换时，先冲走 markdown 残余 + 换行，避免「正文 ... [thinking]」黏一行。
        if st.last_kind == LastKind::Content {
            drop(st); // 释放锁，避免与 md.lock 死锁顺序冲突
            if let Some(remaining) = self.md.lock().flush() {
                self.writer.write_stdout(&remaining);
            }
            st = self.state.lock();
            self.write_thinking("\n");
        }
        // 可见 thinking（summary/full 模式下的 summary/raw）统一走单行流式追加。
        if !st.thinking_prefix_printed {
            self.write_thinking("\x1b[2m\x1b[90m[thinking]\x1b[0m ");
            st.thinking_prefix_printed = true;
        }
        self.write_thinking(&format!("\x1b[2m\x1b[90m{}\x1b[0m", delta));
        st.last_kind = LastKind::Thinking;
    }

    fn handle_content_delta(&self, delta: &str) {
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            self.writer.write_stdout("\n");
            st.inline_tool_update_active = false;
        }
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

    /// 处理 `tool_call_streaming` 事件：当 write/edit 大参数仍在流式到达时，提前给一条轻量提示。
    pub fn on_tool_call_streaming(&self, payload: &Value) {
        if self.tool_cli_verbosity != ToolCliVerbosity::Full {
            return;
        }
        let tool_name = payload
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let args_preview = payload.get("argsPreview").cloned().unwrap_or(Value::Null);
        let summary = tool_call_streaming_summary(&args_preview);
        if let Some(remaining) = self.md.lock().flush() {
            self.writer.write_stdout(&remaining);
        }
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
        if st.last_kind != LastKind::None {
            self.writer.write_stderr("\n");
        }
        let line = format!("\x1b[90m[tool] {}  {}\x1b[0m\n", tool_name, summary);
        self.writer.write_stderr(&line);
        // 预告行已经自带换行，后续真实 tool_start 不应再额外插空行。
        st.last_kind = LastKind::None;
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
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
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
        if !self.writer.stderr_is_terminal() {
            return;
        }
        let tool_name = payload
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let partial = payload.get("partialResult").cloned().unwrap_or(Value::Null);
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
            .get("waitMs")
            .or_else(|| partial.get("timeoutMs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let remaining_label = format_countdown_ms(remaining);
        let timeout_label = format_countdown_ms(timeout);
        let line = format!(
            "\r\x1b[2K\x1b[90m[tool] {name} … {phase}  task={task} remaining={remaining}/{timeout}\x1b[0m",
            name = tool_name,
            phase = phase,
            task = task_id,
            remaining = remaining_label,
            timeout = timeout_label,
        );
        let mut st = self.state.lock();
        self.writer.write_stderr(&line);
        st.inline_tool_update_active = true;
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
        let mut st = self.state.lock();
        if st.inline_tool_update_active {
            self.writer.write_stderr("\r\x1b[2K");
            st.inline_tool_update_active = false;
        }
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
        let msg_start = bus.on(
            wire::WIRE_MESSAGE_START,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_message_start();
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let msg = bus.on(
            wire::WIRE_MESSAGE_UPDATE,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_message_update(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_call_streaming = bus.on(
            wire::WIRE_TOOL_CALL_STREAMING,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_tool_call_streaming(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_start = bus.on(
            wire::WIRE_TOOL_EXECUTION_START,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_tool_start(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_update = bus.on(
            wire::WIRE_TOOL_EXECUTION_UPDATE,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_tool_update(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let tool_end = bus.on(
            wire::WIRE_TOOL_EXECUTION_END,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_tool_end(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let llm_error = bus.on(
            wire::WIRE_LLM_ERROR,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_llm_error(&evt.payload);
                Ok(())
            }),
        );
        let me = Arc::clone(self);
        let llm_notice = bus.on(
            wire::WIRE_LLM_NOTICE,
            Box::new(move |evt: EventContext| {
                if !me.accepts(evt.session_id.as_deref()) {
                    return Ok(());
                }
                me.on_llm_notice(&evt.payload);
                Ok(())
            }),
        );
        CliTurnRendererListenerIds {
            msg_start,
            msg,
            tool_call_streaming,
            tool_start,
            tool_update,
            tool_end,
            llm_error,
            llm_notice,
        }
    }

    pub fn unregister(bus: &dyn EventBus, ids: &CliTurnRendererListenerIds) {
        bus.off(ids.msg_start);
        bus.off(ids.msg);
        bus.off(ids.tool_call_streaming);
        bus.off(ids.tool_start);
        bus.off(ids.tool_update);
        bus.off(ids.tool_end);
        bus.off(ids.llm_error);
        bus.off(ids.llm_notice);
    }
}

/// 由 [`CliTurnRenderer::register`] 返回的 listener id 句柄，回合结束时反注册。
pub struct CliTurnRendererListenerIds {
    pub msg_start: EventListenerId,
    pub msg: EventListenerId,
    pub tool_call_streaming: EventListenerId,
    pub tool_start: EventListenerId,
    pub tool_update: EventListenerId,
    pub tool_end: EventListenerId,
    pub llm_error: EventListenerId,
    pub llm_notice: EventListenerId,
}

fn parse_thinking_source(event: &Value) -> Option<ThinkingSource> {
    match event.get("source").and_then(|v| v.as_str()) {
        Some("summary") => Some(ThinkingSource::Summary),
        Some("raw") => Some(ThinkingSource::Raw),
        other => {
            tracing::debug!(
                target: "tomcat::cli_turn_renderer",
                source = ?other,
                payload = ?event,
                "ignoring thinking_delta without valid source"
            );
            None
        }
    }
}

fn tool_call_streaming_summary(args_preview: &Value) -> String {
    if let Some(path) = args_preview.get("path").and_then(|v| v.as_str()) {
        format!("path={}  receiving args...", expand_path_for_terminal(path))
    } else {
        "receiving args...".to_string()
    }
}

pub use crate::core::summary::one_line_summary;

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
        return crate::infra::platform::home_dir()
            .map(|h| h.display().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = crate::infra::platform::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    path.to_string()
}

fn display_summary(display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::File { file, .. } => expand_path_for_terminal(file),
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
#[path = "tests/cli_turn_renderer_test.rs"]
mod tests;
