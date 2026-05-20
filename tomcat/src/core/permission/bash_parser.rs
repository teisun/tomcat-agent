//! # Bash 命令路径提取
//!
//! 用 `shell-words` 把命令切成 token，然后启发式提取"看起来像路径"的 token：
//!
//! - 以 `/` 开头（绝对路径）
//! - 以 `~` 开头（home 缩写）
//! - 以 `./` / `../` 开头（相对路径）
//! - 含 `/` 但不以 `-` 开头（相对路径，如 `src/main.rs`）
//!
//! 不会展开 glob、不会触碰 stdin/stdout 重定向；
//! 含 `|` `;` `&` `>` `<` 的命令会先按这些分隔符拆分子命令再分别提取。
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

use super::is_url_like;

/// 把 bash 命令拆成子命令并提取候选路径。
///
/// 返回值未做去重 / 规范化；调用方应负责把结果交给 gate.check 做规范化与判定。
pub fn extract_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for sub in split_subcommands(command) {
        let tokens = match shell_words::split(sub) {
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
        // 跳过命令名；leading assignment-only 子命令已经在上面被消费。
        let _cmd_name = iter.next();
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
    if is_url_like(s) {
        return false;
    }
    // 以 - 开头视作 flag。
    if s.starts_with('-') {
        return false;
    }
    if s.starts_with('/') || s.starts_with("~") || s.starts_with("./") || s.starts_with("../") {
        return true;
    }
    // 含 `/`：相对路径（src/main.rs）。
    s.contains('/')
}

/// 把一条命令按 `|` `;` `&` `>` `<` `&&` `||` 拆成子命令。
fn split_subcommands(cmd: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = cmd.as_bytes();
    let mut start = 0;
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
                    out.push(s);
                }
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
        out.push(s);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_absolute_path() {
        assert_eq!(extract_paths("cat /etc/passwd"), vec!["/etc/passwd"]);
    }

    #[test]
    fn extracts_relative_path() {
        assert_eq!(extract_paths("rm src/main.rs"), vec!["src/main.rs"]);
    }

    #[test]
    fn extracts_dot_relative() {
        assert_eq!(extract_paths("ls ./build"), vec!["./build"]);
    }

    #[test]
    fn extracts_tilde_path() {
        assert_eq!(extract_paths("cat ~/.bashrc"), vec!["~/.bashrc"]);
    }

    #[test]
    fn skips_flags() {
        let v = extract_paths("ls -la /tmp");
        assert_eq!(v, vec!["/tmp"]);
    }

    #[test]
    fn extracts_flag_value_paths() {
        let v = extract_paths("cargo --target-dir=/tmp/target build");
        assert_eq!(v, vec!["/tmp/target"]);
    }

    #[test]
    fn extracts_assignment_in_arg_position() {
        let v = extract_paths("stat -c %s p=/Users/a/file");
        assert_eq!(v, vec!["/Users/a/file"]);
    }

    #[test]
    fn extracts_leading_env_assignment_before_cmd() {
        let v = extract_paths("p=/Users/a/file ls -la \"$p\"");
        assert!(v.contains(&"/Users/a/file".to_string()));
    }

    #[test]
    fn extracts_leading_env_assignment_in_subcommand() {
        let v = extract_paths("p=/Users/a/file; cmd $p");
        assert!(v.contains(&"/Users/a/file".to_string()));
    }

    #[test]
    fn keeps_existing_flag_value_behavior() {
        let v = extract_paths("cargo --target-dir=/tmp/target build");
        assert_eq!(v, vec!["/tmp/target"]);
    }

    #[test]
    fn ignores_empty_rhs() {
        let v = extract_paths("p= cmd");
        assert!(v.is_empty());
    }

    #[test]
    fn ignores_non_identifier_lhs() {
        let v = extract_paths("echo 123=/path");
        assert_eq!(v, vec!["123=/path"]);
    }

    #[test]
    fn multiple_leading_assignments() {
        let v = extract_paths("A=/x B=/y cmd");
        assert_eq!(v, vec!["/x", "/y"]);
    }

    #[test]
    fn handles_pipes_and_subcommands() {
        let v = extract_paths("cat /etc/hosts | grep 127.0.0.1 > /tmp/out");
        // pipe 把命令拆成 [cat /etc/hosts, grep 127.0.0.1, /tmp/out]
        // 第三段 ">" 之后只剩 "/tmp/out" 整段；split_subcommands 得到 "/tmp/out"
        // shell_words::split("/tmp/out") = ["/tmp/out"]，第一个被当作命令名跳过 -> 空
        // 所以这里只拿到 /etc/hosts。
        assert!(v.contains(&"/etc/hosts".to_string()));
    }

    #[test]
    fn handles_quoted_strings() {
        let v = extract_paths("rm \"my file.txt\" /tmp/x");
        assert!(v.contains(&"/tmp/x".to_string()));
    }

    #[test]
    fn skips_http_and_https_urls() {
        let v = extract_paths("curl http://127.0.0.1:4173/ https://example.com/api");
        assert!(v.is_empty(), "URL-like token 不应被当成路径: {:?}", v);
    }

    #[test]
    fn skips_flag_value_urls() {
        let v = extract_paths("curl --url=https://example.com/api --output=/tmp/out");
        assert_eq!(v, vec!["/tmp/out"]);
    }

    #[test]
    fn skips_assignment_urls_but_keeps_real_paths() {
        let v = extract_paths("ENDPOINT=https://example.com/api ROOT=./src tool");
        assert_eq!(v, vec!["./src"]);
    }
}
