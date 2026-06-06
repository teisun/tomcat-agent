use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::infra::config::{get_work_dir, resolve_agent_trail_dir};
use crate::infra::error::AppError;
use crate::AppConfig;

use super::frontmatter::parse;
use super::model::{Skill, SkillDiagnostic, SkillSet, SkillSource};

const FRONTMATTER_READ_LIMIT_BYTES: usize = 4 * 1024;

pub fn skill_roots(
    cfg: &AppConfig,
    agent_workspace_dir: &Path,
) -> Result<Vec<(SkillSource, PathBuf)>, AppError> {
    Ok(vec![
        (
            SkillSource::Project,
            agent_workspace_dir.join(".tomcat").join("skills"),
        ),
        (
            SkillSource::Agent,
            resolve_agent_trail_dir(cfg)?.join("skills"),
        ),
        (SkillSource::Managed, get_work_dir(cfg)?.join("skills")),
    ])
}

pub fn discover(cfg: &AppConfig, agent_workspace_dir: &Path) -> SkillSet {
    if !cfg.skills.enabled {
        return SkillSet::default();
    }

    let mut skill_set = SkillSet::default();
    let disabled = cfg
        .skills
        .disabled
        .iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<HashSet<_>>();

    let Ok(roots) = skill_roots(cfg, agent_workspace_dir) else {
        skill_set
            .warnings
            .push("skills_discovery_roots_failed".to_string());
        return skill_set;
    };

    let mut scanned_candidates = 0usize;
    let mut hit_limit = false;
    for (source, root) in roots {
        if hit_limit {
            break;
        }
        scan_root(
            &root,
            source,
            &disabled,
            cfg.skills.max_skills,
            &mut scanned_candidates,
            &mut hit_limit,
            &mut skill_set,
        );
    }
    if hit_limit {
        skill_set.warnings.push(format!(
            "skills_discovery_truncated:max_skills={}",
            cfg.skills.max_skills
        ));
    }
    skill_set
}

pub fn spawn_discovery_task(
    cfg: AppConfig,
    agent_workspace_dir: PathBuf,
) -> tokio::task::JoinHandle<SkillSet> {
    tokio::spawn(async move { discover(&cfg, &agent_workspace_dir) })
}

fn scan_root(
    root: &Path,
    source: SkillSource,
    disabled: &HashSet<String>,
    max_skills: usize,
    scanned_candidates: &mut usize,
    hit_limit: &mut bool,
    skill_set: &mut SkillSet,
) {
    if !root.exists() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(root) else {
        skill_set.diagnostics.push(SkillDiagnostic {
            path: root.to_path_buf(),
            reason: "skills 根目录不可读取".to_string(),
        });
        return;
    };

    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if *hit_limit {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            skill_set.diagnostics.push(SkillDiagnostic {
                path: entry.path(),
                reason: "无法读取目录项类型".to_string(),
            });
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let candidate = if file_type.is_dir() {
            let skill_file = path.join("SKILL.md");
            skill_file.is_file().then_some((skill_file, path.clone()))
        } else {
            None
        };

        let Some((skill_file, base_dir)) = candidate else {
            continue;
        };
        *scanned_candidates += 1;
        if *scanned_candidates > max_skills {
            *hit_limit = true;
            return;
        }
        inspect_skill_file(&skill_file, &base_dir, source, disabled, skill_set);
    }
}

fn inspect_skill_file(
    skill_file: &Path,
    base_dir: &Path,
    source: SkillSource,
    disabled: &HashSet<String>,
    skill_set: &mut SkillSet,
) {
    let prefix = match read_frontmatter_prefix(skill_file) {
        Ok(prefix) => prefix,
        Err(error) => {
            skill_set.diagnostics.push(SkillDiagnostic {
                path: skill_file.to_path_buf(),
                reason: error,
            });
            return;
        }
    };

    let frontmatter = match parse(&prefix) {
        Ok(frontmatter) => frontmatter,
        Err(error) => {
            skill_set.diagnostics.push(SkillDiagnostic {
                path: skill_file.to_path_buf(),
                reason: error.to_string(),
            });
            return;
        }
    };

    if disabled.contains(&frontmatter.name) {
        return;
    }

    if let Some(existing) = skill_set.by_name.get(&frontmatter.name) {
        skill_set.warnings.push(format!(
            "skill_shadowed:{} by {}",
            frontmatter.name,
            existing.source.as_str()
        ));
        return;
    }

    skill_set.by_name.insert(
        frontmatter.name.clone(),
        Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            file_path: skill_file.to_path_buf(),
            base_dir: base_dir.to_path_buf(),
            source,
            disable_model_invocation: frontmatter.disable_model_invocation,
        },
    );
}

fn read_frontmatter_prefix(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("打开 skill 文件失败: {e}"))?;
    let mut buf = vec![0_u8; FRONTMATTER_READ_LIMIT_BYTES];
    let read = file
        .read(&mut buf)
        .map_err(|e| format!("读取 skill frontmatter 失败: {e}"))?;
    buf.truncate(read);
    String::from_utf8(buf).map_err(|e| format!("skill 文件不是 UTF-8 文本: {e}"))
}
