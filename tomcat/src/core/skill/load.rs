use std::path::{Path, PathBuf};

use crate::core::tools::primitive::PrimitiveExecutor;

use super::Skill;

pub async fn load_skill_payload(
    primitive: &dyn PrimitiveExecutor,
    plugin_id: &str,
    skill: &Skill,
    file: Option<&str>,
) -> Result<String, String> {
    let (target_path, location, strip_frontmatter) = resolve_target_path(skill, file)?;
    let raw = primitive
        .read_file(target_path.to_string_lossy().as_ref(), plugin_id)
        .await
        .map_err(|e| e.to_string())?;
    let payload = if strip_frontmatter {
        crate::core::skill::strip_frontmatter(&raw)
            .map_err(|e| e.to_string())?
            .trim()
            .to_string()
    } else {
        raw.trim().to_string()
    };

    Ok(format!(
        "<skill name=\"{}\" location=\"{}\">\n{}\n</skill>",
        xml_escape(&skill.name),
        xml_escape(&location),
        payload
    ))
}

fn resolve_target_path(
    skill: &Skill,
    file: Option<&str>,
) -> Result<(PathBuf, String, bool), String> {
    let base_dir = std::fs::canonicalize(&skill.base_dir)
        .map_err(|e| format!("load_skill: 技能目录不可访问: {e}"))?;
    if let Some(file) = file {
        let relative = Path::new(file);
        if relative.is_absolute() {
            return Err("load_skill: `file` 必须是技能目录内的相对路径".to_string());
        }
        let target = std::fs::canonicalize(skill.base_dir.join(relative))
            .map_err(|e| format!("load_skill: 读取附件失败: {e}"))?;
        if !target.starts_with(&base_dir) {
            return Err("load_skill: `file` 越出技能目录".to_string());
        }
        let main_skill_path =
            std::fs::canonicalize(&skill.file_path).unwrap_or_else(|_| skill.file_path.clone());
        let strip_frontmatter = target == main_skill_path;
        return Ok((target, file.to_string(), strip_frontmatter));
    }

    let target = std::fs::canonicalize(&skill.file_path)
        .map_err(|e| format!("load_skill: 读取技能正文失败: {e}"))?;
    Ok((target, "SKILL.md".to_string(), true))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
}
