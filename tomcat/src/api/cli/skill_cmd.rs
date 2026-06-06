use std::path::PathBuf;

use crate::{resolve_agent_definition_dir, AppConfig, AppError};

use super::SkillSub;

pub(crate) fn run_skill(sub: SkillSub, cfg: &AppConfig) -> Result<(), AppError> {
    let skill_set = discover_for_cli(cfg)?;
    match sub {
        SkillSub::List => {
            if cfg.skills.enabled {
                println!("[skill] 当前发现结果：");
            } else {
                println!("[skill] 技能系统当前已禁用（[skills].enabled=false），以下为发现结果：");
            }
        }
        SkillSub::Reload => {
            if cfg.skills.enabled {
                println!("[skill] 已重扫技能目录。");
            } else {
                println!("[skill] 技能系统当前已禁用（[skills].enabled=false），以下为重扫结果：");
            }
        }
    }
    println!("{}", crate::core::skill::render_skill_inventory(&skill_set));
    Ok(())
}

fn discover_for_cli(cfg: &AppConfig) -> Result<crate::core::skill::SkillSet, AppError> {
    let agent_workspace_dir = std::env::current_dir().unwrap_or_else(|_| {
        resolve_agent_definition_dir(cfg).unwrap_or_else(|_| PathBuf::from("."))
    });
    Ok(crate::core::skill::discover(cfg, &agent_workspace_dir))
}
