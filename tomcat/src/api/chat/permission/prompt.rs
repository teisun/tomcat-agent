//! 共享路径授权菜单渲染。

use crate::infra::i18n::tr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 普通路径授权菜单结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPromptChoice {
    /// `[s]` 本次会话允许当前目标路径本身。
    AllowSession,
    /// `[w]` 持久允许 suggested_root。
    PersistWorkspaceRoot { root: PathBuf },
    /// `[c]` 取消 / 拒绝当前操作。
    Cancel,
}

/// 渲染普通路径 `[s]/[w]/[c]` 授权菜单并从 stdin 读取选择。
pub fn read_path_prompt(
    target: &Path,
    suggested_root: Option<PathBuf>,
    note: Option<&str>,
) -> io::Result<PathPromptChoice> {
    println!("{}", tr("terminal.permission.title", &[]));
    println!(
        "{}",
        tr(
            "terminal.permission.path",
            &[("path", &target.display().to_string())]
        )
    );
    if let Some(note) = note {
        println!("{}", tr("terminal.permission.note", &[("note", note)]));
    }
    println!("{}", tr("terminal.permission.session", &[]));
    if let Some(root) = &suggested_root {
        println!(
            "{}",
            tr(
                "terminal.permission.persist",
                &[("path", &root.display().to_string())]
            )
        );
    }
    println!("{}", tr("terminal.permission.cancel", &[]));
    print!("{}", tr("terminal.permission.choose", &[]));
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_lowercase();
    let choice = match answer.as_str() {
        "s" | "session" | "once" | "allow" => PathPromptChoice::AllowSession,
        "w" | "workspace" | "persist" => {
            if let Some(root) = suggested_root {
                PathPromptChoice::PersistWorkspaceRoot { root }
            } else {
                PathPromptChoice::Cancel
            }
        }
        "c" | "cancel" | "n" | "no" | "" => PathPromptChoice::Cancel,
        _ => PathPromptChoice::Cancel,
    };
    Ok(choice)
}
