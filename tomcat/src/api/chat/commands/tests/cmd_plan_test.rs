use std::ffi::OsString;

use super::super::cmd_plan::{parse_args, run, PlanCommand};
use super::super::parse::{ChatCommand, ChatCommandOutcome};
use crate::api::chat::ChatContext;
use crate::core::plan_runtime::file_store::{
    self, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem, TodoStatus,
    DEFAULT_LOCK_TIMEOUT_MS, PLAN_FILE_SCHEMA_VERSION,
};
use crate::core::plan_runtime::PlanState;
use crate::AppConfig;
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        // SAFETY: 测试内对进程级 env 的写入受调用方互斥或局部作用域约束，Drop 会恢复。
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => {
                // SAFETY: 与 set 成对，恢复原值。
                unsafe { std::env::set_var(self.key, v) };
            }
            None => {
                // SAFETY: 与 set 成对，恢复缺省态。
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

fn ensure_plans_dir_exists() {
    let plans_dir = file_store::plans_dir().expect("plans dir");
    std::fs::create_dir_all(&plans_dir).expect("create plans dir");
}

#[test]
fn parse_plan_without_args_enters_planning() {
    let cmd = parse_args(vec!["/plan".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Enter)));
}

#[test]
fn parse_plan_exit() {
    let cmd = parse_args(vec!["/plan".into(), "exit".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::Exit)));
}

#[test]
fn parse_plan_build_with_id() {
    let cmd = parse_args(vec!["/plan".into(), "build".into(), "ship-001".into()]);
    assert!(matches!(
        cmd,
        ChatCommand::Plan(PlanCommand::Build { ref plan_target })
            if plan_target.as_deref() == Some("ship-001")
    ));
}

#[test]
fn parse_plan_build_with_path() {
    let cmd = parse_args(vec![
        "/plan".into(),
        "build".into(),
        "/tmp/ship-001.plan.md".into(),
    ]);
    assert!(matches!(
        cmd,
        ChatCommand::Plan(PlanCommand::Build { ref plan_target })
            if plan_target.as_deref() == Some("/tmp/ship-001.plan.md")
    ));
}

#[test]
fn parse_plan_with_extra_arg_returns_usage_error() {
    let cmd = parse_args(vec!["/plan".into(), "ship".into()]);
    assert!(matches!(cmd, ChatCommand::UsageError { .. }));
}

#[test]
fn parse_plan_list() {
    let cmd = parse_args(vec!["/plan".into(), "list".into()]);
    assert!(matches!(cmd, ChatCommand::Plan(PlanCommand::List)));
}

#[test]
fn parse_plan_build_without_id_uses_default_target() {
    let cmd = parse_args(vec!["/plan".into(), "build".into()]);
    assert!(matches!(
        cmd,
        ChatCommand::Plan(PlanCommand::Build { plan_target: None })
    ));
}

#[test]
#[serial(env_lock)]
fn run_plan_build_returns_continue_for_existing_plan() {
    const API_ENV: &str = "TOMCAT_CMD_PLAN_BUILD_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    ensure_plans_dir_exists();
    let plan_id = "ship-001";
    let plan_path = file_store::plan_path_for_id(plan_id).expect("plan path");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.to_string(),
            goal: "ship the release".to_string(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: "2026-05-26T00:00:00Z".to_string(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".to_string(),
                content: "build the release".to_string(),
                status: TodoStatus::Pending,
            }],
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Draft\n- build the release\n".to_string(),
    };
    file_store::write_plan(&plan_path, &plan, DEFAULT_LOCK_TIMEOUT_MS).expect("write plan");

    let outcome = run(
        &ctx,
        PlanCommand::Build {
            plan_target: Some(plan_path.to_string_lossy().to_string()),
        },
    );

    match outcome {
        ChatCommandOutcome::Continue {
            line, echo_user, ..
        } => {
            assert!(echo_user, "/plan build 自动开跑时应回显 user line");
            assert_eq!(
                line,
                format!("start building {}", plan_path.to_string_lossy()),
                "应把 canonical plan path 交给下一轮 chat_loop 继续执行"
            );
        }
        ChatCommandOutcome::Handled => panic!("/plan build 成功时不应被当作纯本地 handled"),
    }

    assert!(
        matches!(ctx.session_runtime.plan_runtime.mode(), PlanState::Executing { ref plan_id } if plan_id == "ship-001"),
        "build 成功后 runtime 应切到 Executing"
    );
}

#[test]
#[serial(env_lock)]
fn run_plan_build_without_target_uses_runtime_default_source() {
    const API_ENV: &str = "TOMCAT_CMD_PLAN_BUILD_DEFAULT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    ensure_plans_dir_exists();
    let plan_id = "ship-default";
    let plan_path = file_store::plan_path_for_id(plan_id).expect("plan path");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.to_string(),
            goal: "ship the default release".to_string(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: "2026-05-26T00:00:00Z".to_string(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![TodoItem {
                id: "todo-1".to_string(),
                content: "build the release".to_string(),
                status: TodoStatus::Pending,
            }],
            unknown: serde_yaml::Mapping::new(),
        },
        body: "## Draft\n- build the release\n".to_string(),
    };
    file_store::write_plan(&plan_path, &plan, DEFAULT_LOCK_TIMEOUT_MS).expect("write plan");
    ctx.session_runtime
        .plan_runtime
        .set_active_planning_plan(plan_id.to_string(), plan_path.clone());

    let outcome = run(&ctx, PlanCommand::Build { plan_target: None });

    match outcome {
        ChatCommandOutcome::Continue {
            line, echo_user, ..
        } => {
            assert!(echo_user);
            assert_eq!(
                line,
                format!("start building {}", plan_path.to_string_lossy())
            );
        }
        ChatCommandOutcome::Handled => panic!("/plan build 默认源成功时不应被当作 handled"),
    }
    assert!(
        matches!(ctx.session_runtime.plan_runtime.mode(), PlanState::Executing { ref plan_id } if plan_id == "ship-default")
    );
}

#[test]
#[serial(env_lock)]
fn run_plan_exit_allows_pending_back_to_chat() {
    const API_ENV: &str = "TOMCAT_CMD_PLAN_EXIT_PENDING_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    ctx.session_runtime
        .plan_runtime
        .set_mode_pending("pending-plan".into());

    let outcome = run(&ctx, PlanCommand::Exit);
    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    assert!(matches!(ctx.session_runtime.plan_runtime.mode(), PlanState::Chat));
}
