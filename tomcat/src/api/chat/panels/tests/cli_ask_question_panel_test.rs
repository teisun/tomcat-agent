use std::ffi::OsString;
use std::sync::OnceLock;

#[cfg(unix)]
use std::os::fd::RawFd;

#[cfg(unix)]
use crate::api::chat::panels::cli_ask_question_panel::read_one_line_from_fd_for_test;
use crate::api::chat::panels::CliAskQuestionPanel;
use crate::core::plan_runtime::panels::{
    AskQuestionOutcome, AskQuestionPanel, AskQuestionTermination, Question, QuestionOption,
};
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
        allow_custom: true,
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
        .ask(vec![sample_question()], AskQuestionTermination::default())
        .await;

    assert_eq!(result.outcome, AskQuestionOutcome::Answered);
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
async fn cli_panel_returns_interrupted_immediately_when_signal_already_set() {
    let panel = CliAskQuestionPanel;
    let termination = AskQuestionTermination::default();
    termination.interrupt();

    let result = panel.ask(vec![sample_question()], termination).await;

    assert_eq!(result.outcome, AskQuestionOutcome::Interrupted);
    assert!(
        result.answers.is_empty(),
        "取消路径不应返回半截 answers，避免 CLI 交互残留脏数据"
    );
}

#[cfg(unix)]
struct TestPty {
    master: RawFd,
    slave: RawFd,
}

#[cfg(unix)]
impl TestPty {
    fn open() -> Self {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: pointers target initialized descriptors; null termios/winsize request defaults.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            result,
            0,
            "openpty failed: {}",
            std::io::Error::last_os_error()
        );
        Self { master, slave }
    }

    fn write_line(&self, line: &str) {
        let bytes = line.as_bytes();
        // SAFETY: master is an open PTY descriptor and bytes is valid for the call duration.
        let count = unsafe {
            libc::write(
                self.master,
                bytes.as_ptr().cast::<libc::c_void>(),
                bytes.len(),
            )
        };
        assert_eq!(count, bytes.len() as isize, "write PTY input");
    }
}

#[cfg(unix)]
impl Drop for TestPty {
    fn drop(&mut self) {
        // SAFETY: descriptors are owned by this test helper and closed once here.
        unsafe {
            if self.master >= 0 {
                libc::close(self.master);
            }
            if self.slave >= 0 {
                libc::close(self.slave);
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cli_pty_waits_without_input_then_sigint_releases_reader_for_next_line() {
    let pty = TestPty::open();
    let first_termination = AskQuestionTermination::default();
    let first = tokio::spawn(read_one_line_from_fd_for_test(
        pty.slave,
        first_termination.clone(),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        !first.is_finished(),
        "PTY reader must have no wall-clock deadline"
    );

    first_termination.interrupt();
    let interrupted = tokio::time::timeout(std::time::Duration::from_secs(1), first)
        .await
        .expect("interrupted PTY reader did not exit")
        .expect("PTY reader task")
        .expect_err("interrupt must terminate the question reader");
    assert_eq!(interrupted.outcome, AskQuestionOutcome::Interrupted);

    pty.write_line("2\n");
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read_one_line_from_fd_for_test(pty.slave, AskQuestionTermination::default()),
    )
    .await
    .expect("next PTY reader was blocked by a leaked owner")
    .expect("next PTY line");
    assert_eq!(
        second, "2\n",
        "the interrupted reader must not swallow next-turn input"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cli_pty_eof_returns_host_disconnected_without_reader_leak() {
    let mut pty = TestPty::open();
    // SAFETY: close the owned master once to deliver an irrecoverable channel closure.
    unsafe { libc::close(pty.master) };
    pty.master = -1;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read_one_line_from_fd_for_test(pty.slave, AskQuestionTermination::default()),
    )
    .await
    .expect("EOF reader did not exit")
    .expect_err("PTY EOF must not be treated as an answer");
    assert_eq!(result.outcome, AskQuestionOutcome::HostDisconnected);
}
