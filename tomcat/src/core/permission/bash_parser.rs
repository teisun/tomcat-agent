//! # Bash 显式路径提取
//!
//! 该模块只用于 `execute_bash` 的**显式路径预检**与相关测试。
//! 过去这里会用启发式从命令字符串里猜测路径，但这会误伤 `node:fs/promises`、
//! `@scope/pkg`、jq 过滤式、`node -e` / heredoc 脚本等大量非文件 token。
//!
//! 现在只保留**显式前缀**路径识别：
//!
//! - 以 `/` 开头（绝对路径）
//! - 以 `~` 开头（home 缩写）
//! - 以 `./` / `../` 开头（相对路径）
//!
//! 不会展开 glob；对 `>` / `<` 仅把后续显式 token 视作重定向目标路径；
//! 含 `|` `;` `&` `>` `<` 的命令会先按这些分隔符拆分子命令/重定向段再分别提取。
//!
//! 命令前缀（如 `rm`、`echo`）不视作路径；
//! `--flag=value` 中的 `value` 若像路径则会被提取。
//! `NAME=/path` 形式的 assignment 会只提取 RHS，覆盖命令前缀、位置参数和子命令首段。
//!
//! # 注意
//!
//! - 解析失败（非法 quoting 等）时静默返回空列表，由调用方决定后续策略
//!   （通常做法是仍让 gate.check_bash regex 跑一遍 forbidden / approval）。
//! - 该解析器是**保守的尽力而为**：不能依赖它发现"所有"路径；
//!   逃逸（如 `eval $X`、`bash -c '...'` 中嵌套）由 gate 顶层的
//!   `bash_forbidden` regex 兜底（plan §4.1/§4.3）。

use std::path::PathBuf;

/// 把 bash 命令拆成子命令并提取候选路径。
///
/// 返回值未做去重 / 规范化；调用方应负责把结果交给 gate.check 做规范化与判定。
pub fn extract_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for segment in split_subcommands(command) {
        let tokens = match shell_words::split(segment.text) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut iter = tokens.iter().peekable();
        while let Some(tok) = iter.peek() {
            let Some(rhs) = is_env_assignment(tok) else {
                break;
            };
            if looks_like_path(rhs) {
                paths.push(rhs.to_string());
            }
            iter.next();
        }
        if matches!(segment.kind, SegmentKind::Command) {
            // 跳过命令名；leading assignment-only 子命令已经在上面被消费。
            let _cmd_name = iter.next();
        }
        for tok in iter {
            collect_candidates(tok, &mut paths);
        }
    }
    paths
}

fn collect_candidates(tok: &str, out: &mut Vec<String>) {
    if let Some(rhs) = is_env_assignment(tok) {
        if looks_like_path(rhs) {
            out.push(rhs.to_string());
        }
        return;
    }

    // 处理 --flag=value
    if let Some(eq) = tok.find('=') {
        // 仅当 token 以 `-` 开头时才视为 flag
        if tok.starts_with('-') {
            let value = &tok[eq + 1..];
            if looks_like_path(value) {
                out.push(value.to_string());
            }
            return;
        }
    }
    if looks_like_path(tok) {
        out.push(tok.to_string());
    }
}

fn is_env_assignment(tok: &str) -> Option<&str> {
    let eq = tok.find('=')?;
    let name = &tok[..eq];
    let rhs = &tok[eq + 1..];
    if name.is_empty() {
        return None;
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(rhs)
}

fn looks_like_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // 以 - 开头视作 flag。
    if s.starts_with('-') {
        return false;
    }
    s.starts_with('/') || s.starts_with("~") || s.starts_with("./") || s.starts_with("../")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentKind {
    Command,
    RedirectTarget,
}

#[derive(Clone, Copy, Debug)]
struct CommandSegment<'a> {
    text: &'a str,
    kind: SegmentKind,
}

/// 把一条命令按 `|` `;` `&` `>` `<` `&&` `||` 拆成子命令。
fn split_subcommands(cmd: &str) -> Vec<CommandSegment<'_>> {
    let mut out = Vec::new();
    let bytes = cmd.as_bytes();
    let mut start = 0;
    let mut next_kind = SegmentKind::Command;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'|' | b'&' | b';' | b'>' | b'<' if !in_single && !in_double => {
                let s = cmd[start..i].trim();
                if !s.is_empty() {
                    out.push(CommandSegment {
                        text: s,
                        kind: next_kind,
                    });
                }
                next_kind = if matches!(c, b'>' | b'<') {
                    SegmentKind::RedirectTarget
                } else {
                    SegmentKind::Command
                };
                // 跳过连续的 |&;<> 组合（&&、||、>>、<<、>>>）。
                i += 1;
                while i < bytes.len() {
                    let nc = bytes[i];
                    if matches!(nc, b'|' | b'&' | b';' | b'>' | b'<') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let s = cmd[start..].trim();
    if !s.is_empty() {
        out.push(CommandSegment {
            text: s,
            kind: next_kind,
        });
    }
    out
}

/// 便利函数：把提取的路径展开 `~` 后转为 `PathBuf`（不 canonicalize）。
#[allow(dead_code)]
pub fn expand_extracted(paths: &[String]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|p| {
            if let Some(rest) = p.strip_prefix("~/") {
                dirs::home_dir().map(|h| h.join(rest))
            } else if p == "~" {
                dirs::home_dir()
            } else {
                Some(PathBuf::from(p))
            }
        })
        .collect()
}

