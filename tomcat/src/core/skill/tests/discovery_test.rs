use std::path::{Path, PathBuf};

use crate::core::skill::{discover, skill_roots, SkillSource};
use crate::AppConfig;

#[test]
fn skill_roots_follow_project_agent_managed_order() {
    let temp = temp_dir("roots_order");
    let project = temp.join("project");
    let work_dir = temp.join("work");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();

    let mut cfg = AppConfig::default();
    cfg.agent.id = "spike".to_string();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());

    let roots = skill_roots(&cfg, &project).unwrap();
    assert_eq!(roots[0].0, SkillSource::Project);
    assert_eq!(roots[0].1, project.join(".tomcat").join("skills"));
    assert_eq!(roots[1].0, SkillSource::Agent);
    assert!(roots[1]
        .1
        .ends_with(Path::new("agents").join("spike").join("skills")));
    assert_eq!(roots[2].0, SkillSource::Managed);
    assert!(roots[2].1.ends_with(Path::new("skills")));

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn discover_prefers_higher_priority_same_name() {
    let temp = temp_dir("prefer_project");
    let project = temp.join("project");
    let work_dir = temp.join("work");
    let mut cfg = base_config(&work_dir);

    write_skill(
        &project
            .join(".tomcat")
            .join("skills")
            .join("commit")
            .join("SKILL.md"),
        "commit",
        "project skill",
    );
    write_skill(
        &work_dir
            .join("agents")
            .join("spike")
            .join("skills")
            .join("commit")
            .join("SKILL.md"),
        "commit",
        "agent skill",
    );
    write_skill(
        &work_dir.join("skills").join("commit").join("SKILL.md"),
        "commit",
        "managed skill",
    );
    write_skill(
        &work_dir.join("skills").join("lint").join("SKILL.md"),
        "lint",
        "lint skill",
    );

    let set = discover(&cfg, &project);
    let commit = set.by_name.get("commit").expect("project winner");
    assert_eq!(commit.source, SkillSource::Project);
    assert_eq!(commit.description, "project skill");
    assert!(set.by_name.contains_key("lint"));
    assert!(set
        .warnings
        .iter()
        .any(|warning| warning == "skill_shadowed:commit by project"));

    cfg.skills.disabled = vec!["lint".to_string()];
    let disabled_set = discover(&cfg, &project);
    assert!(!disabled_set.by_name.contains_key("lint"));

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn discover_keeps_diagnostics_for_bad_skill_without_blocking_good_ones() {
    let temp = temp_dir("bad_skill");
    let project = temp.join("project");
    let work_dir = temp.join("work");
    let cfg = base_config(&work_dir);

    write_raw(
        &work_dir.join("skills").join("broken").join("SKILL.md"),
        "# broken\n",
    );
    write_skill(
        &work_dir.join("skills").join("commit").join("SKILL.md"),
        "commit",
        "commit skill",
    );

    let set = discover(&cfg, &project);
    assert!(set.by_name.contains_key("commit"));
    assert_eq!(set.diagnostics.len(), 1);
    assert!(set.diagnostics[0]
        .reason
        .contains("skill 文件缺少 frontmatter 分隔符"));

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn discover_ignores_loose_markdown_files_in_project_root() {
    let temp = temp_dir("ignore_loose_markdown");
    let project = temp.join("project");
    let work_dir = temp.join("work");
    let cfg = base_config(&work_dir);

    write_skill(
        &project.join(".tomcat").join("skills").join("commit.md"),
        "commit",
        "loose markdown should be ignored",
    );
    write_skill(
        &project
            .join(".tomcat")
            .join("skills")
            .join("lint")
            .join("SKILL.md"),
        "lint",
        "directory skill",
    );

    let set = discover(&cfg, &project);
    assert!(!set.by_name.contains_key("commit"));
    assert!(set.by_name.contains_key("lint"));

    let _ = std::fs::remove_dir_all(&temp);
}

fn base_config(work_dir: &Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.agent.id = "spike".to_string();
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().to_string());
    cfg
}

fn write_skill(path: &Path, name: &str, description: &str) {
    write_raw(
        path,
        &format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    );
}

fn write_raw(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tomcat_skill_test_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
