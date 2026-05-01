//! # `load_config` 路径
//!
//! 三个等价类：
//!
//! - 不传 path → 走默认 / 环境变量回退路径，断言不报错。
//! - 传入临时 `[log] level = "debug"` toml 文件 → 解析后通过 `validate_config`。
//! - 传入仓库根的 `pi.config.toml.example`（若存在）→ 反序列化 + 校验都成功，
//!   防止 example 与代码失去同步。

use super::super::*;
use std::io::Write;

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
fn load_config_rejects_legacy_whitelist_keys() {
    let dir = std::env::temp_dir().join("pi_wasm_legacy_whitelist_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "[primitive]\npath_whitelist=[]\nbash_whitelist=[]\nauto_confirm_whitelist=[]\n",
    )
    .unwrap();

    let err = load_config(Some(path.as_path())).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("primitive.path_whitelist"));
    assert!(msg.contains("workspace.workspace_roots"));
    assert!(msg.contains("primitive.bash_whitelist"));
    assert!(msg.contains("primitive.bash_forbidden"));
    assert!(msg.contains("primitive.auto_confirm_whitelist"));

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
