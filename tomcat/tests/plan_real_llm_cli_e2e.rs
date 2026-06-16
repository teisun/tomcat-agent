//! E2E-PLAN-RL-001：CLI 子进程黑盒真 LLM smoke。
//!
//! 通过 `assert_cmd::Command::cargo_bin("tomcat")` 真起 `tomcat code`，让真 LLM
//! 在真实 CLI 上分别覆盖 planning-only 与 exec-only 两条窄路径：
//!
//! - 进程 A：`tomcat code` + `/plan` + planning prompt；EOF 退出后落盘 state=planning。
//! - 进程 B：预置 planning plan 后，`tomcat code --resume` + `/plan build {plan_id}` + exec
//!   prompt；EOF 只负责结束输入并退出。这个真 LLM smoke 继续覆盖 `--resume` +
//!   `plan_id` 入口、EXEC prompt 可见性与 session 绑定；full completion / artifact /
//!   transcript 顺序由 inprocess/runtime 测试单独回归。
//! - 需要手动观察整条 PLAN→EXEC→Completed 全链路时，保留 ignored 的
//!   `cli_plan_path_with_real_llm_custom_goal` 入口。
//!
//! 之所以仍保留两个子进程，是因为 stdin 是一次性写完的（rustyline pipe 行为），且
//! `--resume` 路径必须真读取 session/transcript 与盘上的 active plan。
//!
//! ## 门禁
//! - `DEEPSEEK_API_KEY` 必须存在；缺失 → panic（E2E-PLAN-RL-001 / E2E_TEST_SPEC §4）。
//! - 默认模型来自 `TOMCAT_E2E_DEEPSEEK_MODEL`，未设 → `deepseek-v4-pro`。
//!
//! ## 数据目录
//! - 子进程**继承真实 HOME**（不注入临时 `HOME`）；plan 落盘到 `~/.tomcat/plans/`。
//! - 默认 cwd 用 `~/.tomcat/temp/<run>/`（内置 workspace_roots）；自定义 cwd 必须显式位于可写根内。
//! - 诊断日志写到仓库内 `workspace-temp/logs/`，运行一开始就打印可点击路径。
//! - 每次 run 都会按当前 `workdir` 生成新的 code-scope `session_id`；
//!   因此 `recover()` 必须按 `session_id` 而不是仅按固定 `DEFAULT_SESSION_KEY`
//!   识别 executing plan，否则旧 run 的盘状态会 hijack 新用例。

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serial_test::serial;
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState,
    TodoItem, TodoStatus,
};
use tomcat::{
    load_config_toml_file, normalize_path, resolve_sessions_dir, resolve_workspace_roots_paths,
};

const COUNTER_PLAN_GOAL: &str = "cli e2e: write counter.py that prints 0";
const CUSTOM_PLAN_GOAL_ENV: &str = "TOMCAT_E2E_PLAN_GOAL";
const CUSTOM_WORKDIR_ENV: &str = "TOMCAT_E2E_WORKDIR";
const ASK_QUESTION_AUTO_PICK_ENV: &str = "TOMCAT_ASK_QUESTION_TEST_AUTO_PICK";

// 真 LLM 端到端路径受上游时延波动影响较大；门限保留硬超时，但给 gpt-5.4
// 的 planning/review/verify 多轮链路留出更现实的缓冲。
const PLANNING_TIMEOUT: Duration = Duration::from_secs(240);
const EXEC_TIMEOUT: Duration = Duration::from_secs(420);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

type ExecPromptOverride = fn(&[String], &Path) -> String;
type PlanningPromptOverride = fn(&str, &Path) -> String;
#[derive(Default)]
struct CliRunDiagState {
    transcript_offset: usize,
    last_plan_snapshot: Option<String>,
    last_workdir_snapshot: Option<String>,
}

#[derive(Default)]
struct Utf8StreamCursor {
    offset: usize,
    carry: Vec<u8>,
}

#[derive(Default)]
struct CliProcessDiagState {
    stdout: Utf8StreamCursor,
    stderr: Utf8StreamCursor,
}

struct DiagLog {
    path: PathBuf,
    file: Arc<Mutex<std::fs::File>>,
}

impl DiagLog {
    fn new(slug: &str) -> Self {
        let filename = format!(
            "plan_real_llm_cli_e2e_{}_{}.log",
            common::filename_timestamp(),
            common::slugify_filename(slug, "run", 48)
        );
        let path = common::repo_workspace_temp_logs_dir().join(filename);
        let file = std::fs::File::create(&path).expect("create cli e2e diag log");
        Self {
            path,
            file: Arc::new(Mutex::new(file)),
        }
    }

    fn emit(&self, text: &str) {
        eprint!("{text}");
        let mut file = self.file.lock().expect("lock diag log file");
        file.write_all(text.as_bytes())
            .expect("write cli e2e diag log");
        file.flush().expect("flush cli e2e diag log");
    }
}

struct CliFixture {
    home: PathBuf,
    workdir: PathBuf,
    run_session_id: String,
    transcript_path: PathBuf,
    config_path: Option<PathBuf>,
    api_key: String,
    model: String,
    current_plan_path: Arc<Mutex<Option<PathBuf>>>,
    diag_state: Arc<Mutex<CliRunDiagState>>,
    diag_log: DiagLog,
}

fn require_api_key() -> String {
    common::require_deepseek_api_key("plan_real_llm_cli_e2e")
}

fn default_model() -> String {
    common::deepseek_test_model()
}

fn load_user_config(config_path: Option<&Path>) -> tomcat::AppConfig {
    let mut cfg = if let Some(path) = config_path {
        load_config_toml_file(path).expect("load ~/.tomcat/tomcat.config.toml 失败")
    } else {
        tomcat::load_config(None).expect("load_config 失败")
    };
    common::apply_deepseek_app_config(&mut cfg);
    cfg
}

fn default_planning_prompt(goal: &str, workdir: &Path) -> String {
    format!(
        concat!(
            "Use the create_plan tool to draft an executable plan for this exact goal: \"{goal}\". ",
            "The exact writable working directory for this run is `{workdir}`. ",
            "If you mention or inspect a directory, use this exact absolute path and do not substitute alternate roots such as `/home/sandbox/...`. ",
            "Prefer to call create_plan immediately; only if a critical ambiguity truly blocks planning may you call ask_question first. ",
            "Make the todos actionable in the current writable working directory. ",
            "The `draft` must be short markdown for the `## Plan` section only and must not include `## Goal`, `## Plan`, or `## Notes` headings. ",
            "Do not use any tools during planning besides an optional ask_question followed by create_plan. ",
            "After create_plan returns, do NOT call update_plan, edit, or any other tool. ",
            "After create_plan returns, reply once with a short acknowledgement and stop."
        ),
        goal = goal,
        workdir = workdir.display()
    )
}

fn build_default_exec_prompt(goal: &str, todo_ids: &[String], workdir: &Path) -> String {
    let mut lines = vec![
        format!("Advance the active plan to completion for this goal: `{goal}`."),
        format!(
            "Use the current writable working directory `{}` as the real execution directory.",
            workdir.display()
        ),
        format!(
            "This exact directory is the only intended workdir for this run: `{}`. Do not substitute aliases such as `/home/sandbox/...`.",
            workdir.display()
        ),
        "Actually perform the work, verify the result yourself when appropriate, and use update_plan to reflect progress.".to_string(),
        "- You may use list_dir, read, search_files, write, edit, bash, and update_plan.".to_string(),
        "- Do NOT call ask_question. Do NOT edit the plan file directly.".to_string(),
        "- Do NOT write outside the current working directory.".to_string(),
        "- When the last todo becomes completed, inspect the `update_plan` tool result.".to_string(),
        "- If `code_review.verdict != pass` or `plan_state_after` stays `executing`, do NOT stop: read the findings, reopen an existing todo or add a fix todo with `update_plan`, fix the work, and complete the plan again.".to_string(),
        "- Only stop once the tool result includes verifier output or the plan reaches `completed`.".to_string(),
        "Todo ids to finish:".to_string(),
    ];
    for id in todo_ids {
        lines.push(format!("- {id}"));
    }
    lines.push(
        "After the work is complete and verified, use update_plan to move every todo to completed. Reply \"done\" and stop."
            .to_string(),
    );
    lines.join(" ")
}

fn build_counter_exec_prompt(todo_ids: &[String], workdir: &Path) -> String {
    let counter = workdir.join("counter.py");
    let mut lines = vec![
        "Advance the active plan to completion by actually implementing the artifact.".to_string(),
        format!("Write a single file at `{}`.", counter.display()),
        format!(
            "The exact working directory for this run is `{}`. Use this path literally and do not substitute `/home/sandbox/...` or other aliases.",
            workdir.display()
        ),
        "Requirements:".to_string(),
        "- The file must be valid Python.".to_string(),
        "- Running `python3 counter.py` from the current working directory must exit 0, print exactly `0\\n` to stdout, and print nothing to stderr.".to_string(),
        "- Use `bash` to run and verify the program yourself before closing the plan.".to_string(),
        "- You may use `list_dir`, `read`, `search_files`, `write`, `edit`, `bash`, and `update_plan`.".to_string(),
        "- Do NOT call ask_question. Do NOT edit the plan file directly.".to_string(),
        "- Do NOT write outside the current working directory.".to_string(),
        "- When the final `update_plan` returns, inspect `code_review` / `verify` in the tool result.".to_string(),
        "- If `code_review.verdict != pass` or `plan_state_after` stays `executing`, do NOT stop: reopen or add a fix todo with `update_plan`, repair the file, and drive the plan to completion again.".to_string(),
        "- Only stop once the tool result includes verifier output or the plan reaches `completed`.".to_string(),
        "Todo ids to finish:".to_string(),
    ];
    for id in todo_ids {
        lines.push(format!("- {id}"));
    }
    lines.push(
        "After the artifact is verified, use update_plan to move every todo to completed. You may combine status updates. Reply \"done\" and stop."
            .to_string(),
    );
    lines.join(" ")
}

fn build_counter_planning_prompt(goal: &str, workdir: &Path) -> String {
    format!(
        concat!(
            "Use the create_plan tool to draft a minimal plan for this exact goal: \"{goal}\". ",
            "The exact writable working directory for this run is `{workdir}`. ",
            "If you mention or inspect a directory, use this exact absolute path and do not substitute alternate roots such as `/home/sandbox/...`. ",
            "Constraints: the deliverable is a single file named `counter.py` in the current writable working directory; ",
            "running `python3 counter.py` must exit 0, write exactly `0\\n` to stdout, and write nothing to stderr; ",
            "todos must be exactly two with ids `t1` and `t2`; `t1` must cover creating `counter.py`; ",
            "`t2` must cover running/verifying it and finishing the plan; ",
            "prefer to call create_plan immediately; only if a critical ambiguity truly blocks planning may you call ask_question first, ",
            "and if you do each question must have exactly one recommended option; ",
            "the `draft` must be short markdown for the `## Plan` section only and must not include `## Goal`, `## Plan`, or `## Notes` headings; ",
            "do not use any tools during planning besides an optional ask_question followed by create_plan; ",
            "after create_plan returns, do NOT call update_plan, edit, or any other tool; ",
            "if reviewer feedback appears in the create_plan result, ignore it for this test and stop; ",
            "after create_plan returns, reply once with a short acknowledgement and stop."
        ),
        goal = goal,
        workdir = workdir.display()
    )
}

fn normalize_custom_workdir(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy().to_string();
    let mut normalized = normalize_path(&raw).expect("normalize custom workdir");
    if normalized.is_relative() {
        normalized = std::env::current_dir()
            .expect("current_dir for custom workdir")
            .join(normalized);
    }
    normalized
}

fn resolve_case_workdir(
    cfg: &tomcat::AppConfig,
    workdir_override: Option<&Path>,
    slug: &str,
) -> PathBuf {
    let Some(raw_workdir) = workdir_override else {
        return common::dot_tomcat_e2e_workdir(&format!("cli_real_llm_{slug}"));
    };

    let normalized = normalize_custom_workdir(raw_workdir);
    std::fs::create_dir_all(&normalized).expect("create custom workdir for cli e2e");
    let canon = normalized
        .canonicalize()
        .unwrap_or_else(|_| normalized.clone());
    let writable_roots =
        resolve_workspace_roots_paths(cfg).expect("resolve workspace roots for custom workdir");
    if writable_roots.iter().any(|root| canon.starts_with(root)) {
        return canon;
    }

    let roots = writable_roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    panic!(
        "自定义 workdir `{}` 不在可写根内；请改用这些可写根中的路径，或在配置中加入 workspace.workspace_roots。当前可写根：{}",
        canon.display(),
        roots
    );
}

fn setup_fixture(
    log_slug: &str,
    workdir_override: Option<&Path>,
    max_code_review_rounds: u32,
) -> CliFixture {
    let api_key = require_api_key();
    let model = default_model();
    let home = dirs::home_dir().expect("无法定位 HOME 目录");
    std::fs::create_dir_all(home.join(".tomcat").join("plans")).unwrap();
    let user_config_path = {
        let p = home.join(".tomcat").join("tomcat.config.toml");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    };
    let mut cfg = load_user_config(user_config_path.as_deref());
    cfg.plan.max_code_review_rounds = max_code_review_rounds;
    let generated_config_dir = common::repo_workspace_temp_dir().join("generated-configs");
    std::fs::create_dir_all(&generated_config_dir).expect("create generated-configs for cli e2e");
    let effective_config_path = generated_config_dir.join(format!(
        "plan_real_llm_cli_e2e_{}_{}.toml",
        common::filename_timestamp(),
        common::slugify_filename(log_slug, "run", 48)
    ));
    let effective_toml =
        toml::to_string_pretty(&cfg).expect("serialize cli real llm effective config");
    std::fs::write(&effective_config_path, effective_toml)
        .expect("write cli real llm effective config");
    let workdir = resolve_case_workdir(&cfg, workdir_override, log_slug);
    let sessions_dir = resolve_sessions_dir(&cfg).expect("resolve sessions dir");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir for cli real llm e2e");
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &workdir);
    let session_mgr = tomcat::SessionManager::new_scoped(sessions_dir.clone(), session_key);
    let session = session_mgr
        .new_current_session(Some(workdir.to_string_lossy().to_string()))
        .expect("create fresh code session for cli real llm e2e");
    let transcript_path = sessions_dir.join(format!("{}.jsonl", session.session_id));
    let diag_log = DiagLog::new(log_slug);

    let fx = CliFixture {
        home,
        workdir,
        run_session_id: session.session_id,
        transcript_path,
        config_path: Some(effective_config_path),
        api_key,
        model,
        current_plan_path: Arc::new(Mutex::new(None)),
        diag_state: Arc::new(Mutex::new(CliRunDiagState::default())),
        diag_log,
    };
    fx.diag_log.emit(&format!(
        "CLI E2E 日志文件: {}\nworkdir: {}\nsession transcript: {}\n\n",
        fx.diag_log.path.display(),
        fx.workdir.display(),
        fx.transcript_path.display()
    ));
    fx
}

fn current_plan_path(fx: &CliFixture) -> Option<PathBuf> {
    fx.current_plan_path
        .lock()
        .expect("lock current plan path")
        .clone()
}

fn set_current_plan_path(fx: &CliFixture, plan_path: PathBuf) {
    *fx.current_plan_path.lock().expect("lock current plan path") = Some(plan_path);
}

fn pick_newest_planning_plan_path(home: &Path) -> Option<PathBuf> {
    let plans_dir = home.join(".tomcat").join("plans");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
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
            _ => best = Some((mtime, path)),
        }
    }
    best.map(|(_, p)| p)
}

fn created_plan_from_current_session(fx: &CliFixture) -> Option<common::CreatedPlanRef> {
    common::extract_created_plan_from_transcript_path(&fx.transcript_path)
}

fn run_tomcat_chat(
    fx: &CliFixture,
    phase: &str,
    args: &[&str],
    stdin_text: &str,
    timeout: Option<Duration>,
) -> Output {
    let mut cmd = StdCommand::new(assert_cmd::cargo::cargo_bin!("tomcat"));
    cmd.current_dir(&fx.workdir)
        .arg("code")
        .args(args)
        .env("SHELL", "/bin/zsh")
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, &fx.api_key)
        .env("TOMCAT_ASK_QUESTION_TIMEOUT_MS", "5000")
        .env("TOMCAT__LLM__PROVIDER", "openai")
        .env("TOMCAT__LLM__API_BASE", common::DEEPSEEK_TEST_API_BASE)
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env(ASK_QUESTION_AUTO_PICK_ENV, "recommended")
        .env("TOMCAT__LLM__DEFAULT_MODEL", &fx.model)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", &fx.model)
        .env("RUST_LOG", "tomcat=info");
    if let Some(cfg) = &fx.config_path {
        cmd.env("TOMCAT__CONFIG_PATH", cfg);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tomcat chat");
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(stdin_text.as_bytes()).unwrap();
    drop(stdin); // EOF
    wait_for_child_output(child, fx, phase, timeout)
}

struct PlanningPhaseResult {
    output: Output,
    created_plan: common::CreatedPlanRef,
}

fn resolve_created_plan_after_planning(fx: &CliFixture, out_a: &Output) -> common::CreatedPlanRef {
    created_plan_from_current_session(fx).unwrap_or_else(|| {
        let path = pick_newest_planning_plan_path(&fx.home).unwrap_or_else(|| {
            dump_diag("no_planning_plan", fx, Some(out_a), None);
            panic!("进程 A 后未找到 state=planning 的 plan 文件");
        });
        common::CreatedPlanRef {
            plan_id: read_plan(&path)
                .expect("read_plan planning fallback")
                .frontmatter
                .plan_id
                .clone(),
            path,
        }
    })
}

fn run_planning_phase(
    fx: &CliFixture,
    goal: &str,
    planning_prompt_override: Option<PlanningPromptOverride>,
    planning_timeout: Option<Duration>,
) -> PlanningPhaseResult {
    let planning_prompt = if let Some(builder) = planning_prompt_override {
        builder(goal, &fx.workdir)
    } else {
        default_planning_prompt(goal, &fx.workdir)
    };

    let stdin_a = format!("/plan\n{prompt}\n", prompt = planning_prompt);
    let out_a = run_tomcat_chat(fx, "planning_proc", &[], &stdin_a, planning_timeout);
    if !out_a.status.success() {
        dump_diag("proc_a_failed", fx, Some(&out_a), None);
        panic!("进程 A 退出码非 0: {:?}", out_a.status);
    }

    let created_plan = resolve_created_plan_after_planning(fx, &out_a);
    set_current_plan_path(fx, created_plan.path.clone());
    emit_plan_resolved_block("planning_plan_resolved", fx);
    if !created_plan.path.exists() {
        dump_diag("no_planning_plan", fx, Some(&out_a), None);
        panic!("进程 A 后未找到 create_plan 生成的盘文件");
    }

    PlanningPhaseResult {
        output: out_a,
        created_plan,
    }
}

fn seeded_exec_plan_id(log_slug: &str) -> String {
    format!(
        "plan_cli_exec_seed_{}",
        common::slugify_filename(
            &format!("{}-{}", common::filename_timestamp(), log_slug),
            "seed",
            48
        )
    )
}

fn seed_counter_planning_plan(fx: &CliFixture, goal: &str) -> common::CreatedPlanRef {
    let plan_id = seeded_exec_plan_id(goal);
    let path = plan_path_for_id(&plan_id).expect("plan_path_for_id 失败");
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.clone(),
            goal: goal.to_string(),
            state: PlanFileState::Planning,
            session_key: None,
            session_id: None,
            created_at: chrono::Local::now().to_rfc3339(),
            schema_version: 1,
            todos: vec![
                TodoItem {
                    id: "t1".into(),
                    content: format!(
                        "Create `counter.py` in `{}` so it prints exactly `0` with a trailing newline.",
                        fx.workdir.display()
                    ),
                    status: TodoStatus::Pending,
                },
                TodoItem {
                    id: "t2".into(),
                    content: format!(
                        "Run `python3 counter.py` in `{}`, verify exit code 0 with stdout exactly `0\\n` and empty stderr, then finish the plan.",
                        fx.workdir.display()
                    ),
                    status: TodoStatus::Pending,
                },
            ],
            unknown: Default::default(),
        },
        body: format!(
            "## Goal\n\n{goal}\n\n## Plan\n\n- Create `counter.py` in `{}`.\n- Verify it prints exactly `0\\n`.\n",
            fx.workdir.display()
        ),
    };
    write_plan(&path, &plan, 2_000).expect("write exec seed plan 失败");
    set_current_plan_path(fx, path.clone());
    emit_plan_resolved_block("exec_seed_plan_resolved", fx);
    common::CreatedPlanRef { plan_id, path }
}

fn run_exec_phase(
    fx: &CliFixture,
    goal: &str,
    plan_id: &str,
    exec_prompt_override: Option<ExecPromptOverride>,
    exec_timeout: Option<Duration>,
) -> Output {
    let plan_path = current_plan_path(fx).expect("exec 阶段应已有 active plan path");
    let current_plan = read_plan(&plan_path).expect("read current exec plan 失败");
    let todo_ids: Vec<String> = current_plan
        .frontmatter
        .todos
        .iter()
        .map(|todo| todo.id.clone())
        .collect();
    let exec_prompt = if let Some(builder) = exec_prompt_override {
        builder(&todo_ids, &fx.workdir)
    } else {
        build_default_exec_prompt(goal, &todo_ids, &fx.workdir)
    };
    let stdin_b = format!(
        "/plan build {plan_id}\n{prompt}\n",
        plan_id = plan_id,
        prompt = exec_prompt
    );
    run_tomcat_chat(fx, "exec_proc", &["--resume"], &stdin_b, exec_timeout)
}

fn counter_path(fx: &CliFixture) -> PathBuf {
    fx.workdir.join("counter.py")
}

fn spawn_reader_thread<R: std::io::Read + Send + 'static>(
    mut reader: R,
    buf: Arc<Mutex<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.lock().unwrap().extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
    })
}

fn tail_chars(bytes: &[u8], max_chars: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
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

fn text_delta_from_bytes(bytes: &[u8], cursor: &mut Utf8StreamCursor) -> Option<String> {
    let start = if cursor.offset <= bytes.len() {
        cursor.offset
    } else {
        0
    };
    let mut chunk = Vec::new();
    if !cursor.carry.is_empty() {
        chunk.extend_from_slice(&cursor.carry);
    }
    chunk.extend_from_slice(&bytes[start..]);
    cursor.offset = bytes.len();
    if chunk.is_empty() {
        None
    } else {
        match std::str::from_utf8(&chunk) {
            Ok(text) => {
                cursor.carry.clear();
                Some(text.to_string())
            }
            Err(err) => {
                if err.error_len().is_some() {
                    cursor.carry.clear();
                    Some(String::from_utf8_lossy(&chunk).to_string())
                } else {
                    let valid = err.valid_up_to();
                    let text = String::from_utf8_lossy(&chunk[..valid]).to_string();
                    cursor.carry = chunk[valid..].to_vec();
                    if text.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                }
            }
        }
    }
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

fn format_workdir_snapshot(fx: &CliFixture) -> String {
    let mut out = String::new();
    let _ = writeln!(&mut out, "---- workdir ----");
    let _ = writeln!(&mut out, "{}", fx.workdir.display());
    match std::fs::read_dir(&fx.workdir) {
        Ok(rd) => {
            let mut entries = rd
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            entries.sort();
            if entries.is_empty() {
                let _ = writeln!(&mut out, "(目录为空)");
            } else {
                for entry in entries.iter().take(20) {
                    let _ = writeln!(&mut out, "- {entry}");
                }
                if entries.len() > 20 {
                    let _ = writeln!(&mut out, "... (其余 {} 项省略)", entries.len() - 20);
                }
            }
        }
        Err(err) => {
            let _ = writeln!(&mut out, "(无法列出 workdir: {err})");
        }
    }

    let counter = counter_path(fx);
    if counter.exists() {
        let _ = writeln!(&mut out, "---- counter.py ----");
        let _ = writeln!(&mut out, "{}", counter.display());
        match std::fs::read_to_string(&counter) {
            Ok(content) => {
                let _ = writeln!(&mut out, "{content}");
            }
            Err(err) => {
                let _ = writeln!(&mut out, "(无法读取 counter.py: {err})");
            }
        }
    }
    out
}

fn format_focus_plan(fx: &CliFixture) -> String {
    let Some(path) = current_plan_path(fx) else {
        return String::new();
    };
    let mut out = String::new();
    let _ = writeln!(&mut out, "---- plan: {} ----", path.display());
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let _ = writeln!(&mut out, "{content}");
        }
        Err(err) => {
            let _ = writeln!(&mut out, "(无法读取当前 plan: {err})");
        }
    }
    out
}

fn emit_diag(fx: &CliFixture, text: &str) {
    fx.diag_log.emit(text);
}

fn push_live_delta_sections(
    body: &mut String,
    stdout: &[u8],
    stderr: &[u8],
    process_state: &mut CliProcessDiagState,
) {
    if let Some(stdout_delta) = text_delta_from_bytes(stdout, &mut process_state.stdout) {
        let _ = writeln!(body, "---- stdout delta ----");
        let _ = writeln!(body, "{stdout_delta}");
    }
    if let Some(stderr_delta) = text_delta_from_bytes(stderr, &mut process_state.stderr) {
        let _ = writeln!(body, "---- stderr delta ----");
        let _ = writeln!(body, "{stderr_delta}");
    }
}

fn push_changed_state_sections(
    body: &mut String,
    fx: &CliFixture,
    run_state: &mut CliRunDiagState,
) {
    if let Some(plan_snapshot) =
        changed_snapshot(format_focus_plan(fx), &mut run_state.last_plan_snapshot)
    {
        let _ = writeln!(body, "---- current plan snapshot (after above deltas) ----");
        body.push_str(&plan_snapshot);
    }
    if let Some(workdir_snapshot) = changed_snapshot(
        format_workdir_snapshot(fx),
        &mut run_state.last_workdir_snapshot,
    ) {
        let _ = writeln!(
            body,
            "---- current workdir snapshot (after above deltas) ----"
        );
        body.push_str(&workdir_snapshot);
    }
}

fn push_transcript_phase_summary(
    body: &mut String,
    fx: &CliFixture,
    run_state: &mut CliRunDiagState,
    heading: &str,
) {
    if let Some(transcript_delta) =
        text_delta_from_file(&fx.transcript_path, &mut run_state.transcript_offset)
    {
        let _ = writeln!(
            body,
            "---- {heading}: {} ----",
            fx.transcript_path.display()
        );
        let _ = writeln!(body, "{transcript_delta}");
    }
}

fn dump_live_diag(
    label: &str,
    fx: &CliFixture,
    stdout: &[u8],
    stderr: &[u8],
    process_state: &mut CliProcessDiagState,
) {
    let mut body = String::new();
    let mut run_state = fx.diag_state.lock().expect("lock cli run diag state");
    push_live_delta_sections(&mut body, stdout, stderr, process_state);
    push_changed_state_sections(&mut body, fx, &mut run_state);
    drop(run_state);

    if body.trim().is_empty() {
        return;
    }

    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "\n==== [{label}] HOME = {} ====",
        fx.home.display()
    );
    out.push_str(&body);
    emit_diag(fx, &out);
}

fn dump_phase_summary(
    label: &str,
    fx: &CliFixture,
    stdout: &[u8],
    stderr: &[u8],
    process_state: &mut CliProcessDiagState,
) {
    let mut body = String::new();
    let mut run_state = fx.diag_state.lock().expect("lock cli run diag state");
    push_live_delta_sections(&mut body, stdout, stderr, process_state);
    push_changed_state_sections(&mut body, fx, &mut run_state);
    push_transcript_phase_summary(
        &mut body,
        fx,
        &mut run_state,
        "persisted transcript summary (phase end, may lag the live deltas above)",
    );
    drop(run_state);

    if body.trim().is_empty() {
        return;
    }

    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "\n==== [{label}] HOME = {} ====",
        fx.home.display()
    );
    out.push_str(&body);
    emit_diag(fx, &out);
}

fn emit_plan_resolved_block(label: &str, fx: &CliFixture) {
    let Some(plan_path) = current_plan_path(fx) else {
        return;
    };
    let plan_snapshot = format_focus_plan(fx);
    let mut body = String::new();
    let _ = writeln!(&mut body, "---- current plan path ----");
    let _ = writeln!(&mut body, "{}", plan_path.display());
    if !plan_snapshot.is_empty() {
        let _ = writeln!(
            &mut body,
            "---- current plan snapshot (resolved after planning phase) ----"
        );
        body.push_str(&plan_snapshot);
    }
    if body.trim().is_empty() {
        return;
    }

    let mut run_state = fx.diag_state.lock().expect("lock cli run diag state");
    if !plan_snapshot.is_empty() {
        run_state.last_plan_snapshot = Some(plan_snapshot);
    }
    drop(run_state);

    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "\n==== [{label}] HOME = {} ====",
        fx.home.display()
    );
    out.push_str(&body);
    emit_diag(fx, &out);
}

fn dump_single_process_diag(label: &str, fx: &CliFixture, stdout: &[u8], stderr: &[u8]) {
    let mut text = String::new();
    let _ = writeln!(
        &mut text,
        "\n==== [{label}] HOME = {} ====",
        fx.home.display()
    );
    text.push_str(&format_focus_plan(fx));
    if let Some(tail) = tail_text_file(&fx.transcript_path, 80) {
        let _ = writeln!(
            &mut text,
            "---- transcript: {} (tail 80) ----",
            fx.transcript_path.display()
        );
        let _ = writeln!(&mut text, "{tail}");
    }
    text.push_str(&format_workdir_snapshot(fx));
    let _ = writeln!(&mut text, "==== stdout tail (前 4000) ====");
    let _ = writeln!(&mut text, "{}", tail_chars(stdout, 4000));
    let _ = writeln!(&mut text, "==== stderr tail (前 2000) ====");
    let _ = writeln!(&mut text, "{}", tail_chars(stderr, 2000));
    emit_diag(fx, &text);
}

fn wait_for_child_output(
    mut child: std::process::Child,
    fx: &CliFixture,
    phase: &str,
    timeout: Option<Duration>,
) -> Output {
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let mut process_state = CliProcessDiagState::default();
    let stdout_handle = spawn_reader_thread(
        child.stdout.take().expect("stdout piped"),
        Arc::clone(&stdout_buf),
    );
    let stderr_handle = spawn_reader_thread(
        child.stderr.take().expect("stderr piped"),
        Arc::clone(&stderr_buf),
    );
    let start = std::time::Instant::now();
    let mut last_heartbeat = start;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                let out = Output {
                    status,
                    stdout: stdout_buf.lock().unwrap().clone(),
                    stderr: stderr_buf.lock().unwrap().clone(),
                };
                dump_phase_summary(
                    &format!("{phase}_completed"),
                    fx,
                    &out.stdout,
                    &out.stderr,
                    &mut process_state,
                );
                return out;
            }
            Ok(None) => {
                if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                    dump_live_diag(
                        &format!("{phase}_heartbeat"),
                        fx,
                        &stdout_buf.lock().unwrap(),
                        &stderr_buf.lock().unwrap(),
                        &mut process_state,
                    );
                    last_heartbeat = std::time::Instant::now();
                }
                if let Some(timeout) = timeout {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        let status = child.wait().unwrap_or_else(|_| panic!("kill+wait timeout"));
                        let _ = stdout_handle.join();
                        let _ = stderr_handle.join();
                        let out = Output {
                            status,
                            stdout: stdout_buf.lock().unwrap().clone(),
                            stderr: stderr_buf.lock().unwrap().clone(),
                        };
                        dump_single_process_diag(
                            &format!("{phase}_timeout"),
                            fx,
                            &out.stdout,
                            &out.stderr,
                        );
                        panic!(
                            "tomcat chat 子进程超时 ({}s)；stdout={}\nstderr={}",
                            timeout.as_secs(),
                            String::from_utf8_lossy(&out.stdout),
                            String::from_utf8_lossy(&out.stderr),
                        );
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                let stdout = stdout_buf.lock().unwrap().clone();
                let stderr = stderr_buf.lock().unwrap().clone();
                dump_single_process_diag(&format!("{phase}_wait_err"), fx, &stdout, &stderr);
                panic!("try_wait 失败：{e}");
            }
        }
    }
}

fn dump_diag(label: &str, fx: &CliFixture, out_a: Option<&Output>, out_b: Option<&Output>) {
    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "\n==== [{label}] HOME = {} ====",
        fx.home.display()
    );
    out.push_str(&format_focus_plan(fx));
    if let Some(tail) = tail_text_file(&fx.transcript_path, 80) {
        let _ = writeln!(
            &mut out,
            "---- transcript: {} (tail 80) ----",
            fx.transcript_path.display()
        );
        let _ = writeln!(&mut out, "{tail}");
    }
    out.push_str(&format_workdir_snapshot(fx));
    if let Some(a) = out_a {
        let _ = writeln!(&mut out, "==== proc A stdout (前 4000) ====");
        let _ = writeln!(
            &mut out,
            "{}",
            String::from_utf8_lossy(&a.stdout)
                .chars()
                .take(4000)
                .collect::<String>()
        );
        let _ = writeln!(&mut out, "==== proc A stderr (前 2000) ====");
        let _ = writeln!(
            &mut out,
            "{}",
            String::from_utf8_lossy(&a.stderr)
                .chars()
                .take(2000)
                .collect::<String>()
        );
    }
    if let Some(b) = out_b {
        let _ = writeln!(&mut out, "==== proc B stdout (前 4000) ====");
        let _ = writeln!(
            &mut out,
            "{}",
            String::from_utf8_lossy(&b.stdout)
                .chars()
                .take(4000)
                .collect::<String>()
        );
        let _ = writeln!(&mut out, "==== proc B stderr (前 2000) ====");
        let _ = writeln!(
            &mut out,
            "{}",
            String::from_utf8_lossy(&b.stderr)
                .chars()
                .take(2000)
                .collect::<String>()
        );
    }
    emit_diag(fx, &out);
}

fn assert_planning_phase_smoke(
    fx: &CliFixture,
    out_a: &Output,
    created_plan: &common::CreatedPlanRef,
) {
    let plan = read_plan(&created_plan.path).expect("read_plan planning smoke");
    if plan.frontmatter.state != PlanFileState::Planning {
        dump_diag("planning_state_not_planning", fx, Some(out_a), None);
        panic!(
            "planning-only 用例结束后 plan 应保持 Planning，实际：{:?}",
            plan.frontmatter.state
        );
    }
    assert!(
        created_plan.plan_id.starts_with("plan_"),
        "plan_id 形态异常: {}",
        created_plan.plan_id
    );
    assert!(
        !plan.frontmatter.todos.is_empty(),
        "planning-only 用例结束后 todos 不应为空"
    );
    let stdout_a = String::from_utf8_lossy(&out_a.stdout);
    assert!(
        stdout_a.contains("u[Plan:planning"),
        "进程 A stdout 应展示 planning user prompt；实际前 4000 字符：{}",
        tail_chars(&out_a.stdout, 4000)
    );
    assert!(
        stdout_a.contains("agent.main[Plan:planning]>"),
        "进程 A stdout 应展示 planning agent prompt；实际前 4000 字符：{}",
        tail_chars(&out_a.stdout, 4000)
    );
}

fn assert_exec_phase_smoke(fx: &CliFixture, out_b: &Output, plan_path: &Path) {
    if !out_b.status.success() {
        dump_diag("proc_b_failed", fx, None, Some(out_b));
        panic!("进程 B 退出码非 0: {:?}", out_b.status);
    }

    let final_plan = read_plan(plan_path).expect("read exec smoke final plan");
    if matches!(final_plan.frontmatter.state, PlanFileState::Planning) {
        dump_diag("exec_smoke_still_planning", fx, None, Some(out_b));
        panic!(
            "exec-only 用例结束后 plan 至少应离开 Planning，实际：{:?}",
            final_plan.frontmatter.state
        );
    }
    let expected_session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &fx.workdir);
    assert_eq!(
        final_plan.frontmatter.session_key.as_deref(),
        Some(expected_session_key.as_str()),
        "exec-only 用例应把 active plan 绑定到当前 workdir 的 code session_key"
    );
    assert_eq!(
        final_plan.frontmatter.session_id.as_deref(),
        Some(fx.run_session_id.as_str()),
        "exec-only 用例应把 active plan 绑定到本次 CLI run 的真实 session_id"
    );
    assert!(
        !out_b.stdout.is_empty(),
        "进程 B 应有用户可见 stdout 输出；日志文件：{}",
        fx.diag_log.path.display()
    );
    let stdout_b = String::from_utf8_lossy(&out_b.stdout);
    assert!(
        stdout_b.contains("u[Plan:executing"),
        "进程 B stdout 应展示自动开跑的 EXEC user prompt；实际前 4000 字符：{}",
        tail_chars(&out_b.stdout, 4000)
    );
    assert!(
        stdout_b.contains("agent.main[Plan:executing]>"),
        "进程 B stdout 应展示 EXEC agent prompt；实际前 4000 字符：{}",
        tail_chars(&out_b.stdout, 4000)
    );
}

fn run_cli_planning_smoke_case(
    goal: &str,
    workdir_override: Option<&Path>,
    planning_prompt_override: Option<PlanningPromptOverride>,
    planning_timeout: Option<Duration>,
) {
    common::setup_logging();
    let slug = common::slugify_filename(goal, "goal", 40);
    let fx = setup_fixture(&slug, workdir_override, 0);
    let planning = run_planning_phase(&fx, goal, planning_prompt_override, planning_timeout);
    assert_planning_phase_smoke(&fx, &planning.output, &planning.created_plan);
}

fn run_cli_exec_smoke_case(
    goal: &str,
    workdir_override: Option<&Path>,
    exec_prompt_override: Option<ExecPromptOverride>,
    exec_timeout: Option<Duration>,
) {
    common::setup_logging();
    let slug = format!("exec-{}", common::slugify_filename(goal, "goal", 36));
    let fx = setup_fixture(&slug, workdir_override, 0);
    let created_plan = seed_counter_planning_plan(&fx, goal);
    let out_b = run_exec_phase(
        &fx,
        goal,
        &created_plan.plan_id,
        exec_prompt_override,
        exec_timeout,
    );
    assert_exec_phase_smoke(&fx, &out_b, &created_plan.path);
}

fn run_cli_real_llm_case(
    goal: &str,
    workdir_override: Option<&Path>,
    planning_prompt_override: Option<PlanningPromptOverride>,
    exec_prompt_override: Option<ExecPromptOverride>,
    planning_timeout: Option<Duration>,
    exec_timeout: Option<Duration>,
) {
    common::setup_logging();
    let slug = common::slugify_filename(goal, "goal", 40);
    let fx = setup_fixture(&slug, workdir_override, 1);
    let planning = run_planning_phase(&fx, goal, planning_prompt_override, planning_timeout);
    let out_a = planning.output;
    let created_plan = planning.created_plan;
    let plan_path = created_plan.path.clone();
    let plan = read_plan(&plan_path).expect("read_plan plan_a");
    let plan_id = created_plan.plan_id;
    assert!(plan_id.starts_with("plan_"), "plan_id 形态异常: {plan_id}");
    assert!(
        plan.frontmatter.session_key.is_none() && plan.frontmatter.session_id.is_none(),
        "planning 阶段 create_plan 不应绑定 session_key/session_id"
    );
    let todo_ids: Vec<String> = plan
        .frontmatter
        .todos
        .iter()
        .map(|todo| todo.id.clone())
        .collect();
    assert!(!todo_ids.is_empty(), "进程 A 结束后 todos 不应为空");

    let out_b = run_exec_phase(&fx, goal, &plan_id, exec_prompt_override, exec_timeout);
    if !out_b.status.success() {
        dump_diag("proc_b_failed", &fx, Some(&out_a), Some(&out_b));
        panic!("进程 B 退出码非 0: {:?}", out_b.status);
    }

    let final_plan = read_plan(&plan_path).expect("read_plan plan_b");
    if final_plan.frontmatter.state != PlanFileState::Completed {
        dump_diag("final_state_not_completed", &fx, Some(&out_a), Some(&out_b));
        panic!(
            "EOF 后 plan 磁盘 state 应为 Completed，实际：{:?}",
            final_plan.frontmatter.state
        );
    }
    let all_done = final_plan
        .frontmatter
        .todos
        .iter()
        .all(|t| matches!(t.status, TodoStatus::Completed));
    if !all_done {
        dump_diag("todos_not_all_completed", &fx, Some(&out_a), Some(&out_b));
        panic!(
            "所有 todos 应 Completed，实际：{:#?}",
            final_plan.frontmatter.todos
        );
    }
    let expected_session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &fx.workdir);
    assert_eq!(
        final_plan.frontmatter.session_key.as_deref(),
        Some(expected_session_key.as_str()),
        "EXEC/completed 盘应绑定当前 workdir 的 code session_key"
    );
    assert_eq!(
        final_plan.frontmatter.session_id.as_deref(),
        Some(fx.run_session_id.as_str()),
        "EXEC/completed 盘应绑定本次 CLI run 的真实 session_id"
    );
    assert!(
        !out_b.stdout.is_empty(),
        "进程 B 应有用户可见 stdout 输出；日志文件：{}",
        fx.diag_log.path.display()
    );
    let stdout_b = String::from_utf8_lossy(&out_b.stdout);
    assert!(
        stdout_b.contains("u[Plan:executing]> start building "),
        "进程 B stdout 应展示自动开跑的 EXEC user prompt；实际前 4000 字符：{}",
        tail_chars(&out_b.stdout, 4000)
    );
    assert!(
        stdout_b.contains("agent.main[Plan:executing]>"),
        "进程 B stdout 应展示 EXEC agent prompt；实际前 4000 字符：{}",
        tail_chars(&out_b.stdout, 4000)
    );

    let transcript =
        std::fs::read_to_string(&fx.transcript_path).expect("read cli real llm transcript");
    let lines: Vec<&str> = transcript.lines().collect();
    let plan_review_idx = lines.iter().position(|l| l.contains("\"plan.review\""));
    let plan_code_review_idx = lines.iter().position(|l| {
        l.contains("\"plan.code_review\"") && !l.contains("\"plan.code_review.warning\"")
    });
    let plan_verify_idx = lines.iter().position(|l| l.contains("\"plan.verify\""));
    if plan_review_idx.is_none() || plan_code_review_idx.is_none() || plan_verify_idx.is_none() {
        dump_diag(
            "transcript_missing_review_events",
            &fx,
            Some(&out_a),
            Some(&out_b),
        );
    }
    assert!(
        plan_review_idx.is_some(),
        "CLI 真 LLM transcript 应含至少一条 plan.review 自定义事件"
    );
    assert!(
        plan_code_review_idx.is_some(),
        "CLI 真 LLM transcript 应含至少一条 plan.code_review 自定义事件"
    );
    assert!(
        plan_verify_idx.is_some(),
        "CLI 真 LLM transcript 应含至少一条 plan.verify 自定义事件"
    );
    assert!(
        plan_code_review_idx.unwrap() < plan_verify_idx.unwrap(),
        "CLI 真 LLM transcript 中 plan.code_review 应早于 plan.verify"
    );
}

#[test]
#[serial]
fn cli_planning_path_with_real_llm() {
    run_cli_planning_smoke_case(
        COUNTER_PLAN_GOAL,
        None,
        Some(build_counter_planning_prompt),
        Some(PLANNING_TIMEOUT),
    );
}

#[test]
#[serial]
fn cli_exec_resume_path_with_real_llm() {
    run_cli_exec_smoke_case(
        COUNTER_PLAN_GOAL,
        None,
        Some(build_counter_exec_prompt),
        Some(EXEC_TIMEOUT),
    );
}

/// 手动真 LLM 观察用例：自定义 goal 经 planning prompt 注入（`/plan` 仅进入 PLAN，不再带目标参数）。
///
/// ```text
/// cd /Users/yankeben/workspace/Tomcat/tomcat
///
/// TOMCAT_E2E_PLAN_GOAL='为当前目录实现一个最小可运行的脚本并自验证' \
/// TOMCAT_E2E_WORKDIR='/绝对路径/你的工作目录' \
/// cargo test -p tomcat --test plan_real_llm_cli_e2e cli_plan_path_with_real_llm_custom_goal -- --ignored --nocapture
/// ```
///
/// - `TOMCAT_E2E_PLAN_GOAL`：必填。
/// - `TOMCAT_E2E_WORKDIR`：可选；不传则用 `~/.tomcat/temp/...` 临时目录。若指定，须在配置的 `workspace.workspace_roots` 可写根内。
/// - 该观察用例不设 planning / exec 墙钟超时；仅保留 heartbeat 诊断输出。
/// - 该用例每次都会创建 fresh `session_id`；若一启动就落到 EXEC，说明产品侧
///   `PlanRuntime::recover()` 仍错误地只按 `DEFAULT_SESSION_KEY` 认盘，而不是按本次 run 的 `session_id`。
#[test]
#[ignore = "manual real-LLM observation test; run with --ignored --nocapture"]
#[serial]
fn cli_plan_path_with_real_llm_custom_goal() {
    common::load_openai_test_env();
    let goal = std::env::var(CUSTOM_PLAN_GOAL_ENV).unwrap_or_else(|_| {
        panic!(
            "缺少环境变量 {}。示例：{}='write a hello world script' cargo test -p tomcat --test plan_real_llm_cli_e2e cli_plan_path_with_real_llm_custom_goal -- --ignored --nocapture",
            CUSTOM_PLAN_GOAL_ENV,
            CUSTOM_PLAN_GOAL_ENV
        )
    });
    let workdir_buf = std::env::var(CUSTOM_WORKDIR_ENV).ok().map(PathBuf::from);
    run_cli_real_llm_case(&goal, workdir_buf.as_deref(), None, None, None, None);
}
