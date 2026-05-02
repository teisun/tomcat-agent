//! `/path` command implementation.
//!
//! This command owns path-token validation, the authorization menu data model,
//! user-visible menu text, and the permission/config updates selected from the
//! menu.

use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};

use crate::api::chat::ChatContext;
use crate::core::permission::{PathRuleMode, PermissionDecision, PermissionGate};
use crate::infra::error::AppError;

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>, original_line: &str) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, path] if is_path_token(path) => ChatCommand::Path {
            path: PathBuf::from(path),
            original_line: original_line.to_string(),
        },
        [_cmd] => ChatCommand::UsageError {
            message: "用法错误：/path 需要一个路径参数。".to_string(),
        },
        [_cmd, _path] => ChatCommand::UsageError {
            message: "用法错误：/path 参数必须是一个路径。".to_string(),
        },
        [_cmd, ..] => ChatCommand::UsageError {
            message: "用法错误：/path 仅支持一个路径参数。".to_string(),
        },
        _ => ChatCommand::UsageError {
            message: "用法错误：/path 需要一个路径参数。".to_string(),
        },
    }
}

pub(crate) fn run(
    ctx: &ChatContext,
    path: PathBuf,
    _original_line: String,
    rl: &mut rustyline::DefaultEditor,
) -> ChatCommandOutcome {
    let opts = render_path_menu(&path, &*ctx.gate);
    let choice = render_menu_and_read(&path, &opts, rl);
    if choice != PathMenuChoice::Cancel {
        if let Err(e) = apply_menu_choice(ctx, &path, choice) {
            eprintln!("✗ {}: {}", path.display(), e);
        }
    }
    ChatCommandOutcome::Handled
}

/// 路径前缀判定：以 `/` 或 `~/` 开头，且长度 > 1。
///
/// 仅用作快速过滤；真正的纯路径合法性由 [`is_path_token`] 决定。
fn is_path_prefix_token(tok: &str) -> bool {
    if tok == "/" || tok == "~" {
        return false;
    }
    tok.starts_with('/') || tok.starts_with("~/")
}

pub fn is_path_token(tok: &str) -> bool {
    if !is_path_prefix_token(tok) {
        return false;
    }
    if Path::new(tok).exists() {
        return true;
    }
    tok.is_ascii()
}

/// TUI 菜单可用选项集合。`render_path_menu` 根据 path_rule 预检查结果裁剪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMenuOptions {
    /// `[a]` 本会话允许（SessionGrant）。
    pub allow_once: bool,
    /// `[w]` 加入工作区持久化（workspace_roots）。
    pub persist_extra_root: bool,
    /// `[r]` 加入只读规则（path_rules readonly）。
    pub persist_readonly: bool,
    /// `[d]` 加入禁止规则（path_rules deny）。
    pub persist_deny: bool,
    /// `[c]` 取消，按聊天处理。
    pub cancel: bool,
    /// 菜单顶部的提示信息（builtin deny / readonly 命中时给出说明）。
    pub note: Option<String>,
}

impl PathMenuOptions {
    /// 5 选项全开（默认场景）。
    pub fn full() -> Self {
        Self {
            allow_once: true,
            persist_extra_root: true,
            persist_readonly: true,
            persist_deny: true,
            cancel: true,
            note: None,
        }
    }

    /// 命中 deny —— 不再显示任何授权选项，只允许取消。
    pub fn deny_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: false,
            persist_extra_root: false,
            persist_readonly: false,
            persist_deny: false,
            cancel: true,
            note: Some(note.into()),
        }
    }

    /// 命中 readonly path_rule —— 允许确认本次读取，但不允许持久写入工作区。
    pub fn readonly_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: true,
            persist_extra_root: false,
            persist_readonly: true,
            persist_deny: true,
            cancel: true,
            note: Some(note.into()),
        }
    }
}

/// 基于 path_rules 预检查决定可用菜单选项（plan §7）。
///
/// 用 [`PermissionGate::check`] 模拟一次 read 操作：
///
/// - 命中 `Deny` —— 仅 `[c]`，警告"此路径已被禁止访问"；
/// - 命中 `PathRuleReadOnly` —— `[a]/[r]/[d]/[c]`，不允许 `[w]`；
/// - 其它 —— 全 5 选项。
pub fn render_path_menu(path: &Path, gate: &dyn PermissionGate) -> PathMenuOptions {
    use crate::core::primitives::PrimitiveOperation;

    let probe = gate.check(PrimitiveOperation::Read, &path.to_string_lossy());
    match probe {
        Ok(PermissionDecision::Deny { .. }) => {
            PathMenuOptions::deny_only(format!("该路径已被禁止读写访问：{}", path.display()))
        }
        Ok(PermissionDecision::Allow { grant, .. })
            if grant.grant_type == crate::core::permission::GrantType::PathRuleReadOnly =>
        {
            PathMenuOptions::readonly_only(format!(
                "这是只读路径，本次会话可以读取其中内容，但不能写入、修改或删除：{}",
                path.display()
            ))
        }
        _ => PathMenuOptions::full(),
    }
}

/// 用户在 TUI 菜单上选择的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMenuChoice {
    /// `[a]` SessionGrant（仅本会话）。
    AllowOnce,
    /// `[w]` 加入 `[workspace] workspace_roots`（持久化）。
    PersistWorkspaceRoot,
    /// `[r]` 追加 `path_rules` `readonly` 规则。
    PersistReadonly,
    /// `[d]` 追加 `path_rules` `deny` 规则。
    PersistDeny,
    /// `[c]` 取消，按聊天处理。
    Cancel,
}

impl PathMenuChoice {
    pub fn from_input(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "a" | "allow" | "allow_once" => Some(Self::AllowOnce),
            "w" | "workspace" | "persist" => Some(Self::PersistWorkspaceRoot),
            "r" | "readonly" => Some(Self::PersistReadonly),
            "d" | "deny" => Some(Self::PersistDeny),
            "c" | "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }
}

fn render_menu_and_read(
    path: &Path,
    opts: &PathMenuOptions,
    rl: &mut rustyline::DefaultEditor,
) -> PathMenuChoice {
    println!("\n--- 路径授权（/path）---");
    println!("路径: {}", path.display());
    if let Some(note) = &opts.note {
        println!("提示: {}", note);
    }
    if opts.allow_once {
        println!("  [a] 本次会话允许访问");
    }
    if opts.persist_extra_root {
        println!("  [w] 以后也允许访问（写入配置 workspace.workspace_roots）");
    }
    if opts.persist_readonly {
        println!("  [r] 设为只读：允许读取，禁止写入");
    }
    if opts.persist_deny {
        println!("  [d] 禁止访问：拒绝读取和写入");
    }
    if opts.cancel {
        println!("  [c] 取消授权，不发送给 LLM");
    }
    print!("选择: ");
    let _ = io::stdout().flush();

    let line = rl.readline("").unwrap_or_else(|_| "c".to_string());
    let Some(choice) = PathMenuChoice::from_input(&line) else {
        return PathMenuChoice::Cancel;
    };
    if is_choice_enabled(choice, opts) {
        choice
    } else {
        PathMenuChoice::Cancel
    }
}

fn is_choice_enabled(choice: PathMenuChoice, opts: &PathMenuOptions) -> bool {
    match choice {
        PathMenuChoice::AllowOnce => opts.allow_once,
        PathMenuChoice::PersistWorkspaceRoot => opts.persist_extra_root,
        PathMenuChoice::PersistReadonly => opts.persist_readonly,
        PathMenuChoice::PersistDeny => opts.persist_deny,
        PathMenuChoice::Cancel => opts.cancel,
    }
}

fn apply_menu_choice(
    ctx: &ChatContext,
    path: &Path,
    choice: PathMenuChoice,
) -> Result<(), AppError> {
    use crate::core::permission::{GrantTrigger, PathRule};

    match choice {
        PathMenuChoice::AllowOnce => {
            let canon = precheck_read_allow(ctx, path)?;
            ctx.gate.grant_session(canon, GrantTrigger::DraggedPathMenu);
            eprintln!("✓ {} 本次会话期间允许访问", path.display());
            Ok(())
        }
        PathMenuChoice::PersistWorkspaceRoot => {
            precheck_read_allow(ctx, path)?;
            let canon = std::fs::canonicalize(path).map_err(AppError::Io)?;
            let cfg_path = crate::api::cli::config_file_path()?;
            crate::infra::config::append_workspace_root_to_disk(
                &cfg_path,
                canon.to_string_lossy().into_owned(),
            )?;
            ctx.gate
                .grant_session(canon.clone(), GrantTrigger::DraggedPathMenu);
            eprintln!("✓ 已更新配置：以后允许访问 {}", canon.display());
            Ok(())
        }
        PathMenuChoice::PersistReadonly | PathMenuChoice::PersistDeny => {
            let mode = match choice {
                PathMenuChoice::PersistReadonly => PathRuleMode::Readonly,
                PathMenuChoice::PersistDeny => PathRuleMode::Deny,
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
            Ok(())
        }
        PathMenuChoice::Cancel => Ok(()),
    }
}

fn precheck_read_allow(ctx: &ChatContext, path: &Path) -> Result<PathBuf, AppError> {
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
