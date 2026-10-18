//! 架构不变量：平台差异只能写在 `infra/platform`，业务代码对上层统一接口编程。

use std::fs;
use std::path::{Path, PathBuf};

const SYSTEM_APIS: &[&str] = &[
    "std::os::",
    "libc::",
    "windows_sys::",
    "windows_registry::",
    "process_wrap::",
];
const CFG_OPENERS: &[&str] = &["cfg(", "cfg!(", "cfg_attr("];
const PLATFORM_PREDICATES: &[&str] = &["unix", "windows", "target_os", "target_family"];

fn walk_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn is_exempt(rel: &str) -> bool {
    rel.starts_with("infra/platform/")
        || rel.contains("/tests/")
        || rel.ends_with("_test.rs")
        || rel == "test_support.rs"
}

/// 违规所在的行号（从 1 开始）。整行注释不算；cfg 谓词可以跨行，测试门控（含 `test`）放行。
fn violations(source: &str) -> Vec<usize> {
    let code: Vec<&str> = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect();
    let mut lines: Vec<usize> = code
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.replace("std::os::raw", "");
            SYSTEM_APIS.iter().any(|api| line.contains(api))
        })
        .map(|(index, _)| index + 1)
        .collect();

    let text = code.join("\n");
    for opener in CFG_OPENERS {
        for (start, _) in text.match_indices(opener) {
            let predicate = balanced_parens(&text[start + opener.len()..]);
            if !predicate.contains("test")
                && PLATFORM_PREDICATES
                    .iter()
                    .any(|word| predicate.contains(word))
            {
                lines.push(text[..start].matches('\n').count() + 1);
            }
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// `rest` 从左括号之后开始，返回与之配对的右括号之前的内容。
fn balanced_parens(rest: &str) -> &str {
    let mut depth = 1;
    for (index, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[..index];
                }
            }
            _ => {}
        }
    }
    rest
}

#[test]
fn matcher_flags_platform_code_and_lets_test_gates_and_comments_pass() {
    for flagged in [
        "#[cfg(windows)]",
        "    if cfg!(target_os = \"macos\") {",
        "    unsafe { libc::killpg(group, libc::SIGKILL) };",
        "use std::os::unix::fs::PermissionsExt;",
        "#[cfg_attr(unix, allow(unused_mut))]",
        "let wrap = process_wrap::tokio::CommandWrap::from(command);",
    ] {
        assert_eq!(violations(flagged), [1], "should flag: {flagged}");
    }
    for allowed in [
        "#[cfg(all(test, unix))]",
        "// cfg(windows) 只是注释",
        "    /// 调用 `std::os::unix` 的说明",
        "#[cfg(feature = \"x\")]",
        "fn f(_: std::os::raw::c_int) {}",
        "let windows = cfg!(debug_assertions);",
    ] {
        assert!(violations(allowed).is_empty(), "should allow: {allowed}");
    }
    assert_eq!(
        violations("fn a() {}\n#[cfg(any(\n    target_os = \"macos\",\n))]\nfn b() {}"),
        [2],
        "a predicate spanning several lines is reported at its first line"
    );
}

#[test]
fn business_code_has_no_platform_cfg_or_system_crates() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk_rs_files(&src_root, &mut files);
    assert!(!files.is_empty(), "no sources under {}", src_root.display());

    let mut offenders = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&src_root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if is_exempt(&rel) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        offenders.extend(
            violations(&text)
                .into_iter()
                .map(|line| format!("{rel}:{line}")),
        );
    }
    assert!(
        offenders.is_empty(),
        "平台差异请放到 infra/platform，对上层只暴露统一接口：\n{}",
        offenders.join("\n")
    );
}
