//! # Agent Loop 工具执行子模块
//!
//! 职责单一：把 `ToolCallInfo` 解析为具体 primitive 调用，返回 `(content, is_error)`。
//! 7 分支（read_file / write_file / edit_file / execute_bash / list_dir /
//! 未知工具 / 参数解析失败）逐字搬自 `run.rs`，**不依赖 `AgentLoop`**——
//! 只接 `&Arc<dyn PrimitiveExecutor>` + `&ToolCallInfo`，便于独立单测。
//!
//! ## 语义约定
//!
//! - `write_file` / `edit_file` 的"应用层拒绝"（`written=false` / `applied=false`）
//!   **不是错误**：`is_error` 保持 `false`，返回文案以"写入被拒绝" / "编辑被拒绝"
//!   开头，与原语义严格一致。
//! - 未知工具名、参数 JSON 解析失败 → `is_error = true`。
//! - `execute_bash` 的失败通过 `PrimitiveExecutor::execute_bash` 的 `Result::Err`
//!   传出；`exit_code != 0` 本身**不**置 `is_error`（与原行为一致，保留给下游
//!   LLM 自行判断）。
//!
//! ## `AGENT_PLUGIN_ID`
//!
//! Primitive 层需要一个 `plugin_id` 标签做 hostcall 审计；Agent Loop 直接执行
//! 的工具调用（与"插件上下文中触发的工具调用"相对）统一使用 `"__agent__"`
//! 字面值。本模块顶部常量化后，未来若需重命名只改一处，避免散落。

use std::sync::Arc;

use crate::core::primitives::{EditOperation, EditOperationType, PrimitiveExecutor};

use super::types::ToolCallInfo;

/// Agent Loop 直接触发的工具调用使用的固定 `plugin_id` 标签。
/// 与"插件上下文中触发的工具调用"区分，便于 hostcall 审计层分桶。
pub(super) const AGENT_PLUGIN_ID: &str = "__agent__";

/// 执行单次 tool call 并返回 `(输出文本, is_error)`。
///
/// 自由函数设计（**不**接收 `&AgentLoop`）：调用方持有 `Arc<dyn PrimitiveExecutor>`
/// 即可直接调用；test 只需 mock `PrimitiveExecutor`，不必 mock 整个 AgentLoop。
pub(super) async fn execute_tool(
    primitive: &Arc<dyn PrimitiveExecutor>,
    tc: &ToolCallInfo,
) -> (String, bool) {
    let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
        Ok(v) => v,
        Err(e) => return (format!("参数解析失败: {}", e), true),
    };

    let out = match tc.name.as_str() {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("");
            primitive
                .read_file(path, AGENT_PLUGIN_ID)
                .await
                .map_err(|e| e.to_string())
        }
        "write_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            primitive
                .write_file(path, content, overwrite, AGENT_PLUGIN_ID)
                .await
                .map(|r| {
                    if r.written {
                        format!("已写入: {}", r.path)
                    } else {
                        format!("写入被拒绝: {}", r.path)
                    }
                })
                .map_err(|e| e.to_string())
        }
        "edit_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let old_content = args["old_content"].as_str().unwrap_or("");
            let new_content = args["new_content"].as_str().unwrap_or("");
            let edits = vec![EditOperation {
                operation_type: EditOperationType::Replace,
                start_line: None,
                end_line: None,
                old_content: Some(old_content.to_string()),
                new_content: new_content.to_string(),
            }];
            primitive
                .edit_file(path, edits, AGENT_PLUGIN_ID)
                .await
                .map(|r| {
                    if r.applied {
                        format!("已编辑: {}", r.path)
                    } else {
                        format!("编辑被拒绝: {}", r.path)
                    }
                })
                .map_err(|e| e.to_string())
        }
        "execute_bash" => {
            let command = args["command"].as_str().unwrap_or("");
            let cwd = args["cwd"].as_str();
            let argv_store: Option<Vec<String>> =
                args.get("args").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                });
            let argv_ref = argv_store.as_deref();
            primitive
                .execute_bash(command, cwd, AGENT_PLUGIN_ID, argv_ref)
                .await
                .map(|r| {
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
                })
                .map_err(|e| e.to_string())
        }
        "list_dir" => {
            let path = args["path"].as_str().unwrap_or("");
            primitive
                .list_dir(path, AGENT_PLUGIN_ID)
                .await
                .map(|entries| {
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
                })
                .map_err(|e| e.to_string())
        }
        other => Err(format!("未知工具: {}", other)),
    };

    match out {
        Ok(s) => (s, false),
        Err(s) => (s, true),
    }
}
