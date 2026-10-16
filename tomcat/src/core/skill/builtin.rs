//! 内置 skill 的物化。
//!
//! skill discovery 只扫描磁盘目录；把内嵌内容写到 managed root 后，内置 skill 与
//! 用户安装的 skill 共用同一套 frontmatter、权限和 `load_skill` 路径。

use std::path::{Path, PathBuf};

use crate::infra::config::get_work_dir;
use crate::infra::error::AppError;
use crate::AppConfig;

// `build.rs` 生成：`assets/skills/<skill>/` 下的源文件，路径形如 `verify/SKILL.md`。
include!(concat!(env!("OUT_DIR"), "/builtin_skills.rs"));

pub fn materialize_builtin_skills(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    let managed_skills_root = get_work_dir(cfg)?.join("skills");
    for (relative, contents) in BUILTIN_SKILL_FILES {
        materialize_file(&managed_skills_root.join(relative), contents)?;
    }
    Ok(managed_skills_root.join("verify").join("SKILL.md"))
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
