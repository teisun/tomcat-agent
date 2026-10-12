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
    _lock: crate::test_support::TestLockGuard<'static>,
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
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
        _foreground_wait_ms: Option<u64>,
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
    let skill_dir = workspace.join(".agents").join("skills").join(name);
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
async fn run_command_shortcuts_and_exact_ids_preserve_local_routing() {
    use super::super::{cmd_command, parse_chat_command, ChatCommand};
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _api = EnvGuard::set("TOMCAT_COMMAND_TEST_KEY", "stub");
    let _cwd = CurrentDirGuard::set(workspace.path());
    let dir = workspace.path().join(".cursor/commands");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("review.md"), "COMMAND_BODY").unwrap();
    std::fs::write(dir.join("reload.md"), "RELOAD_PROMPT").unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().into());
    cfg.llm.api_key_env = Some("TOMCAT_COMMAND_TEST_KEY".into());
    let ctx = ChatContext::from_config(cfg).unwrap();
    for line in ["/review", "/review check tests"] {
        let ChatCommandOutcome::UserMessage { message, .. } =
            cmd_command::shortcut(&ctx, line.into())
        else {
            panic!("expected invocation");
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(
            json.contains("COMMAND_BODY") && json.contains("command:.cursor/commands/review.md")
        );
    }
    assert!(matches!(
        parse_chat_command("/reload"),
        ChatCommand::Shared { .. }
    ));
    assert!(matches!(
        cmd_command::run(
            &ctx,
            "/command use command:.cursor/commands/reload.md".into()
        ),
        ChatCommandOutcome::UserMessage { .. }
    ));
    assert!(matches!(
        cmd_command::shortcut(&ctx, "/foo".into()),
        ChatCommandOutcome::Continue { .. }
    ));
    let native = workspace.path().join(".agents/commands");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(native.join("review.md"), "SECOND").unwrap();
    assert!(matches!(
        cmd_command::shortcut(&ctx, "/review".into()),
        ChatCommandOutcome::Handled
    ));
    assert!(matches!(
        cmd_command::run(&ctx, "/command list".into()),
        ChatCommandOutcome::Handled
    ));
    assert!(matches!(
        cmd_command::run(&ctx, "/command use command:../../etc/passwd".into()),
        ChatCommandOutcome::Handled
    ));
}

#[tokio::test]
#[serial(env_lock)]
async fn command_printed_ids_round_trip_spaces_and_quotes() {
    use super::super::cmd_command;
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("HOME", home.path().as_os_str());
    let _api = EnvGuard::set("TOMCAT_COMMAND_QUOTE_TEST_KEY", "stub");
    let _cwd = CurrentDirGuard::set(workspace.path());
    for (source, name, body) in [
        (".cursor", "my review", "SPACE_BODY"),
        (".cursor", "review's", "CURSOR_QUOTE_BODY"),
        (".agents", "review's", "AGENTS_QUOTE_BODY"),
    ] {
        let directory = workspace.path().join(source).join("commands");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.md")), body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().into());
    cfg.llm.api_key_env = Some("TOMCAT_COMMAND_QUOTE_TEST_KEY".into());
    let ctx = ChatContext::from_config(cfg).unwrap();
    let commands = ctx.project_commands();
    let list = cmd_command::list_text(&commands.files);
    let printed = list
        .lines()
        .find(|line| line.contains("/my review  "))
        .unwrap();
    let quoted_id = printed.split_once("  .cursor  ").unwrap().1;
    let check = |line: String, expected_id: &str, body: &str| {
        let ChatCommandOutcome::UserMessage { message, .. } = cmd_command::run(&ctx, line) else {
            panic!("printed command did not invoke");
        };
        let serialized = serde_json::to_string(&message).unwrap();
        assert!(
            serialized.contains(expected_id) && serialized.contains(body),
            "{serialized}"
        );
    };
    check(
        format!("/command use {quoted_id}"),
        "command:.cursor/commands/my review.md",
        "SPACE_BODY",
    );
    assert!(matches!(
        cmd_command::shortcut(&ctx, "/review's".into()),
        ChatCommandOutcome::Handled
    ));
    let duplicates = commands
        .files
        .iter()
        .filter(|file| file.card.name == "review's")
        .collect::<Vec<_>>();
    let prompt = cmd_command::duplicate_text("review's", &duplicates);
    assert_eq!(duplicates.len(), 2);
    for (line, file) in prompt.lines().skip(1).zip(duplicates) {
        let body = if file.card.source == ".cursor" {
            "CURSOR_QUOTE_BODY"
        } else {
            "AGENTS_QUOTE_BODY"
        };
        check(line.trim().into(), &file.card.id, body);
    }
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
        ChatCommandOutcome::UserMessage {
            message,
            history_line,
        } => {
            let text = serde_json::to_string(&message).unwrap();
            assert!(text.contains("Follow the requested procedure"));
            assert!(text.contains("summarize the request"));
            assert!(text.contains("skill:secret"));
            assert!(!text.contains("User explicitly requested skill"));
            assert_eq!(history_line, "/skill use secret summarize the request");
        }
        _ => panic!("/skill use should produce a structured user message"),
    }
}
