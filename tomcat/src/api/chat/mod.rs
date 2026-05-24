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
    /// P1（bash background monitor）：session 级 `follow_up_queue`，跨 turn 共享。
    /// `run_chat_turn` 通过 `AgentLoop::with_shared_follow_up_queue(...)` 注入；
    /// host lifecycle subscriber 在后台 shell 完成时把 synthetic notification 推入此 queue。
    pub follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    /// P1：claim-on-entry 模型的 `task_id → CompletionRoute` 路由表。
    /// dispatcher（`task_output(block=true)`）与 host lifecycle subscriber
    /// 共享同一把锁串行化交付决策，杜绝双回灌 TOCTOU race。
    pub completion_routes: crate::core::agent_loop::BackgroundCompletionRoutes,
    /// P1：lifecycle subscriber → chat_loop 主循环的唤醒信号；当 host 推入
    /// synthetic notification 后 `notify_one()`，让主循环在 between-turns drain
    /// 路径上立即看到 queue 非空。
    pub follow_up_signal: Arc<tokio::sync::Notify>,
    /// P1：host 内部去重——已 push synthetic 的 `task_id` 集合。即便
    /// `completion_routes` 已经通过锁挡掉，这里再兜一道，覆盖
    /// "broadcast 多 receiver / stop+wait 双触发"等极端情况。
    pub delivered_completion: Arc<Mutex<std::collections::HashSet<crate::core::tools::primitive::BashTaskId>>>,
    /// P1：lifecycle subscriber 后台守护 task 的 abort handle。
    /// `chat_loop` 启动前 spawn，`ChatContext::shutdown_completion_subscriber` /
    /// drop 时 abort，避免泄漏。
    pub completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
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
    /// 多 Agent 派生注册表（multi-agent.md §14）。reviewer 子 Agent 通过
    /// `agent_registry.spawn_subagent_internal` 派发；顶层 chat session 启动时
    /// 注册 root handle，guard 与 ctx 同生命周期（drop 时自动注销）。
    pub agent_registry: Arc<crate::core::agent_registry::AgentRegistry>,
    /// root agent handle 的注销 guard；drop 时自动从 `agent_registry` 移除。
    _root_agent_guard: crate::core::agent_registry::RegistrationGuard,
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
        let sessions_path_for_appender = sessions_path.clone();
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);
        let session_cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        let current_session_entry = session.ensure_current_session(session_cwd.clone())?;

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
        // P1（bash background monitor）：session 级共享对象。
        // - `follow_up_queue`：lifecycle subscriber → AgentLoop 一层 conv loop 的纸条信箱。
        // - `completion_routes`：claim-on-entry race-free 模型的状态表。
        // - `follow_up_signal`：lifecycle subscriber → chat_loop 主循环的唤醒信号。
        // - `delivered_completion`：host 内部 broadcast 去重 guard。
        let follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>> =
            Arc::new(Mutex::new(Vec::new()));
        let completion_routes: crate::core::agent_loop::BackgroundCompletionRoutes =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let follow_up_signal = Arc::new(tokio::sync::Notify::new());
        let delivered_completion: Arc<
            Mutex<std::collections::HashSet<crate::core::tools::primitive::BashTaskId>>,
        > = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(None));

        // 在 `config` 被 move 进 Self 之前求值：`PI_CHAT_SHOW_THINKING` 已设置时优先环境变量，
        // 否则回落到 `config.llm.thinking.show`（架构 §3.1 / §1 已决策「`show` 初值来源」）。
        let initial_show_thinking = resolve_initial_show_thinking(&config.llm.thinking);

        // T2-P1-002 PR-PLA：PlanRuntime per-session，与 session_key 绑定。
        let plan_runtime = plan_runtime::PlanRuntime::new_with_session_id(
            session.current_session_key(),
            current_session_entry.session_id.clone(),
        );
        // T2-P1-* GAP-N12 / G3：把 ChatContext 已知的 plan/ask_question/todos 配置注入运行时。
        // 注意：这些 setter 都是幂等 atomic store；后续 hot-reload 也走同入口。
        // reviewer 改稿权 (`allow_review_edit`) 已硬编码为 true，不再有配置开关。
        plan_runtime.set_ask_question_timeout_ms(Some(config.ask_question.timeout_ms));
        plan_runtime.set_todos_persist_base(Some(agent_trail_dir.clone()));
        // build checkpoint 自动化。
        plan_runtime.set_auto_checkpoint_on_build(config.plan.auto_checkpoint_on_build);
        plan_runtime.set_verify_gate_mode(config.plan.verify_gate.clone());
        plan_runtime.set_max_code_review_rounds(config.plan.max_code_review_rounds);
        plan_runtime.attach_checkpoint_store(checkpoint_store.clone());
        // E：CLI 默认 panel——把 todos / update_plan 后的 snapshot 渲染到 stderr，
        // 让用户在 CLI 下也能感知 in_progress 切换；IDE 适配后可替换为 IpcTodosPanel。
        plan_runtime.register_todos_panel(Arc::new(plan_runtime::CliTodosPanel));
        // CLI 运行态默认也应提供 ask_question 面板；否则真实对话里模型一旦调用
        // ask_question，会直接得到 "PlanRuntime 未配置 AskQuestionPanel" 并卡死等待。
        plan_runtime.attach_ask_question_panel(Arc::new(
            plan_runtime::ask_question_panel::CliAskQuestionPanel,
        ));

        // ─── 多 Agent 注册表 + reviewer 子 Agent 真派发装配 ─────────────────
        // multi-agent.md §14：reviewer / 未来 dispatch_agent 走唯一构造点
        // `agent_registry.spawn_subagent_internal`。顶层 chat session 注册 root handle
        // 与 ctx 同生命周期；guard 落在 ChatContext._root_agent_guard，drop 时自动注销。
        let agent_registry =
            crate::core::agent_registry::AgentRegistry::new().attach_event_bus(event_bus.clone());
        let root_agent_guard = agent_registry
            .register_root(current_session_entry.session_id.clone())
            .map_err(|e| AppError::Config(format!("agent_registry root register 失败: {e}")))?;

        // reviewer max_turns 上限优先读 env（便于 CI 临时调）；env 未设 → ReviewerConfig.max_turns。
        let reviewer_max_turns = std::env::var("TOMCAT_REVIEWER_MAX_TURNS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(config.reviewer.max_turns);
        let reviewer_model = config
            .reviewer
            .model_override
            .clone()
            .unwrap_or_else(|| config.llm.default_model.clone());
        let read_file_state =
            Arc::new(crate::core::tools::pipeline::read_state::ReadFileState::default());
        let prod_reviewer = plan_runtime::prod_reviewer::ProdReviewerDispatcher::new(
            "chat_context",
            plan_runtime::prod_reviewer::ProdReviewerDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm: llm.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                read_file_state: read_file_state.clone(),
                openai_files_runtime: openai_files_runtime.clone(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model: reviewer_model,
                max_turns: reviewer_max_turns,
            },
        );
        plan_runtime.attach_reviewer(Arc::new(prod_reviewer));
        let prod_verifier = plan_runtime::verify::ProdVerifierDispatcher::new(
            "chat_context",
            plan_runtime::verify::ProdVerifierDeps {
                agent_registry: agent_registry.clone(),
                parent_session_id: current_session_entry.session_id.clone(),
                llm: llm.clone(),
                primitive: primitive.clone(),
                event_bus: event_bus.clone(),
                agent_trail_dir: agent_trail_dir.to_string_lossy().to_string(),
                checkpoint_store: checkpoint_store.clone(),
                context_config: config.context.clone(),
                read_file_state: read_file_state.clone(),
                openai_files_runtime: openai_files_runtime.clone(),
                agent_workspace_dir: agent_workspace_dir.clone(),
                plan_runtime: Arc::downgrade(&plan_runtime),
                model: config.llm.default_model.clone(),
            },
        );
        plan_runtime.attach_verifier(Arc::new(prod_verifier));

        // transcript 自定义事件 appender：把 SessionManager::append_custom_entry 包成闭包
        // 注入 PlanRuntime，dispatch_reviewer 完成后写 `plan.review` / `plan.review.warning`。
        // SessionManager 内部只持有 sessions_dir 路径，重新构造廉价；这样不需要把
        // ChatContext.session 改为 Arc。
        {
            plan_runtime.attach_transcript_appender(Arc::new(move |extra| {
                let sm = SessionManager::new(sessions_path_for_appender.clone());
                sm.append_custom_entry(extra)
            }));
        }

        // E8：装配完成后再 recover——确保扫盘失败也仅 warning，主流程继续以 Chat 启动。
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
            follow_up_queue,
            completion_routes,
            follow_up_signal,
            delivered_completion,
            completion_subscriber_handle,
            gate,
            read_file_state,
            show_thinking: Arc::new(std::sync::atomic::AtomicBool::new(initial_show_thinking)),
            openai_files_runtime,
            plan_runtime,
            agent_registry,
            _root_agent_guard: root_agent_guard,
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
    let agent_plans_dir = crate::infra::config::resolve_plans_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

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
        } else if agent_plans_dir.as_deref() == Some(&s) {
            "agent_plans_dir"
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

// ─── P1（bash background monitor）：lifecycle subscriber + between-turns drain ───

/// P1：每个真实用户输入之间最多连续 K 次 auto-turn，防止
/// "synthetic → LLM 启 bash → bash 立刻完成 → 又 synthetic …" 的风暴。
/// 详见 `docs/architecture/tools/bash.md` 与 plan §P1-D。
const AUTO_TURN_BUDGET: u32 = 8;

/// P1：把后台 shell 完成事件桥接到 follow_up_queue 的守护 task。
///
/// 启动时机：`chat_loop` 进入主循环之前；abort 时机：`ChatContext::shutdown_completion_subscriber` /
/// drop。
///
/// 行为见 plan §P1-D：
/// 1. 收到 lifecycle finished 事件；
/// 2. 在 `completion_routes` 上原子检查：若已 `ToolWillDeliver` / `Delivered`
///    → 丢弃（dispatcher 在交付/已交付）；否则 `insert(Delivered)`；
/// 3. 进 host 内部 `delivered_completion` 去重；
/// 4. 取 `tail_log(task_id, 4096)`；
/// 5. 构造 `<background-task-finished ...>...</background-task-finished>`
///    text，push 到 `follow_up_queue`；
/// 6. `follow_up_signal.notify_one()` 唤醒主循环（用于 between-turns 路径）。
fn spawn_completion_subscriber(ctx: &ChatContext) -> tokio::task::JoinHandle<()> {
    use crate::core::tools::primitive::{BackgroundTaskLifecycleEvent, BashTaskStatus};

    let registry = ctx.bash_task_registry.clone();
    let routes = ctx.completion_routes.clone();
    let queue = ctx.follow_up_queue.clone();
    let signal = ctx.follow_up_signal.clone();
    let delivered = ctx.delivered_completion.clone();

    let mut rx = registry.subscribe_lifecycle();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(BackgroundTaskLifecycleEvent {
                    task_id,
                    final_status,
                    log_path,
                    command,
                }) => {
                    // host 内部去重：保证同 task_id 一次会话内最多 push 一次。
                    {
                        let mut g = delivered.lock();
                        if g.contains(&task_id) {
                            continue;
                        }
                        g.insert(task_id.clone());
                    }
                    // claim-on-entry 状态机：dispatcher 已 claim/已 Delivered → 丢弃。
                    let should_push = {
                        let mut g = routes.lock();
                        match g.get(&task_id).copied() {
                            Some(crate::core::agent_loop::CompletionRoute::ToolWillDeliver)
                            | Some(crate::core::agent_loop::CompletionRoute::Delivered) => false,
                            _ => {
                                g.insert(
                                    task_id.clone(),
                                    crate::core::agent_loop::CompletionRoute::Delivered,
                                );
                                true
                            }
                        }
                    };
                    if !should_push {
                        continue;
                    }
                    let exit_code = match final_status {
                        BashTaskStatus::Finished { exit_code } => exit_code,
                        BashTaskStatus::Stopped => -1,
                        BashTaskStatus::Running => continue, // 不应到达
                    };
                    let tail = registry.tail_log(&task_id, 4096).await;
                    let text = format!(
                        "<background-task-finished task_id=\"{task_id}\" exit_code=\"{exit_code}\" log_path=\"{log_path}\" command=\"{cmd}\">\n{tail}\n</background-task-finished>",
                        task_id = task_id,
                        exit_code = exit_code,
                        log_path = log_path,
                        cmd = command.replace('"', "\\\""),
                    );
                    queue
                        .lock()
                        .push(crate::core::llm::ChatMessage::user(text));
                    signal.notify_one();
                    // CLI 提示一行（不打断 readline；终端会被 readline 重绘但提示已被记录）。
                    eprintln!(
                        "\n[bg] task {} finished (exit={}); queued for next turn.",
                        task_id, exit_code
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        target: "tomcat_chat_diag",
                        phase = "completion_subscriber_lagged",
                        skipped = n,
                        "lifecycle broadcast subscriber lagged; some events skipped"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

impl ChatContext {
    /// P1：abort lifecycle subscriber 守护 task。`chat_loop` 退出时调用一次；
    /// `Drop` 也兜底一次，防止泄漏。幂等。
    fn shutdown_completion_subscriber(&self) {
        if let Some(handle) = self.completion_subscriber_handle.lock().take() {
            handle.abort();
        }
    }
}

impl Drop for ChatContext {
    fn drop(&mut self) {
        self.shutdown_completion_subscriber();
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
        agent_plans_dir: plan_runtime::file_store::plans_dir()
            .map(|path| crate::infra::platform::format_home_path(&path))
            .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
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

    // P1（bash background monitor）：在主循环之前 spawn lifecycle subscriber。
    // 它会把后台 shell 完成事件桥接成 synthetic notification 推入 session 级
    // follow_up_queue；主循环每轮结束后做 between-turns drain。退出时
    // shutdown_completion_subscriber 兜底（同时由 Drop 再保险一次）。
    if ctx.completion_subscriber_handle.lock().is_none() {
        let handle = spawn_completion_subscriber(ctx);
        *ctx.completion_subscriber_handle.lock() = Some(handle);
    }

    // P1：每个真实用户输入之间累计的 auto-turn 次数，超过 AUTO_TURN_BUDGET
    // 后强制回 readline，避免 synthetic 风暴。
    let mut auto_turn_count: u32 = 0;

    loop {
        // P1：between-turns drain——若 follow_up_queue 非空且 auto-turn 预算仍有，
        // 跳过 readline 直接以 input="" 触发下一轮 run_chat_turn（一层 conv loop
        // 会 drain queue 注入 synthetic message）。否则走正常 readline 路径，并清零
        // auto-turn 计数。
        let auto_drain: bool = {
            let qlen = ctx.follow_up_queue.lock().len();
            qlen > 0 && auto_turn_count < AUTO_TURN_BUDGET
        };
        if !auto_drain {
            // 风暴预算已耗尽且 queue 仍非空 → 提示并强制回 readline（用户可手动“回车”再驱动）。
            if auto_turn_count >= AUTO_TURN_BUDGET && !ctx.follow_up_queue.lock().is_empty() {
                eprintln!(
                    "\n[bg] auto-turn budget exhausted ({}); falling back to user input.",
                    AUTO_TURN_BUDGET
                );
            }
            auto_turn_count = 0;
        }

        let input = if auto_drain {
            String::new()
        } else {
            let raw = match rl.readline("u> ") {
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
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                // 空回车：若 queue 有 synthetic，也走 auto-drain；否则再次进 readline。
                if !ctx.follow_up_queue.lock().is_empty() {
                    auto_turn_count = 0;
                } else {
                    continue;
                }
                String::new()
            } else {
                // 真实用户输入：聊天命令解析后走正常 LLM 回合。
                let parsed = match dispatch_chat_command(ctx, parse_chat_command(&trimmed), &mut rl)
                {
                    ChatCommandOutcome::Continue { line } => line,
                    ChatCommandOutcome::Handled => continue,
                };
                let _ = rl.add_history_entry(&parsed);
                parsed
            }
        };

        // P1：本轮是 auto-turn（input 为空且 queue 中有 synthetic）→ 计数 +1；
        // 真实用户输入清零计数。计数用于风暴防护（AUTO_TURN_BUDGET=K=8）。
        if input.is_empty() {
            auto_turn_count += 1;
        } else {
            auto_turn_count = 0;
        }

        // 读到新输入后重建 CancellationToken。
        // 关键约束：token 一旦 cancel 不可逆——如果用户 Ctrl+C 落在 prompt 处，
        // 旧 token 已被 cancel；这里替换成新 token，新回合才能正常运行。
        // 必须在 `run_chat_turn` 之前完成；SIGINT handler 通过 `ctx.cancel_token`
        // 看到新 token，从而只取消本回合而不污染未来回合。
        let turn_token = {
            let mut guard = ctx.cancel_token.lock();
            *guard = CancellationToken::new();
            guard.clone()
        };

        let outcome =
            run_chat_turn(ctx, &input, &system_text, &mut context_state, turn_token).await?;

        match outcome {
            AgentRunOutcome::Completed(_) => {}
            AgentRunOutcome::Interrupted(_) => {
                eprintln!("\n^C 已中断（partial 已保存）");
            }
            AgentRunOutcome::Failed(e) => {
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
        }

        println!();
    }

    cleanup_openai_files_on_session_end(ctx, "session_end").await;
    events::stderr::unregister_chat_session_stderr_listeners(&*ctx.event_bus, &session_stderr_ids);
    Ok(())
}

// ─── Single-turn driver ──────────────────────────────────────────────────────

/// 驱动单轮 chat 推理（不含 rustyline / `dispatch_chat_command`），供 `chat_loop`
/// 与集成 / 真 LLM E2E 测试共用。
///
/// 调用方需自备 `ContextState`（首轮用 [`init_context_state`] 构造、跨轮复用），
/// 并预先 build 好 `system_text`（避免每轮重算 `compute_workspace_state`）。
/// `turn_token` 由调用方持有；CLI 路径在 `chat_loop` 中将其与 `ctx.cancel_token`
/// 同步（让 SIGINT handler 取消当前回合）。
///
/// 本函数承担 `chat_loop` 原内层循环的全部副作用：preheat / boundary check /
/// PlanMode prompt 装配 / AgentLoop 装配与渲染器注册 / `AgentLoop::run` /
/// outcome 持久化（`persist_turn_result` 或 `persist_context_observability`）。
/// 错误打印 / fatal 退出 / `^C 已中断` 提示由调用方根据返回的 `AgentRunOutcome` 处理。
pub async fn run_chat_turn(
    ctx: &ChatContext,
    input: &str,
    system_text: &str,
    context_state: &mut crate::core::ContextState,
    turn_token: CancellationToken,
) -> Result<AgentRunOutcome, AppError> {
    // T2-P1-* C：PlanRuntime 监听本轮 token——一旦 Ctrl+C 触发，runtime 在 EXEC
    // 模式下会自动 demote 到 Pending，写盘 frontmatter.mode=pending 并清 active id。
    ctx.plan_runtime.attach_cancel_hook(turn_token.clone());

    let entry = ctx.session.get_session(ctx.session.current_session_key())?;
    let model = ctx.effective_model(entry.as_ref());
    let session_id = ctx
        .session
        .current_session_id()?
        .ok_or_else(|| AppError::Config("无当前会话".to_string()))?;
    let context_config = &ctx.config.context;

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
    check_before_request(context_state, &*ctx.event_bus).await;
    info!(
        target: "tomcat_chat_diag",
        phase = "chat_after_timing2_check",
        session_stderr_listeners_active = true,
        message_stream_listener_registered = false,
        ratio = context_state.usage_ratio(),
        compaction_count = context_state.session_obs.compaction_count
    );

    let mut messages = build_context_from_state(context_state);

    // T2-P1-002 PR-PLA：按当前 PlanMode 决定 system reminder / tool_definitions / user prefix
    let plan_mode = ctx.plan_runtime.mode();
    let system_text_with_reminder = match &plan_mode {
        plan_runtime::PlanMode::Planning => {
            format!(
                "{}{}",
                system_text,
                &*plan_runtime::prompts::PLANNER_REMINDER
            )
        }
        plan_runtime::PlanMode::Executing { plan_id } => format!(
            "{}{}",
            system_text,
            plan_runtime::prompts::render_executor_reminder(plan_id)
        ),
        _ => system_text.to_string(),
    };
    let current_plan_path = match &plan_mode {
        plan_runtime::PlanMode::Planning | plan_runtime::PlanMode::Executing { .. } => {
            ctx.plan_runtime.active_plan_path().or_else(|| {
                ctx.plan_runtime
                    .active_planning_plan_id()
                    .and_then(|plan_id| plan_runtime::file_store::plan_path_for_id(&plan_id).ok())
            })
        }
        plan_runtime::PlanMode::Chat
        | plan_runtime::PlanMode::Pending { .. }
        | plan_runtime::PlanMode::Completed { .. } => None,
    };
    let user_prefix = plan_runtime::session_prefix::user_prefix_for_mode(
        &plan_mode,
        current_plan_path.as_deref(),
    );
    let decorated_user_text = if user_prefix.is_empty() {
        input.to_string()
    } else {
        format!("{}{}", user_prefix, input)
    };

    messages.insert(0, ChatMessage::system(&system_text_with_reminder));

    // T2-P1-* C：进入 EXEC 后的第一轮，先把 plan body 作为 system_meta user 消息注入，
    // 让 LLM 直接看到完整计划文本；后续轮次返回 None，避免重复注入超长计划。
    if matches!(plan_mode, plan_runtime::PlanMode::Executing { .. }) {
        if let Some(body) = ctx.plan_runtime.consume_first_exec_turn_user_meta() {
            messages.push(ChatMessage::user(format!(
                "<plan_meta>\n{body}\n</plan_meta>"
            )));
        }
    }
    if !input.is_empty() {
        messages.push(ChatMessage::user(&decorated_user_text));
    }
    // P1（bash background monitor）：在首轮 reasoning 之前 drain 一次 session 级
    // follow_up_queue。这条路径覆盖两种场景：
    // 1) 真实用户输入 + 上一轮跨 turn 之间已积压的 synthetic notification → 用户句之后追加；
    // 2) auto-turn（input=""）：无用户输入，messages 末尾仅有 system_meta 等，由
    //    drain 提供本轮的 user content（synthetic 是 ChatMessage::user）。
    // AgentLoop 一层 conv loop 仍保留 drain 行为，覆盖"turn 内新到达"的 synthetic。
    {
        let mut q = ctx.follow_up_queue.lock();
        if !q.is_empty() {
            messages.extend(q.drain(..));
        }
    }

    let renderer = Arc::new(parking_lot::Mutex::new(MarkdownRenderer::new()));
    let config = AgentLoopConfig {
        max_attempts: 3,
        max_tool_rounds: usize::MAX,
        retry_base_delay_ms: 300,
        model,
        session_id,
        tool_definitions: build_tool_definitions(ctx),
        context_config: context_config.clone(),
        agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
        read_file_state: ctx.read_file_state.clone(),
        openai_files_runtime: ctx.openai_files_runtime.clone(),
        checkpoint_store: ctx.checkpoint_store.clone(),
        parent_session_id: None,
        spawn_depth: 0,
        subagent_type: crate::core::agent_loop::SubagentType::User,
        review_kind: None,
        plan_runtime: Some(ctx.plan_runtime.clone()),
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
    // P1（bash background monitor）：注入 session 级共享 follow_up_queue
    // 与 completion_routes，让 host lifecycle subscriber 推入的 synthetic
    // notification 能在同一 turn 内被消费、并与 dispatcher 走 claim-on-entry 去重。
    agent_loop = agent_loop.with_shared_follow_up_queue(ctx.follow_up_queue.clone());
    agent_loop = agent_loop.with_completion_routes(ctx.completion_routes.clone());

    // ContextState ownership 切换：临时 swap 出 owned 给 AgentLoop；
    // 运行结束后 take_context_state 写回调用方 `&mut`。
    let prev_state = std::mem::replace(
        context_state,
        make_fallback_context_state(ctx, system_text, context_config),
    );
    agent_loop.set_context_state(Some(prev_state));

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

    // J1：CLI 状态行——把当前 PlanMode 渲染为 `[PLAN]` / `[EXEC plan_id]` / `[PENDING plan_id]` /
    // `[DONE plan_id]` tag，便于用户在 readline 提示符里识别"自己处在哪种模式"。
    let mode_tag = match ctx.plan_runtime.mode() {
        plan_runtime::PlanMode::Chat => String::new(),
        plan_runtime::PlanMode::Planning => " [PLAN]".to_string(),
        plan_runtime::PlanMode::Executing { plan_id } => format!(" [EXEC {plan_id}]"),
        plan_runtime::PlanMode::Pending { plan_id } => format!(" [PENDING {plan_id}]"),
        plan_runtime::PlanMode::Completed { plan_id } => format!(" [DONE {plan_id}]"),
    };
    print!("\ntomcat.{}{}> ", ctx.config.agent.id, mode_tag);
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

    if let Some(remaining) = renderer.lock().flush() {
        print!("{}", remaining);
        let _ = io::stdout().flush();
    }

    *context_state = agent_loop.take_context_state().unwrap_or_else(|| {
        init_context_state(&ctx.session, context_config, system_text)
            .unwrap_or_else(|_| make_fallback_context_state(ctx, system_text, context_config))
    });

    // T-004 / T-017：`Completed` 与 `Interrupted` 走**同一条**持久化路径——
    // partial assistant（content_buf 截短处）+ 已完成的 tool_result 都已被
    // `AgentLoop::run` 装进 `AgentRunResult.new_messages`，这里只需 append +
    // observability，不区分成功与中断。
    match &outcome {
        AgentRunOutcome::Completed(result) => {
            persist_turn_result(
                ctx,
                context_state,
                result.new_messages.clone(),
                CheckpointKind::TurnEnd,
            )?;
        }
        AgentRunOutcome::Interrupted(result) => {
            persist_turn_result(
                ctx,
                context_state,
                result.new_messages.clone(),
                CheckpointKind::Interrupt,
            )?;
        }
        AgentRunOutcome::Failed(_) => {
            let _ = ctx.session.persist_context_observability(context_state);
        }
    }

    Ok(outcome)
}

fn make_fallback_context_state(
    ctx: &ChatContext,
    system_text: &str,
    context_config: &crate::infra::ContextConfig,
) -> crate::core::ContextState {
    crate::core::ContextState {
        messages: Vec::new(),
        estimate_context_chars: system_text.len(),
        context_budget_chars: crate::infra::config::compute_context_budget_chars(context_config),
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
    }
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
    let Ok(Some(session_id)) = ctx.session.current_session_id() else {
        return;
    };
    let Some(request) = build_turn_checkpoint_request(&session_id, kind, appended_row_ids) else {
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
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let _ = ctx.session.ensure_current_session(cwd)?;
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
