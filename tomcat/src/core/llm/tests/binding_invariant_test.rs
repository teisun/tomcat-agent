//! 架构不变量：生产路径不得绕过 [`ResolvedCall`] 密封构造。

use std::fs;
use std::path::{Path, PathBuf};

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

fn rel_src(path: &Path, src_root: &Path) -> String {
    path.strip_prefix(src_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn is_allowed_from_parts_path(rel: &str) -> bool {
    rel == "core/llm/resolver.rs"
        || rel.contains("/tests/")
        || rel.starts_with("tests/")
        || rel.ends_with("_test.rs")
        || rel.ends_with("/test_support.rs")
        || rel.contains("/test_support/")
}

#[test]
fn from_parts_unchecked_only_appears_in_resolver_and_test_code() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk_rs_files(&src_root, &mut files);

    let mut offenders = Vec::new();
    for path in &files {
        let rel = rel_src(path, &src_root);
        if is_allowed_from_parts_path(&rel) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if ["from_parts_unchecked", "from_provider_and_entry_unchecked"]
            .iter()
            .any(|name| text.contains(name))
        {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "生产路径不得调用 unchecked binding 工厂；违规文件: {offenders:?}"
    );
}

#[test]
fn resolved_call_struct_literal_only_allowed_inside_resolver() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk_rs_files(&src_root, &mut files);

    let mut offenders = Vec::new();
    for path in &files {
        let rel = rel_src(path, &src_root);
        if rel == "core/llm/resolver.rs" || is_allowed_from_parts_path(&rel) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        // 生产路径禁止结构体字面量（排除 `-> ResolvedCall {` 函数体开括号误伤）。
        let looks_like_literal = text.contains("ResolvedCall {\n")
            && (text.contains("provider_impl:")
                || text.contains("catalog_id:")
                || text.contains("sealed:"));
        if looks_like_literal {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "ResolvedCall 结构体字面量只能出现在 resolver.rs（或测试）；违规文件: {offenders:?}"
    );
}
