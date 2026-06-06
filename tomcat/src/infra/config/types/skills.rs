use serde::{Deserialize, Serialize};

/// `[skills]` 顶层配置：控制 Skill 子系统的发现、提示词预算与子 Agent 暴露行为。
///
/// 与 `docs/architecture/skill-system.md` 一致，Skill 是跨 prompt / tool_exec / CLI 的独立子系统，
/// **不**挂在 `[tools]` 下，避免与单个工具的资源上限语义混淆。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillsConfig {
    #[serde(default = "default_skills_enabled")]
    pub enabled: bool,
    #[serde(default = "default_skills_prompt_budget_pct")]
    pub prompt_budget_pct: u8,
    #[serde(default = "default_skills_prompt_budget_floor_chars")]
    pub prompt_budget_floor_chars: usize,
    #[serde(default = "default_skills_max_description_chars")]
    pub max_description_chars: usize,
    #[serde(default = "default_skills_max_skills")]
    pub max_skills: usize,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub expose_to_reviewer: bool,
}

pub const DEFAULT_SKILLS_SYSTEM_ENABLED: bool = true;
pub const DEFAULT_SKILLS_PROMPT_BUDGET_PCT: u8 = 1;
pub const DEFAULT_SKILLS_PROMPT_BUDGET_FLOOR_CHARS: usize = 2_000;
pub const DEFAULT_SKILLS_MAX_DESCRIPTION_CHARS: usize = 250;
pub const DEFAULT_SKILLS_MAX_SKILLS: usize = 1_000;

fn default_skills_enabled() -> bool {
    DEFAULT_SKILLS_SYSTEM_ENABLED
}

fn default_skills_prompt_budget_pct() -> u8 {
    DEFAULT_SKILLS_PROMPT_BUDGET_PCT
}

fn default_skills_prompt_budget_floor_chars() -> usize {
    DEFAULT_SKILLS_PROMPT_BUDGET_FLOOR_CHARS
}

fn default_skills_max_description_chars() -> usize {
    DEFAULT_SKILLS_MAX_DESCRIPTION_CHARS
}

fn default_skills_max_skills() -> usize {
    DEFAULT_SKILLS_MAX_SKILLS
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: default_skills_enabled(),
            prompt_budget_pct: default_skills_prompt_budget_pct(),
            prompt_budget_floor_chars: default_skills_prompt_budget_floor_chars(),
            max_description_chars: default_skills_max_description_chars(),
            max_skills: default_skills_max_skills(),
            disabled: Vec::new(),
            expose_to_reviewer: false,
        }
    }
}
