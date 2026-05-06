//! # Agent Loop 工具执行子模块
//!
//! 职责单一：把 `ToolCallInfo` 解析为具体 primitive 调用，返回 `(content, is_error)`。
//! 7 分支（read / write / edit / execute_bash / list_dir /
//! 未知工具 / 参数解析失败）逐字搬自 `run.rs`，**不依赖 `AgentLoop`**——
//!
//! ## 命名切换（PR-RA / T2-P0-016 / T2-P0-017 PR-命名）
//!
//! 工具名 `read_file` / `write_file` / `edit_file` 已弃用，改为短名
//! `read` / `write` / `edit`（与 pi-mono / cc-fork 短名生态对齐）。
//! 运行时**无别名 / 无重定向**：调用旧名走 `unknown` 分支，等同拼错工具名。
//! transcript 中的旧名调用由 `session::manager::context` 在加载时统一
//! `tracing::warn!`，但**不**重写，老对话只是历史记录。
//! 只接 `&Arc<dyn PrimitiveExecutor>` + `&ToolCallInfo`，便于独立单测。
//!
//! ## 语义约定
//!
//! - **`edit`** 的"应用层拒绝"（`applied=false`）**不是错误**：`is_error` 保持
//!   `false`，返回文案以"编辑被拒绝"开头，与原语义严格一致。
//! - **`write`**（T2-P0-016 PR-C）的 **策略拒绝**（`Exists` / `NoPriorRead` /
//!   `Stale`）**是错误**：`is_error: true`，与 `Stale` 在 `edit` 中的处理一致，
//!   避免模型把策略拒绝当成功 tool 结果。primitive 内 `written=false` 已作为
//!   `AppError::Tool` 早退；本编排层不再产生 `written=false` 文本。
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
    read_file_state: Option<&Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
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
                let mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
                if stamp.matches_request(mtime, meta.len(), offset, limit) {
                    Some(crate::core::tools::pipeline::read_state::FILE_UNCHANGED_STUB.to_string())
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
                        let stamp = crate::core::tools::pipeline::read_state::ReadStamp {
                            mtime_ms: crate::core::tools::pipeline::read_state::metadata_mtime_ms(
                                &meta,
                            ),
                            size: meta.len(),
                            content_hash: crate::core::tools::pipeline::read_state::hash_content(
                                hash_input,
                            ),
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
        // T2-P0-016 PR-C：write 编排层硬门禁
        //   - `resolved` 由 `normalize_path` 派生（与 read 分支 L98 / L154 同形 key）；
        //   - `exists && !overwrite` → `Exists`（`is_error: true`，与 `Stale` 一致）；
        //   - `exists && overwrite` → 走 `check_mutation_stamp` 强拒 NoPriorRead / Stale；
        //   - 任何成功写盘 → `state.invalidate(&resolved)`，避免下一轮 read 误命中
        //     `FILE_UNCHANGED` 撒谎（write.md §6.1 / §6.2）。
        //   - primitive 内 `write_file_impl` 还有一道 `exists && !overwrite` 二道防线，
        //     防止 trait 直调（dispatcher / extension）绕过本编排（write.md §3.4.2）。
        "write" => {
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            let resolved = crate::infra::platform::normalize_path(path)
                .unwrap_or_else(|_| std::path::PathBuf::from(path));
            let exists = resolved.exists();
            if exists && !overwrite {
                return (
                    format!(
                        "Exists: 路径 `{}` 已存在；如需替换请先 `read` 该文件，然后再用 `overwrite=true` 调用 `write`",
                        path
                    ),
                    true,
                    Vec::new(),
                );
            }
            if exists && overwrite {
                if let Some(state) = read_file_state {
                    if let Err(msg) = check_mutation_stamp(state, path, "write") {
                        return (msg, true, Vec::new());
                    }
                }
            }
            let result = primitive
                .write_file(path, content, overwrite, AGENT_PLUGIN_ID)
                .await;
            match result {
                Ok(r) => {
                    if let Some(state) = read_file_state {
                        // 写后失效：与 read 同形 key（不再额外 canonicalize），无 key 时是 no-op。
                        state.invalidate(&resolved);
                    }
                    if r.written {
                        // PR-G 回执：created/updated + 字节数 + 可选 diff 摘要。
                        let verb = if r.diff_hint.is_some() {
                            "已覆盖"
                        } else {
                            "已写入"
                        };
                        let mut msg = format!("{}: {} ({} bytes)", verb, r.path, r.bytes_written);
                        if let Some(diff) = r.diff_hint.as_ref() {
                            if !diff.is_empty() {
                                msg.push_str("\n--- diff (truncated)\n");
                                msg.push_str(diff);
                            }
                        }
                        Ok(msg)
                    } else {
                        // PR-C 之后 primitive 走 Err 早退，理论上不再出现 written=false；
                        // 保留这条文案兜底（dispatcher / extension 直调）。
                        Ok(format!("写入被拒绝: {}", r.path))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        }
        // T2-P0-017 Phase1（PR-命名 + PR-D）：
        //   - 短名 `edit`（旧 `edit_file` 走 unknown 分支；transcript warn 在 session/manager/context.rs）
        //   - oneOf 入参（A: 顶层 old/new；B: edits[]）；同时存在时 `edits` 优先
        //   - staleness：与 `read` 共用 `ReadFileState`，mtime+size 与 stamp 不一致 → `Stale`
        //   - 多段语义在 primitive (`write_edit::edit_file_impl`) 中对原文快照一次应用 + 重叠检测
        //   - `NoPriorRead`（无 stamp 时是否硬拒）与 T2-P0-016 write 同 PR 锁；本 Phase 不单边强拒
        "edit" => match parse_edit_args(&args) {
            Err(msg) => Err(msg),
            Ok((path, edits)) => {
                // PR-H：`.ipynb` 在 primitive 之前直接拒，避免读盘 / 占位 .bak。
                if crate::core::tools::pipeline::edit_normalize::is_unsupported_structured_file(
                    path,
                ) {
                    return (
                        format!(
                            "Notebook: `{}` 是 Jupyter 笔记本（.ipynb），edit 不支持；请使用专用 nbformat 工具或先把目标 cell 导出为 .py / .md 再 edit",
                            path
                        ),
                        true,
                        Vec::new(),
                    );
                }
                if let Some(state) = read_file_state {
                    if let Err(stale_msg) = check_mutation_stamp(state, path, "edit") {
                        return (stale_msg, true, Vec::new());
                    }
                }
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
        },
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
        "hashline_edit" => match parse_hashline_edit_args(&args) {
            Err(msg) => Err(msg),
            Ok((path, segments)) => {
                if let Some(state) = read_file_state {
                    if let Err(stale_msg) = check_mutation_stamp(state, path, "edit") {
                        return (stale_msg, true, Vec::new());
                    }
                }
                primitive
                    .hashline_edit(path, segments, AGENT_PLUGIN_ID)
                    .await
                    .map(|r| {
                        if r.applied {
                            format!("已 hashline 编辑: {}", r.path)
                        } else {
                            format!("hashline 编辑被拒绝: {}", r.path)
                        }
                    })
                    .map_err(|e| e.to_string())
            }
        },
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

/// T2-P0-017 PR-D：`edit` 工具入参解析（oneOf 形状 A / B）。
///
/// **形状 A**：`{ path, old_content, new_content, replace_all? }`
/// **形状 B**：`{ path, edits: [{ old_content, new_content, replace_all? }, ...] }`
///
/// 当同时存在 `edits` 与顶层 `old_content`/`new_content` 时 **`edits` 优先**
/// （与 [edit.md §4.2](../../../openspec/specs/architecture/tools/edit.md) 对齐）。
///
/// 解析后转换为 [`EditOperation`]（仅 `Replace`、无行号；行号 API 仅留给 dispatcher
/// extension 内部使用）。`replace_all` 通过 `new_content` 字段携带的 magic 前缀
/// 传递给 primitive 是不可行的——这里只做形状归一化，多段语义在
/// [`crate::core::tools::primitive::executor::write_edit::edit_file_impl`] 落地。
///
/// **决策（lock，详见计划文件 Phase1 决策 6）**：保留 `PrimitiveExecutor::edit_file`
/// trait 方法签名不动，避免牵动 dispatcher / 多个 mock。`replace_all` 信号通过
/// [`crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER`] 编码到段的
/// `old_content` 前缀，由 `write_edit::edit_file_impl` 在分段解析时识别并剥离。
fn parse_edit_args(args: &serde_json::Value) -> Result<(&str, Vec<EditOperation>), String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少必填字段 `path`".to_string())?;

    // Shape B：优先识别 edits 数组。
    if let Some(edits_v) = args.get("edits") {
        let arr = edits_v
            .as_array()
            .ok_or_else(|| "edit: `edits` 必须是数组".to_string())?;
        if arr.is_empty() {
            return Err("edit: `edits` 至少需要一条编辑段".to_string());
        }
        let mut ops = Vec::with_capacity(arr.len());
        for (i, seg) in arr.iter().enumerate() {
            let old = seg
                .get("old_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edit: edits[{}].old_content 缺失或非字符串", i))?;
            let new_c = seg
                .get("new_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edit: edits[{}].new_content 缺失或非字符串", i))?;
            let replace_all = seg
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            ops.push(make_edit_op(old, new_c, replace_all));
        }
        return Ok((path, ops));
    }

    // Shape A：顶层 old_content / new_content。
    let old = args
        .get("old_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少 `old_content`（或 `edits`）".to_string())?;
    let new_c = args
        .get("new_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少 `new_content`".to_string())?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok((path, vec![make_edit_op(old, new_c, replace_all)]))
}

fn make_edit_op(old: &str, new_c: &str, replace_all: bool) -> EditOperation {
    let encoded_old = if replace_all {
        format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER,
            old
        )
    } else {
        old.to_string()
    };
    EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: None,
        end_line: None,
        old_content: Some(encoded_old),
        new_content: new_c.to_string(),
    }
}

/// T2-P0-017 Phase3 / PR-M：`hashline_edit` 入参解析。
///
/// JSON 形状：
/// ```jsonc
/// {
///   "path": "src/foo.rs",
///   "edits": [
///     { "op": "replace", "pos": "42#Ab", "lines": "x\n" },
///     { "op": "replace", "pos": "55#Cd", "end": "57#Ef", "lines": "y\n" }
///   ]
/// }
/// ```
fn parse_hashline_edit_args(
    args: &serde_json::Value,
) -> Result<(&str, Vec<crate::core::tools::primitive::HashlineSegment>), String> {
    use crate::core::tools::primitive::{HashlineOp, HashlineSegment};
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "hashline_edit: 缺少必填字段 `path`".to_string())?;
    let edits_v = args
        .get("edits")
        .ok_or_else(|| "hashline_edit: 缺少必填字段 `edits`".to_string())?;
    let arr = edits_v
        .as_array()
        .ok_or_else(|| "hashline_edit: `edits` 必须是数组".to_string())?;
    if arr.is_empty() {
        return Err("hashline_edit: `edits` 至少需要一条段".to_string());
    }
    let mut segments = Vec::with_capacity(arr.len());
    for (i, seg) in arr.iter().enumerate() {
        let op_str = seg
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{}].op 缺失或非字符串", i))?;
        let op = match op_str {
            "replace" => HashlineOp::Replace,
            "insert" => HashlineOp::Insert,
            "delete" => HashlineOp::Delete,
            other => {
                return Err(format!(
                    "hashline_edit: edits[{}].op 必须是 replace|insert|delete，实际 `{}`",
                    i, other
                ))
            }
        };
        let pos = seg
            .get("pos")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{}].pos 缺失或非字符串", i))?;
        let (start_line, start_hash) =
            HashlineSegment::parse_anchor(pos, i, "pos").map_err(|e| e.to_string())?;
        let (end_line, end_hash) = match seg.get("end").and_then(|v| v.as_str()) {
            Some(end_s) => {
                HashlineSegment::parse_anchor(end_s, i, "end").map_err(|e| e.to_string())?
            }
            None => (start_line, start_hash.clone()),
        };
        let lines = seg
            .get("lines")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        segments.push(HashlineSegment {
            op,
            start_line,
            start_hash,
            end_line,
            end_hash,
            lines,
        });
    }
    Ok((path, segments))
}

/// T2-P0-016 PR-C / T2-P0-017 PR-D：`edit` / `write`（覆盖写）前共享的 staleness + NoPriorRead 兜底。
///
/// 与 read 同形态读 `ReadFileState`：用 [`crate::infra::platform::normalize_path`] 算出
/// `resolved`（**与 read primitive 内 `put_stamp` 的 key 一致** —— `tool_exec`
/// `read` 分支 L98 / L154 也以此 `resolved` 为 key），再：
///
/// - `state.get(&resolved) == None` → `NoPriorRead`（自 T2-P0-016 同 PR 起对 edit 与 write
///   均**强拒**，与 write.md §9 / edit.md §10.2 一致）；
/// - `state.get` 命中且 `mtime_ms` / `size` 与 `metadata` 漂移 → `Stale`，要求重新 read；
/// - 路径 `normalize_path` 失败、`metadata` 读不到等情况让 primitive 自己用更具体的
///   IO / permission 错误回执（不在编排层猜原因）。
///
/// `op_label` 仅用于错误文案（`"edit"` / `"write"`），不影响判定逻辑。
fn check_mutation_stamp(
    state: &Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    path: &str,
    op_label: &str,
) -> Result<(), String> {
    let resolved = match crate::infra::platform::normalize_path(path) {
        Ok(p) => p,
        Err(_) => return Ok(()), // 让 primitive 报权限/IO 具体错
    };
    let Some(stamp) = state.get(&resolved) else {
        return Err(format!(
            "NoPriorRead: 当前会话未对 `{}` 执行过 `read`，禁止盲写/盲改；请先 `read` 再 `{}`",
            path, op_label
        ));
    };
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return Ok(()); // 让 primitive 报具体 IO
    };
    if meta.is_dir() {
        return Err(format!(
            "{}: 目标 `{}` 是目录，不能作为入参",
            op_label, path
        ));
    }
    let cur_mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
    if stamp.mtime_ms != cur_mtime || stamp.size != meta.len() {
        return Err(format!(
            "Stale: 文件 `{}` 自上次 read 后已被修改（mtime/size 不一致），请先重新 `read` 再 `{}`",
            path, op_label
        ));
    }
    Ok(())
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
