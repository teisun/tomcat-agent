//! # 嵌入式资产抽取（`assets` 子模块）
//!
//! 覆盖：
//!
//! - `compute_file_sha256` / `compute_dir_sha256`：返回 64 位 hex、相同内容
//!   产生相同哈希、不同内容产生不同哈希。
//! - `write_atomic`：能创建嵌套目录与覆盖既存文件。
//! - `acquire_assets_lock`：会创建 `.lock` 文件；并发持锁不会死锁。
//! - `ensure_embedded_assets`：抽取 wasm + modules，幂等再次调用不报错；
//!   被本地篡改的 wasm 会被重新覆盖恢复（SHA mismatch 触发升级）。
//! - `extract_wasm_if_needed`：SHA 命中时跳过重写（mtime 不变）。
//! - 编译期常量 `EMBEDDED_WASM_SHA256` / `EMBEDDED_MODULES_SHA256` 必须为
//!   64 位 hex。

use super::super::assets::{
    acquire_assets_lock, compute_dir_sha256, compute_file_sha256, extract_wasm_if_needed,
    write_atomic, EMBEDDED_MODULES_SHA256, EMBEDDED_WASM_SHA256,
};
use super::super::*;
use super::mocks::cfg_with_work_dir;

#[test]
fn compute_file_sha256_returns_hex() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.bin");
    std::fs::write(&file, b"hello").unwrap();
    let hash = compute_file_sha256(&file).unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn compute_file_sha256_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("a.bin");
    let f2 = dir.path().join("b.bin");
    std::fs::write(&f1, b"same content").unwrap();
    std::fs::write(&f2, b"same content").unwrap();
    assert_eq!(
        compute_file_sha256(&f1).unwrap(),
        compute_file_sha256(&f2).unwrap()
    );
}

#[test]
fn compute_dir_sha256_deterministic() {
    let d1 = tempfile::tempdir().unwrap();
    std::fs::write(d1.path().join("a.txt"), b"aaa").unwrap();
    std::fs::write(d1.path().join("b.txt"), b"bbb").unwrap();

    let d2 = tempfile::tempdir().unwrap();
    std::fs::write(d2.path().join("a.txt"), b"aaa").unwrap();
    std::fs::write(d2.path().join("b.txt"), b"bbb").unwrap();

    assert_eq!(
        compute_dir_sha256(d1.path()).unwrap(),
        compute_dir_sha256(d2.path()).unwrap()
    );
}

#[test]
fn compute_dir_sha256_changes_on_content_diff() {
    let d1 = tempfile::tempdir().unwrap();
    std::fs::write(d1.path().join("a.txt"), b"aaa").unwrap();

    let d2 = tempfile::tempdir().unwrap();
    std::fs::write(d2.path().join("a.txt"), b"bbb").unwrap();

    assert_ne!(
        compute_dir_sha256(d1.path()).unwrap(),
        compute_dir_sha256(d2.path()).unwrap()
    );
}

#[test]
fn write_atomic_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sub").join("output.bin");
    write_atomic(&target, b"data").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"data");
}

#[test]
fn write_atomic_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("output.bin");
    std::fs::write(&target, b"old").unwrap();
    write_atomic(&target, b"new").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}

#[test]
fn acquire_assets_lock_creates_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    let _lock = acquire_assets_lock(dir.path()).unwrap();
    assert!(dir.path().join("assets").join(".lock").exists());
}

#[test]
fn ensure_embedded_assets_extracts_wasm_and_modules() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_with_work_dir(dir.path());
    ensure_work_dir_structure(&cfg).unwrap();
    ensure_embedded_assets(&cfg).unwrap();

    let wasm_path = dir
        .path()
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    assert!(wasm_path.exists(), "wasm file should be extracted");
    assert!(wasm_path.metadata().unwrap().len() > 0);

    let modules_dir = dir.path().join("assets").join("modules");
    assert!(modules_dir.is_dir(), "modules dir should be extracted");
    let count = std::fs::read_dir(&modules_dir).unwrap().count();
    assert!(count > 0, "modules dir should contain files");

    let versions = dir.path().join("assets").join(".versions.json");
    assert!(versions.exists(), ".versions.json should be created");
    let content = std::fs::read_to_string(&versions).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(!v["wasm_sha256"].as_str().unwrap_or("").is_empty());
    assert!(!v["modules_sha256"].as_str().unwrap_or("").is_empty());
}

#[test]
fn ensure_embedded_assets_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_with_work_dir(dir.path());
    ensure_work_dir_structure(&cfg).unwrap();
    ensure_embedded_assets(&cfg).unwrap();
    ensure_embedded_assets(&cfg).unwrap();

    let wasm_path = dir
        .path()
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    assert!(wasm_path.exists());
}

#[test]
fn ensure_embedded_assets_upgrades_on_sha_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_with_work_dir(dir.path());
    ensure_work_dir_structure(&cfg).unwrap();
    ensure_embedded_assets(&cfg).unwrap();

    let wasm_path = dir
        .path()
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    let original = std::fs::read(&wasm_path).unwrap();

    std::fs::write(&wasm_path, b"tampered content").unwrap();
    assert_ne!(std::fs::read(&wasm_path).unwrap(), original);

    ensure_embedded_assets(&cfg).unwrap();
    assert_eq!(
        std::fs::read(&wasm_path).unwrap(),
        original,
        "tampered wasm should be overwritten with embedded version"
    );
}

#[test]
fn extract_wasm_skips_when_sha_matches() {
    let dir = tempfile::tempdir().unwrap();
    extract_wasm_if_needed(dir.path()).unwrap();

    let wasm_path = dir
        .path()
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    let mtime_before = std::fs::metadata(&wasm_path).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(50));

    let result = extract_wasm_if_needed(dir.path()).unwrap();
    assert_eq!(result, wasm_path);

    let mtime_after = std::fs::metadata(&wasm_path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "file should not be rewritten when SHA matches"
    );
}

#[test]
fn embedded_sha256_constants_are_nonempty() {
    assert!(
        !EMBEDDED_WASM_SHA256.is_empty(),
        "compile-time wasm SHA-256 must be set"
    );
    assert!(
        !EMBEDDED_MODULES_SHA256.is_empty(),
        "compile-time modules SHA-256 must be set"
    );
    assert_eq!(EMBEDDED_WASM_SHA256.len(), 64);
    assert_eq!(EMBEDDED_MODULES_SHA256.len(), 64);
}

#[test]
fn concurrent_lock_does_not_deadlock() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = std::sync::Arc::new(dir.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();

    for _ in 0..2 {
        let p = Arc::clone(&path);
        let b = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            b.wait();
            let _lock = acquire_assets_lock(&p).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
