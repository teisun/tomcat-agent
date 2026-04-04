use super::assets::{
    acquire_assets_lock, compute_dir_sha256, compute_file_sha256, extract_wasm_if_needed,
    write_atomic, EMBEDDED_MODULES_SHA256, EMBEDDED_WASM_SHA256,
};
use super::*;
use std::io::Write;

#[test]
fn default_app_config_roundtrip() {
    let cfg = AppConfig::default();
    let j = serde_json::to_string(&cfg).unwrap();
    let _: AppConfig = serde_json::from_str(&j).unwrap();
}

#[test]
fn security_config_default() {
    let _ = SecurityConfig::default();
}

#[test]
fn validate_config_accepts_valid() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_invalid_log_level() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "invalid".to_string();
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_zero_audit_retention() {
    let mut cfg = AppConfig::default();
    cfg.security.audit_log_retention_days = 0;
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_invalid_proxy() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.llm.proxy = Some("socks5://127.0.0.1:1080".to_string());
    assert!(validate_config(&cfg).is_err());
    cfg.llm.proxy = Some("http://127.0.0.1:7890".to_string());
    assert!(validate_config(&cfg).is_ok());
    cfg.llm.proxy = Some("https://proxy.example.com".to_string());
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn validate_config_rejects_duplicate_extra_roots() {
    let dir = tempfile::tempdir().unwrap();
    let c = std::fs::canonicalize(dir.path()).unwrap();
    let s = c.to_string_lossy().into_owned();
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace.extra_roots = vec![s.clone(), s];
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_rejects_nonexistent_extra_root() {
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace
        .extra_roots
        .push("/nonexistent/pi_workspace_root_test_path".to_string());
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn validate_config_accepts_extra_roots_when_dirs_exist() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.log.level = "info".to_string();
    cfg.workspace.extra_roots = vec![
        d1.path().to_str().unwrap().to_string(),
        d2.path().to_str().unwrap().to_string(),
    ];
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn resolve_extra_roots_skips_blank_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.workspace.extra_roots = vec!["  ".to_string(), dir.path().to_str().unwrap().to_string()];
    let roots = resolve_extra_roots_paths(&cfg).unwrap();
    assert_eq!(roots.len(), 1);
}

#[test]
fn load_config_none_path_returns_default_or_env() {
    let r = load_config(None);
    assert!(r.is_ok());
}

#[test]
fn load_config_from_existing_file() {
    let dir = std::env::temp_dir().join("pi_wasm_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"[log]\nlevel = \"debug\"\n").unwrap();
    drop(f);
    let r = load_config(Some(path.as_path()));
    assert!(r.is_ok());
    let cfg = r.unwrap();
    assert!(validate_config(&cfg).is_ok());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_from_example_file() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let example_path = manifest_dir.join("pi.config.toml.example");
    if !example_path.exists() {
        return;
    }
    let content = std::fs::read_to_string(&example_path).unwrap();
    let dir = std::env::temp_dir().join("pi_wasm_example_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let temp_toml = dir.join("config.toml");
    std::fs::write(&temp_toml, &content).unwrap();
    let r = load_config(Some(temp_toml.as_path()));
    let _ = std::fs::remove_file(&temp_toml);
    let _ = std::fs::remove_dir(&dir);
    let cfg = r.unwrap_or_else(|e| {
        panic!(
            "pi.config.toml.example 内容应可被 load_config 反序列化: {}",
            e
        )
    });
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn deserialize_security_config_uses_default_helpers() {
    let s = r#"{"security":{}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert!(cfg.security.enable_audit_log);
    assert_eq!(cfg.security.audit_log_retention_days, 90);
}

fn cfg_with_work_dir(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.to_string_lossy().to_string());
    cfg
}

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
fn context_config_default_values() {
    let cfg = ContextConfig::default();
    assert_eq!(cfg.context_window, 400_000);
    assert_eq!(cfg.max_output_tokens, 128_000);
    assert_eq!(cfg.compaction_turns, 10);
    assert_eq!(cfg.keep_recent_turns, 3);
    assert_eq!(cfg.single_tool_result_max_chars, 400_000);
    assert_eq!(cfg.compaction_model, DEFAULT_LLM_MODEL);
}

#[test]
fn context_budget_chars_gpt52() {
    let cfg = ContextConfig {
        context_window: 400_000,
        max_output_tokens: 128_000,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    // (400000 - 128000) * 4 * 0.75 = 816000
    assert_eq!(budget, 816_000);
}

#[test]
fn context_budget_chars_zero_output() {
    let cfg = ContextConfig {
        context_window: 100_000,
        max_output_tokens: 0,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 300_000); // 100000 * 4 * 0.75
}

#[test]
fn context_budget_chars_overflow_protection() {
    let cfg = ContextConfig {
        context_window: 10,
        max_output_tokens: 100,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 0); // saturating_sub gives 0
}

#[test]
fn app_config_includes_context() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.context.context_window, 400_000);
}

#[test]
fn context_config_toml_override() {
    let dir = std::env::temp_dir().join("pi_wasm_ctx_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"[context]\ncontext_window = 200000\nmax_output_tokens = 64000\ncompaction_model = \"gpt-4o-mini\"\n").unwrap();
    drop(f);
    let r = load_config(Some(path.as_path()));
    assert!(r.is_ok());
    let cfg = r.unwrap();
    assert_eq!(cfg.context.context_window, 200_000);
    assert_eq!(cfg.context.max_output_tokens, 64_000);
    assert_eq!(cfg.context.compaction_model, "gpt-4o-mini");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn concurrent_lock_does_not_deadlock() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = Arc::new(dir.path().to_path_buf());
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
