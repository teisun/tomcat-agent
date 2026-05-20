//! `CliTurnRenderer` 单测：以 `Sink` 替换 stdout/stderr，覆盖
//! - thinking 折叠 vs 展开（`/thinking` 行为）；
//! - thinking → content 切换时的换行；
//! - tool_execution_start/end 排版与失败摘要；
//! - 工具单行摘要的内置类型（read/bash）与回退（未知工具）；
//! - kind 缺省走 content_delta 的向后兼容性。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;

use super::{
    error_extra_lines, one_line_summary, result_summary, result_summary_for_tool, CliTurnRenderer,
    CliWriter,
};
use crate::api::render::MarkdownRenderer;
use crate::infra::config::ToolCliVerbosity;
use crate::infra::events::ToolDisplay;

#[derive(Default)]
struct CapturedWriter {
    stdout: Mutex<String>,
    stderr: Mutex<String>,
}

impl CapturedWriter {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
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
}

fn make_renderer(show_thinking: bool) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    make_renderer_with_tool_verbosity(show_thinking, ToolCliVerbosity::Full)
}

fn make_renderer_with_tool_verbosity(
    show_thinking: bool,
    tool_cli_verbosity: ToolCliVerbosity,
) -> (Arc<CliTurnRenderer>, Arc<CapturedWriter>) {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicBool::new(show_thinking));
    let r = CliTurnRenderer::with_writer(
        md,
        flag,
        writer.clone() as Arc<dyn CliWriter>,
        false,
        tool_cli_verbosity,
    );
    (r, writer)
}

#[test]
fn folded_thinking_only_emits_one_line() {
    let (r, w) = make_renderer(false);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step a"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step b"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step c"}
    }));
    let s = w.stdout();
    let count = s.matches("[thinking]").count();
    assert_eq!(count, 1, "折叠模式下整回合只应打一次 [thinking]: {:?}", s);
    assert!(
        s.contains("[thinking] …"),
        "折叠模式应使用单行 …省略文案: {:?}",
        s
    );
    assert!(
        !s.contains("step a"),
        "折叠模式不应输出 thinking delta 文本: {:?}",
        s
    );
}

#[test]
fn expanded_thinking_streams_each_delta_with_dim_color() {
    let (r, w) = make_renderer(true);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "step a"}
    }));
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": " step b"}
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
fn content_delta_after_thinking_inserts_newline() {
    let (r, w) = make_renderer(true);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "plan"}
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
    let (r, w) = make_renderer(false);
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
    let (r, w) = make_renderer(true);
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": ""}
    }));
    assert!(
        w.stdout().is_empty() && w.stderr().is_empty(),
        "空 delta 不应触发任何输出"
    );
}

#[test]
fn tool_start_emits_gray_summary_on_stderr() {
    let (r, w) = make_renderer(false);
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

#[test]
fn tool_end_success_uses_green_check_and_elapsed() {
    let (r, w) = make_renderer(false);
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
    let (r, w) = make_renderer(false);
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
    let (r, w) = make_renderer(false);
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
        "command=ls -la"
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
        "command=echo hi && pwd"
    );
    assert_eq!(
        one_line_summary(
            "bash",
            &json!({"command": "bash", "args": ["-lc", "\n\n  echo first\npwd"]})
        ),
        "command=echo first"
    );
    assert_eq!(
        one_line_summary("bash", &json!({"command": "bash", "args": ["-lc"]})),
        "command=bash -lc"
    );
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
    let home = dirs::home_dir().expect("HOME");
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
    let (r, w) = make_renderer(false);
    let home = dirs::home_dir().expect("HOME");
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
    let home = dirs::home_dir().expect("HOME");
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
fn update_plan_success_shows_absolute_plan_path() {
    let home = dirs::home_dir().expect("HOME");
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
        "message": "已设置 llm.default_model = gpt-5.2",
    });
    let summary = result_summary_for_tool(
        &json!(serde_json::to_string(&payload).unwrap()),
        Some(&ToolDisplay::Text {
            text: "已设置 llm.default_model = gpt-5.2".to_string(),
        }),
        false,
    );
    assert_eq!(summary, "已设置 llm.default_model = gpt-5.2");
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
    let (r, w) = make_renderer(false);
    let home = dirs::home_dir().expect("HOME");
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

#[test]
fn show_thinking_flag_can_flip_at_runtime() {
    let writer = CapturedWriter::new();
    let md = Arc::new(Mutex::new(MarkdownRenderer::new()));
    let flag = Arc::new(AtomicBool::new(false));
    let r = CliTurnRenderer::with_writer(
        md,
        flag.clone(),
        writer.clone() as Arc<dyn CliWriter>,
        false,
        ToolCliVerbosity::Full,
    );
    // 折叠：只有省略号
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "secret"}
    }));
    assert!(!r.is_show_thinking());
    flag.store(true, Ordering::Release);
    assert!(r.is_show_thinking());
    // 展开：在同一回合切换后可继续打 delta（注意 prefix 已被首次折叠时忽略，
    // 因此再次切到展开必须保证 prefix 重新出现以让用户感知到切换；当前实现用
    // `thinking_prefix_printed` 状态位独立于折叠/展开，因此首次打开后会沿用，
    // 这里至少保证 delta 能输出）。
    r.on_message_update(&json!({
        "assistantMessageEvent": {"kind": "thinking_delta", "delta": "after"}
    }));
    assert!(
        writer.stdout().contains("after"),
        "切换到展开后应输出 delta: {:?}",
        writer.stdout()
    );
}

#[test]
fn tool_cli_verbosity_off_hides_start_and_end_lines() {
    let (r, w) = make_renderer_with_tool_verbosity(false, ToolCliVerbosity::Off);
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
    let (r, w) = make_renderer_with_tool_verbosity(false, ToolCliVerbosity::Brief);
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
