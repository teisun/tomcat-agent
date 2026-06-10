use std::sync::Arc;

use crate::api::chat::panels::{AskQuestionPanel, MockAskQuestionPanel};
use crate::api::chat::{ChatContext, ChatContextOverrides};
use crate::AppConfig;

#[test]
fn chat_context_uses_injected_ask_question_panel_override() {
    const ENV_KEY: &str = "TOMCAT_CHAT_CONTEXT_OVERRIDE_ASKQ_KEY";

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(ENV_KEY.to_string());

    // SAFETY: 测试使用独立 env key，结束后立即清理。
    unsafe { std::env::set_var(ENV_KEY, "stub") };
    let injected: Arc<dyn AskQuestionPanel> = Arc::new(MockAskQuestionPanel::new(vec![]));
    let ctx = ChatContext::from_config_with_overrides(
        cfg,
        ChatContextOverrides::default().with_ask_question_panel(injected.clone()),
    )
    .expect("chat context should be created");
    let attached = ctx
        .session_runtime
        .plan_runtime
        .ask_question_panel()
        .expect("override panel should be attached");
    assert!(
        Arc::ptr_eq(&attached, &injected),
        "显式注入的 AskQuestionPanel 应覆盖 CLI 默认值"
    );
    // SAFETY: 清理测试环境变量。
    unsafe { std::env::remove_var(ENV_KEY) };
}
