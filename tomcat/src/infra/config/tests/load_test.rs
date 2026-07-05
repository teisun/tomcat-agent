//! # `load_config` 路径
//!
//! 三个等价类：
//!
//! - 不传 path → 走默认 / 环境变量回退路径，断言不报错。
//! - 传入临时 `[log] level = "debug"` toml 文件 → 解析后通过 `validate_config`。
//! - 传入仓库根的 `tomcat.config.toml.example`（若存在）→ 反序列化 + 校验都成功，
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
    let dir = std::env::temp_dir().join("tomcat_config_test");
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
fn load_config_accepts_preflight_section() {
    let dir = std::env::temp_dir().join("tomcat_preflight_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "[preflight]\nauto_install_search_tools = false\nauto_install_git = false\nshow_search_tools_ui = true\nshow_git_ui = true\n[log]\nlevel = \"info\"\n",
    )
    .unwrap();
    let cfg = load_config(Some(path.as_path())).unwrap();
    assert!(!cfg.preflight.auto_install_search_tools);
    assert!(!cfg.preflight.auto_install_git);
    assert!(cfg.preflight.show_search_tools_ui);
    assert!(cfg.preflight.show_git_ui);
    assert!(validate_config(&cfg).is_ok());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_rejects_legacy_whitelist_keys() {
    let dir = std::env::temp_dir().join("tomcat_legacy_whitelist_config_test");
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
    let example_path = manifest_dir.join("tomcat.config.toml.example");
    if !example_path.exists() {
        return;
    }
    let content = std::fs::read_to_string(&example_path).unwrap();
    let dir = std::env::temp_dir().join("tomcat_example_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let temp_toml = dir.join("config.toml");
    std::fs::write(&temp_toml, &content).unwrap();
    let r = load_config(Some(temp_toml.as_path()));
    let _ = std::fs::remove_file(&temp_toml);
    let _ = std::fs::remove_dir(&dir);
    let cfg = r.unwrap_or_else(|e| {
        panic!(
            "tomcat.config.toml.example 内容应可被 load_config 反序列化: {}",
            e
        )
    });
    assert!(validate_config(&cfg).is_ok());
}

#[test]
fn load_config_env_overrides_llm_files_expires_after_seconds() {
    let dir = std::env::temp_dir().join("tomcat_files_env_override_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[llm]\ndefault_model = \"gpt-5.4\"\n").unwrap();
    // SAFETY: 用例串行执行；仅在本测试作用域内临时覆盖环境变量。
    unsafe { std::env::set_var("TOMCAT__LLM__FILES__EXPIRES_AFTER_SECONDS", "7200") };
    let cfg = load_config(Some(path.as_path())).unwrap();
    assert_eq!(cfg.llm.files.expires_after_seconds, 7200);
    // SAFETY: 清理测试环境变量，避免污染后续用例。
    unsafe { std::env::remove_var("TOMCAT__LLM__FILES__EXPIRES_AFTER_SECONDS") };
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_rejects_legacy_llm_provider_field() {
    let dir = std::env::temp_dir().join("tomcat_legacy_llm_provider_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[llm]\nprovider = \"openai-responses\"\n").unwrap();

    let err = load_config(Some(path.as_path())).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("llm.provider"));
    assert!(msg.contains("models.toml"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_rejects_legacy_llm_api_base_field() {
    let dir = std::env::temp_dir().join("tomcat_legacy_llm_api_base_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[llm]\napi_base = \"https://example.test\"\n").unwrap();

    let err = load_config(Some(path.as_path())).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("llm.api_base"));
    assert!(msg.contains("models.toml"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_rejects_legacy_llm_api_key_env_field() {
    let dir = std::env::temp_dir().join("tomcat_legacy_llm_api_key_env_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[llm]\napi_key_env = \"OPENAI_API_KEY\"\n").unwrap();

    let err = load_config(Some(path.as_path())).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("llm.api_key_env"));
    assert!(msg.contains("models.toml"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn load_config_toml_overrides_scene_models() {
    let dir = std::env::temp_dir().join("tomcat_scene_model_override_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "[llm]\nvision_model = \"gpt-5.4\"\ntitle_model = \"gpt-5.2\"\n",
    )
    .unwrap();

    let cfg = load_config(Some(path.as_path())).unwrap();
    assert_eq!(cfg.llm.vision_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(cfg.llm.title_model.as_deref(), Some("gpt-5.2"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn resolve_model_thinking_path_uses_work_dir_root() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().to_string());

    let path = resolve_model_thinking_path(&cfg).unwrap();

    assert_eq!(
        std::fs::canonicalize(path.parent().expect("parent path")).unwrap(),
        std::fs::canonicalize(temp.path()).unwrap()
    );
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("model-thinking.json")
    );
    assert!(
        !path.starts_with(
            temp.path()
                .join("agents")
                .join(&cfg.agent.id)
                .join("sessions")
        ),
        "model thinking store must not live under sessions/: {}",
        path.display()
    );
}
