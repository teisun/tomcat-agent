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

mod args;
mod branches;
mod edit_sim;
mod guard;

use std::sync::Arc;

use crate::core::tools::primitive::{BashTaskRegistry, PrimitiveExecutor};
use crate::infra::event_bus::EventBus;
use crate::infra::events::ToolDisplay;
#[cfg(test)]
use crate::infra::error::AppError;

use super::config_backend::SharedConfigBackend;
use super::types::{BackgroundCompletionRoutes, ToolCallInfo};
use guard::{
    is_reviewer_whitelisted_tool, is_verifier_whitelisted_tool,
    reviewer_allowed_tools_description,
};

/// Agent Loop 直接触发的工具调用使用的固定 `plugin_id` 标签。
/// 与"插件上下文中触发的工具调用"区分，便于 hostcall 审计层分桶。
pub(super) const AGENT_PLUGIN_ID: &str = "__agent__";

/// `tool_exec` 提供给 dispatcher 的完整结果：
/// - `model_text` 保持给 transcript / LLM
/// - `display` 只给观察层 UI 使用
pub(super) struct ToolExecOutcome {
    pub(super) model_text: String,
    pub(super) is_error: bool,
    pub(super) follow_up_parts: Vec<crate::core::llm::ChatMessageContentPart>,
    pub(super) display: Option<ToolDisplay>,
}

impl ToolExecOutcome {
    fn ok(model_text: impl Into<String>) -> Self {
        Self {
            model_text: model_text.into(),
            is_error: false,
            follow_up_parts: Vec::new(),
            display: None,
        }
    }

    fn err(model_text: impl Into<String>) -> Self {
        Self {
            model_text: model_text.into(),
            is_error: true,
            follow_up_parts: Vec::new(),
            display: None,
        }
    }

    fn into_legacy_tuple(self) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
        (self.model_text, self.is_error, self.follow_up_parts)
    }
}

struct ToolExecCtx<'a> {
    primitive: &'a Arc<dyn PrimitiveExecutor>,
    config_backend: &'a Option<SharedConfigBackend>,
    bash_task_registry: &'a Option<Arc<BashTaskRegistry>>,
    read_file_state: Option<&'a Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
    openai_files_runtime:
        Option<&'a Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    plan_runtime: Option<&'a Arc<crate::core::plan_runtime::PlanRuntime>>,
    subagent_type: crate::core::agent_loop::types::SubagentType,
    review_kind: Option<crate::core::plan_runtime::review::ReviewKind>,
    cancel: &'a tokio_util::sync::CancellationToken,
    event_bus: Option<&'a Arc<dyn EventBus>>,
    completion_routes: Option<&'a BackgroundCompletionRoutes>,
}

/// 执行单次 tool call 并返回兼容旧测试的 `(输出文本, is_error)`。
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
        None,
        &tokio_util::sync::CancellationToken::new(),
        tc,
        None,
        None,
    )
    .await
    .into_legacy_tuple()
}

/// 完整版工具执行器：在 `execute_tool_with_openai_files` 基础上额外接受
/// `plan_runtime` 与 `subagent_type`，用于：
/// - 分发 `create_plan` / `update_plan` / `todos` / `ask_question` 四个 plan 工具（B1）
/// - 在 `write` / `edit` / `hashline_edit` / `delete` 分支触发
///   [`crate::core::plan_runtime::safety::enforce_write_path_policy`]（B12）
///
/// `plan_runtime = None` 时这四个工具会返回「PlanRuntime 未注入」错误；写工具策略跳过。
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_full(
    primitive: &Arc<dyn PrimitiveExecutor>,
    config_backend: &Option<SharedConfigBackend>,
    bash_task_registry: &Option<Arc<BashTaskRegistry>>,
    read_file_state: Option<&Arc<crate::core::tools::pipeline::read_state::ReadFileState>>,
    openai_files_runtime: Option<&Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    plan_runtime: Option<&Arc<crate::core::plan_runtime::PlanRuntime>>,
    subagent_type: crate::core::agent_loop::types::SubagentType,
    review_kind: Option<crate::core::plan_runtime::review::ReviewKind>,
    cancel: &tokio_util::sync::CancellationToken,
    tc: &ToolCallInfo,
    // P1（bash background monitor）：传 event_bus 给 task_output(block=true) 发倒计时
    // ToolExecutionUpdate；传 completion_routes 让 dispatcher 走 claim-on-entry 去重。
    // 二者均可为 None（向后兼容独立单测/未注入路径）。
    event_bus: Option<&Arc<dyn EventBus>>,
    completion_routes: Option<&BackgroundCompletionRoutes>,
) -> ToolExecOutcome {
    let mut display = None;
    let ctx = ToolExecCtx {
        primitive,
        config_backend,
        bash_task_registry,
        read_file_state,
        openai_files_runtime,
        plan_runtime,
        subagent_type,
        review_kind,
        cancel,
        event_bus,
        completion_routes,
    };
    let (model_text, is_error, follow_up_parts) =
        execute_tool_tuple_full(&ctx, tc, &mut display).await;

    ToolExecOutcome {
        display,
        model_text,
        is_error,
        follow_up_parts,
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_tuple_full(
    ctx: &ToolExecCtx<'_>,
    tc: &ToolCallInfo,
    display_out: &mut Option<ToolDisplay>,
) -> (String, bool, Vec<crate::core::llm::ChatMessageContentPart>) {
    let args: serde_json::Value = match serde_json::from_str(&tc.arguments) {
        Ok(v) => v,
        Err(e) => return (format!("参数解析失败: {}", e), true, Vec::new()),
    };

    // B3-guard：reviewer 子 Agent 不允许调 catalog 白名单外的任何工具（双保险——
    // catalog 已被 `resolve_internal_tools` 过滤过，这里再拦一道，防 dispatcher 直调或
    // catalog 漂移；与 reviewer.md §5.2 / §5.5 一致）。
    if ctx.subagent_type == crate::core::agent_loop::types::SubagentType::Reviewer
        && !is_reviewer_whitelisted_tool(tc.name.as_str(), ctx.review_kind)
    {
        return (
            format!(
                "reviewer 子 Agent 禁止调用工具 `{}`（仅允许 {}；create_plan 防套娃；write/dispatch_agent/checkpoint 永不可用）",
                tc.name,
                reviewer_allowed_tools_description(ctx.review_kind),
            ),
            true,
            Vec::new(),
        );
    }
    if ctx.subagent_type == crate::core::agent_loop::types::SubagentType::Verifier
        && !is_verifier_whitelisted_tool(tc.name.as_str())
    {
        return (
            format!(
                "verifier 子 Agent 禁止调用工具 `{}`（仅允许 read/search_files/list_dir/bash；create_plan/update_plan/todos/ask_question/edit/write/dispatch_agent/checkpoint 永不可用）",
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
        return branches::dispatch_plan_tool(ctx, &tc.name, &args, display_out)
            .await
            .into_legacy_tuple();
    }

    // B12：write 类工具在主体之前做路径策略守卫。
    if matches!(
        tc.name.as_str(),
        "write" | "edit" | "hashline_edit" | "delete"
    ) {
        if let Some(rt) = ctx.plan_runtime {
            let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !path_arg.is_empty() {
                let mode = rt.mode();
                let subagent_kind = match (ctx.subagent_type, ctx.review_kind) {
                    (
                        crate::core::agent_loop::types::SubagentType::Reviewer,
                        Some(crate::core::plan_runtime::review::ReviewKind::Code),
                    ) => crate::core::plan_runtime::safety::SubagentKind::CodeReviewer,
                    (crate::core::agent_loop::types::SubagentType::Reviewer, _) => {
                        crate::core::plan_runtime::safety::SubagentKind::Reviewer
                    }
                    _ => crate::core::plan_runtime::safety::SubagentKind::Other,
                };
                if let Err(denied) = crate::core::plan_runtime::safety::enforce_write_path_policy(
                    &mode,
                    subagent_kind,
                    std::path::Path::new(path_arg),
                ) {
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
        "read" => match branches::handle_read(ctx, &args).await {
            Ok((text, parts)) => {
                follow_up_parts = parts;
                Ok(text)
            }
            Err(err) => Err(err),
        },
        "write" => branches::handle_write(ctx, &args, display_out).await,
        "edit" => branches::handle_edit(ctx, &args, display_out).await,
        "bash" => branches::handle_bash(ctx, &args).await,
        "task_output" => branches::handle_task_output(ctx, tc, &args).await,
        "task_stop" => branches::handle_task_stop(ctx.bash_task_registry, &args).await,
        "task_list" => branches::handle_task_list(ctx.bash_task_registry).await,
        "list_dir" => branches::handle_list_dir(ctx, &args).await,
        "hashline_edit" => branches::handle_hashline_edit(ctx, &args, display_out).await,
        "search_files" => branches::handle_search_files(ctx, &args).await,
        "config_get" => branches::handle_config_get(ctx, &args).await,
        "config_set" => branches::handle_config_set(ctx, &args, display_out).await,
        other => Err(format!("未知工具: {}", other)),
    };

    match out {
        Ok(s) => (s, false, follow_up_parts),
        Err(s) => (s, true, Vec::new()),
    }
}

#[cfg(test)]
mod tests;
