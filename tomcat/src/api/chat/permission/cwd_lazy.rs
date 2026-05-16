//! # Cwd Lazy First-Touch 范围级授权（plan §8.2 hotfix）
//!
//! 装饰器 [`CwdLazyPrompt`] 包裹底层的 [`UserConfirmationProvider`]
//! ([`crate::api::chat::CliConfirmation`])，在 LLM 工具调用首次落到 `cwd` 子树
//! 内未授权路径时，弹出**一次性**的 3 选项范围级提示：
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ PermissionGate.check(target)                                     │
//! │     └─ NeedConfirm                                               │
//! │         └─ confirmation.confirm_decision(...)                    │
//! │                 └─ CwdLazyPrompt::confirm_decision               │
//! │                     ├─ dismissed? ────────────────► inner        │
//! │                     ├─ Bash op? ───────────────────► inner       │
//! │                     ├─ target ∉ cwd 子树? ──────────► inner       │
//! │                     ├─ cwd 已授权? ─────────────────► inner       │
//! │                     ├─ stdin 非 TTY? dismissed=true ► inner       │
//! │                     └─ 弹 [s]/[w]/[c]                            │
//! │                         ├─ [s] 仅 session_grants ────► AllowOnce │
//! │                         ├─ [w] 写盘 + session_grants ► AllowOnce │
//! │                         └─ [c] dismissed=true ───────► Deny      │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 关键设计
//!
//! - **首次触达原则**：装饰器只在「LLM 真的要碰 cwd 内文件」时才出现一次范围级
//!   提示。`[s]/[w]` 把 cwd 整体写进 `SessionGrants`，下次同子树访问被
//!   `PermissionGate.check` 直接 Allow，根本不再进 confirm 层。
//! - **`AllowOnce` 而非 `AllowAndPersistRoot`**：`[w]` 由本装饰器自己写盘，
//!   返回 `AllowOnce` 是因为执行器不需要再追加一次 `workspace_roots`；执行器侧
//!   `gate_check_path` 收到 `AllowOnce` 后会同步把 cwd 加进 SessionGrants。
//! - **`dismissed` 流程末梢**：用户选 `[c]` 后拒绝当前操作，后续同会话内不再就 cwd
//!   范围弹此提示，退化为 `CliConfirmation` 逐文件 3 选项 UX。配合 `Arc<AtomicBool>`
//!   保证装饰器与 `ChatContext` 同生命周期共享。
//! - **非 TTY 兜底**：CI/管道场景 `stdin().is_terminal() == false` 时设置
//!   dismissed 并 fall-through，避免阻塞读取 stdin。

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::permission::{GrantTrigger, PermissionDecision, PermissionGate, SessionGrants};
use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::error::AppError;

pub const CWD_PROMPT_CHOICES: &str = "[s/w/c]";

/// 用户在 cwd 范围级提示中的选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CwdPromptChoice {
    /// `[s]` 仅本会话允许（写 SessionGrants，不写盘）。
    AllowSessionOnly,
    /// `[w]` 加入工作区（写盘 `workspace_roots` + 当前会话 SessionGrants 同时生效）。
    AddPersistent,
    /// `[c]` 取消当前操作；本会话内 dismissed=true，后续按文件粒度逐次询问。
    Cancel,
}

/// 解析用户输入字符串为 [`CwdPromptChoice`]。
///
/// 返回 `None` 表示无法识别 —— 调用方会提示并按 `[c] Cancel` 处理。
pub fn parse_choice(s: &str) -> Option<CwdPromptChoice> {
    match s.trim().to_lowercase().as_str() {
        "s" | "session" | "once" => Some(CwdPromptChoice::AllowSessionOnly),
        "w" | "workspace" | "persist" => Some(CwdPromptChoice::AddPersistent),
        "c" | "cancel" | "n" | "no" | "skip" => Some(CwdPromptChoice::Cancel),
        _ => None,
    }
}

pub fn unrecognized_choice_message(input: &str) -> String {
    format!(
        "未识别的选项「{}」；可选项为 [s] / [w] / [c]，本次按取消处理。",
        input.trim()
    )
}

/// 判断 `target` 是否在 `cwd` 子树内（含 cwd 自身）。
///
/// 路径已由 caller 规范化（`gate_check_path` 走 `normalize_path` + canonicalize），
/// 这里仅做前缀比对，不再二次 IO。
pub fn target_in_cwd(target: &Path, cwd: &Path) -> bool {
    if target == cwd {
        return true;
    }
    target.starts_with(cwd)
}

fn cwd_already_authorized(cwd: &Path, gate: &dyn PermissionGate) -> bool {
    let er = gate.effective_roots();
    er.read_write.iter().any(|p| p == cwd) || er.read_only.iter().any(|p| p == cwd)
}

/// 从 [`crate::core::tools::primitive::DefaultPrimitiveExecutor::gate_check_path`]
/// 拼装的 `preview` 中提取真实目标路径。
///
/// 现行格式（`gate_check_path`）：
/// ```text
/// [Read] 读取
/// 路径: /Users/yan/work/sub/file.txt
/// 原因: 路径 `/Users/yan/work/sub/file.txt` 不在已授权范围内
/// ```
///
/// 解析失败（`tools::config_tool` 等其它入口不带 `路径:` 行）时返回 `None`，
/// 装饰器将 fall-through 给底层 provider。
fn extract_target_from_preview(preview: &str) -> Option<PathBuf> {
    for line in preview.lines() {
        if let Some(rest) = line.strip_prefix("路径: ") {
            let s = rest.trim();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }
    None
}

/// TTY 场景下从 stdin 读一行；EOF/IO 错误返回 `None`。
fn read_choice_from_stdin() -> Option<String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return None;
    }
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return None;
    }
    Some(line)
}

/// `UserConfirmationProvider` 装饰器：仅当 op 目标 `target` ∈ `cwd` 子树
/// 且 cwd 尚未授权且本会话未 dismiss 时，弹「[s] 仅本会话 / [w] 加入工作区 / [c] 取消」
/// 3 选项范围级提示。其余情况一律转发给 `inner`。
///
/// # Lifetime / Sharing
///
/// `dismissed` 用 `Arc<AtomicBool>` 包装，整个 `ChatContext` 生命周期内单例。
/// `session_grants` / `gate` / `cfg_path` 与 `ChatContext` 共享同一份。
pub struct CwdLazyPrompt {
    inner: Arc<dyn UserConfirmationProvider>,
    cwd: PathBuf,
    gate: Arc<dyn PermissionGate>,
    session_grants: SessionGrants,
    cfg_path: PathBuf,
    dismissed: Arc<AtomicBool>,
}

impl CwdLazyPrompt {
    pub fn new(
        inner: Arc<dyn UserConfirmationProvider>,
        cwd: PathBuf,
        gate: Arc<dyn PermissionGate>,
        session_grants: SessionGrants,
        cfg_path: PathBuf,
    ) -> Self {
        Self {
            inner,
            cwd,
            gate,
            session_grants,
            cfg_path,
            dismissed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 测试钩子：注入预制的 dismissed 标志，便于多 provider 共享 dismissed。
    #[cfg(test)]
    pub fn with_dismissed(mut self, dismissed: Arc<AtomicBool>) -> Self {
        self.dismissed = dismissed;
        self
    }

    fn render_prompt(&self, target: &Path) {
        eprintln!("─────────────────────────────────────────────────────────────");
        eprintln!("当前目录 {} 尚未授权访问。", self.cwd.display());
        eprintln!("即将操作: {}", target.display());
        eprintln!("[s] 本次会话期间允许访问");
        eprintln!(
            "[w] 以后也允许访问（写入配置 ~/.tomcat/tomcat.config.toml workspace.workspace_roots）"
        );
        eprintln!("[c] 取消本次操作（后续按文件粒度逐次询问）");
        eprint!("选择 {}: ", CWD_PROMPT_CHOICES);
        let _ = io::stderr().flush();
    }
}

#[async_trait]
impl UserConfirmationProvider for CwdLazyPrompt {
    async fn confirm(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
    ) -> Result<bool, AppError> {
        // 旧 API 不带 `suggested_root` —— 直接转发。新代码路径走
        // `confirm_decision`（gate_check_path / tools::config_tool 都用此版）。
        self.inner.confirm(operation, preview, plugin_id).await
    }

    async fn confirm_decision(
        &self,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        if self.dismissed.load(Ordering::Acquire) {
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        }

        if matches!(operation, PrimitiveOperation::Bash) {
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        }

        let Some(target) = extract_target_from_preview(preview) else {
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        };

        if !target_in_cwd(&target, &self.cwd) {
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        }

        if cwd_already_authorized(&self.cwd, &*self.gate) {
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        }

        if !io::stdin().is_terminal() {
            self.dismissed.store(true, Ordering::Release);
            return self
                .inner
                .confirm_decision(operation, preview, plugin_id, suggested_root)
                .await;
        }

        self.render_prompt(&target);
        let raw_choice = read_choice_from_stdin().unwrap_or_default();
        let choice = parse_choice(&raw_choice).unwrap_or_else(|| {
            eprintln!("{}", unrecognized_choice_message(&raw_choice));
            CwdPromptChoice::Cancel
        });
        self.apply_choice(choice, operation, preview, plugin_id, suggested_root)
            .await
    }
}

impl CwdLazyPrompt {
    /// 把用户在 [s]/[w]/[c] 中的选择落到副作用：
    ///
    /// - `[s]` AllowSessionOnly：仅加入 SessionGrants → `AllowOnce`
    /// - `[w]` AddPersistent：写盘 `workspace_roots` + 加入 SessionGrants → `AllowOnce`
    /// - `[c]` Cancel：设 dismissed=true 后拒绝当前操作
    ///
    /// 抽离成单独方法是为了让单测可以直接驱动 `[s]/[w]` 分支，无需 TTY 注入。
    async fn apply_choice(
        &self,
        choice: CwdPromptChoice,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
        _suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        match choice {
            CwdPromptChoice::AddPersistent => {
                let canon = std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());
                ensure_not_denied(&*self.gate, &canon)?;
                if let Err(e) = crate::infra::config::append_workspace_root_to_disk(
                    &self.cfg_path,
                    canon.to_string_lossy().into_owned(),
                ) {
                    eprintln!(
                        "✗ 持久化失败：{}；已改为仅本次会话允许访问 {}",
                        e,
                        canon.display()
                    );
                }
                self.session_grants.add(canon, GrantTrigger::CwdLazyPrompt);
                eprintln!("✓ {} 本次会话期间允许访问", self.cwd.display());
                Ok(ConfirmDecision::AllowOnce)
            }
            CwdPromptChoice::AllowSessionOnly => {
                let canon = std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());
                ensure_not_denied(&*self.gate, &canon)?;
                self.session_grants
                    .add(canon.clone(), GrantTrigger::CwdLazyPrompt);
                eprintln!("✓ {} 本次会话期间允许访问", canon.display());
                Ok(ConfirmDecision::AllowOnce)
            }
            CwdPromptChoice::Cancel => {
                self.dismissed.store(true, Ordering::Release);
                eprintln!("✓ 已取消：本会话内不再就 cwd 范围弹此提示，转入逐文件确认");
                Ok(ConfirmDecision::Deny)
            }
        }
    }

    /// 测试钩子：直接驱动 `[s]/[w]/[c]` 三分支副作用，无需 TTY 注入。
    ///
    /// 命名上保留 `_for_test` 后缀以提醒非测试代码路径不应调用；保持
    /// `pub` 而非 `#[cfg(test)]` 是为了让 `tests/cwd_lazy_prompt_e2e.rs`
    /// 这种独立 crate 的集成测试可见——cfg(test) 仅在编译当前 lib 的
    /// test profile 时生效，不会传播到外部 test crate。
    #[doc(hidden)]
    pub async fn apply_choice_for_test(
        &self,
        choice: CwdPromptChoice,
        operation: PrimitiveOperation,
        preview: &str,
        plugin_id: &str,
        suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        self.apply_choice(choice, operation, preview, plugin_id, suggested_root)
            .await
    }

    /// 测试钩子：返回 dismissed 当前值（同 `apply_choice_for_test` 暴露原因）。
    #[doc(hidden)]
    pub fn is_dismissed(&self) -> bool {
        self.dismissed.load(Ordering::Acquire)
    }
}

fn ensure_not_denied(gate: &dyn PermissionGate, path: &Path) -> Result<(), AppError> {
    match gate.check(PrimitiveOperation::Read, &path.to_string_lossy())? {
        PermissionDecision::Deny { reason } => Err(AppError::Permission(format!(
            "该路径已被禁止访问，无法加入当前会话或配置：{} ({})",
            path.display(),
            reason
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "../tests/cwd_lazy_prompt_test.rs"]
mod tests;
