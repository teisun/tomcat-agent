use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::core::skill::{
    compute_skill_prompt_budget_chars, render_available_skills_block, Skill, SkillSet, SkillSource,
};
use crate::infra::config::SkillsConfig;

#[test]
fn compute_skill_prompt_budget_uses_max_of_pct_and_floor() {
    let cfg = SkillsConfig {
        prompt_budget_pct: 1,
        prompt_budget_floor_chars: 2_000,
        ..Default::default()
    };
    assert_eq!(compute_skill_prompt_budget_chars(50_000, &cfg), 2_000);
    assert_eq!(compute_skill_prompt_budget_chars(400_000, &cfg), 4_000);
}

#[test]
fn render_available_skills_hides_model_blocked_entries() {
    let mut by_name = BTreeMap::new();
    by_name.insert("commit".into(), skill("commit", "Create a commit", false));
    by_name.insert("review".into(), skill("review", "Review code", true));
    let set = SkillSet {
        by_name,
        diagnostics: Vec::new(),
        warnings: Vec::new(),
    };
    let rendered = render_available_skills_block(&set, 512, 50);
    assert!(rendered.block.contains("commit"));
    assert!(!rendered.block.contains("review"));
}

#[test]
fn render_available_skills_falls_back_to_name_only_when_budget_tight() {
    let mut by_name = BTreeMap::new();
    by_name.insert(
        "commit".into(),
        skill(
            "commit",
            "Create a carefully described git commit message",
            false,
        ),
    );
    by_name.insert(
        "review".into(),
        skill(
            "review",
            "Review code changes with a long description",
            false,
        ),
    );
    let set = SkillSet {
        by_name,
        diagnostics: Vec::new(),
        warnings: Vec::new(),
    };
    let rendered = render_available_skills_block(&set, 64, 40);
    assert!(rendered.name_only);
    assert!(rendered.block.contains("<skill name=\"commit\" />"));
    assert!(!rendered.block.contains("carefully described"));
    assert!(rendered
        .warnings
        .iter()
        .any(|warning| warning == "skills_prompt_truncated:name_only"));
}

fn skill(name: &str, description: &str, disable_model_invocation: bool) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        file_path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
        base_dir: PathBuf::from(format!("/tmp/{name}")),
        source: SkillSource::Project,
        allowed_tools: None,
        disable_model_invocation,
    }
}
