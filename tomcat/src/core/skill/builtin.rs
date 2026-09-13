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
        .ok_or_else(|| AppError::Config("内置 verify skill 路径缺少父目录".into()))?;
    std::fs::create_dir_all(parent)?;
    crate::infra::platform::write_file_atomic(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        materialize_builtin_skills, CONNECTORS_SKILL_ASSETS, PLUGIN_CREATOR_ASSETS,
        SKILL_CREATOR_ASSETS, VERIFY_SKILL_ASSETS,
    };
    use crate::core::skill::{discover, load_skill_payload, skill_roots, SkillSource};
    use crate::core::tools::primitive::PrimitiveExecutor;
    use crate::infra::error::AppError;
    use crate::AppConfig;

    struct SkillFilePrimitive;

    #[async_trait::async_trait]
    impl PrimitiveExecutor for SkillFilePrimitive {
        async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
            std::fs::read_to_string(path).map_err(AppError::Io)
        }

        async fn list_dir(
            &self,
            _path: &str,
            _plugin_id: &str,
        ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
            unreachable!("the built-in skill load test only reads a file")
        }

        async fn write_file(
            &self,
            _path: &str,
            _content: &str,
            _overwrite: bool,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
            unreachable!("the built-in skill load test only reads a file")
        }

        async fn edit_file(
            &self,
            _path: &str,
            _edits: Vec<crate::core::tools::primitive::EditOperation>,
            _plugin_id: &str,
        ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
            unreachable!("the built-in skill load test only reads a file")
        }

        async fn execute_bash(
            &self,
            _command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
            _foreground_wait_ms: Option<u64>,
        ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
            unreachable!("the built-in skill load test only reads a file")
        }

        async fn require_user_confirmation(
            &self,
            _operation: crate::core::tools::primitive::PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            unreachable!("the built-in skill load test only reads a file")
        }
    }

    #[test]
    fn verify_skill_materialized_content_declares_evidence_contract() {
        let skill = std::str::from_utf8(
            VERIFY_SKILL_ASSETS
                .get_file("SKILL.md")
                .expect("embedded verify SKILL.md")
                .contents(),
        )
        .expect("embedded verify SKILL.md is UTF-8");
        assert!(skill.contains("name: verify"));
        assert!(skill.contains("run_in_background=true"));
        assert!(skill.contains("\"green_build_pass\": true"));
        assert!(skill.contains("\"command\": \"<the exact background bash command>\""));
        assert!(skill.contains("## UI acceptance"));
        assert!(
            VERIFY_SKILL_ASSETS
                .get_file("references/ui-checklist.md")
                .is_some(),
            "the UI checklist must be available for progressive disclosure"
        );
        let connectors = std::str::from_utf8(
            CONNECTORS_SKILL_ASSETS
                .get_file("SKILL.md")
                .expect("embedded connectors SKILL.md")
                .contents(),
        )
        .expect("embedded connectors SKILL.md is UTF-8");
        assert!(connectors.contains("name: connectors"));
        assert!(connectors.contains("tool_search"));
        assert!(connectors.contains("tool_describe"));
        assert!(connectors.contains("tool_call"));
        let skill_creator = std::str::from_utf8(
            SKILL_CREATOR_ASSETS
                .get_file("SKILL.md")
                .expect("embedded skill-creator SKILL.md")
                .contents(),
        )
        .expect("embedded skill-creator SKILL.md is UTF-8");
        assert!(skill_creator.contains("name: skill-creator"));
        assert!(SKILL_CREATOR_ASSETS
            .get_file("scripts/init_skill.py")
            .is_some());
        assert!(SKILL_CREATOR_ASSETS
            .get_file("scripts/quick_validate.py")
            .is_some());
        let plugin_creator = std::str::from_utf8(
            PLUGIN_CREATOR_ASSETS
                .get_file("SKILL.md")
                .expect("embedded plugin-creator SKILL.md")
                .contents(),
        )
        .expect("embedded plugin-creator SKILL.md is UTF-8");
        assert!(plugin_creator.contains("name: plugin-creator"));
        assert!(plugin_creator.contains("package_install"));
        assert!(PLUGIN_CREATOR_ASSETS
            .get_file("scripts/init_plugin.mjs")
            .is_some());
    }

    #[test]
    fn skill_core_does_not_depend_on_connector_runtime() {
        // The static connectors skill is an asset like verify. Its loader must remain generic:
        // live catalog data belongs to tool_search results, not `core::skill`.
        for source in [
            include_str!("mod.rs"),
            include_str!("model.rs"),
            include_str!("catalog.rs"),
            include_str!("discovery.rs"),
            include_str!("frontmatter.rs"),
            include_str!("load.rs"),
        ] {
            assert!(
                !source.contains("McpManager") && !source.contains("ConnectorRegistry"),
                "core::skill must not import the connector runtime"
            );
        }
    }

    #[tokio::test]
    async fn materialized_verify_skill_is_discovered_and_loadable() {
        let temp = tempfile::tempdir().expect("test temp directory");
        let workspace = temp.path().join("workspace");
        let work_dir = temp.path().join("work");
        std::fs::create_dir_all(&workspace).expect("test workspace");

        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());

        let materialized_path =
            materialize_builtin_skills(&cfg).expect("materialize built-in verify skill");
        let skills = discover(&cfg, &workspace);
        let verify = skills
            .by_name
            .get("verify")
            .expect("discovery should find materialized verify skill");
        assert_eq!(verify.source, SkillSource::Managed);
        assert_eq!(
            std::fs::read_to_string(&verify.file_path).expect("read discovered skill"),
            std::fs::read_to_string(&materialized_path).expect("read materialized skill"),
            "discovery must reference the content materialized into the managed skill root"
        );

        let payload = load_skill_payload(&SkillFilePrimitive, "__test__", verify, None)
            .await
            .expect("load materialized verify skill through the normal loader");
        assert!(payload.contains("<skill name=\"verify\" location=\"SKILL.md\">"));
        assert!(payload.contains("## Green-build verification"));
        assert!(
            payload.contains("Decide the acceptance scope with a floor, a ladder, and a ratchet")
        );
        assert!(payload.contains("every declared acceptance command in the plan"));
        assert!(payload.contains("you may only widen, never narrow"));
        assert!(payload.contains("a full project run is not the default"));
        assert!(!payload.contains("complete check set"));
        assert!(!payload.contains("Scale the checks"));
        assert!(payload.contains("## UI acceptance"));
        assert!(!payload.contains("name: verify"));

        let checklist = load_skill_payload(
            &SkillFilePrimitive,
            "__test__",
            verify,
            Some("references/ui-checklist.md"),
        )
        .await
        .expect("load UI checklist through the normal loader");
        assert!(checklist.contains("UI acceptance checklist"));
        assert!(checklist.contains("Capture structural, visual, and runtime evidence"));

        let connectors = skills
            .by_name
            .get("connectors")
            .expect("discovery should find materialized connectors skill");
        assert_eq!(connectors.source, SkillSource::Managed);
        let connector_payload =
            load_skill_payload(&SkillFilePrimitive, "__test__", connectors, None)
                .await
                .expect("load materialized connectors skill through the normal loader");
        assert!(connector_payload.contains("## Deferred connector workflow"));
        assert!(connector_payload.contains("tool_search(source=\"<source>\")"));
    }

    #[tokio::test]
    async fn creator_assets_are_complete_loadable_and_reusable_across_projects() {
        let temp = tempfile::tempdir().expect("test temp directory");
        let work_dir = temp.path().join("work");
        let workspace_a = temp.path().join("workspace-a");
        let workspace_b = temp.path().join("workspace-b");
        std::fs::create_dir_all(&workspace_a).unwrap();
        std::fs::create_dir_all(&workspace_b).unwrap();
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());
        materialize_builtin_skills(&cfg).expect("materialize creators");

        for workspace in [&workspace_a, &workspace_b] {
            let skills = discover(&cfg, workspace);
            for (name, attachment, expected) in [
                (
                    "skill-creator",
                    "scripts/init_skill.py",
                    "Create a portable Tomcat skill",
                ),
                (
                    "plugin-creator",
                    "scripts/init_plugin.mjs",
                    "tomcat.registerTool",
                ),
            ] {
                let skill = skills.by_name.get(name).expect("creator discovered");
                assert_eq!(skill.source, SkillSource::Managed);
                let body = load_skill_payload(&SkillFilePrimitive, "__test__", skill, None)
                    .await
                    .expect("load creator body");
                assert!(
                    body.contains(&format!("<skill name=\"{name}\"")),
                    "the normal loader must return the creator entrypoint"
                );
                let attachment =
                    load_skill_payload(&SkillFilePrimitive, "__test__", skill, Some(attachment))
                        .await
                        .expect("load creator attachment");
                assert!(attachment.contains(expected));
            }
        }
    }

    #[test]
    fn materialization_is_idempotent_and_refreshes_changed_managed_assets() {
        let temp = tempfile::tempdir().expect("test temp directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

        let skill_path = materialize_builtin_skills(&cfg).expect("initial materialization");
        let checklist_path = skill_path
            .parent()
            .expect("SKILL.md parent")
            .join("references")
            .join("ui-checklist.md");
        let original_skill = std::fs::read(&skill_path).expect("read materialized skill");
        let original_checklist =
            std::fs::read(&checklist_path).expect("read materialized checklist");

        materialize_builtin_skills(&cfg).expect("idempotent materialization");
        assert_eq!(
            std::fs::read(&skill_path).expect("read idempotent skill"),
            original_skill
        );

        std::fs::write(&checklist_path, "stale managed content").expect("mutate managed file");
        materialize_builtin_skills(&cfg).expect("refresh changed managed asset");
        assert_eq!(
            std::fs::read(&checklist_path).expect("read refreshed checklist"),
            original_checklist
        );
    }

    #[test]
    fn materialized_verify_skill_writes_all_script_files_without_touching_dependencies() {
        let temp = tempfile::tempdir().expect("test temp directory");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());

        let skill_path = materialize_builtin_skills(&cfg).expect("materialize verify skill");
        let skill_root = skill_path.parent().expect("skill root");
        for relative_path in [
            "SKILL.md",
            "references/ui-checklist.md",
            "scripts/bootstrap.mjs",
            "scripts/browser-path.mjs",
            "scripts/shot.mjs",
            "scripts/package.json",
            "scripts/package-lock.json",
        ] {
            let materialized = skill_root.join(relative_path);
            let embedded = VERIFY_SKILL_ASSETS
                .get_file(relative_path)
                .expect("embedded verify asset");
            assert_eq!(
                std::fs::read(&materialized).expect("read materialized asset"),
                embedded.contents(),
                "asset {relative_path} must be written byte-for-byte"
            );
        }

        let node_modules_sentinel = skill_root.join("scripts/node_modules/sentinel");
        let browser_cache_sentinel = temp.path().join("work/cache/playwright/sentinel");
        assert!(!node_modules_sentinel.exists());
        assert!(!browser_cache_sentinel.exists());
        std::fs::create_dir_all(node_modules_sentinel.parent().unwrap()).unwrap();
        std::fs::create_dir_all(browser_cache_sentinel.parent().unwrap()).unwrap();
        std::fs::write(&node_modules_sentinel, "preserve dependency").unwrap();
        std::fs::write(&browser_cache_sentinel, "preserve browser").unwrap();

        materialize_builtin_skills(&cfg).expect("re-materialize verify skill");
        assert_eq!(
            std::fs::read_to_string(node_modules_sentinel).unwrap(),
            "preserve dependency"
        );
        assert_eq!(
            std::fs::read_to_string(browser_cache_sentinel).unwrap(),
            "preserve browser"
        );
    }

    #[test]
    fn project_and_agent_verify_skills_shadow_the_managed_copy() {
        let temp = tempfile::tempdir().expect("test temp directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        materialize_builtin_skills(&cfg).expect("materialize managed verify");
        let roots = skill_roots(&cfg, &workspace).expect("skill roots");
        let project_root = &roots[0].1;
        let agent_root = &roots[1].1;
        let shadow = "---\nname: verify\ndescription: shadow\n---\nshadow\n";

        std::fs::create_dir_all(project_root.join("verify")).expect("project shadow directory");
        std::fs::write(project_root.join("verify/SKILL.md"), shadow).expect("project shadow");
        std::fs::create_dir_all(agent_root.join("verify")).expect("agent shadow directory");
        std::fs::write(agent_root.join("verify/SKILL.md"), shadow).expect("agent shadow");

        let skills = discover(&cfg, &workspace);
        let verify = skills.by_name.get("verify").expect("verify discovered");
        assert_eq!(verify.source, SkillSource::Project);
        assert_eq!(
            std::fs::read_to_string(&verify.file_path).expect("project content"),
            shadow
        );
    }
}
