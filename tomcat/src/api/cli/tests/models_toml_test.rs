//! `ensure_default_models_toml` 幂等性与生成内容回归。
//!
//! 本测试模块挂在 `api::cli::models_toml` 源文件下（见该文件末尾
//! `#[cfg(test)] #[path] mod tests;`），故不在 `cli/tests/mod.rs` 声明。

use super::{ensure_default_models_toml, ModelsTomlStatus};
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
fn creates_models_toml_with_all_managed_entries_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::Created {
            added_model_ids: vec!["mimo-v2.5-pro", "gpt-5.2", "deepseek-v4-flash"]
        }
    );

    let text = models_toml_text(&cfg);
    assert!(
        text.contains("id = \"mimo-v2.5-pro\""),
        "missing mimo entry:\n{text}"
    );
    assert!(
        text.contains("model_name = \"mimo-v2.5-pro\""),
        "missing mimo model_name:\n{text}"
    );
    assert!(
        text.contains("id = \"gpt-5.2\""),
        "missing gpt-5.2:\n{text}"
    );
    assert!(
        text.contains("model_name = \"gpt-5.2\""),
        "missing gpt-5.2 model_name:\n{text}"
    );
    assert!(
        text.contains("id = \"deepseek-v4-flash\""),
        "missing deepseek-v4-flash:\n{text}"
    );
    assert!(
        text.contains("model_name = \"deepseek-v4-flash\""),
        "missing deepseek-v4-flash model_name:\n{text}"
    );
    assert!(
        text.contains("# Tomcat 模型清单"),
        "missing header comment:\n{text}"
    );

    // 生成的条目必须能被 catalog 正确解析。
    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    let entry = catalog.lookup("mimo-v2.5-pro").expect("mimo entry");
    assert_eq!(entry.api, "openai");
    assert_eq!(entry.provider, "mimo");
    assert_eq!(
        entry.base_url.as_deref(),
        Some("https://token-plan-cn.xiaomimimo.com")
    );
    assert_eq!(entry.model_name.as_deref(), Some("mimo-v2.5-pro"));
    assert_eq!(entry.thinking_format.as_deref(), Some("doubao"));
    assert!(!entry.capabilities.vision);
    assert!(!entry.capabilities.files);
    assert!(entry.capabilities.tools);
    assert!(entry.capabilities.reasoning);

    let gpt52 = catalog.lookup("gpt-5.2").expect("gpt-5.2 entry");
    assert_eq!(gpt52.api, "openai-responses");
    assert_eq!(gpt52.provider, "openai");
    assert_eq!(gpt52.model_name.as_deref(), Some("gpt-5.2"));
    assert_eq!(gpt52.api_key_env.as_deref(), Some("OPENAI_API_KEY"));

    let flash = catalog
        .lookup("deepseek-v4-flash")
        .expect("deepseek-v4-flash entry");
    assert_eq!(flash.api, "openai");
    assert_eq!(flash.provider, "deepseek");
    assert_eq!(flash.model_name.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(flash.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[test]
fn second_run_is_idempotent_no_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::Created {
            added_model_ids: vec!["mimo-v2.5-pro", "gpt-5.2", "deepseek-v4-flash"]
        }
    );
    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );

    let text = models_toml_text(&cfg);
    assert_eq!(
        count_occurrences(&text, "id = \"mimo-v2.5-pro\""),
        1,
        "repeated init must not duplicate the mimo entry:\n{text}"
    );
    assert_eq!(
        count_occurrences(&text, "model_name = \"mimo-v2.5-pro\""),
        1
    );
    assert_eq!(count_occurrences(&text, "id = \"gpt-5.2\""), 1);
    assert_eq!(count_occurrences(&text, "model_name = \"gpt-5.2\""), 1);
    assert_eq!(count_occurrences(&text, "id = \"deepseek-v4-flash\""), 1);
    assert_eq!(
        count_occurrences(&text, "model_name = \"deepseek-v4-flash\""),
        1
    );
}

#[test]
fn appends_missing_managed_entries_preserving_existing_user_entries() {
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

[[models]]
id = \"gpt-5.2\"
api = \"openai-responses\"
provider = \"openai\"
api_key_env = \"OPENAI_API_KEY\"
base_url = \"https://api.openai.com\"
capabilities = { vision = true, files = true, tools = true, reasoning = true }
";
    std::fs::write(&path, user_content).unwrap();

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::UpdatedExisting {
            added_model_ids: vec!["mimo-v2.5-pro", "deepseek-v4-flash"],
            updated_model_name_ids: vec!["gpt-5.2"]
        }
    );

    let text = models_toml_text(&cfg);
    assert!(
        text.contains("# my own notes"),
        "user comment lost:\n{text}"
    );
    assert!(
        text.contains("id = \"my-custom-model\""),
        "user entry lost:\n{text}"
    );
    assert!(
        text.contains("id = \"mimo-v2.5-pro\""),
        "mimo not appended:\n{text}"
    );
    assert!(
        text.contains("id = \"deepseek-v4-flash\""),
        "deepseek-v4-flash not appended:\n{text}"
    );
    assert!(
        text.contains("model_name = \"gpt-5.2\""),
        "gpt-5.2 model_name not backfilled:\n{text}"
    );
    assert!(
        text.contains("model_name = \"mimo-v2.5-pro\""),
        "mimo model_name not written:\n{text}"
    );
    assert!(
        text.contains("model_name = \"deepseek-v4-flash\""),
        "deepseek-v4-flash model_name not written:\n{text}"
    );

    // 两个条目都应能解析。
    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    assert!(catalog.lookup("my-custom-model").is_some());
    assert!(catalog.lookup("mimo-v2.5-pro").is_some());
    assert!(catalog.lookup("gpt-5.2").is_some());
    assert!(catalog.lookup("deepseek-v4-flash").is_some());

    // 再跑一次保持幂等。
    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    let text2 = models_toml_text(&cfg);
    assert_eq!(count_occurrences(&text2, "id = \"mimo-v2.5-pro\""), 1);
    assert_eq!(count_occurrences(&text2, "id = \"gpt-5.2\""), 1);
    assert_eq!(count_occurrences(&text2, "id = \"deepseek-v4-flash\""), 1);
    assert_eq!(
        count_occurrences(&text2, "model_name = \"mimo-v2.5-pro\""),
        1
    );
    assert_eq!(count_occurrences(&text2, "model_name = \"gpt-5.2\""), 1);
    assert_eq!(
        count_occurrences(&text2, "model_name = \"deepseek-v4-flash\""),
        1
    );
}

#[test]
fn backfills_missing_model_name_for_existing_managed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let path = ModelCatalog::default_user_path(&cfg).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let existing = "\
[[models]]
id = \"mimo-v2.5-pro\"
api = \"openai\"
provider = \"mimo\"
api_key_env = \"MIMO_API_KEY\"
base_url = \"https://token-plan-cn.xiaomimimo.com\"
thinking_format = \"doubao\"
context_window = 1000000
capabilities = { vision = false, files = false, tools = true, reasoning = true }

[[models]]
id = \"gpt-5.2\"
api = \"openai-responses\"
provider = \"openai\"
api_key_env = \"OPENAI_API_KEY\"
base_url = \"https://api.openai.com\"
thinking_format = \"openai\"
capabilities = { vision = true, files = true, tools = true, reasoning = true }

[[models]]
id = \"deepseek-v4-flash\"
api = \"openai\"
provider = \"deepseek\"
api_key_env = \"DEEPSEEK_API_KEY\"
base_url = \"https://api.deepseek.com\"
thinking_format = \"deepseek\"
capabilities = { vision = false, files = false, tools = true, reasoning = true }
";
    std::fs::write(&path, existing).unwrap();

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::UpdatedExisting {
            added_model_ids: vec![],
            updated_model_name_ids: vec!["mimo-v2.5-pro", "gpt-5.2", "deepseek-v4-flash"]
        }
    );

    let text = models_toml_text(&cfg);
    assert_eq!(
        count_occurrences(&text, "model_name = \"mimo-v2.5-pro\""),
        1
    );
    assert_eq!(count_occurrences(&text, "model_name = \"gpt-5.2\""), 1);
    assert_eq!(
        count_occurrences(&text, "model_name = \"deepseek-v4-flash\""),
        1
    );
}
