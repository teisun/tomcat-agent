use std::ffi::OsString;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use crate::api::chat::panels::CliAskQuestionPanel;
use crate::core::plan_runtime::panels::{AskQuestionPanel, Question, QuestionOption};
use tokio::sync::Mutex;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old = std::env::var_os(key);
        // SAFETY: 测试用 env 变量受进程级互斥锁保护，Drop 时恢复。
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => {
                // SAFETY: 与 set 配对，仍在测试互斥锁保护范围内。
                unsafe { std::env::set_var(self.key, v) };
            }
            None => {
                // SAFETY: 与 set 配对，仍在测试互斥锁保护范围内。
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

fn sample_question() -> Question {
    Question {
        id: "deploy_target".into(),
        prompt: "选择发布目标".into(),
        options: vec![
            QuestionOption {
                id: "staging".into(),
                label: "Staging".into(),
                recommended: true,
            },
            QuestionOption {
                id: "prod".into(),
                label: "Production".into(),
                recommended: false,
            },
        ],
    }
}

#[tokio::test]
async fn cli_panel_auto_picks_recommended_answer_when_test_env_enabled() {
    let _lock = env_lock().lock().await;
    let _guard = EnvGuard::set("TOMCAT_ASK_QUESTION_TEST_AUTO_PICK", "recommended");
    let panel = CliAskQuestionPanel;

    let result = panel
        .ask(vec![sample_question()], Arc::new(AtomicBool::new(false)))
        .await;

    assert!(!result.cancelled, "auto-pick 不应走 cancelled");
    assert_eq!(result.answers.len(), 1, "应返回 1 个回答");
    assert_eq!(result.answers[0].question_id, "deploy_target");
    assert_eq!(result.answers[0].option_ids, vec!["staging"]);
    assert!(
        result.answers[0].picked_recommended,
        "auto-pick 应命中 recommended 选项"
    );
    assert!(!result.answers[0].skipped);
    assert_eq!(result.answers[0].custom_text, None);
}

#[tokio::test]
async fn cli_panel_returns_cancelled_immediately_when_signal_already_set() {
    let panel = CliAskQuestionPanel;
    let cancel = Arc::new(AtomicBool::new(true));

    let result = panel.ask(vec![sample_question()], cancel).await;

    assert!(result.cancelled, "cancel_signal 已置位时应立即取消");
    assert!(
        result.answers.is_empty(),
        "取消路径不应返回半截 answers，避免 CLI 交互残留脏数据"
    );
}
