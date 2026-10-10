//! `ensure_default_models_toml` 幂等性与生成内容回归。
//!
//! 本测试模块挂在 `api::cli::models_toml` 源文件下（见该文件末尾
//! `#[cfg(test)] #[path] mod tests;`），故不在 `cli/tests/mod.rs` 声明。

use super::{builtin_seed_blocks, ensure_default_models_toml, ModelsTomlStatus};
use crate::core::llm::catalog::{builtin_seed_entries, builtin_seed_toml_text};
use crate::core::llm::{ModelCatalog, ModelEntry};
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

fn seed_entries(cfg: &AppConfig) -> Vec<ModelEntry> {
    builtin_seed_entries(&cfg.context)
}

fn seed_entry(cfg: &AppConfig, model_id: &str) -> ModelEntry {
    seed_entries(cfg)
        .into_iter()
        .find(|entry| entry.id == model_id)
        .unwrap_or_else(|| panic!("missing builtin seed entry: {model_id}"))
}

fn seed_blocks() -> Vec<(String, String)> {
    builtin_seed_blocks()
        .expect("embedded seed blocks")
        .into_iter()
        .map(|entry| (entry.id, entry.block))
        .collect()
}

fn seed_ids() -> Vec<String> {
    seed_blocks()
        .into_iter()
        .map(|(model_id, _)| model_id)
        .collect()
}

fn seed_block_text(model_id: &str) -> String {
    seed_blocks()
        .into_iter()
        .find(|(id, _)| id == model_id)
        .map(|(_, block)| block)
        .unwrap_or_else(|| panic!("missing embedded seed block: {model_id}"))
}

fn expected_seed_file_text() -> String {
    format!(
        "{}\n{}",
        crate::infra::i18n::tr_in(crate::infra::i18n::Locale::En, "config.modelsHeader", &[]),
        builtin_seed_toml_text()
    )
}

fn expected_seed_blocks_text() -> String {
    format!(
        "{}\n",
        seed_blocks()
            .into_iter()
            .map(|(_, block)| block)
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

fn seed_ids_except(excluded: &[&str]) -> Vec<String> {
    seed_ids()
        .into_iter()
        .filter(|model_id| !excluded.contains(&model_id.as_str()))
        .collect()
}

fn strip_model_name(block: &str) -> String {
    block
        .lines()
        .filter(|line| !line.trim_start().starts_with("model_name = "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn localized_header_is_comment_only_and_keeps_identical_model_values() {
    use crate::infra::i18n::{tr_in, Locale};
    let seed: toml::Value = toml::from_str(builtin_seed_toml_text()).unwrap();
    for locale in [Locale::En, Locale::ZhCn] {
        let header = tr_in(locale, "config.modelsHeader", &[]);
        assert!(header
            .lines()
            .all(|line| line.starts_with('#') || line.trim().is_empty()));
        let text = format!("{header}\n{}", builtin_seed_toml_text());
        assert_eq!(toml::from_str::<toml::Value>(&text).unwrap(), seed);
    }
}

#[test]
fn initialized_builtin_descriptions_use_catalog_unless_user_explicitly_overrides() {
    use crate::core::llm::list_model_views;
    use crate::infra::i18n::{tr_in, Locale};

    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());
    ensure_default_models_toml(&cfg).unwrap();
    let saved: toml::Value = toml::from_str(&models_toml_text(&cfg)).unwrap();
    assert!(saved["models"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model.get("description").is_none()));
    let catalog = ModelCatalog::load(&cfg).unwrap();
    for view in list_model_views(&catalog) {
        let key = format!("model.description.{}", view.id);
        let english = tr_in(Locale::En, &key, &[]);
        assert_ne!(english, tr_in(Locale::En, "error.unknown", &[]), "{key}");
        assert_eq!(view.description, Some(english), "{key}");
    }

    let description = "My custom model description";
    let path = ModelCatalog::default_user_path(&cfg).unwrap();
    std::fs::write(
        &path,
        format!("[[models]]\nid = \"gpt-5.4\"\ndescription = \"{description}\"\n"),
    )
    .unwrap();
    let catalog = ModelCatalog::load(&cfg).unwrap();
    let view = list_model_views(&catalog)
        .into_iter()
        .find(|view| view.id == "gpt-5.4")
        .unwrap();
    assert_eq!(view.description.as_deref(), Some(description));
}

#[test]
fn creates_models_toml_with_all_seed_entries_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::Created {
            added_model_ids: seed_ids()
        }
    );

    let text = models_toml_text(&cfg);
    assert_eq!(text, expected_seed_file_text());

    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    for entry in seed_entries(&cfg) {
        assert_eq!(catalog.lookup(&entry.id).cloned(), Some(entry.clone()));
        assert!(
            catalog.is_builtin_seed(&entry.id),
            "seeded preset should still be marked as builtin: {}",
            entry.id
        );
        assert!(
            catalog.is_user_model(&entry.id),
            "seeded preset should be marked as user-owned after init: {}",
            entry.id
        );
    }
}

#[test]
fn second_run_is_idempotent_no_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::Created {
            added_model_ids: seed_ids()
        }
    );
    let first_text = models_toml_text(&cfg);
    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );

    let text = models_toml_text(&cfg);
    assert_eq!(text, first_text);
    assert_eq!(count_occurrences(&text, "[[models]]"), seed_ids().len());
    for model_id in seed_ids() {
        assert_eq!(
            count_occurrences(&text, &format!("id = \"{model_id}\"")),
            1,
            "repeated init must not duplicate the {model_id} entry:\n{text}"
        );
    }
}

#[test]
fn appends_missing_seed_entries_preserving_existing_user_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let path = ModelCatalog::default_user_path(&cfg).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let user_content = format!(
        "\
# my own notes
[[models]]
id = \"my-custom-model\"
api = \"openai\"
provider = \"acme\"
base_url = \"https://api.acme.example\"

{}
",
        strip_model_name(&seed_block_text("gpt-5.2"))
    );
    std::fs::write(&path, user_content).unwrap();

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::UpdatedExisting {
            added_model_ids: seed_ids_except(&["gpt-5.2"]),
            updated_model_name_ids: vec!["gpt-5.2".to_string()]
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
    for model_id in seed_ids_except(&["gpt-5.2"]) {
        assert!(
            text.contains(&format!("id = \"{model_id}\"")),
            "{model_id} not appended:\n{text}"
        );
    }
    assert!(
        text.contains("model_name = \"gpt-5.2\""),
        "gpt-5.2 model_name not backfilled:\n{text}"
    );

    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    assert!(catalog.lookup("my-custom-model").is_some());
    for entry in seed_entries(&cfg) {
        assert!(catalog.lookup(&entry.id).is_some(), "missing {}", entry.id);
        assert!(
            catalog.is_builtin_seed(&entry.id),
            "seeded preset should still be builtin after append: {}",
            entry.id
        );
        assert!(
            catalog.is_user_model(&entry.id),
            "seeded preset should be user-owned after append: {}",
            entry.id
        );
    }

    assert_eq!(
        ensure_default_models_toml(&cfg).unwrap(),
        ModelsTomlStatus::AlreadyPresent
    );
    let text2 = models_toml_text(&cfg);
    assert_eq!(
        count_occurrences(&text2, "[[models]]"),
        seed_ids().len() + 1
    );
    for model_id in seed_ids() {
        assert_eq!(
            count_occurrences(&text2, &format!("id = \"{model_id}\"")),
            1
        );
    }
}

#[test]
fn backfills_missing_model_name_for_existing_seed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    let path = ModelCatalog::default_user_path(&cfg).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let missing_model_name_ids = ["mimo-v2.5-pro", "gpt-5.2", "deepseek-v4-flash"];
    let expected_updated_model_name_ids = seed_ids()
        .into_iter()
        .filter(|model_id| missing_model_name_ids.contains(&model_id.as_str()))
        .collect::<Vec<_>>();
    let mut existing = seed_blocks()
        .into_iter()
        .map(|(model_id, block)| {
            if missing_model_name_ids.contains(&model_id.as_str()) {
                strip_model_name(&block)
            } else {
                block
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    existing.push('\n');
    std::fs::write(&path, existing).unwrap();

    let status = ensure_default_models_toml(&cfg).expect("ensure");
    assert_eq!(
        status,
        ModelsTomlStatus::UpdatedExisting {
            added_model_ids: vec![],
            updated_model_name_ids: expected_updated_model_name_ids
        }
    );

    let text = models_toml_text(&cfg);
    assert_eq!(text, expected_seed_blocks_text());
    for model_id in missing_model_name_ids {
        let entry = seed_entry(&cfg, model_id);
        assert_eq!(
            count_occurrences(
                &text,
                &format!("model_name = \"{}\"", entry.request_model_name())
            ),
            if model_id == "deepseek-v4-flash" {
                2
            } else {
                1
            }
        );
    }
}

#[test]
fn generated_models_toml_matches_embedded_seed_without_drift() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_with_work_dir(dir.path());

    ensure_default_models_toml(&cfg).expect("ensure");
    let rendered = expected_seed_file_text();
    let written = models_toml_text(&cfg);
    assert_eq!(written, rendered);

    let catalog = ModelCatalog::load(&cfg).expect("catalog load");
    for entry in seed_entries(&cfg) {
        assert_eq!(catalog.lookup(&entry.id).cloned(), Some(entry));
    }
}
