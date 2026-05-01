//! 共享路径授权菜单渲染。

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
    println!("\n--- 路径授权 ---");
    println!("路径: {}", target.display());
    if let Some(note) = note {
        println!("提示: {}", note);
    }
    println!("  [s] 本次会话允许访问当前目标路径");
    if let Some(root) = &suggested_root {
        println!(
            "  [w] 以后也允许访问 {}（写入 workspace.workspace_roots）",
            root.display()
        );
    }
    println!("  [c] 取消 / 拒绝当前操作");
    print!("选择: ");
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
