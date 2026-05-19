//! # CLI 对话主循环
//!
//! [`chat_loop`] 是 `tomcat chat` 子命令的事件循环：装配 [`ChatContext`]、读用户输入、
//! 触发 preheat / 边界压缩、跑 [`AgentLoop`]、流式渲染回执、把消息写回 transcript，
//! 并处理 Ctrl+C 双击退出。
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  ChatContext::from_config(AppConfig)                ① 装配阶段           │
//! │   ├─ SessionManager      （sessions_dir，transcript JSONL 持久层）       │
//! │   ├─ Arc<dyn LlmProvider>（resolve_llm 按 [llm] provider 路由）          │
//! │   ├─ Arc<dyn PrimitiveExecutor>（DefaultPrimitiveExecutor + 白名单）     │
//! │   ├─ Arc<dyn ToolRegistry>     （内置 + 插件 tool）                       │
//! │   ├─ Arc<dyn EventBus>         （DefaultEventBus）                       │
//! │   ├─ Arc<Mutex<CancellationToken>>（每回合重建，Ctrl+C 用）              │
//! │   ├─ Arc<Mutex<Option<Instant>>>  （last_interrupt_at，双击窗口）         │
//! │   └─ workspace_dir       （system prompt + path 白名单默认根）           │
//! └─────────────────────────────────────────────────────────────────────────┘
//!    │
//!    ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  chat_loop(ctx, resume)                              ② 主循环           │
//! │                                                                          │
//! │  ensure_session → init_context_state → register_chat_session_stderr     │
//! │                                                                          │
//! │  loop {                                                                  │
//! │    rl.readline("u> ")                                                    │
//! │      ├ Ok(line)                  ► trim + add_history                    │
//! │      ├ Err(Eof)                  ► preheat.abort() + break               │
//! │      └ Err(Interrupted)          ► continue（Ctrl+C 在 prompt 处忽略）   │
//! │                                                                          │
//! │    cancel_token = CancellationToken::new() ◄─ 重建（cancel 不可逆）     │
//! │    context_state.on_message_appended(input.len())                        │
//! │                                                                          │
//! │    Timing ②  preheat.try_restart_if_pending                              │
//! │              check_before_request（auto-compaction 边界判定）            │
//! │                                                                          │
//! │    messages = build_context_from_state                                   │
//! │             ◄ system_prompt（首位）                                      │
//! │             ◄ user(input)                                                │
//! │                                                                          │
//! │    listener = event_bus.on(MESSAGE_UPDATE, |delta| renderer.push)        │
//! │                                                                          │
//! │    AgentLoop::new(...).run(messages)                                     │
//! │      ├ Completed   ► renderer.flush + session.append_messages            │
//! │      ├ Interrupted ► flush + append（partial）+ ctrl+c 双击 ► exit(130)  │
//! │      └ Failed      ► is_fatal_error? ► break : 打印错误并 continue       │
//! │                                                                          │
//! │    event_bus.off(listener)                                               │
//! │  }                                                                       │
//! │                                                                          │
//! │  preheat.abort + event_bus.off(session_stderr_ids)                       │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 关键约束
//!
//! - **CancellationToken 必须每回合新建**：`tokio_util::sync::CancellationToken`
//!   一旦 cancel 不可逆；上一回合的 Ctrl+C 不能污染下一回合的输入。
//! - **system prompt 不入 transcript**：每次现拼，`messages.insert(0, system)`
//!   仅给 LLM 看，避免 transcript 体积膨胀与 prompt 升级时的历史污染。
//! - **Ctrl+C 双击窗口**：第一击触发 `cancel_token.cancel()`（soft cancel，留
//!   transcript）；2s 内第二击直接 `std::process::exit(130)`（hard exit）。
//!
//! ## 同目录子模块
//!
//! - [`super::render`]：Markdown 流式渲染器。
//! - `events::stderr`：把 `ToolResult` / `Compaction` 等事件按用户视角
//!   渲染到 stderr，与主流 stdout 解耦。
//! - `tests`：CLI 集成测试入口。

use std::io::{self, Write as IoWrite};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{build_context_from_state, init_context_state};
use crate::core::session::read_entries_tail;
use crate::infra::error::AppError;
use crate::infra::{
    AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder, TracingAuditRecorder,
};
use crate::{
    compound_turn_id, resolve_agent_definition_dir, resolve_agent_trail_dir, resolve_sessions_dir,
    resolve_workspace_roots_paths, AgentLoop, AgentLoopConfig, AppConfig, CheckpointKind,
    CheckpointRecordRequest, DefaultPrimitiveExecutor, DefaultToolRegistry, LlmProvider,
    PrimitiveExecutor, SessionEntry, SessionManager, Tool, ToolExecutor, ToolRegistry,
};

use super::render::MarkdownRenderer;

#[cfg(test)]
mod tests;

pub mod cli_turn_renderer;
pub mod commands;
pub mod events;
pub mod permission;
pub mod plan_runtime;
pub mod preflight;

use commands::{dispatch_chat_command, parse_chat_command, ChatCommandOutcome};

// ─── ChatContext ──────────────────────────────────────────────────────────────

pub struct ChatContext {
    pub session: SessionManager,
    pub llm: Arc<dyn LlmProvider>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditRecorder>,
    pub checkpoint_switcher: Arc<crate::core::SwitchingCheckpointStore>,
    pub checkpoint_store: Arc<dyn crate::core::CheckpointStore>,
    /// 当前回合用户中断令牌。ctrlc handler 会 `lock().cancel()`；
    /// `chat_loop` 在每次 readline 读到非空输入后**重建**它（`CancellationToken`
    /// 一旦 cancel 不可逆），保证新回合不会被上一回合的中断信号污染。
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    /// 上一次 Ctrl+C 按下的时刻；ctrlc handler 判双击用。
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
    /// 用户启动 `tomcat chat` 时的 shell 工作目录。
    pub agent_workspace_dir: std::path::PathBuf,
    /// Agent 设计态目录，用于 AGENTS.md / SOUL.md / skills / memory 等长期配置。
    pub agent_definition_dir: std::path::PathBuf,
    /// Agent 运行态轨迹目录，用于 sessions / logs / audit / tmp / tool-results。
    pub agent_trail_dir: std::path::PathBuf,
    /// `tomcat.config.toml` 的解析后绝对路径快照，避免在权限决策路径上重复调用 `config_file_path()`。
    /// `CwdLazyPrompt::AllowAndPersistRoot` 分支用它持久化 workspace root。
    pub cfg_path: std::path::PathBuf,
    /// 会话级临时授权（`/path` 命令 + 用户 confirm AllowOnce 共享）。
    pub session_grants: crate::core::permission::SessionGrants,
    /// `config_get` / `config_set` LLM 工具后端（plan §6 / PR-7）。
    /// 为 `None` 时工具命中返回"未启用"错误，正常 4 原语 / chat 流程不受影响。
    pub config_backend: Option<crate::core::agent_loop::SharedConfigBackend>,
    /// T2-P0-016 PR-I：bash 后台任务三件套（task_output / task_stop / task_list）的
    /// 共享注册表；落盘根目录 = `<agent_trail_dir>/tool-results/`。每个 ChatContext
    /// 单实例，跨 turn 内复用。
    pub bash_task_registry: Arc<crate::core::tools::primitive::BashTaskRegistry>,
    /// 三层权限决策 gate（plan §3 / PR-1）：与 executor / system prompt / 路径授权 UI
    /// 共享同一份 SessionGrants 视图，保证三处的授权变更彼此可见。
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
    /// PR-RF（T2-b/c）`read` 工具的会话级 dedup / staleness 状态。
    /// 由 `ChatContext` 持有 → 每次 turn 创建 `AgentLoopConfig` 时 `Arc::clone` 注入，
    /// 多轮 turn 内复用同一张表（实现「同 session 跨 turn dedup」）。
    pub read_file_state: Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    /// `CliTurnRenderer` 的 thinking 折叠/展开开关（T2-P0-006 P0/P4）。
    /// 进程级初值由 `PI_CHAT_SHOW_THINKING=0/1` 环境变量决定，缺省 `false`；
    /// `/thinking on|off|toggle` 命令在运行时切换；`CliTurnRenderer` 持有同一 Arc 读位。
    pub show_thinking: Arc<std::sync::atomic::AtomicBool>,
    /// T2-P0-015：OpenAI Files 会话级 runtime（不支持 Files 的 provider 为 `None`）。
    pub openai_files_runtime: Option<Arc<crate::core::llm::openai_files::OpenAiFilesRuntime>>,
    /// T2-P1-002 PR-PLA：PLAN 模式 per-session 编排器。挂在 ChatContext 上跨 turn 持久；
    /// chat_loop 装配 `tool_definitions` / system reminder / user prefix 时基于 `plan_runtime.mode()`。
    /// 禁止每轮重建（会丢失 PLAN/EXEC 的持续语义）。
    pub plan_runtime: Arc<plan_runtime::PlanRuntime>,
}

fn git_available_for_checkpoints() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);

        let agent_definition_dir = resolve_agent_definition_dir(&config)?;
        std::fs::create_dir_all(&agent_definition_dir).map_err(AppError::Io)?;
        let agent_trail_dir = resolve_agent_trail_dir(&config)?;
        std::fs::create_dir_all(&agent_trail_dir).map_err(AppError::Io)?;
        migrate_legacy_layer0_tool_results(&agent_definition_dir, &agent_trail_dir);

        // 启动 snapshot：agent_workspace_dir / cfg_path 在整个 chat 生命周期内固定，避免后续 cd
        // 让 system prompt 与权限决策视图漂移。`current_dir()` 失败时退化到 agent_definition_dir
        // （兜底场景：被 chroot 或者 cwd 不可读，仍能继续聊天）。
        let agent_workspace_dir =
            std::env::current_dir().unwrap_or_else(|_| agent_definition_dir.clone());
        let cfg_path_snapshot =
            crate::api::cli::config_file_path().unwrap_or_else(|_| std::path::PathBuf::new());

        let llm: Arc<dyn LlmProvider> = crate::core::llm::resolve_llm(&config.llm)?;
        let openai_files_runtime = crate::core::llm::openai_files::build_runtime_for_provider(
            llm.as_ref(),
            &config.llm.files,
            session.sessions_dir(),
            session.current_session_key(),
        )
        .map(Arc::new);

        let audit: Arc<dyn AuditRecorder> = match AuditStore::open_if_enabled(&config)? {
            Some(store) => Arc::new(FileAuditRecorder::new(Arc::new(store))),
            None => Arc::new(TracingAuditRecorder),
        };
        let workspace_roots = resolve_workspace_roots_paths(&config)?;
        let cli_confirmation: Arc<dyn UserConfirmationProvider> = Arc::new(CliConfirmation);

        // PR-9：构造 3 层权限 gate；与 executor / chat 共享 SessionGrants。
        // agent_trail_readonly_dirs：sessions/logs/audit + agent 凭据目录（凭据子目录由
        // builtin path_rules 单独 deny，read_only 集合允许 read 但禁 write）。
        let session_grants = crate::core::permission::SessionGrants::new();
        let agent_trail_readonly_dirs: Vec<std::path::PathBuf> = vec![
            Some(agent_trail_dir.clone()),
            crate::infra::config::resolve_sessions_dir(&config).ok(),
            crate::infra::config::resolve_log_dir(&config).ok(),
            crate::infra::config::resolve_audit_dir(&config).ok(),
            crate::infra::config::resolve_agent_dir(&config).ok(),
        ]
        .into_iter()
        .flatten()
        .collect();
        // gate-root-remediation：默认 writable root 是 agent_definition_dir
        // （`workspace-<agentId>/`），而不是启动 cwd。启动 cwd 仅在 system prompt
        // 中作为「当前目录 / this project / relative paths」的语义来源，
        // 实际访问 cwd 子树需要 `workspace_roots` / `session_grants`
        // / `permission::cwd_lazy` 提供的会话级授权。
        let gate_cfg = crate::core::permission::GateConfig {
            agent_definition_dir: agent_definition_dir.clone(),
            workspace_roots: workspace_roots.clone(),
            agent_trail_readonly_dirs: agent_trail_readonly_dirs.clone(),
            user_path_rules: config.primitive.path_rules.clone(),
            user_bash_forbidden: config.primitive.bash_forbidden.clone(),
            user_bash_approval: config.primitive.bash_approval_required.clone(),
            auto_confirm: config.primitive.auto_confirm,
        };
        let gate: Arc<dyn crate::core::permission::PermissionGate> = Arc::new(
            crate::core::permission::DefaultPermissionGate::new(gate_cfg, session_grants.clone()),
        );

        // Hotfix §A.3：用 CwdLazyPrompt 装饰 CliConfirmation。
        // 装饰器：当 LLM 工具调用首次落到 cwd 子树未授权路径时弹「[s]/[w]/[c]」
        // 范围级提示，其余情况转发给 CliConfirmation 既有行为。
        // 注意：dismissed flag 在装饰器内部以 Arc<AtomicBool> 创建，与 ctx 同生命周期。
        let confirmation: Arc<dyn UserConfirmationProvider> =
            Arc::new(permission::cwd_lazy::CwdLazyPrompt::new(
                cli_confirmation,
                agent_workspace_dir.clone(),
                gate.clone(),
                session_grants.clone(),
                cfg_path_snapshot.clone(),
            ));

        let _ = workspace_roots; // 已通过 GateConfig.workspace_roots 落入 gate；executor 不再单独保存。
        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(
            DefaultPrimitiveExecutor::new(
                config.primitive.clone(),
                confirmation.clone(),
                audit.clone(),
                gate.clone(),
            )
            // T2-P0-016 PR-G：把 `[tools.write] normalize_crlf` 注入 executor。
            .with_write_normalize_crlf(config.tools.write.normalize_crlf),
        );

        // PR-7：构造 config_get / config_set 工具后端。失败（无法解析 config_path）
        // 时降级为 `None`，工具命中返回"未启用"错误，主流程不阻塞。
        let config_backend: Option<crate::core::agent_loop::SharedConfigBackend> =
            match crate::api::cli::config_file_path() {
                Ok(p) => Some(Arc::new(
                    crate::core::tools::config_tool::ChatConfigBackend {
                        ctx: crate::core::tools::config_tool::ConfigToolContext::new(
                            p,
                            confirmation.clone(),
                        )
                        .with_gate(gate.clone()),
                    },
                )),
                Err(_) => None,
            };

        let checkpoint_switcher = Arc::new(crate::core::SwitchingCheckpointStore::new(
            agent_trail_dir.clone(),
            agent_workspace_dir.clone(),
            git_available_for_checkpoints(),
        ));
        let checkpoint_store: Arc<dyn crate::core::CheckpointStore> = checkpoint_switcher.clone();

        let tool_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(tool_executor, audit.clone()));

        let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));

        // T2-P0-016 PR-I：bash 后台任务注册表；persist_dir 与 PR-E.3 同盘——
        // `<agent_trail_dir>/tool-results/` 既装超时/超长输出落盘，也装后台任务日志。
        let bash_task_registry = Arc::new(crate::core::tools::primitive::BashTaskRegistry::new(
            agent_trail_dir.join("tool-results"),
        ));

        // 在 `config` 被 move 进 Self 之前求值：`PI_CHAT_SHOW_THINKING` 已设置时优先环境变量，
        // 否则回落到 `config.llm.thinking.show`（架构 §3.1 / §1 已决策「`show` 初值来源」）。
        let initial_show_thinking = resolve_initial_show_thinking(&config.llm.thinking);

        // T2-P1-002 PR-PLA：PlanRuntime per-session，与 session_key 绑定。
        let plan_runtime = plan_runtime::PlanRuntime::new(session.current_session_key());
        if let Err(err) = plan_runtime.recover() {
            warn!(error = %err, "plan_runtime recover failed; continuing with Chat mode");
        }

        Ok(Self {
            session,
            llm,
            config,
            primitive,
            tool_registry,
            event_bus,
            audit,
            checkpoint_switcher,
            checkpoint_store,
            cancel_token,
            last_interrupt_at,
            agent_workspace_dir,
            agent_definition_dir,
            agent_trail_dir,
            cfg_path: cfg_path_snapshot,
            session_grants,
            config_backend,
            bash_task_registry,
            gate,
            read_file_state: Arc::new(
                crate::core::tools::pipeline::read_state::ReadFileState::default(),
            ),
            show_thinking: Arc::new(std::sync::atomic::AtomicBool::new(initial_show_thinking)),
            openai_files_runtime,
            plan_runtime,
        })
    }

    fn effective_model(&self, entry: Option<&SessionEntry>) -> String {
        entry
            .and_then(|e| e.model_override.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.config.llm.default_model)
            .to_string()
    }
}

// ─── CLI UserConfirmationProvider ─────────────────────────────────────────────

use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;

pub struct CliConfirmation;

#[async_trait::async_trait]
impl UserConfirmationProvider for CliConfirmation {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        println!("\n--- 操作确认 ---");
        let source_label = if plugin_id == "__agent__" {
            "host".to_string()
        } else {
            plugin_id.to_string()
        };
        println!("类型: {:?}  来源: {}", operation, source_label);
        if !preview.is_empty() {
            let lines: Vec<&str> = preview.lines().collect();
            let display = if lines.len() > 20 {
                format!(
                    "{}\n  ... ({} 行已省略)",
                    lines[..20].join("\n"),
                    lines.len() - 20
                )
            } else {
                preview.to_string()
            };
            println!("预览:\n{}", display);
        }
        print!("是否执行？[y/N] ");
        io::stdout().flush().map_err(AppError::Io)?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).map_err(AppError::Io)?;
        let answer = line.trim().to_lowercase();
        Ok(answer == "y" || answer == "yes")
    }

    async fn confirm_decision(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        suggested_root: Option<std::path::PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        if operation == PrimitiveOperation::Bash {
            return match self.confirm(operation, preview, plugin_id).await? {
                true => Ok(ConfirmDecision::AllowOnce),
                false => Ok(ConfirmDecision::Deny),
            };
        }

        let target = extract_path_from_preview(preview).unwrap_or_else(|| {
            suggested_root
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        });
        match permission::prompt::read_path_prompt(
            &target,
            suggested_root,
            Some(&format!("类型: {:?}  来源: {}", operation, plugin_id)),
        )
        .map_err(AppError::Io)?
        {
            permission::prompt::PathPromptChoice::AllowSession => Ok(ConfirmDecision::AllowOnce),
            permission::prompt::PathPromptChoice::PersistWorkspaceRoot { root } => {
                let cfg_path = crate::api::cli::config_file_path()?;
                crate::infra::config::append_workspace_root_to_disk(
                    &cfg_path,
                    root.to_string_lossy().into_owned(),
                )?;
                Ok(ConfirmDecision::AllowAndPersistRoot { root })
            }
            permission::prompt::PathPromptChoice::Cancel => Ok(ConfirmDecision::Deny),
        }
    }
}

fn extract_path_from_preview(preview: &str) -> Option<std::path::PathBuf> {
    preview
        .lines()
        .find_map(|line| line.strip_prefix("路径: "))
        .map(std::path::PathBuf::from)
}

// ─── NoopToolExecutor ─────────────────────────────────────────────────────────

struct NoopToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for NoopToolExecutor {
    async fn execute(
        &self,
        tool: &Tool,
        _params: serde_json::Value,
        _caller_plugin_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        Err(AppError::Tool(format!(
            "对话模式下不支持插件工具执行: {}",
            tool.name
        )))
    }
}

// ─── Tool definitions for LLM ─────────────────────────────────────────────────

fn build_tool_definitions(ctx: &ChatContext) -> Vec<serde_json::Value> {
    // T2-P1-002 PR-PLA：按 PlanMode 过滤 LLM 可见工具集。
    // - Chat / Pending / Completed：等价默认视图（含 chat 默认工具，排除 plan_only）
    // - Planning：包含 create_plan / ask_question / todos / update_plan，排除 write/edit/bash
    // - Executing：含 update_plan / todos，排除 create_plan / ask_question
    plan_runtime::catalog::visible_tools_for_mode(&ctx.plan_runtime.mode())
}

// ─── Workspace state for system prompt（plan §8 / PR-8） ─────────────────────

/// 把 `ChatContext` 的 workspace 配置 + session_grants 合并成
/// `system_prompt::WorkspaceState`，喂给 [`build_system_prompt_with_state`]。
///
/// 直接读 `ctx.gate.effective_roots()` / `effective_path_rules()`，与 executor /
/// 路径授权 UI 共享同一份决策视图。
fn compute_workspace_state(ctx: &ChatContext) -> crate::core::llm::system_prompt::WorkspaceState {
    use crate::core::llm::system_prompt::{
        PathRuleSummary, WorkspaceRootDescriptor, WorkspaceState,
    };
    use crate::core::permission::PathRuleMode;
    use std::collections::HashSet;

    let cfg = &ctx.config;
    let agent_definition_dir = ctx.agent_definition_dir.clone();
    let workspace_roots = resolve_workspace_roots_paths(cfg).unwrap_or_default();

    let agent_trail_readonly_dirs: Vec<std::path::PathBuf> = vec![
        Some(ctx.agent_trail_dir.clone()),
        crate::infra::config::resolve_sessions_dir(cfg).ok(),
        crate::infra::config::resolve_log_dir(cfg).ok(),
        crate::infra::config::resolve_audit_dir(cfg).ok(),
        crate::infra::config::resolve_agent_dir(cfg).ok(),
    ]
    .into_iter()
    .flatten()
    .collect();

    // workspace.entries 中的 alias / description（按 path 索引以便回填）。
    let mut entry_meta: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();
    for e in &cfg.workspace.entries {
        if !e.path.trim().is_empty() {
            let key = crate::infra::platform::normalize_path(&e.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| e.path.clone());
            entry_meta.insert(key, (e.alias.clone(), e.description.clone()));
        }
    }

    // ── read_write 列表 ──
    let agent_definition_canon = agent_definition_dir.to_string_lossy().to_string();
    let workspace_root_set: HashSet<String> = workspace_roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let session_set: HashSet<String> = ctx
        .session_grants
        .snapshot()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let er = ctx.gate.effective_roots();
    let mut read_write: Vec<WorkspaceRootDescriptor> = Vec::new();
    let mut seen_rw: HashSet<String> = HashSet::new();
    for p in er.read_write {
        let s = p.to_string_lossy().to_string();
        if !seen_rw.insert(s.clone()) {
            continue;
        }
        let label = if s == agent_definition_canon {
            "agent_definition_dir"
        } else if workspace_root_set.contains(&s) {
            "agent_workspace_root"
        } else if session_set.contains(&s) {
            "session_grant"
        } else {
            "workspace_root"
        };
        let (alias, description) = entry_meta.get(&s).cloned().unwrap_or((None, None));
        read_write.push(WorkspaceRootDescriptor {
            path: s,
            label: label.to_string(),
            alias,
            description,
        });
    }

    // ── read_only 列表 ──
    let mut read_only: Vec<WorkspaceRootDescriptor> = Vec::new();
    let mut seen_ro: HashSet<String> = HashSet::new();
    let agent_trail_set: HashSet<String> = agent_trail_readonly_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    for p in er.read_only {
        let s = p.to_string_lossy().to_string();
        if !seen_ro.insert(s.clone()) {
            continue;
        }
        let label = if agent_trail_set.contains(&s) {
            "agent_trail_dir"
        } else {
            "path_rule_readonly"
        };
        read_only.push(WorkspaceRootDescriptor {
            path: s,
            label: label.to_string(),
            alias: None,
            description: None,
        });
    }

    // ── path_rules（builtin + user ∪ session） ──
    let user_paths: HashSet<String> = cfg
        .primitive
        .path_rules
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let mut path_rules: Vec<PathRuleSummary> = Vec::new();
    for r in ctx.gate.effective_path_rules() {
        path_rules.push(PathRuleSummary {
            path: r.path.clone(),
            mode: match r.mode {
                PathRuleMode::Deny => "deny".to_string(),
                PathRuleMode::Readonly => "readonly".to_string(),
            },
            builtin: !user_paths.contains(&r.path),
        });
    }

    WorkspaceState {
        read_write,
        read_only,
        path_rules,
    }
}

// ─── Main chat loop ───────────────────────────────────────────────────────────

pub async fn chat_loop(ctx: &ChatContext, resume: bool) -> Result<(), AppError> {
    ensure_session(ctx)?;

    let entry = ctx.session.get_session(ctx.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());

    if resume {
        println!("恢复会话: {}", ctx.session.current_session_key());
    }
    println!("tomcat 对话模式 (模型: {})", model);
    println!("输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。");
    println!("输入 /help 查看命令列表。\n");

    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))?;

    let search_tools_printer = rl.create_external_printer().ok().map(|p| {
        Arc::new(std::sync::Mutex::new(
            Box::new(p) as Box<dyn rustyline::ExternalPrinter + Send>
        ))
    });

    // ContextState: 在 loop 外一次性初始化，跨迭代复用
    let context_config = &ctx.config.context;
    let workspace_context = crate::core::llm::system_prompt::WorkspaceContext {
        agent_workspace_dir: ctx.agent_workspace_dir.to_string_lossy().to_string(),
        agent_definition_dir: ctx.agent_definition_dir.to_string_lossy().to_string(),
        agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
    };
    let workspace_state = compute_workspace_state(ctx);
    let system_text = crate::core::llm::system_prompt::build_system_prompt_with_state(
        workspace_context,
        workspace_state,
    );
    schedule_checkpoint_prune(ctx);
    if let Some(path) = ctx.session.current_transcript_path()? {
        let tail = read_entries_tail(&path, 64).unwrap_or_default();
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &tail);
    } else {
        let _ = crate::core::compute_resume_plan(entry.as_ref(), &[]);
    }
    let mut context_state = init_context_state(&ctx.session, context_config, &system_text)?;
    let session_stderr_ids = events::stderr::register_chat_session_stderr_listeners(
        &*ctx.event_bus,
        search_tools_printer,
    );
    preflight::start_search_tools_preflight(&ctx.config, ctx.event_bus.clone());
    preflight::start_git_preflight(
        &ctx.config,
        ctx.event_bus.clone(),
        ctx.checkpoint_switcher.clone(),
    );

    loop {
        let input = match rl.readline("u> ") {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("\n再见！");
                context_state.preheat.abort();
                break;
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                continue;
            }
            Err(e) => {
                eprintln!("输入错误: {}", e);
                context_state.preheat.abort();
                break;
            }
        };

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        // 聊天命令解析：目前支持 `/path` 和 `/help`；其它输入进入普通 LLM 回合。
        let input = match dispatch_chat_command(ctx, parse_chat_command(&input), &mut rl) {
            ChatCommandOutcome::Continue { line } => line,
            ChatCommandOutcome::Handled => continue,
        };

        let _ = rl.add_history_entry(&input);

        // 读到新输入后重建 CancellationToken。
        // 关键约束：token 一旦 cancel 不可逆——如果用户 Ctrl+C 落在 prompt 处，
        // 旧 token 已被 cancel；这里替换成新 token，新回合才能正常运行。
        // 必须在 `agent_loop.run` 之前完成。
        let turn_token = {
            let mut guard = ctx.cancel_token.lock();
            *guard = CancellationToken::new();
            guard.clone()
        };

        let entry = ctx.session.get_session(ctx.session.current_session_key())?;
        let model = ctx.effective_model(entry.as_ref());

        // Update context estimate for the new user input
        context_state.on_message_appended(input.len());
        info!(
            target: "tomcat_chat_diag",
            phase = "chat_after_user_append",
            ratio = context_state.usage_ratio(),
            compaction_count = context_state.session_obs.compaction_count,
            turns = context_state.turn_count()
        );

        // Timing ②: restore pending preheat + apply boundary before request
        context_state.preheat.try_restart_if_pending(
            context_state.usage_ratio(),
            &context_state.messages,
            &context_state.transcript_path,
            ctx.llm.clone(),
            context_config,
            ctx.event_bus.clone(),
        );
        check_before_request(&mut context_state, &*ctx.event_bus).await;
        info!(
            target: "tomcat_chat_diag",
            phase = "chat_after_timing2_check",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = false,
            ratio = context_state.usage_ratio(),
            compaction_count = context_state.session_obs.compaction_count
        );

        // Build messages from ContextState
        let mut messages = build_context_from_state(&context_state);

        // T2-P1-002 PR-PLA：按当前 PlanMode 决定 system reminder / tool_definitions / user prefix
        let plan_mode = ctx.plan_runtime.mode();
        let system_text_with_reminder = match &plan_mode {
            plan_runtime::PlanMode::Planning => {
                format!("{}{}", system_text, plan_runtime::prompts::PLANNER_REMINDER)
            }
            plan_runtime::PlanMode::Executing { plan_id } => format!(
                "{}{}",
                system_text,
                plan_runtime::prompts::render_executor_reminder(plan_id)
            ),
            _ => system_text.clone(),
        };
        let user_prefix = plan_runtime::session_prefix::user_prefix_for_mode(&plan_mode);
        let decorated_user_text = if user_prefix.is_empty() {
            input.clone()
        } else {
            format!("{}{}", user_prefix, input)
        };

        messages.insert(0, ChatMessage::system(&system_text_with_reminder));
        messages.push(ChatMessage::user(&decorated_user_text));

        let renderer = Arc::new(parking_lot::Mutex::new(MarkdownRenderer::new()));
        let config = AgentLoopConfig {
            max_attempts: 3,
            max_tool_rounds: usize::MAX,
            retry_base_delay_ms: 300,
            model: model.clone(),
            session_id: ctx.session.current_session_key().to_string(),
            tool_definitions: build_tool_definitions(ctx),
            context_config: context_config.clone(),
            agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
            read_file_state: ctx.read_file_state.clone(),
            openai_files_runtime: ctx.openai_files_runtime.clone(),
            checkpoint_store: ctx.checkpoint_store.clone(),
            parent_session_id: None,
            spawn_depth: 0,
            subagent_type: crate::core::agent_loop::SubagentType::User,
        };
        let mut agent_loop = AgentLoop::new(
            ctx.llm.clone(),
            ctx.primitive.clone(),
            ctx.event_bus.clone(),
            config,
            turn_token,
        );
        if let Some(backend) = ctx.config_backend.clone() {
            agent_loop = agent_loop.with_config_backend(backend);
        }
        // T2-P0-016 PR-I：注入 bash 后台任务注册表，启用 task_* 三件套。
        agent_loop = agent_loop.with_bash_task_registry(ctx.bash_task_registry.clone());
        agent_loop.set_context_state(Some(context_state));

        // T2-P0-006 P0：CLI 单订阅者渲染器，统一处理 thinking / 正文 / tool_execution，
        // 避免多个回调各自 `print!` 引起乱序。
        let cli_turn_renderer = cli_turn_renderer::CliTurnRenderer::new(
            Arc::clone(&renderer),
            Arc::clone(&ctx.show_thinking),
            ctx.config.llm.thinking.print_to_stderr,
            ctx.config.llm.tool_cli_verbosity,
        );
        let listener_ids = cli_turn_renderer.register(&*ctx.event_bus);
        let thinking_persist_listener_ids = if ctx.config.llm.thinking.persist {
            let transcript_path = ctx
                .session
                .current_transcript_path()?
                .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
            Some(register_thinking_persist_listeners(
                &*ctx.event_bus,
                transcript_path,
            ))
        } else {
            None
        };

        print!("\ntomcat.{}> ", ctx.config.agent.id);
        io::stdout().flush().map_err(AppError::Io)?;

        info!(
            target: "tomcat_chat_diag",
            phase = "chat_before_agent_run",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = true
        );
        let outcome = agent_loop.run(messages).await;
        if let Some(ids) = &thinking_persist_listener_ids {
            unregister_thinking_persist_listeners(&*ctx.event_bus, ids);
        }
        cli_turn_renderer::CliTurnRenderer::unregister(&*ctx.event_bus, &listener_ids);

        // T-004 / T-017：`Completed` 与 `Interrupted` 走**同一条**持久化路径——
        // partial assistant（content_buf 截短处）+ 已完成的 tool_result 都已被
        // `AgentLoop::run` 装进 `AgentRunResult.new_messages`，这里只需 append +
        // observability，不区分成功与中断。
        let (maybe_result, was_interrupted, maybe_error) = match outcome {
            AgentRunOutcome::Completed(r) => (Some(r), false, None),
            AgentRunOutcome::Interrupted(r) => (Some(r), true, None),
            AgentRunOutcome::Failed(e) => (None, false, Some(e)),
        };

        if let Some(remaining) = renderer.lock().flush() {
            print!("{}", remaining);
            let _ = io::stdout().flush();
        }

        context_state = agent_loop.take_context_state().unwrap_or_else(|| {
            init_context_state(&ctx.session, context_config, &system_text).unwrap_or(
                crate::core::ContextState {
                    messages: Vec::new(),
                    estimate_context_chars: system_text.len(),
                    context_budget_chars: crate::infra::config::compute_context_budget_chars(
                        context_config,
                    ),
                    context_budget_tokens: context_config
                        .context_window
                        .saturating_sub(context_config.max_output_tokens),
                    last_api_usage: None,
                    post_usage_appended_chars: 0,
                    transcript_path: ctx
                        .session
                        .current_transcript_path()
                        .ok()
                        .flatten()
                        .unwrap_or_default(),
                    preheat: Preheat::new(),
                    session_obs: Default::default(),
                    live: Default::default(),
                },
            )
        });

        if let Some(result) = maybe_result {
            persist_turn_result(
                ctx,
                &mut context_state,
                result.new_messages,
                if was_interrupted {
                    CheckpointKind::Interrupt
                } else {
                    CheckpointKind::TurnEnd
                },
            )?;

            if was_interrupted {
                eprintln!("\n^C 已中断（partial 已保存）");
            }
        } else if let Some(e) = maybe_error {
            let _ = ctx.session.persist_context_observability(&context_state);

            let is_fatal = is_fatal_error(&e);
            eprintln!("\n[错误] {}", e);
            if is_fatal {
                eprintln!("(致命错误，退出对话)");
                context_state.preheat.abort();
                cleanup_openai_files_on_session_end(ctx, "chat_fatal_exit").await;
                events::stderr::unregister_chat_session_stderr_listeners(
                    &*ctx.event_bus,
                    &session_stderr_ids,
                );
                return Err(e);
            }
            eprintln!("(可重试，请继续输入)\n");
            continue;
        }

        println!();
    }

    cleanup_openai_files_on_session_end(ctx, "session_end").await;
    events::stderr::unregister_chat_session_stderr_listeners(&*ctx.event_bus, &session_stderr_ids);
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// 判断错误是否致命（配置缺失等不可恢复场景）；API/网络错误为非致命。
fn is_fatal_error(e: &AppError) -> bool {
    matches!(e, AppError::Config(_))
}

fn schedule_checkpoint_prune(ctx: &ChatContext) {
    let store = ctx.checkpoint_store.clone();
    let retention = crate::core::RetentionPolicy {
        retention_max: ctx.config.checkpoint.retention_max,
        retention_days: ctx.config.checkpoint.retention_days,
    };
    std::thread::spawn(move || {
        if let Err(err) = store.prune(retention) {
            warn!(error = %err, "checkpoint prune failed");
        }
    });
}

fn persist_turn_result(
    ctx: &ChatContext,
    context_state: &mut crate::core::ContextState,
    new_messages: Vec<crate::core::llm::ChatMessage>,
    kind: CheckpointKind,
) -> Result<Vec<String>, AppError> {
    let mut appended_row_ids = Vec::new();
    for msg in new_messages {
        // T2-P1-002 PR-PLA / D5：transcript 中 user 文本不写 mode prefix（避免 hydrate 后双重贴前缀）。
        let mut msg = msg;
        if matches!(msg.role, crate::core::llm::ChatMessageRole::User) {
            if let Some(content) = msg.text_content() {
                let stripped = plan_runtime::session_prefix::strip_user_prefix(content);
                if stripped.len() != content.len() {
                    msg.set_text_content(stripped.to_string());
                }
            }
        }
        let row_id = ctx.session.append_message(serde_json::to_value(&msg)?)?;
        let mut cm = msg;
        cm.msg_id = Some(row_id);
        appended_row_ids.push(cm.msg_id.clone().unwrap_or_default());
        context_state.messages.push(cm);
    }
    ctx.session.persist_context_observability(context_state)?;
    maybe_record_turn_checkpoint(ctx, kind, &appended_row_ids);
    Ok(appended_row_ids)
}

fn maybe_record_turn_checkpoint(
    ctx: &ChatContext,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) {
    let Some(request) =
        build_turn_checkpoint_request(ctx.session.current_session_key(), kind, appended_row_ids)
    else {
        return;
    };
    if let Err(err) = ctx.checkpoint_store.record(request) {
        warn!(error = %err, "checkpoint record failed");
    }
}

fn build_turn_checkpoint_request(
    session_id: &str,
    kind: CheckpointKind,
    appended_row_ids: &[String],
) -> Option<CheckpointRecordRequest> {
    let (Some(start_id), Some(end_id)) = (appended_row_ids.first(), appended_row_ids.last()) else {
        return None;
    };
    Some(CheckpointRecordRequest {
        session_id: session_id.to_string(),
        turn_id: compound_turn_id(start_id, end_id),
        kind,
        message_anchor: Some(end_id.clone()),
        notes: None,
    })
}

/// CLI 启动时 `show_thinking` 初值的来源裁决：
///
/// 优先级 `PI_CHAT_SHOW_THINKING` env（已设置）> `config.llm.thinking.show`。
/// env 取值与历史一致：`1 / true / yes / on`（大小写）当作 true，其余字符串当作 false。
/// env **未设置**时直接读 toml 中的 `show`，让用户配置不被静默忽略。
///
/// 见架构 §3.1 G5 与计划 §1 已决策「`show` 初值来源」。
fn resolve_initial_show_thinking(thinking: &crate::infra::config::ThinkingConfig) -> bool {
    match std::env::var("PI_CHAT_SHOW_THINKING") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "True" | "yes" | "on"),
        Err(_) => thinking.show,
    }
}

#[derive(Default)]
struct ThinkingPersistState {
    text: String,
    signature: Option<String>,
}

struct ThinkingPersistListenerIds {
    msg_update: crate::infra::event_bus::EventListenerId,
    msg_end: crate::infra::event_bus::EventListenerId,
}

fn register_thinking_persist_listeners(
    bus: &dyn EventBus,
    transcript_path: std::path::PathBuf,
) -> ThinkingPersistListenerIds {
    let state = Arc::new(Mutex::new(ThinkingPersistState::default()));

    let state_for_update = Arc::clone(&state);
    let msg_update = bus.on(
        crate::infra::wire::WIRE_MESSAGE_UPDATE,
        Box::new(move |evt: crate::infra::event_bus::EventContext| {
            let event = match evt.payload.get("assistantMessageEvent") {
                Some(e) => e,
                None => return Ok(()),
            };
            if event.get("kind").and_then(|v| v.as_str()) != Some("thinking_delta") {
                return Ok(());
            }
            let delta = event.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if delta.is_empty() {
                return Ok(());
            }
            let mut st = state_for_update.lock();
            st.text.push_str(delta);
            if let Some(sig) = event.get("signature").and_then(|v| v.as_str()) {
                st.signature = Some(sig.to_string());
            }
            Ok(())
        }),
    );

    let state_for_end = Arc::clone(&state);
    let msg_end = bus.on(
        crate::infra::wire::WIRE_MESSAGE_END,
        Box::new(move |_evt: crate::infra::event_bus::EventContext| {
            let (text, signature) = {
                let mut st = state_for_end.lock();
                if st.text.is_empty() {
                    return Ok(());
                }
                (std::mem::take(&mut st.text), st.signature.take())
            };
            let entry = crate::core::session::TranscriptEntry::ThinkingTrace(
                crate::core::session::ThinkingTraceEntry {
                    id: None,
                    parent_id: None,
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    text,
                    signature,
                },
            );
            if let Err(e) = crate::core::session::append_entry(&transcript_path, &entry) {
                warn!(error = %e, "append thinking_trace entry failed");
            }
            Ok(())
        }),
    );

    ThinkingPersistListenerIds {
        msg_update,
        msg_end,
    }
}

fn unregister_thinking_persist_listeners(bus: &dyn EventBus, ids: &ThinkingPersistListenerIds) {
    bus.off(ids.msg_update);
    bus.off(ids.msg_end);
}

fn ensure_session(ctx: &ChatContext) -> Result<(), AppError> {
    let key = ctx.session.current_session_key();
    if ctx.session.get_session(key)?.is_none() {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        ctx.session.create_session(key, cwd)?;
    }
    Ok(())
}

async fn cleanup_openai_files_on_session_end(ctx: &ChatContext, reason: &str) {
    let Some(runtime) = ctx.openai_files_runtime.as_ref() else {
        return;
    };
    let summary = runtime.cleanup_registered_files(reason).await;
    if summary.total == 0 {
        return;
    }
    if summary.failed > 0 {
        warn!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            failed = summary.failed,
            "openai files cleanup finished with failures"
        );
    } else {
        info!(
            reason = reason,
            total = summary.total,
            deleted = summary.deleted,
            "openai files cleanup completed"
        );
    }
}

fn migrate_legacy_layer0_tool_results(
    agent_definition_dir: &std::path::Path,
    agent_trail_dir: &std::path::Path,
) {
    let legacy_root = agent_definition_dir.join("workspace");
    if !legacy_root.exists() {
        return;
    }
    let target_root = agent_trail_dir.join("tool-results");
    if let Ok(entries) = std::fs::read_dir(&legacy_root) {
        let _ = std::fs::create_dir_all(&target_root);
        for entry in entries.flatten() {
            let from = entry.path();
            let name = entry.file_name();
            let to = target_root.join(name);
            if to.exists() {
                continue;
            }
            let _ = std::fs::rename(&from, &to);
        }
    }
}
