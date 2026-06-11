use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use super::super::cmd_skill::{run, SkillCommand};
use super::super::parse::ChatCommandOutcome;
use crate::api::chat::ChatContext;
use crate::infra::error::AppError;
use crate::AppConfig;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

struct CurrentDirGuard {
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

struct SkillReadPrimitive;

#[async_trait::async_trait]
impl crate::core::tools::primitive::PrimitiveExecutor for SkillReadPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        std::fs::read_to_string(path).map_err(AppError::Io)
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::DirEntry>, AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::WriteFileResult, AppError> {
        unreachable!()
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<crate::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::EditFileResult, AppError> {
        unreachable!()
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<crate::BashResult, AppError> {
        unreachable!()
    }

    async fn require_user_confirmation(
        &self,
        _operation: crate::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

fn write_skill(workspace: &Path, name: &str, description: &str, user_only: bool) {
    let skill_dir = workspace.join(".tomcat").join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let mut content = format!("---\nname: {name}\ndescription: {description}\n");
    if user_only {
        content.push_str("disable-model-invocation: true\n");
    }
    content.push_str("---\n# Skill Body\nFollow the requested procedure.\n");
    std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
}

#[tokio::test]
#[serial(env_lock)]
async fn run_skill_reload_replaces_runtime_skill_set() {
    const API_ENV: &str = "TOMCAT_CMD_SKILL_RELOAD_TEST_KEY";

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_skill(workspace.path(), "commit", "Create a git commit.", false);

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let outcome = run(&ctx, SkillCommand::Reload).await;
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(ctx.skill_set_snapshot().resolve_any("commit").is_some());
}

#[tokio::test]
#[serial(env_lock)]
async fn run_skill_use_allows_user_only_skill_and_injects_body() {
    const API_ENV: &str = "TOMCAT_CMD_SKILL_USE_TEST_KEY";

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_skill(workspace.path(), "secret", "User only skill.", true);

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let mut ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    ctx.global_services.primitive = Arc::new(SkillReadPrimitive);
    let _ = run(&ctx, SkillCommand::Reload).await;

    let outcome = run(
        &ctx,
        SkillCommand::Use {
            name: "secret".to_string(),
            intent: "summarize the request".to_string(),
        },
    )
    .await;

    match outcome {
        ChatCommandOutcome::Continue {
            line,
            echo_user,
            history_line,
        } => {
            assert!(!echo_user);
            assert!(line.contains("<skill name=\"secret\""));
            assert!(line.contains("Current user intent:\nsummarize the request"));
            assert_eq!(
                history_line.as_deref(),
                Some("/skill use secret summarize the request")
            );
        }
        ChatCommandOutcome::Handled => panic!("/skill use should continue into the next turn"),
    }
}
