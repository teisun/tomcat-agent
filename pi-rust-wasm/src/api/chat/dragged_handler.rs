//! 拖拽路径授权菜单处理。
//!
//! `dragged_path` 只负责把用户输入分类为纯路径或普通文本；本模块负责菜单交互、
//! 授权落盘，以及 deny/cancel 后写入可追溯的合成 user note。

use std::io::{self, Write as IoWrite};

use crate::infra::error::AppError;

use super::dragged_path::{
    interpret_dragged_paths, render_drag_menu, DragOutcome, MenuChoice, MenuOptions,
};
use super::ChatContext;

pub(super) enum DragHandleResult {
    /// 把 `line` 作为本回合用户消息发给 LLM。
    Continue { line: String },
    /// 菜单选项已经处理完，本轮不进入 LLM。
    Skip,
    /// 写入合成 user note 后，本轮不进入 LLM。
    RecordUserAndSkip { synth_user_msg: String },
}

struct CancelRecord {
    path: String,
    deny: bool,
}

pub(super) fn handle_dragged_input(
    ctx: &ChatContext,
    input: &str,
    rl: &mut rustyline::DefaultEditor,
) -> DragHandleResult {
    match interpret_dragged_paths(input) {
        DragOutcome::None => DragHandleResult::Continue {
            line: input.to_string(),
        },
        DragOutcome::PromptMenu {
            paths,
            original_line,
        } => {
            let mut any_persisted = false;
            let mut cancels = Vec::new();
            for p in &paths {
                let opts = render_drag_menu(p, &*ctx.gate);
                let choice = render_menu_and_read(p, &opts, rl);
                if choice == MenuChoice::Cancel {
                    cancels.push(CancelRecord {
                        path: p.display().to_string(),
                        deny: is_deny_only(&opts),
                    });
                    continue;
                }
                match apply_menu_choice(ctx, p, choice) {
                    Ok(persisted) => {
                        any_persisted |= persisted;
                    }
                    Err(e) => {
                        eprintln!("✗ {}: {}", p.display(), e);
                    }
                }
            }
            if !cancels.is_empty() {
                return DragHandleResult::RecordUserAndSkip {
                    synth_user_msg: build_drag_cancel_note(&cancels),
                };
            }
            if any_persisted {
                DragHandleResult::Skip
            } else {
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
        println!("  [c] 取消授权，不发送给 LLM");
    }
    print!("选择: ");
    let _ = io::stdout().flush();

    let line = rl.readline("").unwrap_or_else(|_| "c".to_string());
    let Some(choice) = MenuChoice::from_input(&line) else {
        return MenuChoice::Cancel;
    };
    if is_choice_enabled(choice, opts) {
        choice
    } else {
        MenuChoice::Cancel
    }
}

fn is_choice_enabled(choice: MenuChoice, opts: &MenuOptions) -> bool {
    match choice {
        MenuChoice::AllowOnce => opts.allow_once,
        MenuChoice::PersistExtraRoot => opts.persist_extra_root,
        MenuChoice::PersistReadonly => opts.persist_readonly,
        MenuChoice::PersistDeny => opts.persist_deny,
        MenuChoice::Cancel => opts.cancel,
    }
}

fn is_deny_only(opts: &MenuOptions) -> bool {
    opts.cancel
        && !opts.allow_once
        && !opts.persist_extra_root
        && !opts.persist_readonly
        && !opts.persist_deny
}

fn build_drag_cancel_note(records: &[CancelRecord]) -> String {
    if records.len() == 1 {
        let record = &records[0];
        if record.deny {
            return format!(
                "[drag-cancel] 用户拖拽 {} 后命中 deny；本次输入未发送给 LLM。",
                record.path
            );
        }
        return format!(
            "[drag-cancel] 用户拖拽 {} 后选择取消；本次输入未发送给 LLM。",
            record.path
        );
    }

    let details = records
        .iter()
        .map(|record| {
            let reason = if record.deny {
                "命中 deny"
            } else {
                "用户取消"
            };
            format!("{}: {}", record.path, reason)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("[drag-cancel] 用户拖拽路径后未发送给 LLM：{}。", details)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_cancel_note_uses_fixed_prefix() {
        let msg = build_drag_cancel_note(&[CancelRecord {
            path: "/tmp/secret".to_string(),
            deny: true,
        }]);

        assert!(msg.starts_with("[drag-cancel]"));
        assert!(msg.contains("/tmp/secret"));
        assert!(msg.contains("命中 deny"));
        assert!(msg.contains("未发送给 LLM"));
    }

    #[test]
    fn normal_cancel_note_uses_fixed_prefix() {
        let msg = build_drag_cancel_note(&[CancelRecord {
            path: "/tmp/project".to_string(),
            deny: false,
        }]);

        assert!(msg.starts_with("[drag-cancel]"));
        assert!(msg.contains("/tmp/project"));
        assert!(msg.contains("选择取消"));
        assert!(msg.contains("未发送给 LLM"));
    }
}
