//! `SkillsConfig` 默认值、TOML 覆盖与环境变量覆盖。

use super::super::*;
use std::io::Write;
use serial_test::serial;

#[test]
fn skills_config_default_values() {
    let cfg = SkillsConfig::default();
    assert!(cfg.enabled);
    assert_eq!(cfg.prompt_budget_pct, DEFAULT_SKILLS_PROMPT_BUDGET_PCT);
    assert_eq!(
        cfg.prompt_budget_floor_chars,
        DEFAULT_SKILLS_PROMPT_BUDGET_FLOOR_CHARS
    );
    assert_eq!(
        cfg.max_description_chars,
        DEFAULT_SKILLS_MAX_DESCRIPTION_CHARS
    );
    assert_eq!(cfg.max_skills, DEFAULT_SKILLS_MAX_SKILLS);
    assert!(cfg.disabled.is_empty());
    assert!(!cfg.expose_to_reviewer);
}

#[test]
fn app_config_includes_skills_defaults() {
    let cfg = AppConfig::default();
    assert!(cfg.skills.enabled);
    assert_eq!(
        cfg.skills.prompt_budget_pct,
        DEFAULT_SKILLS_PROMPT_BUDGET_PCT
    );
    assert_eq!(
        cfg.skills.prompt_budget_floor_chars,
        DEFAULT_SKILLS_PROMPT_BUDGET_FLOOR_CHARS
    );
}

#[test]
fn deserialize_missing_skills_section_uses_default() {
    let cfg: AppConfig = serde_json::from_str("{}").unwrap();
    assert!(cfg.skills.enabled);
    assert_eq!(cfg.skills.max_skills, DEFAULT_SKILLS_MAX_SKILLS);
}

#[test]
fn skills_toml_override() {
    let dir = std::env::temp_dir().join("tomcat_skills_cfg_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(
        br#"[skills]
enabled = false
prompt_budget_pct = 2
prompt_budget_floor_chars = 4096
max_description_chars = 120
max_skills = 42
disabled = ["commit", "code-review"]
expose_to_reviewer = true
"#,
    )
    .unwrap();
    drop(f);
    let cfg = load_config_toml_file(path.as_path()).expect("load_config_toml_file");
    assert!(!cfg.skills.enabled);
    assert_eq!(cfg.skills.prompt_budget_pct, 2);
    assert_eq!(cfg.skills.prompt_budget_floor_chars, 4096);
    assert_eq!(cfg.skills.max_description_chars, 120);
    assert_eq!(cfg.skills.max_skills, 42);
    assert_eq!(cfg.skills.disabled, vec!["commit", "code-review"]);
    assert!(cfg.skills.expose_to_reviewer);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
#[serial(env_lock)]
fn skills_env_override_beats_toml() {
    let dir = std::env::temp_dir().join("tomcat_skills_env_override_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[skills]\nenabled = true\nprompt_budget_pct = 1\n").unwrap();
    // SAFETY: 用例串行执行；仅在本测试作用域内临时覆盖环境变量。
    unsafe { std::env::set_var("TOMCAT__SKILLS__PROMPT_BUDGET_PCT", "3") };
    let cfg = load_config(Some(path.as_path())).unwrap();
    assert_eq!(cfg.skills.prompt_budget_pct, 3);
    // SAFETY: 清理测试环境变量，避免污染后续用例。
    unsafe { std::env::remove_var("TOMCAT__SKILLS__PROMPT_BUDGET_PCT") };
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn disabled_list_filters_skill() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(project.join(".tomcat").join("skills").join("commit")).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::write(
        project
            .join(".tomcat")
            .join("skills")
            .join("commit")
            .join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();

    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[storage]\nwork_dir = \"{}\"\n\n[skills]\ndisabled = [\"commit\"]\n",
            work_dir.to_string_lossy()
        ),
    )
    .unwrap();

    let cfg = load_config_toml_file(path.as_path()).expect("load_config_toml_file");
    let skill_set = crate::core::skill::discover(&cfg, &project);
    assert!(!skill_set.by_name.contains_key("commit"));
}
