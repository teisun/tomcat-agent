//! # Agent Loop 工具执行子模块
//!
//! 职责单一：把 `ToolCallInfo` 解析为具体 primitive 调用，返回 `(content, is_error)`。
//! 7 分支（read / write_file / edit_file / execute_bash / list_dir /
//! 未知工具 / 参数解析失败）逐字搬自 `run.rs`，**不依赖 `AgentLoop`**——
//!
//! ## 命名切换（PR-RA）
//!
//! 工具名 `read_file` 已弃用，改为短名 `read`（与 pi-mono / cc-fork 短名生态对齐）。
//! 运行时**无别名 / 无重定向**：调用 `read_file` 走 `unknown` 分支，等同拼错工具名。
//! transcript 中的旧 `read_file` 调用由 `session::manager::context` 在加载时
//! `tracing::warn!`，但**不**重写，老对话只是历史记录。
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

use crate::core::tools::primitive::{
    EditOperation, EditOperationType, PrimitiveExecutor, SearchFilesArgs,
};
use crate::infra::error::AppError;

use super::config_backend::SharedConfigBackend;
use super::types::ToolCallInfo;

/// Agent Loop 直接触发的工具调用使用的固定 `plugin_id` 标签。
/// 与"插件上下文中触发的工具调用"区分，便于 hostcall 审计层分桶。
pub(super) const AGENT_PLUGIN_ID: &str = "__agent__";

/// 执行单次 tool call 并返回 `(输出文本, is_error)`。
///
/// 自由函数设计（**不**接收 `&AgentLoop`）：调用方持有 `Arc<dyn PrimitiveExecutor>`
/// 即可直接调用；test 只需 mock `PrimitiveExecutor`，不必 mock 整个 AgentLoop。
///
/// `config_backend` 为可选注入：未注入时 `config_get` / `config_set` 命中后返回
/// 错误文案（参考 [`super::config_backend::ConfigBackend`] 的契约）。
pub(super) async fn execute_tool(
    primitive: &Arc<dyn PrimitiveExecutor>,
    config_backend: &Option<SharedConfigBackend>,
    read_file_state: Option<&Arc<crate::core::tools::read_state::ReadFileState>>,
    tc: &ToolCallInfo,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
        Ok(v) => v,
        Err(e) => return (format!("参数解析失败: {}", e), true, Vec::new()),
    };

    // PR-RJ T3-c：read 命中 image / pdf 时，要把 InputImage / InputFile part
    // 注入「**下一条** user 消息」（OpenAI 的 `role: "tool"` 不接受非 text part），
    // 由 [`crate::core::agent_loop::tool_dispatcher`] 在拿到 follow_up_parts 后
    // 紧跟着 push 一条 `ChatMessage::user_with_parts(parts)`。其它工具一律 `vec![]`。
    let mut follow_up_parts: Vec<crate::core::llm::ChatMessageContentPart> = Vec::new();

    let out = match tc.name.as_str() {
        "read" => {
            let path = args["path"].as_str().unwrap_or("");
            let offset = parse_optional_u64(&args, "offset");
            let limit = parse_optional_u64(&args, "limit");
            // PR-RB §2.6 horizontal gate：在主体之前对 offset/limit 做边界兜底。
            if let Err(msg) = validate_read_bounds(offset, limit) {
                return (msg, true, Vec::new());
            }
            // PR-RF §3.1：line_numbers 默认 true（cat -n 风格行号）；LLM 可显式传 false
            // 以便把内容 pipe 给 diff 工具等不需要行号的场景。
            let line_numbers = args
                .get("line_numbers")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            // PR-RM §4.3：hashline 默认 false；为 true 时输出 `N#AB:line`，**优先于** cat-n。
            let hashline = args
                .get("hashline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // PR-RF §3.2 dedup：注入了 ReadFileState 时，在调用 primitive 之前
            // 用 metadata（mtime + size）做廉价短路。规范化 path 失败时直接降级
            // 给 primitive 自己处理（路径不存在会在 gate 后报权限/IO 错）。
            let resolved =
                crate::infra::platform::normalize_path(path).unwrap_or_else(|_| path.into());
            let stub_short_circuit = read_file_state.and_then(|state| {
                let stamp = state.get(&resolved)?;
                let meta = std::fs::metadata(&resolved).ok()?;
                if meta.is_dir() {
                    return None;
                }
                let mtime = crate::core::tools::read_state::metadata_mtime_ms(&meta);
                if stamp.matches_request(mtime, meta.len(), offset, limit) {
                    Some(crate::core::tools::read_state::FILE_UNCHANGED_STUB.to_string())
                } else {
                    None
                }
            });
            if let Some(stub) = stub_short_circuit {
                return (stub, false, Vec::new());
            }

            // PR-RJ T3-a：primitive.read 现在返回 [`ReadResult`] discriminated union
            // （Text / Image / Pdf / FileUnchanged）；本期只接 Text 路径——image / pdf
            // 走 [`ReadResult::to_tool_text`] 输出占位句，真正的 inline part 注入由
            // T3-c 在 OpenAI 路径上完成。
            let exec_result = primitive
                .read(path, offset, limit, line_numbers, hashline, AGENT_PLUGIN_ID)
                .await;

            // 成功 → 更新 stamp（dedup 下一次同窗口短路 + 给 staleness/edit 兜底）。
            // 失败 → **不**写 stamp：避免给「读不到但模型还会重试」的路径留旧指纹。
            // 注意：image / pdf 也同样落 stamp，hash 使用 base64 字符串而非占位句，
            // 避免「同图重读返回不同占位句」破坏 dedup 命中。
            if let (Ok(result), Some(state)) = (exec_result.as_ref(), read_file_state) {
                if let Ok(meta) = std::fs::metadata(&resolved) {
                    if !meta.is_dir() {
                        // PR-RJ T3-b：image/pdf 的字节由 LLM helper 自己 read + base64
                        // （PR-RJ-0），primitive 不持有字节；这里 hash 用 path 作为
                        // 稳定代理（mtime+size 已经做了主要 staleness 判定）。
                        let path_bytes: Vec<u8>;
                        let hash_input: &[u8] = match result {
                            crate::core::tools::primitive::ReadResult::Text(t) => {
                                t.content.as_bytes()
                            }
                            crate::core::tools::primitive::ReadResult::Image(b)
                            | crate::core::tools::primitive::ReadResult::Pdf(b) => {
                                path_bytes = b.path.as_os_str().as_encoded_bytes().to_vec();
                                &path_bytes[..]
                            }
                            crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => &[],
                        };
                        let stamp = crate::core::tools::read_state::ReadStamp {
                            mtime_ms: crate::core::tools::read_state::metadata_mtime_ms(&meta),
                            size: meta.len(),
                            content_hash: crate::core::tools::read_state::hash_content(hash_input),
                            offset,
                            limit,
                            is_partial_view: offset.is_some() || limit.is_some(),
                        };
                        state.put(resolved.clone(), stamp);
                    }
                }
            }
            // PR-RJ T3-c：把 image/pdf variant 转成 ChatMessageContentPart 推到
            // follow_up_parts。helper 内部 metadata 二次校验 + 读盘 + base64
            // （详见 PR-RJ-0 重构）。helper 失败时**不**整体 fail——退化成
            // tool 文本占位句，记一条 warn 让模型也能感知；避免 read 工具因为
            // 后续 wire 准备失败而整把丢回错。
            match exec_result {
                Ok(result) => {
                    match &result {
                        crate::core::tools::primitive::ReadResult::Image(b) => {
                            match crate::core::llm::ChatMessageContentPart::image_b64(
                                b.mime.clone(),
                                &b.path,
                            ) {
                                Ok(part) => follow_up_parts.push(part),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    path = %b.path.display(),
                                    "read T3-c: failed to build InputImage part; falling back to text-only tool message"
                                ),
                            }
                        }
                        crate::core::tools::primitive::ReadResult::Pdf(b) => {
                            match crate::core::llm::ChatMessageContentPart::file_b64(
                                b.filename.clone(),
                                b.mime.clone(),
                                &b.path,
                            ) {
                                Ok(part) => follow_up_parts.push(part),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    path = %b.path.display(),
                                    "read T3-c: failed to build InputFile part; falling back to text-only tool message"
                                ),
                            }
                        }
                        crate::core::tools::primitive::ReadResult::Text(_)
                        | crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => {}
                    }
                    Ok(result.to_tool_text())
                }
                Err(e) => Err(e.to_string()),
            }
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
        "search_files" => {
            let search_args: SearchFilesArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(e) => {
                    return (
                        format!("search_files 参数解析失败: {}", e),
                        true,
                        Vec::new(),
                    )
                }
            };
            primitive
                .search_files(search_args, AGENT_PLUGIN_ID)
                .await
                .and_then(|output| serde_json::to_string_pretty(&output).map_err(AppError::from))
                .map_err(|e| e.to_string())
        }
        "config_get" => {
            let Some(backend) = config_backend.as_ref() else {
                return (
                    "config 工具未启用：当前会话不允许通过 LLM 读改配置".to_string(),
                    true,
                    Vec::new(),
                );
            };
            let key = args["key"].as_str().unwrap_or("");
            backend
                .config_get(key)
                .await
                .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()))
                .map_err(|e| e.to_string())
        }
        "config_set" => {
            let Some(backend) = config_backend.as_ref() else {
                return (
                    "config 工具未启用：当前会话不允许通过 LLM 读改配置".to_string(),
                    true,
                    Vec::new(),
                );
            };
            let key = args["key"].as_str().unwrap_or("");
            let value = args["value"].as_str().unwrap_or("");
            backend
                .config_set(key, value)
                .await
                .map(|(applied, msg)| {
                    serde_json::json!({
                        "applied": applied,
                        "message": msg,
                    })
                    .to_string()
                })
                .map_err(|e| e.to_string())
        }
        other => Err(format!("未知工具: {}", other)),
    };

    match out {
        Ok(s) => (s, false, follow_up_parts),
        Err(s) => (s, true, Vec::new()),
    }
}

/// PR-RB（§2.6）解析 `read` 工具的可选整数入参（`offset` / `limit`）。
///
/// 接受 JSON `null` / 缺失 → `None`；接受任何非负整数（`u64`）。
/// **不**在此做范围校验——交给 [`validate_read_bounds`] 做统一边界兜底。
fn parse_optional_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    let v = args.get(key)?;
    if v.is_null() {
        return None;
    }
    v.as_u64()
}

/// PR-RB（§2.6）`read` 入参 horizontal gate：边界违反返回结构化错误，使模型可自我修正。
///
/// 边界（与 `openspec/specs/architecture/tools/read.md` §2.1 / §2.6 一致）：
/// - `offset` 若提供则必须 ≥ 1；
/// - `limit` 若提供则必须在 `[1, 10000]`（cc-fork 同档）；
/// - 入参不是整数（`as_u64` 解析失败）由调用方先用 [`parse_optional_u64`]
///   过滤为 `None`，此处不重复校验。
fn validate_read_bounds(offset: Option<u64>, limit: Option<u64>) -> Result<(), String> {
    if let Some(o) = offset {
        if o < 1 {
            return Err(
                "read.offset must be >= 1 (1-based line number; pass `1` to start from the first line)"
                    .to_string(),
            );
        }
    }
    if let Some(l) = limit {
        if !(1..=10_000).contains(&l) {
            return Err(format!(
                "read.limit must be in [1, 10000] (got {}); split large reads with multiple offset+limit calls",
                l
            ));
        }
    }
    Ok(())
}
