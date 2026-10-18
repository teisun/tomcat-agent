//! 在 PATH 中查找可执行文件。
//!
//! Windows 只认 PATHEXT 里的后缀（`.EXE` 优先于 `.CMD`）：npm/scoop 会在同一目录放一个
//! 无后缀的 sh 脚本（如 `rg`）和 `rg.cmd`，前者在 Windows 上无法直接启动。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// 按 PATH 目录顺序查找 `candidates` 中任一名字（同一目录内按 candidates 顺序）。
pub fn find_executable(candidates: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var_os("PATHEXT");
    find_executable_in(&path, pathext.as_deref(), candidates, |p| p.is_file())
}

/// 纯函数版本：PATH、PATHEXT 与文件存在性都由调用方提供。
pub(crate) fn find_executable_in(
    path: &OsStr,
    pathext: Option<&OsStr>,
    candidates: &[&str],
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let names: Vec<String> = candidates
        .iter()
        .flat_map(|name| executable_file_names(name, pathext))
        .collect();
    std::env::split_paths(path).find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| is_file(candidate))
    })
}

fn executable_file_names(name: &str, pathext: Option<&OsStr>) -> Vec<String> {
    if !cfg!(windows) {
        return vec![name.to_string()];
    }
    let pathext = pathext
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_string());
    let extensions: Vec<&str> = pathext
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .collect();
    let has_known_extension = Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|known| known.trim_start_matches('.').eq_ignore_ascii_case(ext))
        });
    if has_known_extension {
        return vec![name.to_string()];
    }
    extensions
        .iter()
        .map(|ext| format!("{name}{}", ext.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
#[path = "tests/find_executable_test.rs"]
mod tests;
