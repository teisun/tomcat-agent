//! Skill 子系统：声明式技能的磁盘契约与运行时元数据。
//!
//! v1 先落地 `Skill` / `SkillFrontmatter` 基础契约；发现、catalog 与工具分发在后续阶段接入。

pub mod catalog;
pub mod discovery;
pub mod frontmatter;
pub mod load;
pub mod model;

pub use catalog::{
    available_skill_names_csv, compute_skill_prompt_budget_chars, render_available_skills_block,
    render_skill_inventory, visible_skill_names_csv, AvailableSkillsRender,
};
pub use discovery::{discover, skill_roots, spawn_discovery_task};
pub use frontmatter::{
    parse, split_frontmatter, strip_frontmatter, SkillFrontmatter, SkillParseError,
    ALLOWED_SKILL_FRONTMATTER,
};
pub use load::load_skill_payload;
pub use model::{Skill, SkillDiagnostic, SkillSet, SkillSource};

#[cfg(test)]
mod tests;
