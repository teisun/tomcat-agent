use crate::infra::config::SkillsConfig;

use super::model::{Skill, SkillSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableSkillsRender {
    pub block: String,
    pub warnings: Vec<String>,
    pub name_only: bool,
}

impl SkillSet {
    pub fn resolve(&self, name: &str) -> Option<&Skill> {
        self.by_name
            .get(name)
            .filter(|skill| !skill.disable_model_invocation)
    }

    pub fn resolve_any(&self, name: &str) -> Option<&Skill> {
        self.by_name.get(name)
    }

    pub fn visible_skills(&self) -> Vec<&Skill> {
        self.by_name
            .values()
            .filter(|skill| !skill.disable_model_invocation)
            .collect()
    }
}

pub fn available_skill_names_csv(skill_set: &SkillSet) -> String {
    skill_set
        .by_name
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn visible_skill_names_csv(skill_set: &SkillSet) -> String {
    skill_set
        .visible_skills()
        .into_iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn render_skill_inventory(skill_set: &SkillSet) -> String {
    let mut lines = Vec::new();

    if skill_set.by_name.is_empty() {
        lines.push("发现 0 条 skill。".to_string());
    } else {
        lines.push(format!("发现 {} 条 skill：", skill_set.by_name.len()));
        for skill in skill_set.by_name.values() {
            let mut tags = vec![skill.source.as_str().to_string()];
            if skill.disable_model_invocation {
                tags.push("user-only".to_string());
            }
            lines.push(format!(
                "  - {} [{}] {}",
                skill.name,
                tags.join(", "),
                skill.description
            ));
        }
    }

    if !skill_set.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &skill_set.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    if !skill_set.diagnostics.is_empty() {
        lines.push("diagnostics:".to_string());
        for diagnostic in &skill_set.diagnostics {
            lines.push(format!(
                "  - {}: {}",
                diagnostic.path.display(),
                diagnostic.reason
            ));
        }
    }

    lines.join("\n")
}

pub fn compute_skill_prompt_budget_chars(context_budget_chars: usize, cfg: &SkillsConfig) -> usize {
    let pct_budget = context_budget_chars.saturating_mul(cfg.prompt_budget_pct as usize) / 100;
    cfg.prompt_budget_floor_chars.max(pct_budget)
}

pub fn render_available_skills_block(
    skill_set: &SkillSet,
    budget_chars: usize,
    max_description_chars: usize,
) -> AvailableSkillsRender {
    let visible = skill_set.visible_skills();
    if visible.is_empty() || budget_chars == 0 {
        return AvailableSkillsRender {
            block: String::new(),
            warnings: Vec::new(),
            name_only: false,
        };
    }

    let with_descriptions =
        render_entries_until_budget(&visible, budget_chars, Some(max_description_chars));
    if with_descriptions.included_all {
        return AvailableSkillsRender {
            block: with_descriptions.block,
            warnings: Vec::new(),
            name_only: false,
        };
    }

    let name_only = render_entries_until_budget(&visible, budget_chars, None);
    if name_only.included_all {
        return AvailableSkillsRender {
            block: name_only.block,
            warnings: vec!["skills_prompt_truncated:name_only".to_string()],
            name_only: true,
        };
    }

    AvailableSkillsRender {
        block: name_only.block,
        warnings: vec![
            "skills_prompt_truncated:name_only".to_string(),
            format!("skills_prompt_truncated:budget={budget_chars}"),
        ],
        name_only: true,
    }
}

struct RenderAttempt {
    block: String,
    included_all: bool,
}

fn render_entries_until_budget(
    visible: &[&Skill],
    budget_chars: usize,
    max_description_chars: Option<usize>,
) -> RenderAttempt {
    let mut block = String::new();
    let mut included = 0usize;
    for skill in visible {
        let entry = render_skill_entry(skill, max_description_chars);
        if !block.is_empty() && block.len().saturating_add(entry.len() + 1) > budget_chars {
            break;
        }
        if block.is_empty() {
            if entry.len() > budget_chars {
                break;
            }
            block.push_str(&entry);
        } else {
            block.push('\n');
            block.push_str(&entry);
        }
        included += 1;
    }
    RenderAttempt {
        block,
        included_all: included == visible.len(),
    }
}

fn render_skill_entry(skill: &Skill, max_description_chars: Option<usize>) -> String {
    match max_description_chars {
        Some(limit) => format!(
            "  <skill name=\"{}\">{}</skill>",
            xml_escape(&skill.name),
            xml_escape(&truncate_chars(&skill.description, limit))
        ),
        None => format!("  <skill name=\"{}\" />", xml_escape(&skill.name)),
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    if limit <= 3 {
        return value.chars().take(limit).collect();
    }
    let mut out = value.chars().take(limit - 3).collect::<String>();
    out.push_str("...");
    out
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
