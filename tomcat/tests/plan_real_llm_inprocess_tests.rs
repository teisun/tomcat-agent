//! E2E-PLAN-RL-002：进程内真 LLM 全路径测试（真 LlmProvider + real reviewer subagent）。
//!
//! 与 [`plan_e2e_with_mock_llm_tests.rs`](./plan_e2e_with_mock_llm_tests.rs) 互补：那个
//! 测试把 "LLM 决策一次 tool_call" 用直接调 `tools::execute` 代替，本测试不做任何 mock，
//! 真的让主 LLM 调 `create_plan` / `update_plan`、真的让 reviewer 子 Agent 跑一轮。
//!
//! ## 门禁
//! - `OPENAI_API_KEY` 必须存在；缺失 → 测试 panic 失败（E2E_TEST_SPEC §4）。
//! - 默认模型来自 `TOMCAT_E2E_LLM_MODEL` env，未设则 `gpt-5.2`。
//!
//! ## 数据目录
//! - 使用进程**真实 HOME**（不覆盖 `HOME` env）；plan 落盘到 `~/.tomcat/plans/`。
//! - 读取 `~/.tomcat/tomcat.config.toml`（存在时），与日常 `tomcat chat` 一致。
//! - cwd 切到 `~/.tomcat/temp/<run>/`（内置 workspace_roots），避免 `cargo test` 目录触发路径授权。
//! - 与 `cli_full_plan_path_with_real_llm` 共用盘目录，故标记 `#[serial]` 串行执行。
//!
//! ## 业务断言（硬门禁）
//! 1. `~/.tomcat/plans/<plan_id>.plan.md` 存在且 `frontmatter.mode == Completed`
//! 2. 所有 `frontmatter.todos[].status == Completed`
//! 3. 内存 `PlanRuntime::mode()` 与磁盘同步
//! 4. `finalize_completed_to_chat()` 返回 `Some(plan_id)`；之后 mode = Chat
//! 5. transcript 至少有一条 `plan.review` 自定义事件
//!
//! ## 软断言（不强求）
//! - reviewer summary aborted=false（reviewer LLM 可能格式漂移）
//! - 具体 tool_call 次数（LLM 可能合并）
//!
//! 这个测试整体包在 `tokio::time::timeout(600s)` 内，避免 LLM/网络挂死。

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serial_test::serial;
use tokio_util::sync::CancellationToken;

use tomcat::api::chat::plan_runtime::file_store::{
    plan_path_for_id, read_plan, PlanFileMode, TodoStatus,
};
use tomcat::api::chat::plan_runtime::PlanMode;
use tomcat::core::llm::system_prompt::{
    build_system_prompt_with_state, WorkspaceContext, WorkspaceState,
};
use tomcat::core::session::ContextState;
use tomcat::{
    init_context_state, load_config_toml_file, resolve_sessions_dir, run_chat_turn,
    AgentRunOutcome, ChatContext, SessionManager,
};

const PLANNING_PROMPT: &str = r##"You are helping draft an internal plan. Use the create_plan tool to draft a minimal 2-todo plan for the active discussion goal in this PLAN session.
Constraints:
- todos: exactly two, ids "t1" and "t2", short content (<= 30 chars each)
- draft: markdown content for the `## Plan` section only; do NOT include `## Goal`, `## Plan`, or `## Notes` headings yourself
- Do NOT call ask_question. Do NOT call any other tool. After create_plan returns successfully, reply with a short acknowledgement (<= 1 line) and stop."##;

const EXEC_PROMPT: &str = r##"Advance the active plan to completion using only update_plan.
Sequence (you MUST follow exactly this order; combine into fewer calls is fine):
1. update_plan set_status t1 in_progress
2. update_plan set_status t1 completed
3. update_plan set_status t2 in_progress
4. update_plan set_status t2 completed
Do NOT edit the plan file directly. Do NOT call ask_question. After step 4 lands successfully, reply "done" and stop."##;

const TOTAL_TIMEOUT: Duration = Duration::from_secs(420);
const PLANNING_TIMEOUT: Duration = Duration::from_secs(180);
const EXEC_TURN_TIMEOUT: Duration = Duration::from_secs(120);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

fn require_api_key() {
    common::load_openai_test_env();
    if std::env::var("OPENAI_API_KEY").is_err() {
        panic!(
            "plan_real_llm_inprocess_tests 必须设置 OPENAI_API_KEY（环境变量或 tomcat/.env；E2E-PLAN-RL-002 / E2E_TEST_SPEC §4）"
        );
    }
}

fn default_model() -> String {
    std::env::var("TOMCAT_E2E_LLM_MODEL").unwrap_or_else(|_| "gpt-5.2".to_string())
}

fn real_home() -> PathBuf {
    dirs::home_dir().expect("无法定位 HOME 目录")
}

fn ensure_plans_dir() {
    std::fs::create_dir_all(real_home().join(".tomcat").join("plans"))
        .expect("创建 ~/.tomcat/plans 失败");
}

fn user_config_path() -> PathBuf {
    real_home().join(".tomcat").join("tomcat.config.toml")
}

fn load_user_config() -> tomcat::AppConfig {
    let cfg_path = user_config_path();
    if cfg_path.exists() {
        load_config_toml_file(&cfg_path).expect("load ~/.tomcat/tomcat.config.toml 失败")
    } else {
        tomcat::load_config(None).expect("load_config 失败")
    }
}

fn build_system_text_minimal(ctx: &ChatContext) -> String {
    fn format_home_path(path: &std::path::Path) -> String {
        let Some(home) = dirs::home_dir() else {
            return path.display().to_string();
        };
        if path == home {
            return "~".to_string();
        }
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
        path.display().to_string()
    }

    let workspace_context = WorkspaceContext {
        agent_workspace_dir: ctx.agent_workspace_dir.to_string_lossy().to_string(),
        agent_definition_dir: ctx.agent_definition_dir.to_string_lossy().to_string(),
        agent_plans_dir: tomcat::api::chat::plan_runtime::file_store::plans_dir()
            .map(|path| format_home_path(&path))
            .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
        agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
    };
    build_system_prompt_with_state(
        workspace_context,
        WorkspaceState {
            read_write: Vec::new(),
            read_only: Vec::new(),
            path_rules: Vec::new(),
        },
    )
}

fn ensure_session(ctx: &ChatContext) {
    let key = ctx.session.current_session_key();
    if ctx.session.get_session(key).unwrap().is_none() {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        ctx.session.create_session(key, cwd).unwrap();
    }
}

/// 扫盘挑出 mode=planning 的最新 plan 文件，返回 plan_id。
fn pick_newest_planning_plan_id(home: &Path) -> Option<String> {
    let plans_dir = home.join(".tomcat").join("plans");
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&plans_dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(plan) = read_plan(&path) else { continue };
        if plan.frontmatter.mode != PlanFileMode::Planning {
            continue;
        }
        let mtime = entry.metadata().ok()?.modified().ok()?;
        match &best {
            Some((m, _)) if *m >= mtime => {}
            _ => best = Some((mtime, plan.frontmatter.plan_id.clone())),
        }
    }
    best.map(|(_, id)| id)
}

fn created_plan_from_outcome(outcome: &AgentRunOutcome) -> Option<common::CreatedPlanRef> {
    match outcome {
        AgentRunOutcome::Completed(result) | AgentRunOutcome::Interrupted(result) => {
            common::extract_created_plan_from_messages(&result.new_messages)
        }
        AgentRunOutcome::Failed(_) => None,
    }
}

#[derive(Default)]
struct InprocessDiagState {
    transcript_offset: usize,
    last_plan_snapshot: Option<String>,
}

fn tail_text_file(path: &Path, lines: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(
        content
            .lines()
            .rev()
            .take(lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn text_delta_from_file(path: &Path, offset: &mut usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let bytes = content.as_bytes();
    let start = if *offset <= bytes.len() { *offset } else { 0 };
    let delta = &bytes[start..];
    *offset = bytes.len();
    if delta.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(delta).to_string())
    }
}

fn format_plan_snapshot(plan_path: Option<&Path>) -> String {
    let Some(path) = plan_path else {
        return String::new();
    };
    let mut out = String::new();
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!("---- plan: {} ----\n", path.display()),
    );
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{content}\n"));
        }
        Err(err) => {
            let _ =
                std::fmt::Write::write_fmt(&mut out, format_args!("(无法读取当前 plan: {err})\n"));
        }
    }
    out
}

fn changed_snapshot(current: String, last: &mut Option<String>) -> Option<String> {
    if current.is_empty() {
        return None;
    }
    match last {
        Some(prev) if prev == &current => None,
        _ => {
            *last = Some(current.clone());
            Some(current)
        }
    }
}

fn push_plan_snapshot(body: &mut String, plan_path: Option<&Path>, state: &mut InprocessDiagState) {
    if let Some(plan_snapshot) = changed_snapshot(
        format_plan_snapshot(plan_path),
        &mut state.last_plan_snapshot,
    ) {
        let _ = std::fmt::Write::write_str(body, "---- current plan snapshot ----\n");
        body.push_str(&plan_snapshot);
    }
}

fn push_transcript_phase_summary(
    body: &mut String,
    ctx: &ChatContext,
    state: &mut InprocessDiagState,
) {
    if let Ok(Some(t)) = ctx.session.current_transcript_path() {
        if let Some(delta) = text_delta_from_file(&t, &mut state.transcript_offset) {
            let _ = std::fmt::Write::write_fmt(
                body,
                format_args!(
                    "---- persisted transcript summary (phase end, may lag live state changes): {} ----\n",
                    t.display()
                ),
            );
            let _ = std::fmt::Write::write_fmt(body, format_args!("{delta}\n"));
        }
    }
}

fn dump_diagnostic(
    home: &Path,
    ctx: &ChatContext,
    label: &str,
    plan_path: Option<&Path>,
    diag_state: Option<&mut InprocessDiagState>,
    full_snapshot: bool,
) {
    if !full_snapshot && diag_state.is_none() {
        return;
    }

    let mut out = String::new();
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!("\n==== [{label}] HOME = {} ====\n", home.display()),
    );
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!("plan_runtime.mode = {:?}\n", ctx.plan_runtime.mode()),
    );

    if full_snapshot {
        out.push_str(&format_plan_snapshot(plan_path));
        if let Ok(Some(t)) = ctx.session.current_transcript_path() {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!("---- transcript: {} (tail 80) ----\n", t.display()),
            );
            if let Some(tail) = tail_text_file(&t, 80) {
                for line in tail.lines() {
                    let _ = std::fmt::Write::write_fmt(&mut out, format_args!("  {line}\n"));
                }
            }
        }
        eprint!("{out}");
        return;
    }

    let state = diag_state.expect("diag state required for delta diagnostic");
    let mut body = String::new();
    push_plan_snapshot(&mut body, plan_path, state);
    if body.trim().is_empty() {
        return;
    }
    out.push_str(&body);
    eprint!("{out}");
}

fn dump_phase_summary(
    home: &Path,
    ctx: &ChatContext,
    label: &str,
    plan_path: Option<&Path>,
    diag_state: &mut InprocessDiagState,
) {
    let mut out = String::new();
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!("\n==== [{label}] HOME = {} ====\n", home.display()),
    );
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!("plan_runtime.mode = {:?}\n", ctx.plan_runtime.mode()),
    );

    let mut body = String::new();
    push_plan_snapshot(&mut body, plan_path, diag_state);
    push_transcript_phase_summary(&mut body, ctx, diag_state);
    if body.trim().is_empty() {
        return;
    }
    out.push_str(&body);
    eprint!("{out}");
}

#[allow(clippy::too_many_arguments)]
async fn run_chat_turn_observed(
    home: &Path,
    ctx: &ChatContext,
    label: &str,
    plan_path: Option<&Path>,
    diag_state: &mut InprocessDiagState,
    prompt: &str,
    system_text: &str,
    context_state: &mut ContextState,
    timeout: Duration,
) -> AgentRunOutcome {
    let mut turn = std::pin::pin!(run_chat_turn(
        ctx,
        prompt,
        system_text,
        context_state,
        CancellationToken::new(),
    ));
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                dump_diagnostic(
                    home,
                    ctx,
                    &format!("{label}_heartbeat"),
                    plan_path,
                    Some(diag_state),
                    false,
                );
            }
            _ = &mut deadline => {
                dump_diagnostic(home, ctx, &format!("{label}_timeout"), plan_path, None, true);
                panic!("{label} 在 {}s 内未完成", timeout.as_secs());
            }
            res = &mut turn => {
                let outcome = res.unwrap_or_else(|e| {
                    dump_diagnostic(home, ctx, &format!("{label}_err"), plan_path, None, true);
                    panic!("{label} run_chat_turn 返回 Err: {e}");
                });
                dump_phase_summary(
                    home,
                    ctx,
                    &format!("{label}_phase_summary"),
                    plan_path,
                    diag_state,
                );
                return outcome;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial]
async fn inprocess_full_plan_path_with_real_llm() {
    require_api_key();
    common::setup_logging();
    ensure_plans_dir();
    std::env::set_var("TOMCAT_ASK_QUESTION_TIMEOUT_MS", "5000");
    std::env::set_var("TOMCAT__LLM__DEFAULT_MODEL", default_model());
    let home = real_home();
    let config = load_user_config();
    let workdir = common::dot_tomcat_e2e_workdir("inprocess_real_llm");
    let _cwd = common::CwdGuard::set(&workdir);
    let sessions_dir = resolve_sessions_dir(&config).expect("resolve sessions dir");
    let fresh_session = common::begin_fresh_default_session(&sessions_dir, Some(&workdir));

    let result = tokio::time::timeout(TOTAL_TIMEOUT, async {
        let ctx = ChatContext::from_config(config).expect("ChatContext::from_config 失败");
        ensure_session(&ctx);
        let mut diag_state = InprocessDiagState::default();

        let system_text = build_system_text_minimal(&ctx);
        let mut context_state = init_context_state(&ctx.session, &ctx.config.context, &system_text)
            .expect("init_context_state 失败");

        // 1) /plan → Planning
        ctx.plan_runtime
            .enter_planning()
            .expect("enter_planning 失败");
        assert!(matches!(ctx.plan_runtime.mode(), PlanMode::Planning));

        // 2) 用真 LLM 跑 PLANNING_PROMPT；期望它调 create_plan
        let outcome = run_chat_turn_observed(
            &home,
            &ctx,
            "planning_phase",
            None,
            &mut diag_state,
            PLANNING_PROMPT,
            &system_text,
            &mut context_state,
            PLANNING_TIMEOUT,
        )
        .await;
        match &outcome {
            AgentRunOutcome::Completed(_) => {}
            AgentRunOutcome::Interrupted(_) | AgentRunOutcome::Failed(_) => {
                dump_diagnostic(&home, &ctx, "planning_phase", None, None, true);
                panic!("planning 阶段未 Completed: {outcome:?}");
            }
        }

        // 3) 优先从本次 run 的 tool result 取 create_plan 产物；失败时退回扫盘做诊断。
        let created_plan = created_plan_from_outcome(&outcome).unwrap_or_else(|| {
            let plan_id = pick_newest_planning_plan_id(&home).unwrap_or_else(|| {
                dump_diagnostic(&home, &ctx, "no_planning_plan", None, None, true);
                panic!("create_plan 未生成任何 mode=planning 的盘文件");
            });
            let plan_path = plan_path_for_id(&plan_id).expect("plan_path_for_id 失败");
            common::CreatedPlanRef {
                plan_id,
                path: plan_path,
            }
        });
        let plan_id = created_plan.plan_id.clone();
        let plan_path = created_plan.path.clone();
        assert!(plan_path.exists(), "{plan_path:?} 应存在");
        let planning_plan = read_plan(&plan_path).expect("read planning plan 失败");
        assert!(
            planning_plan.frontmatter.session_key.is_none()
                && planning_plan.frontmatter.session_id.is_none(),
            "planning 阶段 create_plan 不应绑定 session_key/session_id"
        );

        // 4) /plan build <plan_id/path> → Executing
        ctx.plan_runtime
            .build_plan(&plan_id, Some(fresh_session.session_id.clone()))
            .expect("build_plan 失败");
        match ctx.plan_runtime.mode() {
            PlanMode::Executing { plan_id: pid } => assert_eq!(pid, plan_id),
            other => panic!("build_plan 后期望 Executing，实际：{other:?}"),
        }

        // 5) 用真 LLM 跑 EXEC_PROMPT，最多 3 轮兜底（每轮跑完读盘判断是否 completed）
        let mut exec_rounds = 0;
        let mut completed = false;
        while exec_rounds < 3 && !completed {
            exec_rounds += 1;
            let prompt = if exec_rounds == 1 {
                EXEC_PROMPT.to_string()
            } else {
                "Continue advancing the plan with update_plan as specified. Do NOT do anything else.".to_string()
            };
            let outcome = run_chat_turn_observed(
                &home,
                &ctx,
                &format!("exec_round_{exec_rounds}"),
                Some(&plan_path),
                &mut diag_state,
                &prompt,
                &system_text,
                &mut context_state,
                EXEC_TURN_TIMEOUT,
            )
            .await;
            if matches!(outcome, AgentRunOutcome::Failed(_)) {
                dump_diagnostic(
                    &home,
                    &ctx,
                    &format!("exec_round_{exec_rounds}_failed"),
                    Some(&plan_path),
                    None,
                    true,
                );
                panic!("exec round {exec_rounds} 失败：{outcome:?}");
            }
            let plan = read_plan(&plan_path).expect("read_plan 失败");
            if plan.frontmatter.mode == PlanFileMode::Completed
                && plan
                    .frontmatter
                    .todos
                    .iter()
                    .all(|t| matches!(t.status, TodoStatus::Completed))
            {
                completed = true;
            }
        }
        if !completed {
            dump_diagnostic(
                &home,
                &ctx,
                "exec_not_completed_after_3_rounds",
                Some(&plan_path),
                None,
                true,
            );
            panic!("EXEC 阶段 3 轮内未 completed");
        }
        let final_plan = read_plan(&plan_path).expect("read final plan 失败");
        assert_eq!(
            final_plan.frontmatter.session_key.as_deref(),
            Some(tomcat::DEFAULT_SESSION_KEY),
            "EXEC/completed 盘应绑定固定 DEFAULT_SESSION_KEY"
        );
        assert_eq!(
            final_plan.frontmatter.session_id.as_deref(),
            Some(fresh_session.session_id.as_str()),
            "EXEC/completed 盘应绑定本次 inprocess run 的真实 session_id"
        );

        // 6) finalize_completed_to_chat → Chat
        let finalized = ctx.plan_runtime.finalize_completed_to_chat();
        assert_eq!(finalized.as_deref(), Some(plan_id.as_str()));
        assert!(matches!(ctx.plan_runtime.mode(), PlanMode::Chat));

        // 7) reviewer transcript 软断言：至少一条 plan.review
        let transcript_path = ctx
            .session
            .current_transcript_path()
            .expect("current_transcript_path 失败")
            .expect("transcript path 缺失");
        let transcript = std::fs::read_to_string(&transcript_path).expect("read transcript 失败");
        let has_plan_review = transcript.lines().any(|l| l.contains("\"plan.review\""));
        assert!(
            has_plan_review,
            "transcript 应含至少一条 plan.review 自定义事件，实际未发现"
        );
    })
    .await;

    if let Err(_e) = result {
        // 兜底诊断输出（timeout 时也写一遍）
        if let Ok(ctx) = ChatContext::from_config(load_user_config()) {
            dump_diagnostic(&home, &ctx, "timeout_600s", None, None, true);
        }
        panic!(
            "inprocess_full_plan_path_with_real_llm 在 {}s 内未完成",
            TOTAL_TIMEOUT.as_secs()
        );
    }
}

/// 强制 SessionManager 在 lib 端被引用，避免 unused-import 警告。
#[allow(dead_code)]
fn _force_use(_: &SessionManager) {}
