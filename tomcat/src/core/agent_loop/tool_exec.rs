//! # Agent Loop 工具执行子模块
//!
//! 职责单一：把 `ToolCallInfo` 解析为具体 primitive 调用，返回 `(content, is_error)`。
//! 7 分支（read / write / edit / bash / list_dir /
//! 未知工具 / 参数解析失败）逐字搬自 `run.rs`，**不依赖 `AgentLoop`**——
//! 旧名 `execute_bash` **不**做运行时 fallback（与 read / write / edit 同口径）：
//! transcript 旧名由 `session::manager::context::warn_if_legacy_tool_name` 加载时
//! 一次性 `tracing::warn!`，新一轮 LLM 调用旧名走 `unknown` 分支。
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
//! - `bash` 的失败通过 `PrimitiveExecutor::execute_bash` 的 `Result::Err` 传出
//!   （**trait 方法名**保留 `execute_bash`，与 `write_file` / `edit_file` 同形；
//!   仅 LLM 可见的工具名为短名 `bash`）；`exit_code != 0` 本身**不**置
//!   `is_error`（与原行为一致，保留给下游 LLM 自行判断）。
//!
//! ## `AGENT_PLUGIN_ID`
//!
//! Primitive 层需要一个 `plugin_id` 标签做 hostcall 审计；Agent Loop 直接执行
//! 的工具调用（与"插件上下文中触发的工具调用"相对）统一使用 `"__agent__"`
//! 字面值。本模块顶部常量化后，未来若需重命名只改一处，避免散落。

use std::sync::Arc;

use crate::core::tools::primitive::{
    BashTaskRegistry, EditOperation, EditOperationType, PrimitiveExecutor, SearchFilesArgs,
};
use crate::infra::error::AppError;

use super::config_backend::SharedConfigBackend;
use super::types::ToolCallInfo;

/// Agent Loop 直接触发的工具调用使用的固定 `plugin_id` 标签。
/// 与"插件上下文中触发的工具调用"区分，便于 hostcall 审计层分桶。
pub(super) const AGENT_PLUGIN_ID: &str = "__agent__";

/// reviewer 子 Agent 在 tool_exec 层允许调用的工具名白名单（与
/// `prod_reviewer::REVIEWER_ALLOWED_TOOLS` 保持一致）。
///
/// 这是 catalog 过滤之外的第二道防线：即便上游误注入了 catalog 之外的工具
/// 定义，或 dispatcher 绕过了 catalog，tool_exec 也要把这些调用拦在外面。
fn is_reviewer_whitelisted_tool(name: &str) -> bool {
    matches!(
        name,
        "read" | "search_files" | "list_dir" | "todos" | "update_plan" | "edit"
    )
}

/// 在内存里模拟 `primitive.edit_file` 的字符串替换语义，供 reviewer 段守卫做
/// dry-run。与 [`crate::core::tools::primitive::executor::write_edit::edit_file_impl`]
/// 保持等价的简化实现：
///
/// - 单段：`EDIT_REPLACE_ALL_MARKER` 前缀 → `replace_all`，否则 `replacen(old, new, 1)`；
/// - 多段：按数组顺序依次应用到累计字符串上（与 primitive 相同的「全段串行 apply」）。
///
/// 注意：本 simulate 不做重叠检测、不处理换行 normalization——这些边界 primitive
/// 内部自己会处理 / 拒绝，本预检失败时 primitive 同样会失败，反之亦然；reviewer
/// 段守卫只关心「是否会改到非 `## Review` 段」，与上述边界正交。
fn simulate_apply_edits(
    original: &str,
    edits: &[crate::core::tools::primitive::EditOperation],
) -> String {
    let marker = crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER;
    let mut cur = original.to_string();
    for op in edits {
        let Some(raw_old) = op.old_content.as_deref() else {
            continue;
        };
        let (replace_all, old_text) = if let Some(stripped) = raw_old.strip_prefix(marker) {
            (true, stripped)
        } else {
            (false, raw_old)
        };
        if old_text.is_empty() {
            continue;
        }
        if replace_all {
            cur = cur.replace(old_text, &op.new_content);
        } else {
            cur = cur.replacen(old_text, &op.new_content, 1);
        }
    }
    cur
}

/// 执行单次 tool call 并返回 `(输出文本, is_error)`。
///
/// 自由函数设计（**不**接收 `&AgentLoop`）：调用方持有 `Arc<dyn PrimitiveExecutor>`
/// 即可直接调用；test 只需 mock `PrimitiveExecutor`，不必 mock 整个 AgentLoop。
///
/// `config_backend` 为可选注入：未注入时 `config_get` / `config_set` 命中后返回
/// 错误文案（参考 [`super::config_backend::ConfigBackend`] 的契约）。
#[cfg(test)]
pub(super) async fn execute_tool(
    primitive: &Arc<dyn PrimitiveExecutor>,
    config_backend: &Option<SharedConfigBackend>,
    bash_task_registry: &Option<Arc<BashTaskRegistry>>,
    read_file_state: Option<&Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
    tc: &ToolCallInfo,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    execute_tool_with_openai_files(
        primitive,
        config_backend,
        bash_task_registry,
        read_file_state,
        None,
        tc,
    )
    .await
}

#[allow(dead_code)]
pub(super) async fn execute_tool_with_openai_files(
    primitive: &Arc<dyn PrimitiveExecutor>,
    config_backend: &Option<SharedConfigBackend>,
    bash_task_registry: &Option<Arc<BashTaskRegistry>>,
    read_file_state: Option<&Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
    openai_files_runtime: Option<&Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    tc: &ToolCallInfo,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    execute_tool_full(
        primitive,
        config_backend,
        bash_task_registry,
        read_file_state,
        openai_files_runtime,
        None,
        crate::core::agent_loop::types::SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        tc,
    )
    .await
}

/// 完整版工具执行器：在 `execute_tool_with_openai_files` 基础上额外接受
/// `plan_runtime` 与 `subagent_type`，用于：
/// - 分发 `create_plan` / `update_plan` / `todos` / `ask_question` 四个 plan 工具（B1）
/// - 在 `write` / `edit` / `hashline_edit` / `delete` 分支触发
///   [`crate::api::chat::plan_runtime::safety::enforce_write_path_policy`]（B12）
///
/// `plan_runtime = None` 时这四个工具会返回「PlanRuntime 未注入」错误；写工具策略跳过。
pub(super) async fn execute_tool_full(
    primitive: &Arc<dyn PrimitiveExecutor>,
    config_backend: &Option<SharedConfigBackend>,
    bash_task_registry: &Option<Arc<BashTaskRegistry>>,
    read_file_state: Option<&Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
    openai_files_runtime: Option<&Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    plan_runtime: Option<&Arc<crate::api::chat::plan_runtime::PlanRuntime>>,
    subagent_type: crate::core::agent_loop::types::SubagentType,
    cancel: &tokio_util::sync::CancellationToken,
    tc: &ToolCallInfo,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
        Ok(v) => v,
        Err(e) => return (format!("参数解析失败: {}", e), true, Vec::new()),
    };

    // B3-guard：reviewer 子 Agent 不允许调 catalog 白名单外的任何工具（双保险——
    // catalog 已被 `resolve_internal_tools` 过滤过，这里再拦一道，防 dispatcher 直调或
    // catalog 漂移；与 reviewer.md §5.2 / §5.5 一致）。
    if subagent_type == crate::core::agent_loop::types::SubagentType::Reviewer
        && !is_reviewer_whitelisted_tool(tc.name.as_str())
    {
        return (
            format!(
                "reviewer 子 Agent 禁止调用工具 `{}`（仅允许 read/search_files/list_dir/todos/update_plan/edit；create_plan 防套娃；bash/write/dispatch_agent/checkpoint 永不可用）",
                tc.name
            ),
            true,
            Vec::new(),
        );
    }

    // B1：plan 工具分发（优先于 primitive，因为这些工具不走 primitive）。
    if matches!(
        tc.name.as_str(),
        "create_plan" | "update_plan" | "todos" | "ask_question"
    ) {
        return dispatch_plan_tool(&tc.name, &args, plan_runtime, subagent_type, cancel).await;
    }

    // B12：write 类工具在主体之前做路径策略守卫。
    if matches!(
        tc.name.as_str(),
        "write" | "edit" | "hashline_edit" | "delete"
    ) {
        if let Some(rt) = plan_runtime {
            let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !path_arg.is_empty() {
                let mode = rt.mode();
                let subagent_kind = match subagent_type {
                    crate::core::agent_loop::types::SubagentType::Reviewer => {
                        crate::api::chat::plan_runtime::safety::SubagentKind::Reviewer
                    }
                    _ => crate::api::chat::plan_runtime::safety::SubagentKind::Other,
                };
                if let Err(denied) =
                    crate::api::chat::plan_runtime::safety::enforce_write_path_policy(
                        &mode,
                        subagent_kind,
                        std::path::Path::new(path_arg),
                    )
                {
                    return (denied.to_string(), true, Vec::new());
                }
            }
        }
    }

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
                            let decision = crate::core::llm::openai_files::upload_decision_by_size(
                                b.original_size,
                            );
                            let mut uploaded = false;
                            if let Some(runtime) = openai_files_runtime {
                                if !matches!(
                                    decision,
                                    crate::core::llm::openai_files::UploadDecision::InlinePreferred
                                ) {
                                    match runtime
                                        .resolve_or_upload_path(
                                            &b.path,
                                            &b.mime,
                                            &b.filename,
                                            crate::core::llm::openai_files::FilePurpose::Vision,
                                        )
                                        .await
                                    {
                                        Ok(meta) => {
                                            match crate::core::llm::ChatMessageContentPart::image_file_id(meta.id) {
                                                Ok(part) => {
                                                    follow_up_parts.push(part);
                                                    uploaded = true;
                                                }
                                                Err(e) => tracing::warn!(
                                                    error = %e,
                                                    path = %b.path.display(),
                                                    "read T3-c: upload succeeded but failed to build image_file_id part"
                                                ),
                                            }
                                        }
                                        Err(e) => {
                                            if matches!(
                                                decision,
                                                crate::core::llm::openai_files::UploadDecision::UploadRequired
                                            ) {
                                                return (
                                                    format!(
                                                        "Read attachment upload failed (required by policy): {}",
                                                        e
                                                    ),
                                                    true,
                                                    Vec::new(),
                                                );
                                            }
                                            tracing::warn!(
                                                error = %e,
                                                path = %b.path.display(),
                                                "read T3-c: upload failed on preferred path; fallback to inline"
                                            );
                                        }
                                    }
                                }
                            } else if matches!(
                                decision,
                                crate::core::llm::openai_files::UploadDecision::UploadRequired
                            ) {
                                return (
                                    "Read attachment requires OpenAI Files upload, but current provider/runtime does not support it; 请改用支持 Files API 的 provider 或缩小附件后走 inline".to_string(),
                                    true,
                                    Vec::new(),
                                );
                            }

                            if !uploaded {
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
                        }
                        crate::core::tools::primitive::ReadResult::Pdf(b) => {
                            let decision = crate::core::llm::openai_files::upload_decision_by_size(
                                b.original_size,
                            );
                            let mut uploaded = false;
                            if let Some(runtime) = openai_files_runtime {
                                if !matches!(
                                    decision,
                                    crate::core::llm::openai_files::UploadDecision::InlinePreferred
                                ) {
                                    match runtime
                                        .resolve_or_upload_path(
                                            &b.path,
                                            &b.mime,
                                            &b.filename,
                                            crate::core::llm::openai_files::FilePurpose::UserData,
                                        )
                                        .await
                                    {
                                        Ok(meta) => {
                                            match crate::core::llm::ChatMessageContentPart::file_file_id(
                                                meta.id,
                                                Some(b.filename.clone()),
                                            ) {
                                                Ok(part) => {
                                                    follow_up_parts.push(part);
                                                    uploaded = true;
                                                }
                                                Err(e) => tracing::warn!(
                                                    error = %e,
                                                    path = %b.path.display(),
                                                    "read T3-c: upload succeeded but failed to build file_file_id part"
                                                ),
                                            }
                                        }
                                        Err(e) => {
                                            if matches!(
                                                decision,
                                                crate::core::llm::openai_files::UploadDecision::UploadRequired
                                            ) {
                                                return (
                                                    format!(
                                                        "Read attachment upload failed (required by policy): {}",
                                                        e
                                                    ),
                                                    true,
                                                    Vec::new(),
                                                );
                                            }
                                            tracing::warn!(
                                                error = %e,
                                                path = %b.path.display(),
                                                "read T3-c: upload failed on preferred path; fallback to inline"
                                            );
                                        }
                                    }
                                }
                            } else if matches!(
                                decision,
                                crate::core::llm::openai_files::UploadDecision::UploadRequired
                            ) {
                                return (
                                    "Read attachment requires OpenAI Files upload, but current provider/runtime does not support it; 请改用支持 Files API 的 provider 或缩小附件后走 inline".to_string(),
                                    true,
                                    Vec::new(),
                                );
                            }

                            if !uploaded {
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
                // B3-guard：reviewer 子 Agent 在 plan 文件上的 edit 允许改正文，但不能 raw 改
                // frontmatter。simulate apply edits 之后做 diff，越界即拒（不真正写盘）。
                if subagent_type == crate::core::agent_loop::types::SubagentType::Reviewer {
                    let normalized_path = match crate::infra::platform::normalize_path(path) {
                        Ok(path) => path,
                        Err(e) => {
                            return (
                                format!("reviewer edit 预检路径解析失败：{e}"),
                                true,
                                Vec::new(),
                            );
                        }
                    };
                    match std::fs::read_to_string(&normalized_path) {
                        Ok(old) => {
                            let new = simulate_apply_edits(&old, &edits);
                            if let Err(denied) =
                                crate::api::chat::plan_runtime::safety::reviewer_body_diff_guard(
                                    &old, &new,
                                )
                            {
                                return (format!("reviewer edit 被拒：{denied}"), true, Vec::new());
                            }
                        }
                        Err(e) => {
                            return (
                                format!("reviewer edit 预检读原文失败：{e}"),
                                true,
                                Vec::new(),
                            );
                        }
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
        "bash" => {
            let command = args["command"].as_str().unwrap_or("");
            let cwd = args["cwd"].as_str();
            let argv_store: Option<Vec<String>> =
                args.get("args").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                });
            let argv_ref = argv_store.as_deref();
            // T2-P0-016 PR-E.2：解析 schema `timeout_ms`，clamp 到 [1, MAX_TOOLS_BASH_TIMEOUT_MS]
            // 后传给 primitive；None / 0 / 越界一律由 primitive 兜底为 config 默认。
            let timeout_ms_override: Option<u64> = args
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .map(|v| v.min(crate::infra::MAX_TOOLS_BASH_TIMEOUT_MS));
            let run_in_background = args
                .get("run_in_background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // T2-P0-016 PR-I：run_in_background=true 走后台注册表，立即返回 ticket；
            // 同步路径完全不变（PR-E 行为）。
            if run_in_background {
                handle_bash_background(bash_task_registry, command, cwd, argv_store).await
            } else {
                primitive
                    .execute_bash(command, cwd, AGENT_PLUGIN_ID, argv_ref, timeout_ms_override)
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
        }
        // T2-P0-016 PR-I：bash 后台任务三件套；未注入 registry 时返回友好错误，
        // 主流程不阻塞（与 config_get / config_set 「未启用」语义一致）。
        "task_output" => handle_task_output(bash_task_registry, &args).await,
        "task_stop" => handle_task_stop(bash_task_registry, &args).await,
        "task_list" => handle_task_list(bash_task_registry).await,
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

/// T2-P0-016 PR-I：`bash run_in_background=true` 进入后台路径，立即返回
/// `task_id` + `log_path`；不阻塞当前 tool 轮次。返回的 JSON 结构与 catalog
/// `bash` description 中给模型的承诺一致。
async fn handle_bash_background(
    registry: &Option<Arc<BashTaskRegistry>>,
    command: &str,
    cwd: Option<&str>,
    argv: Option<Vec<String>>,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("bash 后台任务未启用：未注入 BashTaskRegistry".to_string());
    };
    let cwd_pb = cwd.map(std::path::PathBuf::from);
    registry
        .spawn(command.to_string(), argv, cwd_pb)
        .await
        .map(|t| serde_json::to_string(&t).unwrap_or_else(|_| "{}".to_string()))
        .map_err(|e| e.to_string())
}

async fn handle_task_output(
    registry: &Option<Arc<BashTaskRegistry>>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("task_output 未启用：未注入 BashTaskRegistry".to_string());
    };
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "task_output 缺少 task_id".to_string())?;
    let since = args.get("since").and_then(|v| v.as_u64());
    registry
        .read_output(task_id, since)
        .await
        .map(|c| serde_json::to_string(&c).unwrap_or_else(|_| "{}".to_string()))
        .map_err(|e| e.to_string())
}

async fn handle_task_stop(
    registry: &Option<Arc<BashTaskRegistry>>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("task_stop 未启用：未注入 BashTaskRegistry".to_string());
    };
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "task_stop 缺少 task_id".to_string())?;
    registry
        .stop(task_id)
        .await
        .map(|_| format!("已停止: {}", task_id))
        .map_err(|e| e.to_string())
}

async fn handle_task_list(registry: &Option<Arc<BashTaskRegistry>>) -> Result<String, String> {
    let Some(registry) = registry.as_ref() else {
        return Err("task_list 未启用：未注入 BashTaskRegistry".to_string());
    };
    let infos = registry.list();
    Ok(serde_json::to_string(&infos).unwrap_or_else(|_| "[]".to_string()))
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
/// （与 [edit.md §4.2](../../../docs/architecture/tools/edit.md) 对齐）。
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
/// 边界（与 `docs/architecture/tools/read.md` §2.1 / §2.6 一致）：
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

// ─── B1：plan 工具分发 ─────────────────────────────────────────────────────────

/// 将四个 plan 工具（create_plan / update_plan / todos / ask_question）的调用路由到
/// `plan_runtime::tools` 实现；plan_runtime 未注入时返回错误文案。
///
/// 文案约定：返回 `(content, is_error=true, no_follow_up)`；与 `unknown` 分支一致——
/// 让 LLM 用收到的错误文本自我修正（一般是上下文里没装 PlanRuntime，比如 reviewer 子 Agent 调本
/// 不该出现的工具，或单测路径未注入）。
async fn dispatch_plan_tool(
    name: &str,
    args: &serde_json::Value,
    plan_runtime: Option<&Arc<crate::api::chat::plan_runtime::PlanRuntime>>,
    subagent_type: crate::core::agent_loop::types::SubagentType,
    cancel: &tokio_util::sync::CancellationToken,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    let Some(rt) = plan_runtime else {
        return (
            format!(
                "plan 工具 `{name}` 不可用：当前 AgentLoop 未注入 PlanRuntime（reviewer 子 Agent 或独立测试路径）"
            ),
            true,
            Vec::new(),
        );
    };
    // B3-guard：reviewer 路径再拦一次 create_plan（防套娃 / 防绕过）。
    if name == "create_plan"
        && subagent_type == crate::core::agent_loop::types::SubagentType::Reviewer
    {
        return (
            "reviewer 子 Agent 禁止调用 `create_plan`（防套娃；reviewer.md §5.2 / §5.5）".into(),
            true,
            Vec::new(),
        );
    }
    use crate::api::chat::plan_runtime::tools as plan_tools;
    let result: Result<serde_json::Value, plan_tools::ToolError> = match name {
        "create_plan" => {
            match serde_json::from_value::<plan_tools::create_plan::CreatePlanArgs>(args.clone()) {
                // `allow_review_edit` 在生产路径恒为 `true`（reviewer.md §5.2 / §5.5 拍板）；
                // Mock 单测可以通过 trait 直接构造 `MockReviewerDispatcher` 注入 false。
                Ok(a) => plan_tools::create_plan::execute_with_reviewer(rt, a, true).await,
                Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
            }
        }
        "update_plan" => {
            match serde_json::from_value::<plan_tools::update_plan::UpdatePlanArgs>(args.clone()) {
                Ok(a) => plan_tools::update_plan::execute(rt, a),
                Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
            }
        }
        "todos" => match serde_json::from_value::<plan_tools::todos::TodosArgs>(args.clone()) {
            Ok(a) => plan_tools::todos::execute(rt, a),
            Err(e) => Err(plan_tools::ToolError::BadArgs(e.to_string())),
        },
        "ask_question" => {
            let Some(panel) = rt.ask_question_panel() else {
                return (
                    "ask_question 不可用：PlanRuntime 未配置 AskQuestionPanel".into(),
                    true,
                    Vec::new(),
                );
            };
            // 将 CancellationToken 桥接为 AtomicBool（ask_question 接口）。
            let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let watcher_flag = cancel_flag.clone();
            let cancel_clone = cancel.clone();
            let bridge = tokio::spawn(async move {
                cancel_clone.cancelled().await;
                watcher_flag.store(true, std::sync::atomic::Ordering::Release);
            });
            // N13：从 PlanRuntime 取 ask_question.timeout_ms（None → ask_question 内部默认 300s）。
            let timeout_ms = rt.ask_question_timeout_ms();
            let res = plan_tools::ask_question::execute_with_timeout(
                rt,
                panel.as_ref(),
                args,
                cancel_flag,
                timeout_ms,
            )
            .await;
            bridge.abort();
            res
        }
        _ => unreachable!("dispatch_plan_tool called with unknown name {name}"),
    };
    match result {
        Ok(v) => (v.to_string(), false, Vec::new()),
        Err(e) => (format!("{name} 失败：{e}"), true, Vec::new()),
    }
}

#[cfg(test)]
mod reviewer_guards_tests {
    use super::*;
    use crate::core::agent_loop::types::SubagentType;
    use serial_test::serial;

    /// 占位 PrimitiveExecutor——本套测试不会真正进 primitive 分支（reviewer 守卫早退），
    /// 所有方法 unreachable!() 反向证伪：一旦真有调用就会 panic。
    struct UnusedPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for UnusedPrimitive {
        async fn read(
            &self,
            _path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
            _line_numbers: bool,
            _hashline: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::ReadResult, AppError> {
            unreachable!("reviewer guard 应在 primitive 之前 short-circuit")
        }
        async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
            unreachable!()
        }
        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
            unreachable!()
        }
        async fn write_file(
            &self,
            _path: &str,
            _content: &str,
            _overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
            unreachable!()
        }
        async fn edit_file(
            &self,
            _path: &str,
            _edits: Vec<crate::core::tools::primitive::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
            unreachable!()
        }
        async fn execute_bash(
            &self,
            _command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
            _argv: Option<&[String]>,
            _timeout_ms_override: Option<u64>,
        ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
            unreachable!()
        }
        async fn hashline_edit(
            &self,
            _path: &str,
            _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
            unreachable!()
        }
        async fn search_files(
            &self,
            _args: crate::core::tools::primitive::SearchFilesArgs,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::SearchFilesOutput, AppError> {
            unreachable!()
        }
        async fn require_user_confirmation(
            &self,
            _operation: crate::core::tools::primitive::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            unreachable!()
        }
    }

    struct EditOkPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for EditOkPrimitive {
        async fn read(
            &self,
            _path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
            _line_numbers: bool,
            _hashline: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::ReadResult, AppError> {
            unreachable!()
        }
        async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
            unreachable!()
        }
        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
            unreachable!()
        }
        async fn write_file(
            &self,
            _path: &str,
            _content: &str,
            _overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
            unreachable!()
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<crate::core::tools::primitive::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
            Ok(crate::core::tools::primitive::EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            _command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
            _argv: Option<&[String]>,
            _timeout_ms_override: Option<u64>,
        ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
            unreachable!()
        }
        async fn hashline_edit(
            &self,
            _path: &str,
            _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
            unreachable!()
        }
        async fn search_files(
            &self,
            _args: crate::core::tools::primitive::SearchFilesArgs,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::SearchFilesOutput, AppError> {
            unreachable!()
        }
        async fn require_user_confirmation(
            &self,
            _operation: crate::core::tools::primitive::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            unreachable!()
        }
    }

    /// reviewer.md §11 RV-T2：tool_exec 在 reviewer 路径下，对白名单外的工具直接 tool error。
    #[tokio::test]
    async fn reviewer_blocks_non_whitelisted_tool() {
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
        let tc = ToolCallInfo {
            id: "tc1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
        };
        let (msg, is_err, _) = execute_tool_full(
            &primitive,
            &None,
            &None,
            None,
            None,
            None,
            SubagentType::Reviewer,
            &tokio_util::sync::CancellationToken::new(),
            &tc,
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("reviewer 子 Agent 禁止调用工具"));
    }

    /// reviewer.md §11 RV-T4：reviewer 路径下 `create_plan` 被白名单守卫早退（防套娃）。
    #[tokio::test]
    async fn reviewer_blocks_create_plan_subagent() {
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
        let tc = ToolCallInfo {
            id: "tc1".into(),
            name: "create_plan".into(),
            arguments: "{}".into(),
        };
        let (msg, is_err, _) = execute_tool_full(
            &primitive,
            &None,
            &None,
            None,
            None,
            None,
            SubagentType::Reviewer,
            &tokio_util::sync::CancellationToken::new(),
            &tc,
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("reviewer 子 Agent 禁止调用工具 `create_plan`"));
    }

    #[tokio::test]
    #[serial(env_lock)]
    async fn reviewer_edit_precheck_accepts_tilde_plan_path() {
        let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
        let old_home = std::env::var("HOME").ok();
        let temp_home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp_home.path());

        struct HomeGuard(Option<String>);
        impl Drop for HomeGuard {
            fn drop(&mut self) {
                match &self.0 {
                    Some(home) => std::env::set_var("HOME", home),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
        let _guard = HomeGuard(old_home);

        let plan_id = "reviewer_tilde_smoke";
        let plan_path =
            crate::api::chat::plan_runtime::file_store::plan_path_for_id(plan_id).unwrap();
        std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        std::fs::write(
            &plan_path,
            "---\nplan_id: reviewer_tilde_smoke\ngoal: smoke\nmode: planning\nschema_version: 1\ntodos: []\n---\n## Goal\n\nsmoke\n\n## Notes\n\nold note\n\n## Todos Board\n\n<!-- todos-board:auto:begin -->\n<!-- todos-board:auto:end -->\n",
        )
        .unwrap();

        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(EditOkPrimitive);
        let tc = ToolCallInfo {
            id: "tc1".into(),
            name: "edit".into(),
            arguments: serde_json::json!({
                "path": format!("~/.tomcat/plans/{plan_id}.plan.md"),
                "old_content": "## Goal\n\nsmoke\n\n## Notes",
                "new_content": "## Goal\n\nupdated smoke\n\n## Notes"
            })
            .to_string(),
        };
        let (msg, is_err, _) = execute_tool_full(
            &primitive,
            &None,
            &None,
            None,
            None,
            None,
            SubagentType::Reviewer,
            &tokio_util::sync::CancellationToken::new(),
            &tc,
        )
        .await;

        assert!(!is_err, "unexpected error: {msg}");
        assert!(msg.contains("已编辑: ~/.tomcat/plans/reviewer_tilde_smoke.plan.md"));
    }
}
