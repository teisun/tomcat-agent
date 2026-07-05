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
        "ask_question" => format!(
            "questions={}",
            args.get("questions")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0)
        ),
        "create_plan" => key_value_summary(args, "goal")
            .or_else(|| key_value_summary(args, "path"))
            .unwrap_or_else(|| "goal=plan".to_string()),
        "update_plan" => key_value_summary(args, "plan_id")
            .or_else(|| key_value_summary(args, "planId"))
            .or_else(|| count_summary(args, "todos", "todos"))
            .unwrap_or_else(|| "plan=update".to_string()),
        "todos" => count_summary(args, "todos", "todos").unwrap_or_else(|| "todos=0".to_string()),
        "web_search" => key_value_summary(args, "query")
            .or_else(|| key_value_summary(args, "search_term"))
            .unwrap_or_else(|| "query=search".to_string()),
        "search_workspace" | "search_files" | "grep" => key_value_summary(args, "query")
            .or_else(|| key_value_summary(args, "pattern"))
            .or_else(|| key_value_summary(args, "path"))
            .unwrap_or_else(|| "query=search".to_string()),
        "web_fetch" => key_value_summary(args, "url").unwrap_or_else(|| "url=fetch".to_string()),
        "list_dir" => key_value_summary(args, "path").unwrap_or_else(|| "path=.".to_string()),
        "config_get" | "config_set" => {
            key_value_summary(args, "key").unwrap_or_else(|| "key=config".to_string())
        }
        _ => summarize_known_key(args)
            .or_else(|| object_field_summary(args))
            .unwrap_or_else(|| args.to_string().replace('\n', " ")),
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

fn key_value_summary(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("{key}={value}"))
}

fn count_summary(args: &Value, key: &str, label: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| format!("{label}={}", items.len()))
}

fn summarize_known_key(args: &Value) -> Option<String> {
    for key in [
        "path", "url", "goal", "query", "pattern", "key", "plan_id", "planId", "command",
    ] {
        if let Some(summary) = key_value_summary(args, key) {
            return Some(summary);
        }
    }
    count_summary(args, "questions", "questions").or_else(|| count_summary(args, "todos", "todos"))
}

fn object_field_summary(args: &Value) -> Option<String> {
    let object = args.as_object()?;
    if object.is_empty() {
        return None;
    }
    let parts = object
        .iter()
        .map(|(key, value)| format!("{key}={}", scalar_value_summary(value)))
        .collect::<Vec<_>>();
    Some(parts.join(" "))
}

fn scalar_value_summary(value: &Value) -> String {
    match value {
        Value::String(text) => text.lines().map(str::trim).collect::<Vec<_>>().join(" "),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => "null".to_string(),
        _ => value.to_string().replace('\n', " "),
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
