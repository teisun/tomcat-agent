use std::path::PathBuf;

use crate::infra::i18n::tr;
use crate::{resolve_agent_definition_dir, AppConfig, AppError};

use super::SkillSub;

pub(crate) fn run_skill(sub: SkillSub, cfg: &AppConfig) -> Result<(), AppError> {
    let skill_set = discover_for_cli(cfg)?;
    match sub {
        SkillSub::List => {
            if cfg.skills.enabled {
                println!("{}", tr("cli.skill.discovered", &[]));
            } else {
                println!("{}", tr("cli.skill.disabledList", &[]));
            }
        }
        SkillSub::Reload => {
            if cfg.skills.enabled {
                println!("{}", tr("cli.skill.reloaded", &[]));
            } else {
                println!("{}", tr("cli.skill.disabledReload", &[]));
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
