use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

fn sha256_file(path: &Path) -> String {
    let data = fs::read(path).unwrap_or_default();
    let hash = Sha256::digest(&data);
    format!("{:x}", hash)
}

/// Deterministic hash over an entire directory: sort all file paths, hash each,
/// then hash the concatenated (path, file_hash) pairs.
fn sha256_dir(dir: &Path) -> String {
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_files(dir, dir, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, file_hash) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update(file_hash.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn collect_files(base: &Path, current: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out);
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let hash = sha256_file(&path);
            out.push((rel, hash));
        }
    }
}

fn main() {
    let wasm_path = Path::new("assets/wasm/wasmedge_quickjs.wasm");
    let modules_dir = Path::new("assets/modules");

    let wasm_sha = if wasm_path.exists() {
        sha256_file(wasm_path)
    } else {
        String::new()
    };

    let modules_sha = if modules_dir.is_dir() {
        sha256_dir(modules_dir)
    } else {
        String::new()
    };

    println!("cargo:rustc-env=EMBEDDED_WASM_SHA256={}", wasm_sha);
    println!("cargo:rustc-env=EMBEDDED_MODULES_SHA256={}", modules_sha);

    println!("cargo:rerun-if-changed=assets/wasm/wasmedge_quickjs.wasm");
    println!("cargo:rerun-if-changed=assets/modules/");
    println!("cargo:rerun-if-changed=build.rs");
}
