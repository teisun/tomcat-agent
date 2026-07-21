//! T2-P0-016 PR-I（bash T2 后台）：起后台任务 + task_output / task_stop / task_list。
//!
//! ## 与 PR-E `bash` 同步路径的差异
//!
//! - **同步 bash**：`spawn → 等 wait → 收齐输出 → 一次返回 BashResult`，单轮 tool 阻塞。
//! - **后台 bash**（本模块）：`bash` 工具带 `run_in_background=true` → **立即**返回
//!   `BashTaskTicket{ task_id, log_path }`；后台 `tokio::spawn` 守护把 stdout/stderr
//!   持续写到 `<persist_dir>/bash-<task_id>.log`；模型用三件套自驱：
//!     - `task_output`：按字节偏移拉日志增量；
//!     - `task_stop`：`killpg(SIGKILL)` 杀整组（与 PR-E.2 同口径）；
//!     - `task_list`：枚举所有 task 现状（含 `Finished{ exit_code }` / `Stopped`）。
//!
//! ## 锁分层（避免「stop 等 wait」死锁）
//!
//! - `BashTaskRegistry.tasks: RwLock<HashMap<...>>`：注册表本身，操作短促。
//! - `BashTask.info: RwLock<BashTaskInfo>`：每任务的元信息 + 状态机，操作短促。
//! - 子进程 `Child` 句柄**不**入锁——直接 move 进 wait 任务（独占 `await`）。
//!   stop 走的是 `pid → libc::killpg(SIGKILL)`，不依赖 Child 句柄，杀完
//!   wait 任务自然 `wait()` 返回 → 状态翻成 `Finished{ exit_code }`。

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Notify;

use crate::core::permission::{
    is_url_like, BashAstChecker, GrantTrace, GrantTrigger, GrantType, PermissionDecision,
    PermissionGate, PermissionScope,
};
use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::audit::{AuditPrimitiveOp, AuditRecorder, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;

fn permission_scope_str(scope: PermissionScope) -> String {
    match scope {
        PermissionScope::Read => "read",
        PermissionScope::Write => "write",
        PermissionScope::Bash => "bash",
        PermissionScope::BashApproval => "bash_approval",
        PermissionScope::Forbidden => "forbidden",
    }
    .to_string()
}

fn grant_type_str(s: GrantType) -> String {
    match s {
        GrantType::AgentDefinitionDir => "agent_definition_dir",
        GrantType::AgentPlansDir => "agent_plans_dir",
        GrantType::AgentWorkspaceRoot => "agent_workspace_root",
        GrantType::SessionScope => "session_scope",
        GrantType::PathRuleReadOnly => "path_rule_read_only",
        GrantType::AgentTrailDir => "agent_trail_dir",
        GrantType::BashPolicy => "bash_policy",
    }
    .to_string()
}

fn grant_trigger_str(s: GrantTrigger) -> String {
    match s {
        GrantTrigger::BuiltinDefault => "builtin_default",
        GrantTrigger::WorkspaceRootsConfig => "workspace_roots_config",
        GrantTrigger::PathRulesConfig => "path_rules_config",
        GrantTrigger::BashRegexConfig => "bash_regex_config",
        GrantTrigger::UserConfirm => "user_confirm",
        GrantTrigger::CwdLazyPrompt => "cwd_lazy_prompt",
        GrantTrigger::DraggedPathMenu => "dragged_path_menu",
        GrantTrigger::AutoConfirmFlag => "auto_confirm_flag",
    }
    .to_string()
}

fn normalize_launcher_argv(
    command: String,
    argv: Option<Vec<String>>,
) -> (String, Option<Vec<String>>) {
    let Some(mut argv) = argv else {
        return (command, None);
    };
    let trimmed = command.trim();
    let mut parts = trimmed.split_whitespace();
    let Some(program) = parts.next() else {
        return (command, Some(argv));
    };
    if !matches!(
        program,
        "sh" | "bash" | "zsh" | "cmd" | "powershell" | "pwsh"
    ) {
        return (command, Some(argv));
    }
    let launcher_args: Vec<String> = parts.map(str::to_string).collect();
    if launcher_args.is_empty() {
        return (command, Some(argv));
    }
    let mut merged = launcher_args;
    merged.append(&mut argv);
    (program.to_string(), Some(merged))
}

fn op_summary(op: PrimitiveOperation) -> &'static str {
    match op {
        PrimitiveOperation::Read => "读取",
        PrimitiveOperation::Write => "写入",
        PrimitiveOperation::Edit => "编辑",
        PrimitiveOperation::Bash => "执行命令",
    }
}

fn resolve_preflight_path(raw: &str, cwd_path: &Path) -> PathBuf {
    if raw == "~" {
        return crate::infra::platform::home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return crate::infra::platform::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    if raw.starts_with("./") || raw.starts_with("../") {
        let base = if cwd_path == Path::new(".") {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            cwd_path.to_path_buf()
        };
        return base.join(raw);
    }
    PathBuf::from(raw)
}

#[derive(Clone)]
pub struct BackgroundBashGuard {
    plugin_id: String,
    gate: Arc<dyn PermissionGate>,
    confirmation: Arc<dyn UserConfirmationProvider>,
    audit: Arc<dyn AuditRecorder>,
    bash_ast: BashAstChecker,
}

impl BackgroundBashGuard {
    pub fn new(
        plugin_id: impl Into<String>,
        gate: Arc<dyn PermissionGate>,
        confirmation: Arc<dyn UserConfirmationProvider>,
        audit: Arc<dyn AuditRecorder>,
        bash_ast: BashAstChecker,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            gate,
            confirmation,
            audit,
            bash_ast,
        }
    }

    fn record_failure(&self, audit_cmd: &str, err: &AppError) {
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Bash,
            path_or_cmd: audit_cmd.to_string(),
            plugin_id: self.plugin_id.clone(),
            user_approved: false,
            success: false,
            detail: Some(err.to_string()),
            ..Default::default()
        });
    }

    fn record_spawn_result(
        &self,
        audit_cmd: &str,
        scope: PermissionScope,
        grant: GrantTrace,
        success: bool,
        detail: String,
    ) {
        self.audit.record_primitive(PrimitiveAuditEntry {
            operation: AuditPrimitiveOp::Bash,
            path_or_cmd: audit_cmd.to_string(),
            plugin_id: self.plugin_id.clone(),
            user_approved: true,
            success,
            detail: Some(detail),
            permission_scope: Some(permission_scope_str(scope)),
            grant_type: Some(grant_type_str(grant.grant_type)),
            grant_trigger: Some(grant_trigger_str(grant.trigger)),
        });
    }

    async fn gate_check_path(
        &self,
        op: PrimitiveOperation,
        path: &str,
    ) -> Result<(PathBuf, PermissionScope, GrantTrace), AppError> {
        if is_url_like(path) && op != PrimitiveOperation::Bash {
            let scope = match op {
                PrimitiveOperation::Read => PermissionScope::Read,
                PrimitiveOperation::Write | PrimitiveOperation::Edit => PermissionScope::Write,
                PrimitiveOperation::Bash => unreachable!("bash URL-like path should never bypass"),
            };
            return Ok((
                PathBuf::from(path),
                scope,
                GrantTrace::new(GrantType::SessionScope, GrantTrigger::BuiltinDefault),
            ));
        }

        let normalized = normalize_path(path)?;
        loop {
            match self.gate.check(op, &normalized.to_string_lossy())? {
                PermissionDecision::Allow { grant, scope } => {
                    return Ok((normalized, scope, grant))
                }
                PermissionDecision::Deny { reason } => {
                    return Err(AppError::Permission(reason));
                }
                PermissionDecision::NeedConfirm {
                    reason,
                    suggested_root,
                } => {
                    let preview = format!(
                        "[{:?}] {}\n路径: {}\n原因: {}",
                        op,
                        op_summary(op),
                        normalized.display(),
                        reason
                    );
                    match self
                        .confirmation
                        .confirm_decision(op, &preview, &self.plugin_id, suggested_root.clone())
                        .await?
                    {
                        ConfirmDecision::Deny => {
                            return Err(AppError::Permission(format!(
                                "用户拒绝授权: {}。下次工具再次访问该路径时会重新弹出 [s]/[w]/[c] 授权选项；也可以执行 `tomcat workspace add {}` 一次性永久授权。",
                                normalized.display(),
                                normalized.display()
                            )));
                        }
                        ConfirmDecision::AllowOnce => {
                            self.gate
                                .grant_session(normalized.clone(), GrantTrigger::UserConfirm);
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            self.gate.grant_session(root, GrantTrigger::UserConfirm);
                        }
                    }
                }
            }
        }
    }

    async fn gate_check_bash(
        &self,
        command: &str,
    ) -> Result<(PermissionScope, GrantTrace), AppError> {
        match self.gate.check_bash(command)? {
            PermissionDecision::Allow { grant, scope } => Ok((scope, grant)),
            PermissionDecision::Deny { reason } => Err(AppError::Permission(reason)),
            PermissionDecision::NeedConfirm { reason, .. } => {
                let preview = format!(
                    "[Bash] 危险命令命中确认列表\n命令: {}\n原因: {}",
                    command, reason
                );
                match self
                    .confirmation
                    .confirm_decision(PrimitiveOperation::Bash, &preview, &self.plugin_id, None)
                    .await?
                {
                    ConfirmDecision::AllowOnce | ConfirmDecision::AllowAndPersistRoot { .. } => {
                        Ok((
                            PermissionScope::BashApproval,
                            GrantTrace::new(GrantType::BashPolicy, GrantTrigger::UserConfirm),
                        ))
                    }
                    ConfirmDecision::Deny => {
                        Err(AppError::Permission("用户拒绝 bash 确认".to_string()))
                    }
                }
            }
        }
    }

    async fn bash_preflight_and_gate(
        &self,
        audit_cmd: &str,
        cwd: Option<&Path>,
    ) -> Result<(PermissionScope, GrantTrace), AppError> {
        let cwd_path = if let Some(cwd) = cwd {
            self.gate_check_path(PrimitiveOperation::Read, cwd.to_string_lossy().as_ref())
                .await?
                .0
        } else {
            PathBuf::from(".")
        };

        if let Err(reject) = self.bash_ast.check(audit_cmd) {
            return Err(AppError::Primitive(reject.to_string()));
        }

        // TODO: 与 `executor/bash.rs::preflight_command_paths` 同口径——勿扩展重定向 parser；
        // 重定向写盘等见 bash_parser 模块顶 TODO / T-151。
        for raw in crate::core::permission::bash_parser::extract_paths(audit_cmd) {
            let candidate = resolve_preflight_path(&raw, &cwd_path);
            let candidate_owned = candidate.to_string_lossy().into_owned();
            let _ = self
                .gate_check_path(PrimitiveOperation::Bash, &candidate_owned)
                .await?;
        }

        self.gate_check_bash(audit_cmd).await
    }
}

/// 任务唯一 ID（`<unix_ms>-<rand6>`，避免 `uuid` 依赖）。
pub type BashTaskId = String;

/// `bash` 后台任务的状态机：`Running` → (`Stopped` | `Finished { exit_code }`).
///
/// `Stopped` 由 `task_stop` 主动触发；其后 wait 任务感知到 `child.wait()`
/// 返回也**不**回退覆盖（避免「人为 stop」被覆盖成「自然 Finished」误判）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BashTaskStatus {
    Running,
    Stopped,
    Finished { exit_code: i32 },
}

/// `task_list` 返回的单条快照；同时也是 `BashTaskRegistry::spawn` 内部的元信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskInfo {
    pub task_id: BashTaskId,
    pub command: String,
    pub started_at_unix_ms: u128,
    pub log_path: String,
    pub status: BashTaskStatus,
}

/// `bash run_in_background=true` 的回执：模型只拿到 `task_id` + `log_path`，
/// 不阻塞当前 tool 轮次。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskTicket {
    pub task_id: BashTaskId,
    pub log_path: String,
    pub started_at_unix_ms: u128,
}

/// `task_output` 返回的增量：`content` 是 `[start_offset, next_offset)`
/// 字节窗口的 UTF-8 lossy 解码；模型下次传 `since=next_offset` 拉续读。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashTaskOutputChunk {
    pub task_id: BashTaskId,
    pub content: String,
    pub start_offset: u64,
    pub next_offset: u64,
    pub finished: bool,
    pub exit_code: Option<i32>,
}

struct BashTask {
    info: RwLock<BashTaskInfo>,
    /// 子进程 PID（spawn 后立即记录）；stop 路径 `libc::killpg(pid, SIGKILL)`
    /// 杀整组，**不**依赖 `Child` 句柄（句柄已 move 进 wait 任务独占 await）。
    pid: Option<u32>,
    /// P1：每 task 一份 `Notify`。pump 任务每 flush 一次后 `notify_waiters()`；
    /// wait 任务把 status 翻成 `Finished` / `Stopped` 时也 `notify_waiters()`。
    /// 配合 [`BashTaskRegistry::wait_for_finish`] 实现"按文件长度 vs since"判定，
    /// 不依赖事件计数，避免 lost wakeup 与 read 与 wait 之间的字节丢失竞态。
    notify: Arc<Notify>,
    /// P1：lifecycle event 已经发出过的去重 guard（pump close + wait task return
    /// 都可能命中"翻终态"，但只允许 broadcast 一次）。
    lifecycle_emitted: parking_lot::Mutex<bool>,
}

/// P1：registry 级 broadcast 事件。lifecycle subscriber（host/chat_loop 侧）
/// 用它驱动 completion auto-feed → synthetic notification。
///
/// 一次 task 终态翻转**只发一次**（由 [`BashTask::lifecycle_emitted`] 兜底）。
#[derive(Debug, Clone)]
pub struct BackgroundTaskLifecycleEvent {
    pub task_id: BashTaskId,
    pub final_status: BashTaskStatus,
    pub log_path: String,
    pub command: String,
}

/// `bash` 后台任务三件套的注册表。生产路径：`api/chat` 装配时 `Arc::new` 一份，
/// 通过 `AgentLoop::with_bash_task_registry` 注入；测试路径可注入 `tempfile::tempdir()`。
pub struct BashTaskRegistry {
    tasks: RwLock<HashMap<BashTaskId, Arc<BashTask>>>,
    persist_dir: PathBuf,
    background_guard: Option<BackgroundBashGuard>,
    /// P1：所有 task 共用的 lifecycle broadcast。`subscribe_lifecycle()` 取
    /// `Receiver`；`spawn` 创建 task 时把 sender clone 给 wait 任务，task
    /// 终态翻转一次性 send。channel 容量按"启动后短期未消费的最大堆积量"
    /// 估算 256 足够大；满时旧事件丢失但不会阻塞翻转路径。
    lifecycle_tx: broadcast::Sender<BackgroundTaskLifecycleEvent>,
}

impl BashTaskRegistry {
    pub fn new(persist_dir: PathBuf) -> Self {
        let (lifecycle_tx, _) = broadcast::channel(256);
        Self {
            tasks: RwLock::new(HashMap::new()),
            persist_dir,
            background_guard: None,
            lifecycle_tx,
        }
    }

    pub fn with_background_guard(mut self, guard: BackgroundBashGuard) -> Self {
        self.background_guard = Some(guard);
        self
    }

    /// P1：host/chat_loop 订阅 lifecycle 事件用。同一个 `task_id` 的终态翻转
    /// 一次会话内**只会被 broadcast 一次**（由 [`BashTask::lifecycle_emitted`]
    /// 兜底，pump close + wait task return 双触发收敛）。
    ///
    /// 返回的 `Receiver` 在 lag 时会跳过中间事件——P1 完成事件本来就极稀疏
    /// （task 数量级 = 个），channel 容量 256 已经足够；满到 lag 即视为
    /// 设计被滥用的信号，host 侧打 warn 即可。
    pub fn subscribe_lifecycle(&self) -> broadcast::Receiver<BackgroundTaskLifecycleEvent> {
        self.lifecycle_tx.subscribe()
    }

    /// 起一个后台 bash：spawn + 起 stdout/stderr pump + 起 wait 任务回写状态。
    /// 立即返回 ticket，**不**等子进程结束。
    pub async fn spawn(
        &self,
        command: String,
        argv: Option<Vec<String>>,
        cwd: Option<PathBuf>,
    ) -> Result<BashTaskTicket, AppError> {
        // 空 argv 与未提供 args 同义，避免 `command="echo hi", args=[]` 被误当成 argv 模式。
        let argv = argv.filter(|args| !args.is_empty());
        // 兼容真 LLM 把 `sh -c` / `bash -lc` 写进 command、把脚本正文放进 args 的形态。
        let (command, argv) = normalize_launcher_argv(command, argv);
        std::fs::create_dir_all(&self.persist_dir).map_err(AppError::Io)?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let task_id = format!("{}-{}", now, simple_rand6());
        let log_path = self.persist_dir.join(format!("bash-{}.log", &task_id));
        let audit_cmd = match argv.as_deref() {
            None => command.clone(),
            Some(args) => {
                let mut text = command.clone();
                for arg in args {
                    text.push(' ');
                    text.push_str(arg);
                }
                text
            }
        };
        let bash_scope_grant = if let Some(guard) = self.background_guard.as_ref() {
            match guard
                .bash_preflight_and_gate(&audit_cmd, cwd.as_deref())
                .await
            {
                Ok(scope_grant) => Some(scope_grant),
                Err(err) => {
                    guard.record_failure(&audit_cmd, &err);
                    return Err(err);
                }
            }
        } else {
            None
        };

        let mut cmd = match argv.as_deref() {
            None => {
                #[cfg(unix)]
                let (shell, arg) = ("sh", "-c");
                #[cfg(windows)]
                let (shell, arg) = ("cmd", "/C");
                let mut c = Command::new(shell);
                c.arg(arg).arg(&command);
                c
            }
            Some(args) => {
                let mut c = Command::new(&command);
                c.args(args);
                c
            }
        };
        if let Some(c) = cwd.as_ref() {
            cmd.current_dir(c);
        }
        cmd.kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());
        // 与 PR-E.2 同口径：新进程组 + stop 时 killpg 整组，避免 sh 派生孙子进程被遗弃。
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| {
            let err = AppError::Primitive(e.to_string());
            if let (Some(guard), Some((scope, grant))) =
                (self.background_guard.as_ref(), bash_scope_grant)
            {
                guard.record_spawn_result(&audit_cmd, scope, grant, false, err.to_string());
            }
            err
        })?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(AppError::Io)?;
        let log_writer = Arc::new(AsyncMutex::new(log_file));

        let task = Arc::new(BashTask {
            info: RwLock::new(BashTaskInfo {
                task_id: task_id.clone(),
                command: command.clone(),
                started_at_unix_ms: now,
                log_path: log_path.display().to_string(),
                status: BashTaskStatus::Running,
            }),
            pid,
            notify: Arc::new(Notify::new()),
            lifecycle_emitted: parking_lot::Mutex::new(false),
        });
        self.tasks.write().insert(task_id.clone(), task.clone());
        if let (Some(guard), Some((scope, grant))) =
            (self.background_guard.as_ref(), bash_scope_grant)
        {
            guard.record_spawn_result(
                &audit_cmd,
                scope,
                grant,
                true,
                format!(
                    "background task started: task_id={} log_path={}",
                    task_id,
                    log_path.display()
                ),
            );
        }

        // 两条 pump 任务：stdout / stderr 边读边追加日志。
        // stderr 行前缀 "STDERR: " 让 task_output 拉到的内容仍可肉眼区分两路。
        // P1：每条 pump flush 后 `notify_waiters()`，唤醒所有挂在
        // `wait_for_finish` 上的等待者；按"文件长度 vs since"判定，避免 lost wakeup。
        spawn_pump(stdout, log_writer.clone(), "", task.notify.clone());
        spawn_pump(stderr, log_writer.clone(), "STDERR: ", task.notify.clone());

        // wait 任务：独占 Child handle 等结束 → 翻 status。
        // 注意：stop 已把 status 置为 Stopped 时，**不**回退覆盖成 Finished。
        let task_for_wait = task.clone();
        let lifecycle_tx = self.lifecycle_tx.clone();
        let task_id_for_wait = task_id.clone();
        let log_path_for_wait = log_path.display().to_string();
        let command_for_wait = command.clone();
        tokio::spawn(async move {
            let exit_code = match child.wait().await {
                Ok(status) => {
                    #[cfg(unix)]
                    {
                        status
                            .code()
                            .or_else(|| status.signal().map(|s| 128 + s))
                            .unwrap_or(-1)
                    }
                    #[cfg(not(unix))]
                    {
                        status.code().unwrap_or(-1)
                    }
                }
                Err(_) => -1,
            };
            let final_status = {
                let mut info = task_for_wait.info.write();
                if !matches!(info.status, BashTaskStatus::Stopped) {
                    info.status = BashTaskStatus::Finished { exit_code };
                }
                info.status.clone()
            };
            // P1：先 emit lifecycle（受 lifecycle_emitted guard 保护），再
            // notify_waiters；这样阻塞在 wait_for_finish 的 dispatcher 醒来后能
            // 立刻看到终态，host lifecycle subscriber 也能拿到 broadcast。
            let already_emitted = {
                let mut g = task_for_wait.lifecycle_emitted.lock();
                if *g {
                    true
                } else {
                    *g = true;
                    false
                }
            };
            if !already_emitted {
                let _ = lifecycle_tx.send(BackgroundTaskLifecycleEvent {
                    task_id: task_id_for_wait,
                    final_status,
                    log_path: log_path_for_wait,
                    command: command_for_wait,
                });
            }
            task_for_wait.notify.notify_waiters();
        });

        Ok(BashTaskTicket {
            task_id,
            log_path: log_path.display().to_string(),
            started_at_unix_ms: now,
        })
    }

    /// 拉日志增量：`since=None` 从头读；返回 `[start_offset, next_offset)` 的字节窗口
    /// （UTF-8 lossy 解码）。`finished=true` 时 `exit_code` 一定有值。
    pub async fn read_output(
        &self,
        task_id: &str,
        since: Option<u64>,
    ) -> Result<BashTaskOutputChunk, AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        let info_snap = task.info.read().clone();
        let log_path = Path::new(&info_snap.log_path);
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(log_path)
            .await
            .map_err(AppError::Io)?;
        let start = since.unwrap_or(0);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(AppError::Io)?;
        let mut buf = Vec::with_capacity(64 * 1024);
        file.read_to_end(&mut buf).await.map_err(AppError::Io)?;
        let next_offset = start + buf.len() as u64;
        let (finished, exit_code) = match info_snap.status {
            BashTaskStatus::Finished { exit_code } => (true, Some(exit_code)),
            BashTaskStatus::Stopped => (true, Some(-1)),
            BashTaskStatus::Running => (false, None),
        };
        Ok(BashTaskOutputChunk {
            task_id: task_id.to_string(),
            content: String::from_utf8_lossy(&buf).into_owned(),
            start_offset: start,
            next_offset,
            finished,
            exit_code,
        })
    }

    /// 取自 `since` 之后新增输出的有界尾巴。若自 `since` 起没有新字节，则
    /// `content` 为空；若新增内容超过 `max_bytes`，只保留尾部并把
    /// `start_offset` 前移到实际返回片段的起点。
    pub async fn read_output_tail(
        &self,
        task_id: &str,
        since: Option<u64>,
        max_bytes: u64,
    ) -> Result<BashTaskOutputChunk, AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        let info_snap = task.info.read().clone();
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&info_snap.log_path)
            .await
            .map_err(AppError::Io)?;
        let since = since.unwrap_or(0);
        let len_before = file.metadata().await.map_err(AppError::Io)?.len();
        let start = if len_before > since {
            std::cmp::max(since, len_before.saturating_sub(max_bytes))
        } else {
            since
        };
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(AppError::Io)?;
        let mut buf = Vec::with_capacity(max_bytes as usize);
        let mut limited = file.take(max_bytes);
        limited.read_to_end(&mut buf).await.map_err(AppError::Io)?;
        let next_offset = tokio::fs::metadata(&info_snap.log_path)
            .await
            .map_err(AppError::Io)?
            .len()
            .max(start + buf.len() as u64);
        let (finished, exit_code) = match info_snap.status {
            BashTaskStatus::Finished { exit_code } => (true, Some(exit_code)),
            BashTaskStatus::Stopped => (true, Some(-1)),
            BashTaskStatus::Running => (false, None),
        };
        Ok(BashTaskOutputChunk {
            task_id: task_id.to_string(),
            content: String::from_utf8_lossy(&buf).into_owned(),
            start_offset: start,
            next_offset,
            finished,
            exit_code,
        })
    }

    /// 主动停止：标记 status = Stopped → killpg(SIGKILL) 整组（Unix）；
    /// wait 任务后续 `child.wait()` 返回**不**回退覆盖（见 spawn 内 `if !matches!(...)`）。
    ///
    /// P1：stop 路径同样会 emit lifecycle 一次（由 `lifecycle_emitted` guard 兜底；
    /// stop 抢先 emit 后 wait 任务 return 时再尝试将被忽略），并 notify_waiters
    /// 唤醒所有挂在 `wait_for_finish` 上的等待者，让它们立即返回 `Finished`。
    pub async fn stop(&self, task_id: &str) -> Result<(), AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        let final_status_for_emit;
        let log_path_for_emit;
        let command_for_emit;
        {
            let mut info = task.info.write();
            if matches!(info.status, BashTaskStatus::Running) {
                info.status = BashTaskStatus::Stopped;
            }
            final_status_for_emit = info.status.clone();
            log_path_for_emit = info.log_path.clone();
            command_for_emit = info.command.clone();
        }
        #[cfg(unix)]
        if let Some(pid) = task.pid {
            // SAFETY: POSIX 信号 API；pid 来自仍存活（或已 reaped）的子进程，
            // ESRCH 在已退场景下出现也无副作用。
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        // Windows 下不依赖 killpg；已设置 `kill_on_drop(true)`，且 wait 任务会
        // 在 Child drop 时由 tokio 兜底——此处不再重复 kill 以免 race。

        // P1：emit lifecycle（被 lifecycle_emitted guard 收敛，wait 任务后续
        // 命中相同 task 的翻转尝试会被忽略）+ 唤醒所有挂在 wait_for_finish 的等待者。
        let already_emitted = {
            let mut g = task.lifecycle_emitted.lock();
            if *g {
                true
            } else {
                *g = true;
                false
            }
        };
        if !already_emitted {
            let _ = self.lifecycle_tx.send(BackgroundTaskLifecycleEvent {
                task_id: task_id.to_string(),
                final_status: final_status_for_emit,
                log_path: log_path_for_emit,
                command: command_for_emit,
            });
        }
        task.notify.notify_waiters();
        Ok(())
    }

    /// P1：阻塞等待任务终态翻转。
    ///
    /// 实现按"先 `notified()` 拿 future → 再读当前 status 判定"的标准
    /// race-free 顺序，避免 wait 任务 / stop 路径的 `notify_waiters()` 与等待者
    /// 注册之间出现 lost wakeup。调用方负责自己处理"超时"（在外层
    /// `tokio::select!` 套 `sleep_until`），这里只承诺"终态到了就返回"。
    ///
    /// `task_id` 不存在时返回 `AppError::Primitive`，与 `read_output` / `stop`
    /// 一致。
    pub async fn wait_for_finish(&self, task_id: &str) -> Result<(), AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        loop {
            // 关键顺序：先注册 notified()（等待者句柄），再做条件判定。
            // 反过来会与 wait/stop 的 `notify_waiters()` 之间存在标准
            // lost-wakeup 窗口。
            let notified = task.notify.notified();
            tokio::pin!(notified);

            let status_snap = task.info.read().status.clone();
            if !matches!(status_snap, BashTaskStatus::Running) {
                return Ok(());
            }

            notified.await;
        }
    }

    /// P1：取最近 `max_bytes` 字节（≤ 4 KiB 推荐）的尾部，UTF-8 lossy 解码。
    /// 给 host 构造 synthetic notification 的正文用。task 不存在或日志为空时
    /// 返回空串而**不**报错（tag 仍由 host 包裹）。
    pub async fn tail_log(&self, task_id: &str, max_bytes: u64) -> String {
        let log_path = match self.tasks.read().get(task_id).cloned() {
            Some(t) => t.info.read().log_path.clone(),
            None => return String::new(),
        };
        let mut file = match tokio::fs::OpenOptions::new()
            .read(true)
            .open(&log_path)
            .await
        {
            Ok(f) => f,
            Err(_) => return String::new(),
        };
        let len = match file.metadata().await {
            Ok(m) => m.len(),
            Err(_) => return String::new(),
        };
        let start = len.saturating_sub(max_bytes);
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return String::new();
        }
        let mut buf = Vec::with_capacity(max_bytes as usize);
        if file.read_to_end(&mut buf).await.is_err() {
            return String::new();
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// 取最近 `max_bytes` 字节的结构化 chunk，便于 `task_output(block=true)` 在 timeout
    /// 时回一份最近输出快照，而不是空切片。`start_offset/next_offset` 始终与返回内容一致，
    /// 可直接把 `next_offset` 作为后续续传游标。
    pub async fn tail_output_chunk(
        &self,
        task_id: &str,
        max_bytes: u64,
    ) -> Result<BashTaskOutputChunk, AppError> {
        let task = self
            .tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::Primitive(format!("bash task not found: {}", task_id)))?;
        let info_snap = task.info.read().clone();
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&info_snap.log_path)
            .await
            .map_err(AppError::Io)?;
        let len = file.metadata().await.map_err(AppError::Io)?.len();
        let start = len.saturating_sub(max_bytes);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(AppError::Io)?;
        let mut buf = Vec::with_capacity(max_bytes as usize);
        file.read_to_end(&mut buf).await.map_err(AppError::Io)?;
        let (finished, exit_code) = match info_snap.status {
            BashTaskStatus::Finished { exit_code } => (true, Some(exit_code)),
            BashTaskStatus::Stopped => (true, Some(-1)),
            BashTaskStatus::Running => (false, None),
        };
        Ok(BashTaskOutputChunk {
            task_id: task_id.to_string(),
            content: String::from_utf8_lossy(&buf).into_owned(),
            start_offset: start,
            next_offset: start + buf.len() as u64,
            finished,
            exit_code,
        })
    }

    /// P1：枚举单个 task 的元信息快照。给 host lifecycle subscriber 取
    /// command / log_path 用，避免重复 broadcast 大字段。
    pub fn get_info(&self, task_id: &str) -> Option<BashTaskInfo> {
        self.tasks
            .read()
            .get(task_id)
            .map(|t| t.info.read().clone())
    }

    /// 全量枚举：按 started_at 升序，便于模型一眼看出"谁先起、谁还在跑"。
    pub fn list(&self) -> Vec<BashTaskInfo> {
        let mut v: Vec<BashTaskInfo> = self
            .tasks
            .read()
            .values()
            .map(|t| t.info.read().clone())
            .collect();
        v.sort_by_key(|i| i.started_at_unix_ms);
        v
    }
}

fn spawn_pump<R>(
    reader: Option<R>,
    writer: Arc<AsyncMutex<tokio::fs::File>>,
    prefix: &'static str,
    notify: Arc<Notify>,
) where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
{
    let Some(reader) = reader else {
        return;
    };
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let mut buffered = BufReader::new(reader);
        loop {
            match buffered.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut f = writer.lock().await;
                    // 日志写入失败不应中断 pump：磁盘空间 / 句柄被外部强删等
                    // 都属于「best-effort 旁路落盘」语义，与同步 bash 路径里
                    // accumulate_with_persist 落盘失败仅 warn 同口径——若 fail
                    // 整把 bash 任务，会让 task_output 拿不到任何尾部内容、调试更难。
                    if !prefix.is_empty() {
                        let _ = f.write_all(prefix.as_bytes()).await;
                    }
                    let _ = f.write_all(&buf[..n]).await;
                    let _ = f.flush().await;
                    drop(f);
                    // P1：每次 flush 后唤醒所有挂在 `wait_for_finish` 上的等待者；
                    // 配合"先 notified() 再读 size"的 race-free 顺序，保证不丢字节、不丢唤醒。
                    notify.notify_waiters();
                }
                Err(_) => break,
            }
        }
    });
}

fn simple_rand6() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let chars = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::with_capacity(6);
    let mut x = nanos;
    for _ in 0..6 {
        s.push(chars[(x as usize) % chars.len()] as char);
        x = x.wrapping_mul(2_654_435_761).rotate_left(7) ^ (x >> 16);
    }
    s
}
