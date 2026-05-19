//! E2E-PLAN-RL-001：CLI 子进程黑盒真 LLM 全路径测试。
//!
//! 通过 `assert_cmd::Command::cargo_bin("tomcat")` 真起 `tomcat chat`，让真 LLM
//! 在两段 stdin 里推进 PLAN→EXEC→Completed 全程：
//!
//! - 进程 A：`tomcat chat` + `/plan "..."` + PLANNING_PROMPT；EOF 退出后落盘 mode=planning。
//! - 测试主程：扫盘取 `plan_id`。
//! - 进程 B：`tomcat chat --resume` + `/plan build {plan_id}` + EXEC_PROMPT；EOF 退出后落盘 mode=completed。
//!
//! 拆两个进程是因为 stdin 是一次性写完的（rustyline pipe 行为）：测试主程必须能在
//! 进程 A 结束后扫盘拿到真实派生的 `plan_id`，再写进程 B 的 stdin。
//!
//! ## 门禁
//! - `OPENAI_API_KEY` 必须存在；缺失 → panic（E2E-PLAN-RL-001 / E2E_TEST_SPEC §4）。
//! - 默认模型来自 `TOMCAT_E2E_LLM_MODEL`，未设 → `gpt-5.2`。
//!
//! ## 数据目录
//! - 子进程**继承真实 HOME**（不注入临时 `HOME`）；plan 落盘到 `~/.tomcat/plans/`。
//! - 配置优先 `~/.tomcat/tomcat.config.toml`；cwd 用 `~/.tomcat/temp/<run>/`（内置 workspace_roots）。
//! - 与 inprocess 真 LLM 共用 plans 目录，故 `#[serial]` 串行。

#![allow(clippy::field_reassign_with_default)]

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serial_test::serial;
use tomcat::api::chat::plan_runtime::file_store::{read_plan, PlanFileMode, TodoStatus};

const PLAN_GOAL: &str = "cli e2e: write counter.py that prints 0";
const ASK_QUESTION_AUTO_PICK_ENV: &str = "TOMCAT_ASK_QUESTION_TEST_AUTO_PICK";

const PLANNING_PROMPT: &str = concat!(
    "Use the create_plan tool to draft a minimal plan for this exact goal: ",
    "\"cli e2e: write counter.py that prints 0\". ",
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
);

const PLANNING_TIMEOUT: Duration = Duration::from_secs(180);
const EXEC_TIMEOUT: Duration = Duration::from_secs(240);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

fn require_api_key() -> String {
    common::load_openai_test_env();
    std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "plan_real_llm_cli_e2e 必须设置 OPENAI_API_KEY（环境变量或 tomcat/.env；E2E-PLAN-RL-001）"
        )
    })
}

fn default_model() -> String {
    std::env::var("TOMCAT_E2E_LLM_MODEL").unwrap_or_else(|_| "gpt-5.2".to_string())
}

struct CliFixture {
    home: PathBuf,
    workdir: PathBuf,
    config_path: Option<PathBuf>,
    api_key: String,
    model: String,
}

fn setup_fixture() -> CliFixture {
    let api_key = require_api_key();
    let model = default_model();
    let home = dirs::home_dir().expect("无法定位 HOME 目录");
    std::fs::create_dir_all(home.join(".tomcat").join("plans")).unwrap();
    let workdir = common::dot_tomcat_e2e_workdir("cli_real_llm");
    let config_path = {
        let p = home.join(".tomcat").join("tomcat.config.toml");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    };

    CliFixture {
        home,
        workdir,
        config_path,
        api_key,
        model,
    }
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
        if plan.frontmatter.mode != PlanFileMode::Planning {
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

fn run_tomcat_chat(
    fx: &CliFixture,
    phase: &str,
    args: &[&str],
    stdin_text: &str,
    timeout: Duration,
) -> Output {
    let mut cmd = StdCommand::new(assert_cmd::cargo::cargo_bin!("tomcat"));
    cmd.current_dir(&fx.workdir)
        .arg("chat")
        .args(args)
        .env("SHELL", "/bin/zsh")
        .env("OPENAI_API_KEY", &fx.api_key)
        .env("TOMCAT_ASK_QUESTION_TIMEOUT_MS", "5000")
        .env(ASK_QUESTION_AUTO_PICK_ENV, "recommended")
        .env("TOMCAT__LLM__DEFAULT_MODEL", &fx.model)
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

fn build_exec_prompt(todo_ids: &[String], workdir: &Path) -> String {
    let counter = workdir.join("counter.py");
    let mut lines = vec![
        "Advance the active plan to completion by actually implementing the artifact.".to_string(),
        format!(
            "Write a single file at `{}`.",
            counter.display()
        ),
        "Requirements:".to_string(),
        "- The file must be valid Python.".to_string(),
        "- Running `python3 counter.py` from the current working directory must exit 0, print exactly `0\\n` to stdout, and print nothing to stderr.".to_string(),
        "- Use `bash` to run and verify the program yourself before closing the plan.".to_string(),
        "- You may use `list_dir`, `read`, `search_files`, `write`, `edit`, `bash`, and `update_plan`.".to_string(),
        "- Do NOT call ask_question. Do NOT edit the plan file directly.".to_string(),
        "- Do NOT write outside the current working directory.".to_string(),
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

fn counter_path(fx: &CliFixture) -> PathBuf {
    fx.workdir.join("counter.py")
}

fn dump_counter_artifact(fx: &CliFixture) {
    let counter = counter_path(fx);
    eprintln!("---- artifact cwd ----\n{}", fx.workdir.display());
    eprintln!("---- counter.py ----\n{}", counter.display());
    match std::fs::read_to_string(&counter) {
        Ok(content) => eprintln!("{content}"),
        Err(err) => eprintln!("(无法读取 counter.py: {err})"),
    }
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

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn latest_transcript_tail(home: &Path) -> Option<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_jsonl_files(&home.join(".tomcat").join("agents"), &mut files);
    files.sort();
    let path = files.pop()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let tail = content
        .lines()
        .rev()
        .take(80)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Some((path, tail))
}

fn dump_live_diag(label: &str, fx: &CliFixture, stdout: &[u8], stderr: &[u8]) {
    eprintln!("\n==== [{label}] HOME = {} ====", fx.home.display());
    let plans_dir = fx.home.join(".tomcat").join("plans");
    if let Ok(rd) = std::fs::read_dir(&plans_dir) {
        for entry in rd.flatten() {
            eprintln!("---- plan: {} ----", entry.path().display());
            if let Ok(s) = std::fs::read_to_string(entry.path()) {
                eprintln!("{s}");
            }
        }
    }
    if let Some((path, tail)) = latest_transcript_tail(&fx.home) {
        eprintln!("---- transcript: {} (tail 80) ----", path.display());
        eprintln!("{tail}");
    }
    dump_counter_artifact(fx);
    eprintln!("---- stdout tail ----\n{}", tail_chars(stdout, 4000));
    eprintln!("---- stderr tail ----\n{}", tail_chars(stderr, 2000));
}

fn wait_for_child_output(
    mut child: std::process::Child,
    fx: &CliFixture,
    phase: &str,
    timeout: Duration,
) -> Output {
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
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
                return Output {
                    status,
                    stdout: stdout_buf.lock().unwrap().clone(),
                    stderr: stderr_buf.lock().unwrap().clone(),
                };
            }
            Ok(None) => {
                if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                    dump_live_diag(
                        &format!("{phase}_heartbeat"),
                        fx,
                        &stdout_buf.lock().unwrap(),
                        &stderr_buf.lock().unwrap(),
                    );
                    last_heartbeat = std::time::Instant::now();
                }
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
                    dump_live_diag(&format!("{phase}_timeout"), fx, &out.stdout, &out.stderr);
                    panic!(
                        "tomcat chat 子进程超时 ({}s)；stdout={}\nstderr={}",
                        timeout.as_secs(),
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr),
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("try_wait 失败：{e}"),
        }
    }
}

fn dump_diag(label: &str, fx: &CliFixture, out_a: &Output, out_b: Option<&Output>) {
    eprintln!("\n==== [{label}] HOME = {} ====", fx.home.display());
    let plans_dir = fx.home.join(".tomcat").join("plans");
    if let Ok(rd) = std::fs::read_dir(&plans_dir) {
        for entry in rd.flatten() {
            eprintln!("---- plan: {} ----", entry.path().display());
            if let Ok(s) = std::fs::read_to_string(entry.path()) {
                eprintln!("{}", s);
            }
        }
    }
    dump_counter_artifact(fx);
    eprintln!(
        "==== proc A stdout (前 4000) ====\n{}",
        String::from_utf8_lossy(&out_a.stdout)
            .chars()
            .take(4000)
            .collect::<String>()
    );
    eprintln!(
        "==== proc A stderr (前 2000) ====\n{}",
        String::from_utf8_lossy(&out_a.stderr)
            .chars()
            .take(2000)
            .collect::<String>()
    );
    if let Some(b) = out_b {
        eprintln!(
            "==== proc B stdout (前 4000) ====\n{}",
            String::from_utf8_lossy(&b.stdout)
                .chars()
                .take(4000)
                .collect::<String>()
        );
        eprintln!(
            "==== proc B stderr (前 2000) ====\n{}",
            String::from_utf8_lossy(&b.stderr)
                .chars()
                .take(2000)
                .collect::<String>()
        );
    }
}

fn assert_counter_artifact(fx: &CliFixture, out_a: &Output, out_b: &Output) {
    let counter = counter_path(fx);
    if !counter.exists() {
        dump_diag("counter_missing", fx, out_a, Some(out_b));
        panic!("进程 B 结束后应生成产物文件: {}", counter.display());
    }
    let run = StdCommand::new("python3")
        .current_dir(&fx.workdir)
        .arg("counter.py")
        .output()
        .unwrap_or_else(|e| panic!("运行 counter.py 失败: {e}"));
    if !run.status.success() {
        dump_diag("counter_run_failed", fx, out_a, Some(out_b));
        panic!("counter.py 退出码应为 0，实际：{:?}", run.status);
    }
    if run.stdout != b"0\n" {
        dump_diag("counter_stdout_mismatch", fx, out_a, Some(out_b));
        panic!(
            "counter.py stdout 应恰好为 `0\\n`，实际：{:?}",
            String::from_utf8_lossy(&run.stdout)
        );
    }
    if !run.stderr.is_empty() {
        dump_diag("counter_stderr_not_empty", fx, out_a, Some(out_b));
        panic!(
            "counter.py stderr 应为空，实际：{:?}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
#[serial]
fn cli_full_plan_path_with_real_llm() {
    common::setup_logging();
    let fx = setup_fixture();

    // 进程 A：PLAN
    let stdin_a = format!(
        "/plan \"{goal}\"\n{prompt}\n",
        goal = PLAN_GOAL,
        prompt = PLANNING_PROMPT
    );
    let out_a = run_tomcat_chat(&fx, "planning_proc", &[], &stdin_a, PLANNING_TIMEOUT);
    if !out_a.status.success() {
        dump_diag("proc_a_failed", &fx, &out_a, None);
        panic!("进程 A 退出码非 0: {:?}", out_a.status);
    }

    // 扫盘取 plan_id
    let plan_path = pick_newest_planning_plan_path(&fx.home).unwrap_or_else(|| {
        dump_diag("no_planning_plan", &fx, &out_a, None);
        panic!("进程 A 后未找到 mode=planning 的 plan 文件");
    });
    let plan = read_plan(&plan_path).expect("read_plan plan_a");
    let plan_id = plan.frontmatter.plan_id.clone();
    assert!(plan_id.starts_with("plan_"), "plan_id 形态异常: {plan_id}");
    let todo_ids: Vec<String> = plan
        .frontmatter
        .todos
        .iter()
        .map(|todo| todo.id.clone())
        .collect();
    assert!(!todo_ids.is_empty(), "进程 A 结束后 todos 不应为空");

    // 进程 B：EXEC（--resume 复用 session_key，build_plan 5 闸门都通过）
    let exec_prompt = build_exec_prompt(&todo_ids, &fx.workdir);
    let stdin_b = format!(
        "/plan build {plan_id}\n{prompt}\n",
        plan_id = plan_id,
        prompt = exec_prompt
    );
    let out_b = run_tomcat_chat(&fx, "exec_proc", &["--resume"], &stdin_b, EXEC_TIMEOUT);
    if !out_b.status.success() {
        dump_diag("proc_b_failed", &fx, &out_a, Some(&out_b));
        panic!("进程 B 退出码非 0: {:?}", out_b.status);
    }
    assert_counter_artifact(&fx, &out_a, &out_b);

    // 硬断言：磁盘 mode=completed + todos 全 completed
    let final_plan = read_plan(&plan_path).expect("read_plan plan_b");
    if final_plan.frontmatter.mode != PlanFileMode::Completed {
        dump_diag("final_mode_not_completed", &fx, &out_a, Some(&out_b));
        panic!(
            "EOF 后 plan 磁盘 mode 应为 Completed，实际：{:?}",
            final_plan.frontmatter.mode
        );
    }
    let all_done = final_plan
        .frontmatter
        .todos
        .iter()
        .all(|t| matches!(t.status, TodoStatus::Completed));
    if !all_done {
        dump_diag("todos_not_all_completed", &fx, &out_a, Some(&out_b));
        panic!(
            "所有 todos 应 Completed，实际：{:#?}",
            final_plan.frontmatter.todos
        );
    }
    assert!(!out_b.stdout.is_empty(), "进程 B 应有用户可见 stdout 输出");
}
