//! `CliTurnRenderer` 单测：以 `Sink` 替换 stdout/stderr，覆盖
//! - thinking 折叠 vs 展开（`/thinking` 行为）；
//! - thinking → content 切换时的换行；
//! - tool_execution_start/end 排版与失败摘要；
//! - 工具单行摘要的内置类型（read/bash）与回退（未知工具）；
//! - kind 缺省走 content_delta 的向后兼容性。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;

use super::{
    error_extra_lines, one_line_summary, result_summary, result_summary_for_tool, CliTurnRenderer,
    CliWriter,
};
use crate::api::render::MarkdownRenderer;
use crate::infra::config::{ThinkingDisplay, ToolCliVerbosity};
use crate::infra::event_bus::{DefaultEventBus, EventBus, EventContext};
use crate::infra::events::ToolDisplay;
use crate::infra::wire;

#[derive(Default)]
struct CapturedWriter {
    stdout: Mutex<String>,
    stderr: Mutex<String>,
    stderr_is_terminal: bool,
}

impl CapturedWriter {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn new_tty() -> Arc<Self> {
        Arc::new(Self {
            stderr_is_terminal: true,
            ..Self::default()
        })
    }
    fn stdout(&self) -> String {
        self.stdout.lock().clone()
    }
    fn stderr(&self) -> String {
        self.stderr.lock().clone()
    }
}

impl CliWriter for CapturedWriter {
    fn write_stdout(&self, s: &str) {
        self.stdout.lock().push_str(s);
    }
    fn write_stderr(&self, s: &str) {
        self.stderr.lock().push_str(s);
    }
    fn stderr_is_terminal(&self) -> bool {
        self.stderr_is_terminal
    }
}

fn make_renderer(display: ThinkingDisplay) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    make_renderer_with_tool_verbosity(display, ToolCliVerbosity::Full)
}

fn make_tty_renderer(display: ThinkingDisplay) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    make_tty_renderer_with_tool_verbosity(display, ToolCliVerbosity::Full)
}

fn make_renderer_with_tool_verbosity(
    display: ThinkingDisplay,
    tool_cli_verbosity: ToolCliVerbosity,
) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicU8::new(display.as_u8()));
    let r = CliTurnRenderer::with_writer(
        md,
        flag,
        None,
        writer.clone() as Arc<dyn CliWriter>,
        false,
        tool_cli_verbosity,
    );
    (r, writer)
}

fn make_tty_renderer_with_tool_verbosity(
    display: ThinkingDisplay,
    tool_cli_verbosity: ToolCliVerbosity,
) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    let writer = CapturedWriter::new_tty();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicU8::new(display.as_u8()));
    let r = CliTurnRenderer::with_writer(
        md,
        flag,
        None,
        writer.clone() as Arc<dyn CliWriter>,
        false,
        tool_cli_verbosity,
    );
    (r, writer)
}

fn make_session_bound_renderer(
    display: ThinkingDisplay,
    session_id: &str,
) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicU8::new(display.as_u8()));
    let r = CliTurnRenderer::with_writer(
        md,
        flag,
        Some(session_id.to_string()),
        writer.clone() as Arc<dyn CliWriter>,
        false,
        ToolCliVerbosity::Full,
    );
    (r, writer)
}

#[test]
fn summary_mode_shows_summary_but_hides_raw() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step a", "source": "summary"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "internal raw", "source": "raw"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": " step c", "source": "summary"}
    }));
    let s = w.stdout();
    let count = s.matches("[thinking]").count();
    assert_eq!(count, 1, "折叠模式下整回合只应打一次 [thinking]: {:?}", s);
    assert!(
        s.contains("step a") && s.contains("step c"),
        "折叠模式应显示 summary delta 文本: {:?}",
        s
    );
    assert!(
        !s.contains("internal raw"),
        "折叠模式不应输出 raw thinking delta: {:?}",
        s
    );
}

#[test]
fn full_mode_streams_each_delta_with_dim_color() {
    let (r, w) = make_renderer(ThinkingDisplay::Full);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step a", "source": "raw"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": " step b", "source": "summary"}
    }));
    let s = w.stdout();
    assert_eq!(
        s.matches("[thinking]").count(),
        1,
        "前缀 [thinking] 只应出现一次，后续 delta 直接续写: {:?}",
        s
    );
    assert!(
        s.contains("step a") && s.contains("step b"),
        "展开模式应保留 delta 文本: {:?}",
        s
    );
    assert!(s.contains("\x1b[2m"), "应使用 dim ANSI: {:?}", s);
    assert!(s.contains("\x1b[90m"), "应使用 gray ANSI: {:?}", s);
}

#[test]
fn minimal_mode_prints_placeholder_only_once() {
    let (r, w) = make_renderer(ThinkingDisplay::Minimal);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "summary a", "source": "summary"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": " raw b", "source": "raw"}
    }));
    let s = w.stdout();
    assert_eq!(
        s.matches("[thinking]").count(),
        1,
        "minimal 只应打一行占位: {:?}",
        s
    );
    assert!(
        s.contains("[thinking] ..."),
        "minimal 应输出固定占位: {:?}",
        s
    );
    assert!(
        !s.contains("summary a"),
        "minimal 不应输出 summary 正文: {:?}",
        s
    );
    assert!(!s.contains("raw b"), "minimal 不应输出 raw 正文: {:?}", s);
}

#[test]
fn content_delta_after_thinking_inserts_newline() {
    let (r, w) = make_renderer(ThinkingDisplay::Full);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "plan", "source": "summary"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "content_delta", "delta": "answer"}
    }));
    r.flush_markdown();
    let s = w.stdout();
    let thinking_idx = s.find("[thinking]").expect("thinking 必现");
    let answer_idx = s.find("answer").expect("answer 必现");
    assert!(thinking_idx < answer_idx, "thinking 应早于正文: {:?}", s);
    let between = &s[thinking_idx..answer_idx];
    assert!(
        between.contains('\n'),
        "thinking 与正文之间必须有换行: {:?}",
        between
    );
}

#[test]
fn missing_kind_defaults_to_content_for_back_compat() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"delta": "legacy"}
    }));
    r.flush_markdown();
    assert!(
        w.stdout().contains("legacy"),
        "缺 kind 应当走 content_delta 老路径: {:?}",
        w.stdout()
    );
}

#[test]
fn empty_thinking_delta_is_skipped() {
    let (r, w) = make_renderer(ThinkingDisplay::Full);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "", "source": "raw"}
    }));
    assert!(
        w.stdout().is_empty() && w.stderr().is_empty(),
        "空 delta 不应触发任何输出"
    );
}

#[test]
fn thinking_delta_without_source_is_ignored() {
    let (r, w) = make_renderer(ThinkingDisplay::Full);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "secret"}
    }));
    assert!(
        w.stdout().is_empty() && w.stderr().is_empty(),
        "缺 source 的 thinking_delta 应被忽略"
    );
}

#[test]
fn tool_start_emits_gray_summary_on_stderr() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    // 先有正文，让 last_kind != None，从而在 tool start 前补换行
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "content_delta", "delta": "hi"}
    }));
    r.on_tool_start(&json!({
        "toolCallId": "c1",
        "toolName": "read",
        "args": {"path": "src/main.rs", "limit": 200},
    }));
    let err = w.stderr();
    assert!(
        err.contains("[tool] read"),
        "应有 [tool] read 装饰: {:?}",
        err
    );
    assert!(
        err.contains("path=src/main.rs"),
        "应有 path 摘要: {:?}",
        err
    );
    assert!(err.contains("limit=200"), "应有 limit 摘要: {:?}", err);
    assert!(err.contains("\x1b[90m"), "应使用 gray ANSI: {:?}", err);
}

/// P1（bash background monitor）：真实终端 TTY 下倒计时走单行原地刷新，单位为秒。
#[test]
fn tool_update_emits_inline_countdown_line_on_tty_stderr() {
    let (r, w) = make_tty_renderer(ThinkingDisplay::Summary);
    r.on_tool_update(&json!({
        "toolCallId": "blk-1",
        "toolName": "task_output",
        "args": {"task_id": "t-1", "block": true, "timeout_ms": 3000},
        "partialResult": {
            "phase": "waiting_for_output",
            "taskId": "t-1",
            "since": 0,
            "timeoutMs": 3000,
            "remainingMs": 1500
        },
    }));
    let err = w.stderr();
    assert!(
        err.contains("waiting_for_output"),
        "应包含 phase: {:?}",
        err
    );
    assert!(
        err.contains("task=t-1") && err.contains("remaining=2s/3s"),
        "应展示秒级倒计时: {:?}",
        err
    );
    assert!(
        err.contains("\r\x1b[2K"),
        "TTY 倒计时应使用回车覆盖同一行: {:?}",
        err
    );
    assert!(!err.contains('\n'), "TTY 倒计时不应每次追加换行: {:?}", err);
    assert!(err.contains("\x1b[90m"), "倒计时应使用 dim 灰: {:?}", err);
}

#[test]
fn tool_update_is_suppressed_on_non_tty_stderr() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_update(&json!({
        "toolCallId": "blk-1",
        "toolName": "task_output",
        "args": {"task_id": "t-1", "block": true, "timeout_ms": 3000},
        "partialResult": {
            "phase": "waiting_for_output",
            "taskId": "t-1",
            "since": 0,
            "timeoutMs": 3000,
            "remainingMs": 1500
        },
    }));
    assert!(
        w.stderr().is_empty(),
        "非 TTY stderr 不应绘制倒计时动画，实际: {:?}",
        w.stderr()
    );
}

#[test]
fn tool_end_success_uses_green_check_and_elapsed() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_start(&json!({
        "toolCallId": "c2",
        "toolName": "read",
        "args": {"path": "a.rs"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c2",
        "toolName": "read",
        "result": {"lines": 42},
        "isError": false,
    }));
    let err = w.stderr();
    assert!(err.contains("✓"), "成功应有 ✓: {:?}", err);
    assert!(err.contains("\x1b[32m"), "成功应使用绿色: {:?}", err);
    assert!(err.contains("42 lines"), "应展示行数摘要: {:?}", err);
    assert!(
        err.contains("ms") || err.contains("s)"),
        "应展示 elapsed: {:?}",
        err
    );
}

#[test]
fn tool_end_failure_uses_red_cross_and_extra_lines() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_start(&json!({
        "toolCallId": "c3",
        "toolName": "bash",
        "args": {"command": "cargo build"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c3",
        "toolName": "bash",
        "result": {
            "error": "build failed",
            "stderr": "error[E0308]: mismatched\nhelp: try this\nnote: ignored",
        },
        "isError": true,
    }));
    let err = w.stderr();
    assert!(err.contains("✗"), "失败应有 ✗: {:?}", err);
    assert!(err.contains("\x1b[31m"), "失败应使用红色: {:?}", err);
    assert!(
        err.contains("build failed"),
        "失败应展示 error 摘要: {:?}",
        err
    );
    assert!(
        err.contains("E0308"),
        "失败应展开 stderr 前 3 行: {:?}",
        err
    );
}

#[test]
fn tool_end_failure_with_string_result_shows_real_error_message() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_start(&json!({
        "toolCallId": "c4",
        "toolName": "read",
        "args": {"path": "missing.txt"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c4",
        "toolName": "read",
        "result": "No such file or directory (os error 2)",
        "isError": true,
    }));
    let err = w.stderr();
    assert!(err.contains("✗"), "失败应有 ✗: {:?}", err);
    assert!(
        err.contains("No such file or directory"),
        "字符串错误结果应直接可见，不应退化为 failed: {:?}",
        err
    );
    assert!(
        !err.contains("✗ failed"),
        "有真实字符串错误时不应显示 failed 占位: {:?}",
        err
    );
}

#[test]
fn llm_error_renders_red_status_line() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "content_delta", "delta": "partial"}
    }));
    r.on_llm_error(&json!({
        "reason": "error:boom",
        "errorMessage": "boom"
    }));
    let stdout = w.stdout();
    let stderr = w.stderr();
    assert!(stdout.contains("partial"), "应先冲掉正文残余: {:?}", stdout);
    assert!(
        stderr.contains("[llm] boom"),
        "应展示 llm 错误提示: {:?}",
        stderr
    );
    assert!(stderr.contains("\x1b[31m"), "错误应使用红色: {:?}", stderr);
}

#[test]
fn llm_notice_renders_dim_non_error_hint() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_llm_notice(&json!({
        "finishReason": "max_output_tokens",
        "message": "达到 max_output_tokens，回答可能未完成"
    }));
    let stderr = w.stderr();
    assert!(
        stderr.contains("max_output_tokens"),
        "轻提示应说明截断原因: {:?}",
        stderr
    );
    assert!(
        stderr.contains("\x1b[90m"),
        "轻提示应使用灰色: {:?}",
        stderr
    );
    assert!(
        !stderr.contains("\x1b[31m"),
        "轻提示不应被渲染成红色错误: {:?}",
        stderr
    );
}

#[test]
fn one_line_summary_handles_known_and_unknown_tools() {
    assert_eq!(
        one_line_summary("read", &json!({"path": "a.rs", "offset": 1, "limit": 10})),
        "path=a.rs offset=1 limit=10"
    );
    assert_eq!(
        one_line_summary("write", &json!({"path": "b.rs"})),
        "path=b.rs (overwrite)"
    );
    assert_eq!(
        one_line_summary("bash", &json!({"command": "ls -la\nrm -rf"})),
        "command=ls -la rm -rf"
    );
    let unknown = one_line_summary("custom", &json!({"k": "v"}));
    assert!(
        unknown.contains("\"k\""),
        "未知工具应回退 JSON 串联: {}",
        unknown
    );
}

#[test]
fn one_line_summary_handles_bash_argv_and_script_preview() {
    assert_eq!(
        one_line_summary(
            "bash",
            &json!({"command": "bash", "args": ["-lc", "echo hi && pwd"]})
        ),
        "command=bash -lc echo hi && pwd"
    );
    assert_eq!(
        one_line_summary(
            "bash",
            &json!({"command": "bash", "args": ["-lc", "\n\n  echo first\npwd"]})
        ),
        "command=bash -lc echo first pwd"
    );
    assert_eq!(
        one_line_summary("bash", &json!({"command": "bash", "args": ["-lc"]})),
        "command=bash -lc"
    );
}

#[test]
fn one_line_summary_does_not_truncate_long_bash_command() {
    let long_path = "/Users/yankeben/.tomcat/temp/cli_real_llm_wwww-mi-com_2794_1779277773225413000/docs/screenshots";
    let script = format!("mkdir -p {long_path} && npm i -D tsx && node scripts/snapshot.ts");
    let summary = one_line_summary("bash", &json!({"command": "bash", "args": ["-lc", script]}));
    assert_eq!(summary, format!("command=bash -lc {script}"));
    assert!(!summary.ends_with('…'));
}

#[test]
fn one_line_summary_truncates_extreme_payload() {
    let long = "x".repeat(500);
    let s = one_line_summary("custom", &json!({"v": long}));
    assert!(
        s.chars().count() <= 121, // 120 + 「…」
        "应截断到 ~120 char: len={}",
        s.chars().count()
    );
    assert!(s.ends_with('…'), "截断后应附 …: {}", s);
}

#[test]
fn result_summary_picks_best_field_for_success_and_error() {
    assert_eq!(result_summary(&json!({"lines": 12}), false), "12 lines");
    assert_eq!(
        result_summary(&json!({"summary": "all good"}), false),
        "all good"
    );
    assert_eq!(result_summary(&json!({}), false), "ok");
    assert_eq!(result_summary(&json!({"error": "boom"}), true), "boom");
    assert_eq!(
        result_summary(&json!("No such file or directory"), true),
        "No such file or directory"
    );
}

#[test]
fn create_plan_success_shows_absolute_plan_path() {
    let home = crate::infra::platform::home_dir().expect("HOME");
    let plan_path = home.join(".tomcat/plans/plan_demo_abcd1234.plan.md");
    let payload = json!({
        "plan_id": "plan_demo_abcd1234",
        "path": format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        "mode": "planning",
    });
    let as_string = serde_json::to_string(&payload).unwrap();
    let summary = result_summary_for_tool(
        &json!(as_string),
        Some(&ToolDisplay::Plan {
            plan: format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        }),
        false,
    );
    assert_eq!(summary, plan_path.display().to_string());
}

#[test]
fn tool_end_create_plan_prints_clickable_path() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    let home = crate::infra::platform::home_dir().expect("HOME");
    let plan_path = home.join(".tomcat/plans/plan_cli_e2e__demo.plan.md");
    let result = json!({
        "plan_id": "plan_cli_e2e__demo",
        "path": format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        "mode": "planning",
    });
    r.on_tool_start(&json!({
        "toolCallId": "cp1",
        "toolName": "create_plan",
        "args": {"goal": "demo"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "cp1",
        "toolName": "create_plan",
        "result": serde_json::to_string(&result).unwrap(),
        "display": {
            "kind": "plan",
            "plan": format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        },
        "isError": false,
    }));
    let err = w.stderr();
    assert!(
        err.contains(&plan_path.display().to_string()),
        "create_plan 成功行应包含绝对 plan 路径: {:?}",
        err
    );
    assert!(err.contains("✓"), "应有成功标记: {:?}", err);
}

#[test]
fn path_display_shows_absolute_path() {
    let home = crate::infra::platform::home_dir().expect("HOME");
    let target = home.join("workspace/demo.rs");
    let summary = result_summary_for_tool(
        &json!("已写入: ~/workspace/demo.rs (2 bytes)"),
        Some(&ToolDisplay::File {
            file: format!("~/{}", target.strip_prefix(&home).unwrap().display()),
        }),
        false,
    );
    assert_eq!(summary, target.display().to_string());
}

#[test]
fn bash_one_line_summary_keeps_long_absolute_workdir_path() {
    let long_path = "/Users/yankeben/.tomcat/temp/cli_real_llm_wwww-mi-com_2794_1779277773225413000/docs/screenshots";
    let summary = one_line_summary("bash", &json!({"command": format!("mkdir -p {long_path}")}));
    assert_eq!(summary, format!("command=mkdir -p {long_path}"));
}

#[test]
fn result_summary_keeps_long_permission_error_path() {
    let long_path = "/Users/yankeben/.tomcat/temp/cli_real_llm_wwww-mi-com_2794_1779277773225413000/docs/screenshots";
    let err = format!(
        "权限错误: 用户拒绝授权: {long_path}。下次工具再次访问该路径时会重新弹出 [s]/[w]/[c] 授权选项。"
    );
    let summary = result_summary(&json!(err.clone()), true);
    assert_eq!(summary, err);
}

#[test]
fn update_plan_success_shows_absolute_plan_path() {
    let home = crate::infra::platform::home_dir().expect("HOME");
    let plan_path = home.join(".tomcat/plans/plan_demo_update.plan.md");
    let payload = json!({
        "plan_id": "plan_demo_update",
        "path": format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        "applied": 1,
    });
    let summary = result_summary_for_tool(
        &json!(serde_json::to_string(&payload).unwrap()),
        Some(&ToolDisplay::Plan {
            plan: format!("~/{}", plan_path.strip_prefix(&home).unwrap().display()),
        }),
        false,
    );
    assert_eq!(summary, plan_path.display().to_string());
}

#[test]
fn config_set_display_prefers_text_message() {
    let payload = json!({
        "applied": true,
        "message": "已设置 llm.default_model = gpt-5.4",
    });
    let summary = result_summary_for_tool(
        &json!(serde_json::to_string(&payload).unwrap()),
        Some(&ToolDisplay::Text {
            text: "已设置 llm.default_model = gpt-5.4".to_string(),
        }),
        false,
    );
    assert_eq!(summary, "已设置 llm.default_model = gpt-5.4");
}

#[test]
fn config_set_falls_back_to_message_without_display() {
    let payload = json!({
        "applied": false,
        "message": "user_denied",
    });
    let summary = result_summary_for_tool(
        &json!(serde_json::to_string(&payload).unwrap()),
        None,
        false,
    );
    assert_eq!(summary, "user_denied");
}

#[test]
fn write_plaintext_without_display_falls_back_to_ok() {
    let payload = "已写入: ~/workspace/demo.txt (12 bytes)";
    let summary = result_summary_for_tool(&json!(payload), None, false);
    assert_eq!(summary, "ok");
}

#[test]
fn tool_end_write_prints_clickable_path() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    let home = crate::infra::platform::home_dir().expect("HOME");
    let target = home.join("workspace/demo.txt");
    let result = format!(
        "已写入: ~/{} (2 bytes)",
        target.strip_prefix(&home).unwrap().display()
    );
    r.on_tool_start(&json!({
        "toolCallId": "w1",
        "toolName": "write",
        "args": {"path": "demo.txt"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "w1",
        "toolName": "write",
        "result": result,
        "display": {
            "kind": "file",
            "file": format!("~/{}", target.strip_prefix(&home).unwrap().display()),
        },
        "isError": false,
    }));
    let err = w.stderr();
    assert!(
        err.contains(&target.display().to_string()),
        "write 成功行应包含绝对目标路径: {:?}",
        err
    );
    assert!(err.contains("✓"), "应有成功标记: {:?}", err);
}

#[test]
fn error_extra_lines_caps_at_n_and_skips_blank() {
    let res = json!({
        "stderr": "line1\n\nline2\nline3\nline4\n"
    });
    let lines = error_extra_lines(&res, 2);
    assert_eq!(lines, vec!["line1".to_string(), "line2".to_string()]);
}

/// E2E-CLI-043（场景库登记名）：模拟用户在 chat 内按顺序 `/thinking summary`、
/// `/thinking minimal`、`/thinking full`、`/thinking toggle` 切档，每档发送同样的
/// summary+raw delta，断言三档可见性与 toggle 循环顺序。覆盖 `apply_action` 与
/// `CliTurnRenderer::handle_thinking_delta` 的运行时切换契约。
#[test]
fn test_user_toggles_thinking_display_modes() {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicU8::new(ThinkingDisplay::Summary.as_u8()));
    let r = CliTurnRenderer::with_writer(
        md,
        flag.clone(),
        None,
        writer.clone() as Arc<dyn CliWriter>,
        false,
        ToolCliVerbosity::Full,
    );

    let set_mode = |mode: ThinkingDisplay| {
        flag.store(mode.as_u8(), Ordering::Release);
    };

    let push_delta_pair = |label_summary: &str, label_raw: &str| {
        r.on_message_start();
        r.on_message_update(&serde_json::json!({
            "assistantMessageEvent": {
                "kind": "thinking_delta",
                "delta": label_summary,
                "source": "summary"
            }
        }));
        r.on_message_update(&serde_json::json!({
            "assistantMessageEvent": {
                "kind": "thinking_delta",
                "delta": label_raw,
                "source": "raw"
            }
        }));
    };

    set_mode(ThinkingDisplay::Summary);
    let baseline = writer.stdout().len();
    push_delta_pair("SUM-A", "RAW-A");
    let after_summary = writer.stdout();
    let summary_segment = &after_summary[baseline..];
    assert!(
        summary_segment.contains("SUM-A"),
        "summary 模式应输出 summary delta：{:?}",
        summary_segment
    );
    assert!(
        !summary_segment.contains("RAW-A"),
        "summary 模式应隐藏 raw delta：{:?}",
        summary_segment
    );
    assert_eq!(
        summary_segment.matches("[thinking]").count(),
        1,
        "summary 模式 `[thinking]` 前缀只应出现一次：{:?}",
        summary_segment
    );

    set_mode(ThinkingDisplay::Minimal);
    let baseline = writer.stdout().len();
    push_delta_pair("SUM-B", "RAW-B");
    let minimal_segment = writer.stdout()[baseline..].to_string();
    assert!(
        minimal_segment.contains("[thinking] ..."),
        "minimal 模式应输出占位：{:?}",
        minimal_segment
    );
    assert!(
        !minimal_segment.contains("SUM-B") && !minimal_segment.contains("RAW-B"),
        "minimal 模式不应输出任何 delta 正文：{:?}",
        minimal_segment
    );

    set_mode(ThinkingDisplay::Full);
    let baseline = writer.stdout().len();
    push_delta_pair("SUM-C", "RAW-C");
    let full_segment = writer.stdout()[baseline..].to_string();
    assert!(
        full_segment.contains("SUM-C") && full_segment.contains("RAW-C"),
        "full 模式应同时输出 summary 与 raw：{:?}",
        full_segment
    );

    set_mode(ThinkingDisplay::Summary);
    let cycle: Vec<ThinkingDisplay> = (0..3)
        .scan(ThinkingDisplay::Summary, |state, _| {
            *state = state.next_cycle();
            Some(*state)
        })
        .collect();
    assert_eq!(
        cycle,
        vec![
            ThinkingDisplay::Full,
            ThinkingDisplay::Minimal,
            ThinkingDisplay::Summary
        ],
        "toggle 循环顺序应为 summary -> full -> minimal -> summary"
    );
}

#[test]
fn thinking_display_can_flip_at_runtime() {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicU8::new(ThinkingDisplay::Summary.as_u8()));
    let r = CliTurnRenderer::with_writer(
        md,
        flag.clone(),
        None,
        writer.clone() as Arc<dyn CliWriter>,
        false,
        ToolCliVerbosity::Full,
    );
    // summary：raw 不可见
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "secret", "source": "raw"}
    }));
    assert_eq!(r.thinking_display(), ThinkingDisplay::Summary);
    flag.store(ThinkingDisplay::Full.as_u8(), Ordering::Release);
    assert_eq!(r.thinking_display(), ThinkingDisplay::Full);
    // 切到 full：后续 raw delta 应开始可见。
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "after", "source": "raw"}
    }));
    assert!(
        writer.stdout().contains("after"),
        "切换到展开后应输出 delta: {:?}",
        writer.stdout()
    );
}

#[test]
fn session_bound_renderer_renders_matching_session_events() {
    let (renderer, writer) = make_session_bound_renderer(ThinkingDisplay::Summary, "s1");
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let ids = renderer.register(&*bus);
    bus.emit_sync(
        wire::WIRE_MESSAGE_UPDATE,
        EventContext::new(
            wire::WIRE_MESSAGE_UPDATE,
            json!({
                "assistantMessageEvent": {"kind": "content_delta", "delta": "hello"}
            }),
        )
        .with_session_id("s1"),
    )
    .unwrap();
    renderer.flush_markdown();
    CliTurnRenderer::unregister(&*bus, &ids);
    assert!(writer.stdout().contains("hello"));
}

#[test]
fn session_bound_renderer_drops_other_session_events() {
    let (renderer, writer) = make_session_bound_renderer(ThinkingDisplay::Summary, "s1");
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let ids = renderer.register(&*bus);
    bus.emit_sync(
        wire::WIRE_MESSAGE_UPDATE,
        EventContext::new(
            wire::WIRE_MESSAGE_UPDATE,
            json!({
                "assistantMessageEvent": {"kind": "content_delta", "delta": "blocked"}
            }),
        )
        .with_session_id("s2"),
    )
    .unwrap();
    renderer.flush_markdown();
    CliTurnRenderer::unregister(&*bus, &ids);
    assert!(
        !writer.stdout().contains("blocked"),
        "他 session 事件不应被渲染"
    );
}

#[test]
fn session_bound_renderer_allows_global_events_without_session_id() {
    let (renderer, writer) = make_session_bound_renderer(ThinkingDisplay::Summary, "s1");
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let ids = renderer.register(&*bus);
    bus.emit_sync(
        wire::WIRE_LLM_NOTICE,
        EventContext::new(
            wire::WIRE_LLM_NOTICE,
            json!({
                "message": "global notice"
            }),
        ),
    )
    .unwrap();
    CliTurnRenderer::unregister(&*bus, &ids);
    assert!(writer.stderr().contains("global notice"));
}

#[test]
fn tool_cli_verbosity_off_hides_start_and_end_lines() {
    let (r, w) = make_renderer_with_tool_verbosity(ThinkingDisplay::Summary, ToolCliVerbosity::Off);
    r.on_tool_start(&json!({
        "toolCallId": "c-off",
        "toolName": "read",
        "args": {"path": "a.rs"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c-off",
        "toolName": "read",
        "result": {"lines": 1},
        "isError": false,
    }));
    assert!(
        w.stderr().is_empty(),
        "off 档位不应打印任何 [tool] 行: {:?}",
        w.stderr()
    );
}

#[test]
fn tool_cli_verbosity_brief_prints_end_without_start_and_extra_lines() {
    let (r, w) =
        make_renderer_with_tool_verbosity(ThinkingDisplay::Summary, ToolCliVerbosity::Brief);
    r.on_tool_start(&json!({
        "toolCallId": "c-brief",
        "toolName": "bash",
        "args": {"command": "echo hi"},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c-brief",
        "toolName": "bash",
        "result": {"error":"failed", "stderr":"line1\nline2"},
        "isError": true,
    }));
    let err = w.stderr();
    assert!(
        !err.contains("command=echo hi"),
        "brief 档位不应打印 start 摘要: {:?}",
        err
    );
    assert!(
        err.contains("[tool] bash"),
        "brief 应打印 end 行: {:?}",
        err
    );
    assert!(
        !err.contains("line1"),
        "brief 档位不应展开失败 stderr 额外行: {:?}",
        err
    );
}

#[test]
fn streaming_then_start_both_shown() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_call_streaming(&json!({
        "toolCallId": "c-stream",
        "toolName": "write",
        "argsPreview": {"path": "/tmp/demo.txt"},
    }));
    r.on_tool_start(&json!({
        "toolCallId": "c-stream",
        "toolName": "write",
        "args": {"path": "/tmp/demo.txt", "content": "hello", "overwrite": false},
    }));
    r.on_tool_end(&json!({
        "toolCallId": "c-stream",
        "toolName": "write",
        "result": {"bytes": 5},
        "display": {"kind": "file", "file": "/tmp/demo.txt"},
        "isError": false,
    }));
    let err = w.stderr();
    let receive_idx = err
        .find("receiving args...")
        .expect("应先显示 receiving args 行");
    let start_idx = err
        .find("path=/tmp/demo.txt (overwrite)")
        .expect("应保留原有 start 行");
    let end_idx = err.find("✓").expect("应显示 end 行");
    assert!(
        receive_idx < start_idx && start_idx < end_idx,
        "三行顺序应为 receiving -> start -> end: {:?}",
        err
    );
    assert!(
        err.contains("path=/tmp/demo.txt  receiving args..."),
        "streaming 行应带 path preview: {:?}",
        err
    );
    assert!(
        err.contains("path=/tmp/demo.txt (overwrite)"),
        "原有 start 行不应变化: {:?}",
        err
    );
    assert!(
        err.contains("ms") || err.contains("s)"),
        "end 行仍应展示 elapsed: {:?}",
        err
    );
}

#[test]
fn streaming_suppressed_when_not_full() {
    let (brief, brief_writer) =
        make_renderer_with_tool_verbosity(ThinkingDisplay::Summary, ToolCliVerbosity::Brief);
    brief.on_tool_call_streaming(&json!({
        "toolCallId": "c-brief",
        "toolName": "write",
        "argsPreview": {"path": "a.txt"},
    }));
    assert!(
        brief_writer.stderr().is_empty(),
        "brief 档位不应输出 streaming 行: {:?}",
        brief_writer.stderr()
    );

    let (off, off_writer) =
        make_renderer_with_tool_verbosity(ThinkingDisplay::Summary, ToolCliVerbosity::Off);
    off.on_tool_call_streaming(&json!({
        "toolCallId": "c-off",
        "toolName": "write",
        "argsPreview": {"path": "a.txt"},
    }));
    assert!(
        off_writer.stderr().is_empty(),
        "off 档位不应输出 streaming 行: {:?}",
        off_writer.stderr()
    );
}

#[test]
fn streaming_without_path_falls_back_cleanly() {
    let (r, w) = make_renderer(ThinkingDisplay::Summary);
    r.on_tool_call_streaming(&json!({
        "toolCallId": "c-nopath",
        "toolName": "write",
        "argsPreview": null,
    }));
    let err = w.stderr();
    assert!(
        err.contains("receiving args..."),
        "应显示稳定文案: {:?}",
        err
    );
    assert!(!err.contains("null"), "不应泄露 null 字样: {:?}", err);
    assert!(
        !err.contains("path="),
        "无 path 时不应输出脏 path=: {:?}",
        err
    );
}
