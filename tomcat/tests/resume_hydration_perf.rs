mod common;

use assert_cmd::Command;
use std::io::Write;
use std::path::PathBuf;
use tomcat::{
    init_context_state, load_config_toml_file, resolve_sessions_dir, ContextConfig, SessionManager,
};

#[allow(deprecated)]
fn cmd() -> Command {
    let mut c = Command::cargo_bin("tomcat").expect("binary tomcat should exist");
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c
}

fn apply_deepseek_env(command: &mut Command) {
    let model = common::deepseek_test_model();
    command
        .env(common::DEEPSEEK_TEST_API_KEY_ENV, "dummy-key")
        .env(
            "TOMCAT__LLM__API_KEY_ENV",
            common::DEEPSEEK_TEST_API_KEY_ENV,
        )
        .env("TOMCAT__LLM__PROVIDER", "openai")
        .env("TOMCAT__LLM__API_BASE", common::DEEPSEEK_TEST_API_BASE)
        .env("TOMCAT__LLM__DEFAULT_MODEL", &model)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", &model);
}

struct Fixture {
    _home: tempfile::TempDir,
    home_path: PathBuf,
    workdir: PathBuf,
    session: SessionManager,
}

fn setup_fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();
    let workdir = home.path().join("workspace");
    std::fs::create_dir_all(&workdir).unwrap();

    cmd()
        .args(["init"])
        .env("HOME", &home_path)
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let config_path = home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).expect("config should load");
    common::apply_deepseek_app_config(&mut cfg);
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize deepseek test config"),
    )
    .expect("persist deepseek test config");
    cfg.storage.work_dir = Some(home_path.join(".tomcat").to_string_lossy().to_string());
    let sessions_dir = resolve_sessions_dir(&cfg).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let session_key = tomcat::session_key_for(tomcat::SessionMode::Code, &workdir);
    let session = SessionManager::new_scoped(sessions_dir, session_key.clone());
    session.create_session(&session_key, None).unwrap();

    Fixture {
        _home: home,
        home_path,
        workdir,
        session,
    }
}

fn trace_field(stderr: &str, key: &str) -> Option<String> {
    stderr
        .lines()
        .find(|line| line.starts_with("TOMCAT_RESUME_TRACE "))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|part| part.strip_prefix(&format!("{key}=")).map(str::to_string))
        })
}

fn seed_boundary_session(path: &std::path::Path, old_turns: usize) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("transcript should exist");
    for idx in 0..old_turns {
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "message",
                "id": format!("m_{idx}"),
                "timestamp": "2025-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": format!("old-turn-{idx}") }
            })
        )
        .unwrap();
    }
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "type": "branch_summary",
            "id": "boundary_perf",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "summary": "boundary summary",
            "coveredCount": old_turns,
            "isBoundary": true,
        })
    )
    .unwrap();
    for idx in 0..12usize {
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "message",
                "id": format!("recent_{idx}"),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "message": { "role": "user", "content": format!("recent-turn-{idx}") }
            })
        )
        .unwrap();
    }
}

fn run_resume_trace(fx: &Fixture, extra_env: &[(&str, &str)]) -> String {
    let mut command = cmd();
    command
        .current_dir(&fx.workdir)
        .args(["code", "--resume"])
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("TOMCAT_RESUME_TRACE", "1")
        .write_stdin("");
    apply_deepseek_env(&mut command);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("code --resume should run");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
#[ignore = "manual performance baseline"]
fn perf_cold_rebuild_trace_10k_50k_200k_boundary_sessions() {
    common::setup_logging();
    for scale in [10_000usize, 50_000usize, 200_000usize] {
        let fx = setup_fixture();
        let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
        seed_boundary_session(&transcript_path, scale);
        let stderr = run_resume_trace(&fx, &[]);
        let bytes_scanned: u64 = trace_field(&stderr, "bytes_scanned")
            .expect("trace bytes_scanned")
            .parse()
            .unwrap();
        let entries_scanned: usize = trace_field(&stderr, "entries_scanned")
            .expect("trace entries_scanned")
            .parse()
            .unwrap();
        let elapsed_ms: u64 = trace_field(&stderr, "elapsed_ms")
            .expect("trace elapsed_ms")
            .parse()
            .unwrap();
        println!(
            "scale={scale} bytes_scanned={bytes_scanned} entries_scanned={entries_scanned} elapsed_ms={elapsed_ms}"
        );
        assert!(
            bytes_scanned > 0 && entries_scanned > 0,
            "cold rebuild should emit trace metrics"
        );
    }
}

#[test]
#[ignore = "manual performance baseline"]
fn perf_hot_path_boundary_sessions_stay_bounded() {
    common::setup_logging();
    for scale in [10_000usize, 50_000usize, 200_000usize] {
        let fx = setup_fixture();
        let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
        seed_boundary_session(&transcript_path, scale);
        let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();
        let stderr = run_resume_trace(&fx, &[]);
        let bytes_scanned: u64 = trace_field(&stderr, "bytes_scanned")
            .expect("trace bytes_scanned")
            .parse()
            .unwrap();
        println!("hot scale={scale} bytes_scanned={bytes_scanned}");
        assert!(
            bytes_scanned < 512 * 1024,
            "hot path should stay bounded at scale={scale}, got {bytes_scanned}"
        );
    }
}

#[test]
#[ignore = "manual performance baseline"]
fn perf_kill_switch_full_scans_more_than_tail_mode() {
    common::setup_logging();
    let fx = setup_fixture();
    let transcript_path = fx.session.current_transcript_path().unwrap().unwrap();
    seed_boundary_session(&transcript_path, 50_000);
    let _ = init_context_state(&fx.session, &ContextConfig::default(), "sys").unwrap();

    let tail_stderr = run_resume_trace(&fx, &[]);
    let full_stderr = run_resume_trace(&fx, &[("TOMCAT__CONTEXT__RESUME_HYDRATION_MODE", "full")]);
    let tail_bytes: u64 = trace_field(&tail_stderr, "bytes_scanned")
        .expect("tail bytes")
        .parse()
        .unwrap();
    let full_bytes: u64 = trace_field(&full_stderr, "bytes_scanned")
        .expect("full bytes")
        .parse()
        .unwrap();
    println!("tail_bytes={tail_bytes} full_bytes={full_bytes}");
    assert!(
        full_bytes > tail_bytes,
        "kill switch full path should scan more than tail mode"
    );
}
