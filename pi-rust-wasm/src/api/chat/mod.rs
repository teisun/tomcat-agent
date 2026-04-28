//! # CLI 对话主循环
//!
//! [`chat_loop`] 是 `pi chat` 子命令的事件循环：装配 [`ChatContext`]、读用户输入、
//! 触发 preheat / 边界压缩、跑 [`AgentLoop`]、流式渲染回执、把消息写回 transcript，
//! 并处理 Ctrl+C 双击退出。
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  ChatContext::from_config(AppConfig)                ① 装配阶段           │
//! │   ├─ SessionManager      （sessions_dir，transcript JSONL 持久层）       │
//! │   ├─ Arc<dyn LlmProvider>（OpenAiProvider / ...）                        │
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
//! - `session_stderr_listeners`：把 `ToolResult` / `Compaction` 等事件按用户视角
//!   渲染到 stderr，与主流 stdout 解耦。
//! - `tests`：CLI 集成测试入口。

use std::io::{self, Write as IoWrite};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::core::agent_loop::AgentRunOutcome;
use crate::core::compaction::apply::check_before_request;
use crate::core::compaction::preheat::Preheat;
use crate::core::llm::ChatMessage;
use crate::core::session::manager::{build_context_from_state, init_context_state};
use crate::infra::error::AppError;
use crate::infra::event_bus::EventContext;
use crate::infra::{
    wire, AuditRecorder, AuditStore, DefaultEventBus, EventBus, FileAuditRecorder,
    TracingAuditRecorder,
};
use crate::{
    resolve_agent_workspace_definition_dir, resolve_agent_workspace_trail_dir,
    resolve_extra_roots_paths, resolve_sessions_dir, AgentLoop, AgentLoopConfig, AppConfig,
    DefaultPrimitiveExecutor, DefaultToolRegistry, LlmProvider, OpenAiProvider, PrimitiveExecutor,
    SessionEntry, SessionManager, Tool, ToolExecutor, ToolRegistry,
};

use super::render::MarkdownRenderer;

#[cfg(test)]
mod tests;

pub mod config_tool;
pub mod cwd_lazy_prompt;
pub mod dragged_path;
mod session_stderr_listeners;

// ─── ChatContext ──────────────────────────────────────────────────────────────

pub struct ChatContext {
    pub session: SessionManager,
    pub llm: Arc<dyn LlmProvider>,
    pub config: AppConfig,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    /// 当前回合用户中断令牌。ctrlc handler 会 `lock().cancel()`；
    /// `chat_loop` 在每次 readline 读到非空输入后**重建**它（`CancellationToken`
    /// 一旦 cancel 不可逆），保证新回合不会被上一回合的中断信号污染。
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    /// 上一次 Ctrl+C 按下的时刻；ctrlc handler 判双击用。
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
    /// Agent 设计态工作区，用于 AGENTS.md / SOUL.md / skills / memory 等长期配置。
    pub workspace_dir: std::path::PathBuf,
    /// Agent 运行态轨迹目录，用于 sessions / logs / audit / tmp / tool-results。
    pub agent_workspace_trail: std::path::PathBuf,
    /// `pi chat` 进入时的真实当前工作目录（`std::env::current_dir()` 一次性快照）。
    /// 用途：
    /// - 注入 system prompt「## Current Working Directory」段，让 LLM 优先在 cwd 下查找/操作；
    /// - `CwdLazyPrompt` 装饰器判断「LLM 即将访问的目标是否落在 cwd 子树」；
    /// - 拖拽路径对比（plan §7）。
    ///
    /// **不**等同于 [`Self::workspace_dir`]——后者是 agent 配置的工作区根，前者是用户启动 cli 时所在目录。
    pub cwd: std::path::PathBuf,
    /// `pi.config.toml` 的解析后绝对路径快照，避免在权限决策路径上重复调用 `config_file_path()`。
    /// `CwdLazyPrompt::AllowAndPersistRoot` 分支用它持久化 extra_root。
    pub cfg_path: std::path::PathBuf,
    /// 会话级临时授权（拖拽 + 用户 confirm AllowOnce 共享）。
    pub session_grants: crate::core::permission::SessionGrants,
    /// 拖入路径缓存（与 `session_grants` 同结构，仅审计语义 `GrantSource::DraggedPath` 区分）。
    pub dragged_paths: crate::core::permission::DraggedPaths,
    /// `config_get` / `config_set` LLM 工具后端（plan §6 / PR-7）。
    /// 为 `None` 时工具命中返回"未启用"错误，正常 4 原语 / chat 流程不受影响。
    pub config_backend: Option<crate::core::agent_loop::SharedConfigBackend>,
    /// 三层权限决策 gate（plan §3 / PR-1）：与 executor / system prompt / 拖拽 UI
    /// 共享同一份 SessionGrants + DraggedPaths 视图，保证三处的授权变更彼此可见。
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
}

impl ChatContext {
    pub fn from_config(config: AppConfig) -> Result<Self, AppError> {
        let sessions_path = resolve_sessions_dir(&config)?;
        std::fs::create_dir_all(&sessions_path).map_err(AppError::Io)?;
        let session = SessionManager::new(sessions_path);

        let workspace_dir = resolve_agent_workspace_definition_dir(&config)?;
        std::fs::create_dir_all(&workspace_dir).map_err(AppError::Io)?;
        let agent_workspace_trail = resolve_agent_workspace_trail_dir(&config)?;
        std::fs::create_dir_all(&agent_workspace_trail).map_err(AppError::Io)?;
        migrate_legacy_layer0_tool_results(&workspace_dir, &agent_workspace_trail);

        // 启动 snapshot：cwd / cfg_path 在整个 chat 生命周期内固定，避免后续 cd
        // 让 system prompt 与权限决策视图漂移。`current_dir()` 失败时退化到 workspace_dir
        // （兜底场景：被 chroot 或者 cwd 不可读，仍能继续聊天）。
        let cwd_snapshot = std::env::current_dir().unwrap_or_else(|_| workspace_dir.clone());
        let cfg_path_snapshot =
            crate::api::cli::config_file_path().unwrap_or_else(|_| std::path::PathBuf::new());

        let llm: Arc<dyn LlmProvider> = Arc::new(OpenAiProvider::new(&config.llm)?);

        let audit: Arc<dyn AuditRecorder> = match AuditStore::open_if_enabled(&config)? {
            Some(store) => Arc::new(FileAuditRecorder::new(Arc::new(store))),
            None => Arc::new(TracingAuditRecorder),
        };
        let extra_roots = resolve_extra_roots_paths(&config)?;
        let cli_confirmation: Arc<dyn UserConfirmationProvider> = Arc::new(CliConfirmation);

        // PR-9：构造 3 层权限 gate；与 executor / chat 共享 SessionGrants + DraggedPaths。
        // agent_data_readonly_dirs：sessions/logs/audit + agent 凭据目录（凭据子目录由
        // builtin path_rules 单独 deny，read_only 集合允许 read 但禁 write）。
        let session_grants = crate::core::permission::SessionGrants::new();
        let dragged_paths = crate::core::permission::DraggedPaths::new();
        let agent_data_readonly_dirs: Vec<std::path::PathBuf> = vec![
            Some(agent_workspace_trail.clone()),
            crate::infra::config::resolve_sessions_dir(&config).ok(),
            crate::infra::config::resolve_log_dir(&config).ok(),
            crate::infra::config::resolve_audit_dir(&config).ok(),
            crate::infra::config::resolve_agent_dir(&config).ok(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let gate_cfg = crate::core::permission::GateConfig {
            workspace_dir: workspace_dir.clone(),
            extra_roots: extra_roots.clone(),
            agent_data_readonly_dirs: agent_data_readonly_dirs.clone(),
            user_path_rules: config.primitive.path_rules.clone(),
            user_bash_forbidden: config.primitive.bash_forbidden.clone(),
            user_bash_approval: config.primitive.bash_approval_required.clone(),
            user_bash_whitelist: config.primitive.bash_whitelist.clone(),
            auto_confirm: config.primitive.auto_confirm,
        };
        let gate: Arc<dyn crate::core::permission::PermissionGate> =
            Arc::new(crate::core::permission::DefaultPermissionGate::new(
                gate_cfg,
                session_grants.clone(),
                dragged_paths.clone(),
            ));

        // Hotfix §A.3：用 CwdLazyPrompt 装饰 CliConfirmation。
        // 装饰器：当 LLM 工具调用首次落到 cwd 子树未授权路径时弹「[a]/[s]/[n]」
        // 范围级提示，其余情况转发给 CliConfirmation 既有行为。
        // 注意：dismissed flag 在装饰器内部以 Arc<AtomicBool> 创建，与 ctx 同生命周期。
        let confirmation: Arc<dyn UserConfirmationProvider> =
            Arc::new(cwd_lazy_prompt::CwdLazyPrompt::new(
                cli_confirmation,
                cwd_snapshot.clone(),
                gate.clone(),
                session_grants.clone(),
                cfg_path_snapshot.clone(),
            ));

        let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(
            DefaultPrimitiveExecutor::new(
                config.primitive.clone(),
                confirmation.clone(),
                audit.clone(),
                workspace_dir.clone(),
            )
            .with_extra_roots(extra_roots)
            .with_gate(gate.clone()),
        );

        // PR-7：构造 config_get / config_set 工具后端。失败（无法解析 config_path）
        // 时降级为 `None`，工具命中返回"未启用"错误，主流程不阻塞。
        let config_backend: Option<crate::core::agent_loop::SharedConfigBackend> =
            match crate::api::cli::config_file_path() {
                Ok(p) => Some(Arc::new(config_tool::ChatConfigBackend {
                    ctx: config_tool::ConfigToolContext::new(p, confirmation.clone())
                        .with_gate(gate.clone()),
                })),
                Err(_) => None,
            };

        let tool_executor: Arc<dyn ToolExecutor> = Arc::new(NoopToolExecutor);
        let tool_registry: Arc<dyn ToolRegistry> =
            Arc::new(DefaultToolRegistry::new(tool_executor, audit));

        let event_bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
        let cancel_token = Arc::new(Mutex::new(CancellationToken::new()));
        let last_interrupt_at = Arc::new(Mutex::new(None));

        Ok(Self {
            session,
            llm,
            config,
            primitive,
            tool_registry,
            event_bus,
            cancel_token,
            last_interrupt_at,
            workspace_dir,
            agent_workspace_trail,
            cwd: cwd_snapshot,
            cfg_path: cfg_path_snapshot,
            session_grants,
            dragged_paths,
            config_backend,
            gate,
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

use crate::core::confirmation::UserConfirmationProvider;
use crate::core::primitives::PrimitiveOperation;

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

fn build_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "读取文件内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" }
                    },
                    "required": ["path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "写入文件内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "content": { "type": "string", "description": "文件内容" },
                        "overwrite": { "type": "boolean", "description": "是否覆盖" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "编辑文件（基于内容匹配替换）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径" },
                        "old_content": { "type": "string", "description": "被替换的原内容" },
                        "new_content": { "type": "string", "description": "替换后的新内容" }
                    },
                    "required": ["path", "old_content", "new_content"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "execute_bash",
                "description": "执行 bash 命令",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的命令" },
                        "cwd": { "type": "string", "description": "工作目录（可选）" }
                    },
                    "required": ["command"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "列出目录内容",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "目录路径" }
                    },
                    "required": ["path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "config_get",
                "description": "读取 pi 配置项的当前值。受键级白名单（CONFIG_READ_ALLOWLIST）+ 硬黑名单（CONFIG_HARDCODED_READ_DENY）双重约束：可读 workspace.* / agent.id / primitive.path_rules / primitive.bash_* / llm.default_model 等非敏感字段；llm.api_key* / llm.api_base / security.* / storage.* 等敏感字段一律拒绝。键 dot 路径不存在时返回 'not_set'。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "配置键的 dot 路径，例：'workspace.extra_roots' / 'workspace.entries' / 'primitive.path_rules' / 'primitive.bash_forbidden' / 'agent.id'"
                        }
                    },
                    "required": ["key"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "config_set",
                "description": "向受允许的 pi 配置项追加或修改值，每次调用都会触发用户 confirm（用户看到 unified diff 后 y/N）。受键级白名单（CONFIG_WRITE_ALLOWLIST）+ 硬黑名单（CONFIG_HARDCODED_WRITE_DENY）双重约束。\n\n语义：\n- 数组字段（workspace.extra_roots / workspace.entries / primitive.path_rules / primitive.bash_forbidden / primitive.bash_approval_required）：value 是单个新元素的 JSON 字符串（追加 only，不替换整数组）。例：value='{\"path\":\"~/myproj\",\"mode\":\"deny\"}'\n- 标量字段（llm.default_model / log.level / context.compaction_turns 等）：value 直接是新值字符串（替换语义）\n- 删除/修改不支持：返回 error 引导用户使用 `pi config edit` 手编 TOML（未来版本将提供 `pi pathrules remove` / `pi workspace remove`）\n\n禁止字段：llm.api_key* / security.* / storage.* / agent.id / agent.workspace / primitive.bash_whitelist / primitive.path_whitelist / primitive.auto_confirm 等（自我提权防护）。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "配置键的 dot 路径"
                        },
                        "value": {
                            "type": "string",
                            "description": "标量字段：新值（如 'gpt-4o' / 'debug' / 30）；数组字段：单个元素的 JSON 字符串（如 '\"/Users/x/proj\"' 或 '{\"path\":\"~/.foo\",\"mode\":\"readonly\"}'）"
                        }
                    },
                    "required": ["key", "value"]
                }
            }
        }),
    ]
}

// ─── Workspace state for system prompt（plan §8 / PR-8） ─────────────────────

/// 把 `ChatContext` 的 workspace 配置 + session_grants + dragged_paths 合并成
/// `system_prompt::WorkspaceState`，喂给 [`build_system_prompt_with_state`]。
///
/// 直接读 `ctx.gate.effective_roots()` / `effective_path_rules()`，与 executor /
/// 拖拽 UI 共享同一份决策视图。
fn compute_workspace_state(ctx: &ChatContext) -> crate::core::system_prompt::WorkspaceState {
    use crate::core::permission::PathRuleMode;
    use crate::core::system_prompt::{PathRuleSummary, WorkspaceRootDescriptor, WorkspaceState};
    use std::collections::HashSet;

    let cfg = &ctx.config;
    let workspace_dir = ctx.workspace_dir.clone();
    let extra_roots = resolve_extra_roots_paths(cfg).unwrap_or_default();

    // agent runtime trail：display 用运行态根路径（仅展示，read_only 集合由 gate 提供）。
    let agent_data_dir_for_prompt = Some(ctx.agent_workspace_trail.to_string_lossy().to_string());
    let agent_data_readonly_dirs: Vec<std::path::PathBuf> = vec![
        Some(ctx.agent_workspace_trail.clone()),
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
    let workspace_canon = workspace_dir.to_string_lossy().to_string();
    let extra_set: HashSet<String> = extra_roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let session_set: HashSet<String> = ctx
        .session_grants
        .snapshot()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let dragged_set: HashSet<String> = ctx
        .dragged_paths
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
        let label = if s == workspace_canon {
            "agent_workspace_definition"
        } else if extra_set.contains(&s) {
            "extra_root"
        } else if dragged_set.contains(&s) {
            "dragged_path"
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
    let agent_data_set: HashSet<String> = agent_data_readonly_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    for p in er.read_only {
        let s = p.to_string_lossy().to_string();
        if !seen_ro.insert(s.clone()) {
            continue;
        }
        let label = if agent_data_set.contains(&s) {
            "agent_workspace_trail"
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
        cwd: ctx.cwd.to_string_lossy().to_string(),
        read_write,
        read_only,
        path_rules,
        agent_data_dir: agent_data_dir_for_prompt,
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
    println!("pi 对话模式 (模型: {})", model);
    println!("输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。\n");

    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))?;

    // ContextState: 在 loop 外一次性初始化，跨迭代复用
    let context_config = &ctx.config.context;
    let workspace_str = ctx.workspace_dir.to_string_lossy().to_string();
    let workspace_state = compute_workspace_state(ctx);
    let system_text =
        crate::core::system_prompt::build_system_prompt_with_state(&workspace_str, workspace_state);
    let mut context_state = init_context_state(&ctx.session, context_config, &system_text)?;
    let session_stderr_ids =
        session_stderr_listeners::register_chat_session_stderr_listeners(&*ctx.event_bus);

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

        // 拖拽路径解析：行内含意图 → SessionGrants AllowOnce；纯路径 → 5 选项菜单。
        // 菜单返回 None 时，原行按聊天处理（cancel 行为）。
        let input = match handle_dragged_input(ctx, &input, &mut rl) {
            DragHandleResult::Continue { line } => line,
            DragHandleResult::Skip => continue,
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
            target: "pi_wasm_chat_diag",
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
            target: "pi_wasm_chat_diag",
            phase = "chat_after_timing2_check",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = false,
            ratio = context_state.usage_ratio(),
            compaction_count = context_state.session_obs.compaction_count
        );

        // Build messages from ContextState
        let mut messages = build_context_from_state(&context_state);
        messages.insert(0, ChatMessage::system(&system_text));
        messages.push(ChatMessage::user(&input));

        let renderer = Arc::new(parking_lot::Mutex::new(MarkdownRenderer::new()));
        let config = AgentLoopConfig {
            max_attempts: 3,
            max_tool_rounds: usize::MAX,
            retry_base_delay_ms: 300,
            model: model.clone(),
            session_id: ctx.session.current_session_key().to_string(),
            tool_definitions: build_tool_definitions(),
            context_config: context_config.clone(),
            work_dir: ctx.agent_workspace_trail.to_string_lossy().to_string(),
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
        agent_loop.set_context_state(Some(context_state));

        let renderer_clone = Arc::clone(&renderer);
        let listener_id = ctx.event_bus.on(
            wire::WIRE_MESSAGE_UPDATE,
            Box::new(move |evt: EventContext| {
                if let Some(delta) = evt
                    .payload
                    .get("assistantMessageEvent")
                    .and_then(|e| e.get("delta"))
                    .and_then(|d| d.as_str())
                {
                    renderer_clone.lock().push(delta);
                    while let Some(chunk) = renderer_clone.lock().take_ready() {
                        print!("{}", chunk);
                        let _ = io::stdout().flush();
                    }
                }
                Ok(())
            }),
        );

        print!("\npi.{}> ", ctx.config.agent.id);
        io::stdout().flush().map_err(AppError::Io)?;

        info!(
            target: "pi_wasm_chat_diag",
            phase = "chat_before_agent_run",
            session_stderr_listeners_active = true,
            message_stream_listener_registered = true
        );
        let outcome = agent_loop.run(messages).await;
        ctx.event_bus.off(listener_id);

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
            for msg in result.new_messages {
                let row_id = ctx.session.append_message(serde_json::to_value(&msg)?)?;
                let mut cm = msg;
                cm.msg_id = Some(row_id);
                context_state.messages.push(cm);
            }
            ctx.session.persist_context_observability(&context_state)?;

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
                session_stderr_listeners::unregister_chat_session_stderr_listeners(
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

    session_stderr_listeners::unregister_chat_session_stderr_listeners(
        &*ctx.event_bus,
        &session_stderr_ids,
    );
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// 判断错误是否致命（配置缺失等不可恢复场景）；API/网络错误为非致命。
fn is_fatal_error(e: &AppError) -> bool {
    matches!(e, AppError::Config(_))
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

fn migrate_legacy_layer0_tool_results(
    agent_workspace_definition: &std::path::Path,
    agent_workspace_trail: &std::path::Path,
) {
    let legacy_root = agent_workspace_definition.join("workspace");
    if !legacy_root.exists() {
        return;
    }
    let target_root = agent_workspace_trail.join("tool-results");
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

// ─── 拖拽路径处理（plan §7） ────────────────────────────────────────────────

use dragged_path::{
    interpret_dragged_paths, render_drag_menu, DragOutcome, MenuChoice, MenuOptions,
};

enum DragHandleResult {
    /// 把 `line` 作为本回合用户消息发给 LLM。
    Continue { line: String },
    /// 用户在菜单选了非 `[c]` 选项 / 或路径无效 —— 跳过本轮 readline。
    Skip,
}

fn handle_dragged_input(
    ctx: &ChatContext,
    input: &str,
    rl: &mut rustyline::DefaultEditor,
) -> DragHandleResult {
    match interpret_dragged_paths(input) {
        DragOutcome::None => DragHandleResult::Continue {
            line: input.to_string(),
        },
        DragOutcome::AutoAllow {
            paths,
            original_line,
        } => {
            for p in &paths {
                match precheck_read_allow(ctx, p) {
                    Ok(canon) => {
                        ctx.gate.grant_session(
                            canon,
                            crate::core::permission::GrantSource::DraggedPath,
                        );
                        eprintln!("✓ {} 本次会话期间允许访问", p.display());
                    }
                    Err(e) => {
                        eprintln!("✗ {}: {}", p.display(), e);
                    }
                }
            }
            DragHandleResult::Continue {
                line: original_line,
            }
        }
        DragOutcome::PromptMenu {
            paths,
            original_line,
        } => {
            // 首版菜单：每个路径独立走一次（保证 path_rules 预检查精确）。
            // 多路径用户体验是"逐条菜单"——避免一刀切允许 5 路径中的 builtin deny。
            let mut any_persisted = false;
            for p in &paths {
                let opts = render_drag_menu(p, &*ctx.gate);
                let choice = render_menu_and_read(p, &opts, rl);
                match apply_menu_choice(ctx, p, choice) {
                    Ok(persisted) => {
                        any_persisted |= persisted;
                    }
                    Err(e) => {
                        eprintln!("✗ {}: {}", p.display(), e);
                    }
                }
            }
            if any_persisted {
                DragHandleResult::Skip
            } else {
                // 没人持久化也没 AllowOnce：把原行按聊天处理。
                DragHandleResult::Continue {
                    line: original_line,
                }
            }
        }
    }
}

fn render_menu_and_read(
    path: &std::path::Path,
    opts: &MenuOptions,
    rl: &mut rustyline::DefaultEditor,
) -> MenuChoice {
    println!("\n--- 拖入路径授权 ---");
    println!("路径: {}", path.display());
    if let Some(note) = &opts.note {
        println!("提示: {}", note);
    }
    if opts.allow_once {
        println!("  [a] 本次会话允许访问");
    }
    if opts.persist_extra_root {
        println!("  [w] 以后也允许访问（写入配置 workspace.extra_roots）");
    }
    if opts.persist_readonly {
        println!("  [r] 设为只读：允许读取，禁止写入");
    }
    if opts.persist_deny {
        println!("  [d] 禁止访问：拒绝读取和写入");
    }
    if opts.cancel {
        println!("  [c] 取消授权，作为普通消息发送");
    }
    print!("选择: ");
    let _ = io::stdout().flush();

    let line = rl.readline("").unwrap_or_else(|_| "c".to_string());
    MenuChoice::from_input(&line).unwrap_or(MenuChoice::Cancel)
}

fn apply_menu_choice(
    ctx: &ChatContext,
    path: &std::path::Path,
    choice: MenuChoice,
) -> Result<bool, AppError> {
    use crate::core::permission::{PathRule, PathRuleMode};

    match choice {
        MenuChoice::AllowOnce => {
            let canon = precheck_read_allow(ctx, path)?;
            ctx.gate
                .grant_session(canon, crate::core::permission::GrantSource::SessionGrant);
            eprintln!("✓ {} 本次会话期间允许访问", path.display());
            Ok(true)
        }
        MenuChoice::PersistExtraRoot => {
            precheck_read_allow(ctx, path)?;
            let canon = std::fs::canonicalize(path).map_err(AppError::Io)?;
            let cfg_path = crate::api::cli::config_file_path()?;
            crate::infra::config::append_extra_root_to_disk(
                &cfg_path,
                canon.to_string_lossy().into_owned(),
            )?;
            eprintln!("✓ 已更新配置：以后允许访问 {}", canon.display());
            Ok(true)
        }
        MenuChoice::PersistReadonly | MenuChoice::PersistDeny => {
            let mode = match choice {
                MenuChoice::PersistReadonly => PathRuleMode::Readonly,
                MenuChoice::PersistDeny => PathRuleMode::Deny,
                _ => unreachable!(),
            };
            let cfg_path = crate::api::cli::config_file_path()?;
            crate::infra::config::append_path_rule_to_disk(
                &cfg_path,
                PathRule {
                    path: path.to_string_lossy().into_owned(),
                    mode,
                },
            )?;
            ctx.gate.grant_path_rule(PathRule {
                path: path.to_string_lossy().into_owned(),
                mode,
            });
            let status = match mode {
                PathRuleMode::Readonly => "已设为只读",
                PathRuleMode::Deny => "已禁止访问",
            };
            eprintln!("✓ 已更新访问规则：{} {}", path.display(), status);
            Ok(true)
        }
        MenuChoice::Cancel => Ok(false),
    }
}

fn precheck_read_allow(
    ctx: &ChatContext,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, AppError> {
    use crate::core::permission::PermissionDecision;
    use crate::core::primitives::PrimitiveOperation;

    let canon = crate::infra::platform::normalize_path(&path.to_string_lossy())
        .unwrap_or_else(|_| path.to_path_buf());
    match ctx
        .gate
        .check(PrimitiveOperation::Read, &canon.to_string_lossy())?
    {
        PermissionDecision::Deny { reason } => Err(AppError::Permission(format!(
            "该路径已被禁止访问，无法授权本次会话：{} ({})",
            path.display(),
            reason
        ))),
        _ => Ok(canon),
    }
}
