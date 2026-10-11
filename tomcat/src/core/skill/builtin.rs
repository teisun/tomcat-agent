//! 内置 skill 的物化。
//!
//! skill discovery 只扫描磁盘目录；把内嵌内容写到 managed root 后，内置 skill 与
//! 用户安装的 skill 共用同一套 frontmatter、权限和 `load_skill` 路径。

use include_dir::{include_dir, Dir, DirEntry};
use std::path::{Path, PathBuf};

use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::AppConfig;

static VERIFY_SKILL_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/skills/verify");
static CONNECTORS_SKILL_ASSETS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/assets/skills/connectors");
static SKILL_CREATOR_ASSETS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/assets/skills/skill-creator");
static PLUGIN_CREATOR_ASSETS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/assets/skills/plugin-creator");

pub fn materialize_builtin_skills(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    let managed_skills_root = get_work_dir(cfg)?.join("skills");
    let verify_root = managed_skills_root.join("verify");
    materialize_dir(&VERIFY_SKILL_ASSETS, &verify_root)?;
    materialize_dir(
        &CONNECTORS_SKILL_ASSETS,
        &managed_skills_root.join("connectors"),
    )?;
    materialize_dir(
        &SKILL_CREATOR_ASSETS,
        &managed_skills_root.join("skill-creator"),
    )?;
    materialize_dir(
        &PLUGIN_CREATOR_ASSETS,
        &managed_skills_root.join("plugin-creator"),
    )?;
    Ok(verify_root.join("SKILL.md"))
}

fn materialize_dir(assets: &Dir<'_>, destination: &Path) -> Result<(), AppError> {
    for entry in assets.entries() {
        match entry {
            DirEntry::Dir(dir) => {
                std::fs::create_dir_all(destination.join(dir.path()))?;
                materialize_dir(dir, destination)?;
            }
            DirEntry::File(file) => {
                materialize_file(&destination.join(file.path()), file.contents())?
            }
        }
    }
    Ok(())
}

fn materialize_file(path: &Path, contents: &[u8]) -> Result<(), AppError> {
    if std::fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config(crate::infra::i18n::tr("skill.builtinParent", &[])))?;
    std::fs::create_dir_all(parent)?;
    crate::infra::platform::write_file_atomic(path, contents)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/builtin_test.rs"]
mod tests;
