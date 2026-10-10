//! E2E-PLAN-RL-002：进程内真 LLM 全路径测试（真 LlmProvider + real reviewer subagents）。
//!
//! 与离线的 [`plan_delivery_flow_e2e.rs`](./plan_delivery_flow_e2e.rs) 和 plan-tool
//! 单元测试互补：它们直接调 `tools::execute`，本测试不做任何 mock，真的让主 LLM
//! 调 `create_plan` / `update_plan`。对 Plan 模式真 LLM 验收来说，这里是 final
//! acceptance todo、真实产物、planner review 和 transcript 顺序的主验收锚点。
//!
//! ## 门禁
//! - `IDATATLAS_OPENAI_API_KEY` 必须存在；缺失 → 测试 panic 失败（E2E_TEST_SPEC §4）。
//! - 默认模型来自 `TOMCAT_E2E_IDATATLAS_MODEL` env，未设则
//!   `idatatlas/gpt-5.6-terra`。
//!
//! ## 数据目录
//! - 每个测试进程先切到独立临时 `HOME`，把 `~/.tomcat/*` 隔离到私有 tempdir。
//! - 仍按产品真实路径解析：plan 落盘到该临时 HOME 下的 `~/.tomcat/plans/`。
//! - 若临时 HOME 里存在 `~/.tomcat/tomcat.config.toml` 会读取；否则走默认配置。
//! - cwd 切到临时 HOME 下的 `~/.tomcat/temp/<run>/`（内置 workspace_roots），避免
//!   `cargo test` 目录触发路径授权。
//!
//! ## 业务断言（硬门禁）
//! 1. `~/.tomcat/plans/<plan_id>.plan.md` 存在且 `frontmatter.state == Completed`
//! 2. 所有 `frontmatter.todos[].status == Completed`
//! 3. workdir 中真实生成 `counter.py`，且 `python3 counter.py` 输出严格为 `0\n`
//! 4. 会话 `AgentMode` 与计划文件生命周期相互独立
//! 5. 一个最终 `kind=acceptance` todo 进入 `in_progress` 后引导模型加载 verify skill，
//!    所有 todo 终态后 `update_plan` 把计划推进到 completed；会话仍是 Chat
//! 6. transcript 至少有一条 `plan.review` 自定义事件
//! 7. transcript 不出现已删除 runtime gate 的 `plan.code_review` / `plan.verify` 事件
//!
//! ## 软断言（不强求）
//! - reviewer summary aborted=false（reviewer LLM 可能格式漂移）
//! - 具体 tool_call 次数（LLM 可能合并）
//!
//! 这个测试整体包在 `tokio::time::timeout(600s)` 内，避免 LLM/网络挂死。

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

use serial_test::serial;
use tokio_util::sync::CancellationToken;

use tomcat::core::llm::system_prompt::{
    AvailableSkillsSection, SystemPromptBuilder, WorkspaceContext, WorkspaceState,
    WorkspaceStateSection,
};
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, PlanFileState, TodoKind, TodoStatus,
};
use tomcat::core::session::{AgentMode, ContextState};
use tomcat::{
    init_context_state, load_config_toml_file, resolve_sessions_dir, run_chat_turn,
    AgentRunOutcome, ChatContext, SessionManager,
};

const COUNTER_PLAN_GOAL: &str = "inprocess e2e: write counter.py that prints 0";

// 真 LLM 进程内全路径会串起 planning + reviewer + 执行轮次；
// 保留宽松的阶段时限以覆盖上游 Responses 网关的瞬时延迟，同时仍有硬超时兜底。
const TOTAL_TIMEOUT: Duration = Duration::from_secs(600);
const PLANNING_TIMEOUT: Duration = Duration::from_secs(240);
const EXEC_TURN_TIMEOUT: Duration = Duration::from_secs(300);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const TRANSIENT_LLM_RETRY_DELAY: Duration = Duration::from_secs(2);
const TRANSIENT_LLM_MAX_ATTEMPTS: usize = 3;

fn require_api_key() {
    let _ = common::require_idatatlas_api_key("plan_real_llm_inprocess_tests");
}

fn current_home() -> PathBuf {
    dirs::home_dir().expect("无法定位 HOME 目录")
}

fn ensure_plans_dir() {
    std::fs::create_dir_all(current_home().join(".tomcat").join("plans"))
        .expect("创建 ~/.tomcat/plans 失败");
}

fn user_config_path() -> PathBuf {
    current_home().join(".tomcat").join("tomcat.config.toml")
}

fn load_user_config() -> tomcat::AppConfig {
    let cfg_path = user_config_path();
    let mut cfg = if cfg_path.exists() {
        load_config_toml_file(&cfg_path).expect("load ~/.tomcat/tomcat.config.toml 失败")
    } else {
        tomcat::load_config(None).expect("load_config 失败")
    };
    cfg.storage.work_dir = Some(
        current_home()
            .join(".tomcat")
            .to_string_lossy()
            .into_owned(),
    );
    common::apply_idatatlas_app_config(&mut cfg);
    cfg
}

fn counter_path(workdir: &Path) -> PathBuf {
    workdir.join("counter.py")
}

fn build_counter_planning_prompt(goal: &str, workdir: &Path) -> String {
    format!(
        concat!(
            "Use the create_plan tool to draft a minimal plan for this exact goal: \"{goal}\". ",
            "The exact writable working directory for this run is `{workdir}`. ",
            "If you mention or inspect a directory, use this exact absolute path and do not substitute alternate roots such as `/home/sandbox/...`. ",
            "Constraints: the deliverable is a single file named `counter.py` in the current writable working directory; ",
            "running `python3 counter.py` must exit 0, write exactly `0\\n` to stdout, and write nothing to stderr; ",
            "todos must be exactly two with ids `t1` and `t2`; `t1` must have `kind=work` and cover creating `counter.py`; ",
            "`t2` must be the final `kind=acceptance` todo and cover reviewing, verifying, and finishing the plan; ",
            "prefer to call create_plan immediately; only if a critical ambiguity truly blocks planning may you call ask_question first; ",
            "the `draft` must be short markdown for the `## Plan` section only and must not include `## Goal`, `## Plan`, or `## Notes` headings; ",
            "do not use any tools during planning besides an optional ask_question followed by create_plan; ",
            "after create_plan returns, do NOT call update_plan, edit, or any other tool; ",
            "if reviewer feedback appears in the create_plan result, stop after a short acknowledgement."
        ),
        goal = goal,
        workdir = workdir.display()
    )
}

fn build_counter_exec_prompt(todo_ids: &[String], workdir: &Path) -> String {
    let counter = counter_path(workdir);
    let mut lines = vec![
        format!("Advance the active plan to completion for this exact goal: `{COUNTER_PLAN_GOAL}`."),
        format!("Write a single file at `{}`.", counter.display()),
        format!(
            "The exact working directory for this run is `{}`. Use this path literally and do not substitute `/home/sandbox/...` or other aliases.",
            workdir.display()
        ),
        "Requirements:".to_string(),
        "- The file must be valid Python.".to_string(),
        "- Running `python3 counter.py` from the current working directory must exit 0, print exactly `0\\n` to stdout, and print nothing to stderr.".to_string(),
        "- Use `bash` to run and verify the program yourself before closing the plan.".to_string(),
        "- Use update_plan to claim progress, perform the work, and finish all current todos.".to_string(),
        "- After completing `t1`, start the final `t2` acceptance todo with update_plan. Its result must return `next_step.phase=run_verify`; load_skill(verify), follow the returned guidance, then complete `t2`.".to_string(),
        "The current todo ids are:".to_string(),
    ];
    for id in todo_ids {
        lines.push(format!("- {id}"));
    }
    lines.extend([
        "Rules:".to_string(),
        "- You may use list_dir, read, search_files, write, edit, bash, load_skill, and update_plan.".to_string(),
        "- Use the exact current todo ids from the latest plan/tool results.".to_string(),
        "- Do NOT rewrite, replace, or upsert todo ids/content. Keep exactly the two planned todos.".to_string(),
        "- Prefer `set_status` on the existing todos and include concrete evidence.".to_string(),
        "- When the final update_plan returns, inspect the tool result.".to_string(),
        "- Do not stop while `next_step.phase=continue_work` or `run_verify`; only stop after the final update_plan moves the plan to `completed`.".to_string(),
        "- Do NOT edit the plan file directly. Do NOT call ask_question.".to_string(),
    ]);
    lines.join(" ")
}

fn assert_counter_artifact(workdir: &Path) {
    let counter = counter_path(workdir);
    assert!(counter.exists(), "应生成产物文件: {}", counter.display());
    let run = StdCommand::new("python3")
        .current_dir(workdir)
        .arg("counter.py")
        .output()
        .unwrap_or_else(|e| panic!("运行 counter.py 失败: {e}"));
    assert!(
        run.status.success(),
        "counter.py 退出码应为 0，实际：{:?}",
        run.status
    );
    assert_eq!(
        run.stdout,
        b"0\n",
        "counter.py stdout 应恰好为 `0\\n`，实际：{:?}",
        String::from_utf8_lossy(&run.stdout)
    );
    assert!(
        run.stderr.is_empty(),
        "counter.py stderr 应为空，实际：{:?}",
        String::from_utf8_lossy(&run.stderr)
    );
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
        agent_workspace_dir: ctx
            .scope_services
            .agent_workspace_dir
            .to_string_lossy()
            .to_string(),
        agent_definition_dir: ctx
            .scope_services
            .agent_definition_dir
            .to_string_lossy()
            .to_string(),
        agent_plans_dir: tomcat::core::plan_runtime::file_store::plans_dir()
            .map(|path| format_home_path(&path))
            .unwrap_or_else(|_| "~/.tomcat/plans".to_string()),
        agent_trail_dir: ctx
            .scope_services
            .agent_trail_dir
            .to_string_lossy()
            .to_string(),
        tool_lines: None,
    };
    let skill_set = tomcat::core::skill::discover(&ctx.config, &ctx.scope_services.resource_root);
    assert!(
        skill_set.by_name.contains_key("verify"),
        "built-in verify skill must be discoverable in the real plan test"
    );
    *ctx.scope_services.skill_set.write() = skill_set.clone();

    let mut builder = SystemPromptBuilder::default();
    builder.register(Box::new(WorkspaceStateSection::new(WorkspaceState {
        read_write: Vec::new(),
        read_only: Vec::new(),
        path_rules: Vec::new(),
    })));
    let budget = tomcat::infra::compute_context_budget_chars(&ctx.config.context);
    if let Some(section) =
        AvailableSkillsSection::from_skill_set(&skill_set, budget, &ctx.config.skills)
    {
        builder.register(Box::new(section));
    }
    builder.build(&workspace_context)
}

fn ensure_session(ctx: &ChatContext) {
    let key = ctx.session_runtime.session.current_session_key();
    if ctx
        .session_runtime
        .session
        .get_session(key)
        .unwrap()
        .is_none()
    {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        ctx.session_runtime
            .session
            .create_session(key, cwd)
            .unwrap();
    }
}

/// 扫盘挑出 state=planning 的最新 plan 文件，返回 plan_id。
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
        if plan.frontmatter.state != PlanFileState::Planning {
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
    if let Ok(Some(t)) = ctx.session_runtime.session.current_transcript_path() {
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
        format_args!(
            "plan_runtime.mode = {:?}\n",
            ctx.session_runtime.plan_runtime.mode()
        ),
    );

    if full_snapshot {
        out.push_str(&format_plan_snapshot(plan_path));
        if let Ok(Some(t)) = ctx.session_runtime.session.current_transcript_path() {
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
        format_args!(
            "plan_runtime.mode = {:?}\n",
            ctx.session_runtime.plan_runtime.mode()
        ),
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

fn is_transient_connect_failure(outcome: &AgentRunOutcome) -> bool {
    match outcome {
        AgentRunOutcome::Failed(tomcat::AppError::LlmDetailed(detail)) => {
            detail.stage() == Some(tomcat::LlmErrorStage::Connect)
                || detail
                    .source_chain()
                    .iter()
                    .any(|entry| entry.contains("connection closed via error"))
        }
        AgentRunOutcome::Failed(err) => {
            let text = format!("{err:?}");
            text.contains("connection closed via error") || text.contains("stage: Some(Connect)")
        }
        AgentRunOutcome::Completed(_) | AgentRunOutcome::Interrupted(_) => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_chat_turn_with_transient_retry(
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
    for attempt in 1..=TRANSIENT_LLM_MAX_ATTEMPTS {
        let attempt_label = if attempt == 1 {
            label.to_string()
        } else {
            format!("{label}_retry_{attempt}")
        };
        let outcome = run_chat_turn_observed(
            home,
            ctx,
            &attempt_label,
            plan_path,
            diag_state,
            prompt,
            system_text,
            context_state,
            timeout,
        )
        .await;
        if attempt == TRANSIENT_LLM_MAX_ATTEMPTS || !is_transient_connect_failure(&outcome) {
            return outcome;
        }
        eprintln!(
            "[real-llm retry] {label} transient connect failure; retrying attempt {}/{}",
            attempt + 1,
            TRANSIENT_LLM_MAX_ATTEMPTS
        );
        tokio::time::sleep(TRANSIENT_LLM_RETRY_DELAY).await;
    }
    unreachable!("retry loop should always return");
}

// Only the human decision is fixed; main/reviewer LLMs and production dispatch remain real.
struct ReviewConsentPanel;
#[async_trait::async_trait]
impl tomcat::core::plan_runtime::panels::AskQuestionPanel for ReviewConsentPanel {
    async fn ask(
        &self,
        questions: Vec<tomcat::core::plan_runtime::panels::Question>,
        termination: tomcat::core::plan_runtime::AskQuestionTermination,
    ) -> tomcat::core::plan_runtime::panels::AskQuestionResult {
        use tomcat::core::plan_runtime::panels::{Answer, AskQuestionResult};
        if let Some(result) = termination.result() {
            return result;
        }
        AskQuestionResult::answered(
            questions
                .into_iter()
                .map(|question| {
                    let option = question
                        .options
                        .iter()
                        .find(|option| option.id == "review")
                        .or_else(|| question.options.iter().find(|option| option.recommended))
                        .expect("choice");
                    Answer {
                        question_id: question.id,
                        option_ids: vec![option.id.clone()],
                        custom_text: None,
                        skipped: false,
                        picked_recommended: option.recommended,
                    }
                })
                .collect(),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial]
async fn inprocess_full_plan_path_with_real_llm() {
    require_api_key();
    common::setup_logging();
    let _home_guard = common::TempHomeGuard::new();
    ensure_plans_dir();
    let home = current_home();
    let config = load_user_config();
    let workdir = common::dot_tomcat_e2e_workdir("inprocess_real_llm");
    let _cwd = common::CwdGuard::set(&workdir);
    let sessions_dir = resolve_sessions_dir(&config).expect("resolve sessions dir");
    let fresh_session = common::begin_fresh_default_session(&sessions_dir, Some(&workdir));

    let result = tokio::time::timeout(TOTAL_TIMEOUT, async {
        let ctx = ChatContext::from_config_with_overrides(config,
            tomcat::api::chat::ChatContextOverrides::default().with_ask_question_panel(std::sync::Arc::new(ReviewConsentPanel)))
            .expect("ChatContext::from_config 失败");
        ensure_session(&ctx);
        let mut diag_state = InprocessDiagState::default();

        let system_text = build_system_text_minimal(&ctx);
        let mut context_state = init_context_state(
            &ctx.session_runtime.session,
            &ctx.config.context,
            &system_text,
        )
        .expect("init_context_state 失败");

        // 1) /plan → Plan
        ctx.session_runtime
            .plan_runtime
            .enter_plan()
            .expect("enter_plan 失败");
        assert_eq!(ctx.session_runtime.plan_runtime.mode(), AgentMode::Plan);

        // 2) 用真 LLM 跑 PLANNING_PROMPT；期望它调 create_plan
        let planning_prompt = build_counter_planning_prompt(COUNTER_PLAN_GOAL, &workdir);
        let outcome = run_chat_turn_with_transient_retry(
            &home,
            &ctx,
            "planning_phase",
            None,
            &mut diag_state,
            &planning_prompt,
            &system_text,
            &mut context_state,
            PLANNING_TIMEOUT,
        )
        .await;
        if is_transient_connect_failure(&outcome) {
            dump_diagnostic(
                &home,
                &ctx,
                "planning_phase_transient_connect_exhausted",
                None,
                None,
                true,
            );
            eprintln!(
                "skipping inprocess_full_plan_path_with_real_llm: idatatlas connect failures persisted after {} attempts during planning",
                TRANSIENT_LLM_MAX_ATTEMPTS
            );
            return;
        }
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
                panic!("create_plan 未生成任何 state=planning 的盘文件");
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

        // 4) /plan build <plan_id/path> → Chat + executing plan file
        ctx.session_runtime
            .plan_runtime
            .build_plan(&plan_id, Some(fresh_session.session_id.clone()))
            .expect("build_plan 失败");
        assert_eq!(ctx.session_runtime.plan_runtime.mode(), AgentMode::Chat);
        assert_eq!(
            ctx.session_runtime
                .plan_runtime
                .executing_plan_id()
                .as_deref(),
            Some(plan_id.as_str())
        );

        // 5) 用真 LLM 跑 EXEC_PROMPT，最多 3 轮兜底（每轮跑完读盘判断是否 completed）
        let mut exec_rounds = 0;
        let mut completed = false;
        while exec_rounds < 3 && !completed {
            exec_rounds += 1;
            let current_plan = read_plan(&plan_path).expect("read current exec plan 失败");
            let todo_ids: Vec<String> = current_plan
                .frontmatter
                .todos
                .iter()
                .map(|todo| todo.id.clone())
                .collect();
            let prompt = build_counter_exec_prompt(&todo_ids, &workdir);
            let outcome = run_chat_turn_with_transient_retry(
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
            if is_transient_connect_failure(&outcome) {
                dump_diagnostic(
                    &home,
                    &ctx,
                    &format!("exec_round_{exec_rounds}_transient_connect_exhausted"),
                    Some(&plan_path),
                    None,
                    true,
                );
                eprintln!(
                "skipping inprocess_full_plan_path_with_real_llm: idatatlas connect failures persisted after {} attempts during exec round {}",
                    TRANSIENT_LLM_MAX_ATTEMPTS,
                    exec_rounds
                );
                return;
            }
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
            if plan.frontmatter.state == PlanFileState::Completed
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
            final_plan.frontmatter.todos.len(),
            2,
            "真实模型必须保留精简计划的两个原始 todo"
        );
        let work_todo = final_plan
            .frontmatter
            .todos
            .iter()
            .find(|todo| todo.id == "t1")
            .expect("work todo t1 must exist");
        assert_eq!(work_todo.kind, TodoKind::Work);
        assert_eq!(work_todo.status, TodoStatus::Completed);
        let acceptance_todo = final_plan
            .frontmatter
            .todos
            .iter()
            .find(|todo| todo.id == "t2")
            .expect("final acceptance todo t2 must exist");
        assert_eq!(acceptance_todo.kind, TodoKind::Acceptance);
        assert_eq!(acceptance_todo.status, TodoStatus::Completed);
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
        assert_counter_artifact(&workdir);

        // 6) 完成计划只改变文件生命周期；会话模式在 build 时已回到 Chat。
        assert_eq!(ctx.session_runtime.plan_runtime.mode(), AgentMode::Chat);

        // 7) planner review 独立于运行时 close-out；最终验收由 acceptance todo +
        // verify skill 驱动，不会派发已删除的 code-review / verifier gate。
        let transcript_path = ctx
            .session_runtime
            .session
            .current_transcript_path()
            .expect("current_transcript_path 失败")
            .expect("transcript path 缺失");
        let transcript = std::fs::read_to_string(&transcript_path).expect("read transcript 失败");
        let lines: Vec<&str> = transcript.lines().collect();
        let plan_review_idx = lines.iter().position(|l| l.contains("\"plan.review\""));
        let plan_code_review_idx = lines.iter().position(|l| {
            l.contains("\"plan.code_review\"") && !l.contains("\"plan.code_review.warning\"")
        });
        let plan_verify_idx = lines.iter().position(|l| l.contains("\"plan.verify\""));
        assert!(
            plan_review_idx.is_some(),
            "transcript 应含至少一条 plan.review 自定义事件，实际未发现"
        );
        for line in lines.iter().filter(|line| line.contains("\"plan.review\"")) {
            assert!(!line.contains("\"user_skipped\"") && !line.contains("\"parent_abort\"") && !line.contains("\"not_dispatched\""),
                "the real reviewer must be dispatched, not silently bypassed: {line}");
        }
        assert!(
            plan_code_review_idx.is_none(),
            "transcript 不应再含 plan.code_review runtime gate 事件，实际：{plan_code_review_idx:?}"
        );
        assert!(
            plan_verify_idx.is_none(),
            "transcript 不应再含 plan.verify 自定义事件，实际仍发现旧 verifier 链路"
        );
        assert!(
            transcript.contains(r#""name":"load_skill""#),
            "最终 acceptance 必须调用 load_skill，transcript 中未发现该 tool call"
        );
    })
    .await;

    if let Err(_e) = result {
        // 兜底诊断输出（timeout 时也写一遍）
        if let Ok(ctx) = ChatContext::from_config(load_user_config()) {
            let timeout_label = format!("timeout_{}s", TOTAL_TIMEOUT.as_secs());
            dump_diagnostic(&home, &ctx, &timeout_label, None, None, true);
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
