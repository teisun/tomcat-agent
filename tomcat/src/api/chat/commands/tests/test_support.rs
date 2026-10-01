use crate::api::chat::panels::{AskQuestionPanel, MockAskQuestionPanel};
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::{AppConfig, SessionMode};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

struct EnvGuard(&'static str, Option<OsString>);
impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value.into());
        }
        Self(key, old)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.1.take() {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }
}
pub(super) struct Fixture {
    pub workspace: tempfile::TempDir,
    pub work: tempfile::TempDir,
    _home: tempfile::TempDir,
    _home_guard: EnvGuard,
    _key_guard: EnvGuard,
}
impl Fixture {
    pub fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let home_guard = EnvGuard::set("HOME", home.path().as_os_str());
        let key_guard = EnvGuard::set("TOMCAT_SHARED_SLASH_TEST_KEY", "stub");
        Self {
            workspace: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
            _home: home,
            _home_guard: home_guard,
            _key_guard: key_guard,
        }
    }
    pub fn ctx(&self) -> ChatContext {
        self.ctx_with_panel(Arc::new(MockAskQuestionPanel::new(vec![])))
    }
    pub fn ctx_with_panel(&self, panel: Arc<dyn AskQuestionPanel>) -> ChatContext {
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(self.work.path().to_string_lossy().into_owned());
        cfg.llm.api_key_env = Some("TOMCAT_SHARED_SLASH_TEST_KEY".into());
        crate::test_support::write_models_override(
            self.work.path(),
            &[
                crate::test_support::TestModelOverride::gpt54_openai_responses(
                    "TOMCAT_SHARED_SLASH_TEST_KEY",
                ),
            ],
        );
        ChatContext::from_config_with_mode_and_overrides(
            cfg,
            SessionMode::Code,
            ChatContextOverrides::default()
                .with_session_cwd_override(self.workspace.path().to_path_buf())
                .with_ask_question_panel(panel),
        )
        .unwrap()
    }
}
pub(super) fn skill(root: &Path, name: &str, description: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
    )
    .unwrap();
}
pub(super) fn plugin(root: &Path, id: &str, tool: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("plugin.json"), serde_json::json!({"id":id,"name":id,"description":"test","version":"1.0.0","author":"test","main":"main.js","requiredPermissions":[],"requiredApiVersion":"1.0","tags":[],"tools":[{"name":tool,"description":"test","parameters":{"type":"object"}}]}).to_string()).unwrap();
    std::fs::write(root.join("main.js"), format!("pi.registerTool({{name:'{tool}',description:'test',parameters:{{type:'object'}},execute:function(){{return {{ok:true}};}}}});" )).unwrap();
}
