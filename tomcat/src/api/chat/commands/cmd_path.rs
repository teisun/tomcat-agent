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
use crate::infra::i18n::tr;

use super::parse::{ChatCommand, ChatCommandOutcome};

pub(crate) fn parse_args(tokens: Vec<String>, original_line: &str) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, path] if is_path_token(path) => ChatCommand::Path {
            path: PathBuf::from(path),
            original_line: original_line.to_string(),
        },
        [_cmd] => ChatCommand::UsageError {
            message: tr("slash.path.required", &[]),
        },
        [_cmd, _path] => ChatCommand::UsageError {
            message: tr("slash.path.invalid", &[]),
        },
        [_cmd, ..] => ChatCommand::UsageError {
            message: tr("slash.path.single", &[]),
        },
        _ => ChatCommand::UsageError {
            message: tr("slash.path.required", &[]),
        },
    }
}

pub(crate) fn run(
    ctx: &ChatContext,
    path: PathBuf,
    _original_line: String,
    rl: &mut rustyline::DefaultEditor,
) -> ChatCommandOutcome {
    if let Err(msg) = precheck_existence(&path) {
        eprintln!("✗ {}", msg);
        return ChatCommandOutcome::Handled;
    }
    let opts = render_path_menu(&path, &*ctx.global_services.gate);
    let choice = render_menu_and_read(&path, &opts, rl);
    if choice != PathMenuChoice::Cancel {
        if let Err(e) = apply_menu_choice(ctx, &path, choice) {
            eprintln!("✗ {}: {}", path.display(), e);
        }
    }
    ChatCommandOutcome::Handled
}

/// `/path` 进入菜单前的存在性预检。
///
/// 不修改 [`is_path_token`]（拖拽场景仍允许 ASCII 不存在路径作为 token）；这里
/// 仅在 `/path` 命令链路上拒绝不存在路径，避免后续 `[w]` 写盘被
/// `workspace_roots` 校验回报「不是目录」之类的二段式错误。
pub(super) fn precheck_existence(path: &Path) -> Result<(), String> {
    if path.exists() {
        Ok(())
    } else {
        Err(tr(
            "slash.path.missing",
            &[("path", &path.display().to_string())],
        ))
    }
}

/// 计算 `[w]` 真正写入 `workspace.workspace_roots` 的目标路径。
///
/// - 文件 → 取父目录（`workspace_roots` 仅接受目录，见
///   [`crate::infra::config::resolve_workspace_roots_paths`]）。
/// - 目录或其它 → 原样返回。
///
/// 文件无父目录（理论上仅 `/` 根本身可达此分支，但根不会判为 `is_file()`）的极端情形
/// fallback 回原路径，由后续 `canonicalize` / 校验链给出清晰错误。
pub(super) fn effective_workspace_root(path: &Path) -> PathBuf {
    if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

/// 渲染 `[w]` 一行菜单文案；文件场景显式告知会回退到父目录。
pub(super) fn extra_root_menu_line(path: &Path) -> String {
    if path.is_file() {
        let parent = path.parent().unwrap_or(path);
        tr(
            "slash.path.parent",
            &[("path", &parent.display().to_string())],
        )
    } else {
        tr("slash.path.persist", &[])
    }
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
    use crate::core::tools::primitive::PrimitiveOperation;

    let probe = gate.check(PrimitiveOperation::Read, &path.to_string_lossy());
    match probe {
        Ok(PermissionDecision::Deny { .. }) => PathMenuOptions::deny_only(tr(
            "slash.path.denied",
            &[("path", &path.display().to_string())],
        )),
        Ok(PermissionDecision::Allow { grant, .. })
            if grant.grant_type == crate::core::permission::GrantType::PathRuleReadOnly =>
        {
            PathMenuOptions::readonly_only(tr(
                "slash.path.readonly",
                &[("path", &path.display().to_string())],
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
    println!("{}", tr("slash.path.title", &[]));
    println!(
        "{}",
        tr(
            "terminal.permission.path",
            &[("path", &path.display().to_string())]
        )
    );
    if let Some(note) = &opts.note {
        println!("{}", tr("terminal.permission.note", &[("note", note)]));
    }
    if opts.allow_once {
        println!("{}", tr("slash.path.session", &[]));
    }
    if opts.persist_extra_root {
        println!("{}", extra_root_menu_line(path));
    }
    if opts.persist_readonly {
        println!("{}", tr("slash.path.readonlyOption", &[]));
    }
    if opts.persist_deny {
        println!("{}", tr("slash.path.denyOption", &[]));
    }
    if opts.cancel {
        println!("{}", tr("slash.path.cancelOption", &[]));
    }
    print!("{}", tr("terminal.permission.choose", &[]));
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
            ctx.global_services
                .gate
                .grant_session(canon, GrantTrigger::DraggedPathMenu);
            eprintln!(
                "{}",
                tr(
                    "terminal.cwd.allowed",
                    &[("path", &path.display().to_string())]
                )
            );
            Ok(())
        }
        PathMenuChoice::PersistWorkspaceRoot => {
            precheck_read_allow(ctx, path)?;
            let target = effective_workspace_root(path);
            let canon = std::fs::canonicalize(&target).map_err(AppError::Io)?;
            let cfg_path = crate::api::cli::config_file_path()?;
            crate::infra::config::append_workspace_root_to_disk(
                &cfg_path,
                canon.to_string_lossy().into_owned(),
            )?;
            ctx.global_services
                .gate
                .grant_session(canon.clone(), GrantTrigger::DraggedPathMenu);
            eprintln!(
                "{}",
                tr(
                    "slash.path.persisted",
                    &[("path", &canon.display().to_string())]
                )
            );
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
            ctx.global_services.gate.grant_path_rule(PathRule {
                path: path.to_string_lossy().into_owned(),
                mode,
            });
            let status = match mode {
                PathRuleMode::Readonly => tr("slash.path.readonlyStatus", &[]),
                PathRuleMode::Deny => tr("slash.path.deniedStatus", &[]),
            };
            eprintln!(
                "{}",
                tr(
                    "slash.path.ruleUpdated",
                    &[("path", &path.display().to_string()), ("status", &status)]
                )
            );
            Ok(())
        }
        PathMenuChoice::Cancel => Ok(()),
    }
}

fn precheck_read_allow(ctx: &ChatContext, path: &Path) -> Result<PathBuf, AppError> {
    use crate::core::tools::primitive::PrimitiveOperation;

    let canon = crate::infra::platform::normalize_path(&path.to_string_lossy())
        .unwrap_or_else(|_| path.to_path_buf());
    match ctx
        .global_services
        .gate
        .check(PrimitiveOperation::Read, &canon.to_string_lossy())?
    {
        PermissionDecision::Deny { reason } => Err(AppError::Permission(tr(
            "slash.path.grantDenied",
            &[("path", &path.display().to_string()), ("reason", &reason)],
        ))),
        _ => Ok(canon),
    }
}
