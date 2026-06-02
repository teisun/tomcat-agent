//! `ensure_mimo_models_toml` 幂等性与生成内容回归。
//!
//! 本测试模块挂在 `api::cli::models_toml` 源文件下（见该文件末尾
//! `#[cfg(test)] #[path] mod tests;`），故不在 `cli/tests/mod.rs` 声明。

use super::{ensure_mimo_models_toml, ModelsTomlStatus};
use crate::core::llm::ModelCatalog;
use crate::AppConfig;

fn config_with_work_dir(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.to_str().unwrap().to_string());
    cfg
}

fn models_toml_text(cfg: &AppConfig) -> String {
    let path = ModelCatalog::default_user_path(cfg).expect("models.toml path");
    std::fs::read_to_string(path).expect("read models.toml")
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn creates_models_toml_with_mimo_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let status = ensure_mimo_models_toml(&cfg).expect("ensure");
    assert_eq!(status, ModelsTomlStatus::Created);

    let text = models_toml_text(&cfg);
    assert!(text.contains("id = \"mimo-v2.5-pro\""), "missing mimo entry:\n{text}");
    assert!(text.contains("# Tomcat 模型清单"), "missing header comment:\n{text}");

    // 生成的条目必须能被 catalog 正确解析。
    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    let entry = catalog.lookup("mimo-v2.5-pro").expect("mimo entry");
    assert_eq!(entry.api, "openai");
    assert_eq!(entry.provider, "mimo");
    assert_eq!(entry.base_url.as_deref(), Some("https://token-plan-cn.xiaomimimo.com"));
    assert_eq!(entry.thinking_format.as_deref(), Some("doubao"));
    assert!(!entry.capabilities.vision);
    assert!(!entry.capabilities.files);
    assert!(entry.capabilities.tools);
    assert!(entry.capabilities.reasoning);
}

#[test]
fn second_run_is_idempotent_no_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    assert_eq!(ensure_mimo_models_toml(&cfg).unwrap(), ModelsTomlStatus::Created);
    assert_eq!(
        ensure_mimo_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    assert_eq!(
        ensure_mimo_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );

    let text = models_toml_text(&cfg);
    assert_eq!(
        count_occurrences(&text, "id = \"mimo-v2.5-pro\""),
        1,
        "repeated init must not duplicate the mimo entry:\n{text}"
    );
}

#[test]
fn appends_mimo_preserving_existing_user_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let path = ModelCatalog::default_user_path(&cfg).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let user_content = "\
# my own notes
[[models]]
id = \"my-custom-model\"
api = \"openai\"
provider = \"acme\"
base_url = \"https://api.acme.example\"
";
    std::fs::write(&path, user_content).unwrap();

    let status = ensure_mimo_models_toml(&cfg).expect("ensure");
    assert_eq!(status, ModelsTomlStatus::AppendedMimo);

    let text = models_toml_text(&cfg);
    assert!(text.contains("# my own notes"), "user comment lost:\n{text}");
    assert!(text.contains("id = \"my-custom-model\""), "user entry lost:\n{text}");
    assert!(text.contains("id = \"mimo-v2.5-pro\""), "mimo not appended:\n{text}");

    // 两个条目都应能解析。
    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    assert!(catalog.lookup("my-custom-model").is_some());
    assert!(catalog.lookup("mimo-v2.5-pro").is_some());

    // 再跑一次保持幂等。
    assert_eq!(
        ensure_mimo_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    let text2 = models_toml_text(&cfg);
    assert_eq!(count_occurrences(&text2, "id = \"mimo-v2.5-pro\""), 1);
}
