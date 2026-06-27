//! 工具调用单行摘要（供 title 生成与 CLI 共用）。

use serde_json::Value;

const DEFAULT_MAX_CHARS: usize = 120;

/// 工具调用单行摘要（`[tool] {name}  {summary}` 中间那段）。
pub fn one_line_summary(tool_name: &str, args: &Value) -> String {
    let summary = match tool_name {
        "read" | "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut out = format!("path={path}");
            if let Some(off) = args.get("offset").and_then(|v| v.as_i64()) {
                out.push_str(&format!(" offset={off}"));
            }
            if let Some(lim) = args.get("limit").and_then(|v| v.as_i64()) {
                out.push_str(&format!(" limit={lim}"));
            }
            out
        }
        "write" | "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("path={path} (overwrite)")
        }
        "edit" | "edit_file" | "str_replace" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("path={path} (replace)")
        }
        "bash" | "shell" | "execute_command" => {
            let mut out = format!("command={}", shell_command_preview(args));
            if args
                .get("run_in_background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                out.push_str(" run_in_background=true");
            }
            out
        }
        _ => args.to_string().replace('\n', " "),
    };
    if matches!(tool_name, "bash" | "shell" | "execute_command") {
        summary
    } else {
        truncate_chars(&summary, DEFAULT_MAX_CHARS)
    }
}

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

fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}\u{2026}")
}
