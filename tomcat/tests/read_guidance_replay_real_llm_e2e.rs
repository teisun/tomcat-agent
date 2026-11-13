//! Opt-in replay of the real long-context session from the last complete tool result.
//! Three disposable HOME fixtures resume the production serve path on the original route.
//! Only fixture copies migrate the unpublished signature dictionary to native parts.
//! Reads hit the real checkout; Plan mode confines writes to temporary plans and shell is forbidden.
//! The source includes project code and signatures: run only with explicit authorization.
//! Acceptance: progress within 12 steps, no >2-step single narrow-read chain, and wide first reads.
//! The recorded incident is the baseline; no second binary or A/B runs are needed.
//! Run with TOMCAT_REAL_LLM_E2E=1, TOMCAT_READ_REPLAY_TRANSCRIPT, and
//! TOMCAT_READ_REPLAY_CUT_LINE=416; optional TOMCAT_REAL_LLM_E2E_ARTIFACTS retains local reports.
//! After explicit user consent, TOMCAT_READ_REPLAY_DROP_SOURCE_SIGNATURES=1 removes only
//! source Gemini signature fields in fixture copies when moving between incompatible channels.
//! The report distinguishes that migration from a signature-preserving incident replay.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::serve::{
    cargo_bin_path, isolated_serve_command_with_binary, setup_serve_fixture, spawn_serve_command,
    ServeChild, ServeFixture,
};
use serde::Serialize;
use serde_json::{json, Value};
use serial_test::serial;
use tomcat::load_config_toml_file;

const TRANSCRIPT_ENV: &str = "TOMCAT_READ_REPLAY_TRANSCRIPT";
const CUT_LINE_ENV: &str = "TOMCAT_READ_REPLAY_CUT_LINE";
const RUNS_ENV: &str = "TOMCAT_READ_REPLAY_RUNS";
const STEPS_ENV: &str = "TOMCAT_READ_REPLAY_STEPS";
const DROP_SOURCE_SIGNATURES_ENV: &str = "TOMCAT_READ_REPLAY_DROP_SOURCE_SIGNATURES";
const ARTIFACTS_ENV: &str = "TOMCAT_REAL_LLM_E2E_ARTIFACTS";
const DEFAULT_RUNS: usize = 3;
const DEFAULT_STEPS: usize = 12;
const INIT_TIMEOUT: Duration = Duration::from_secs(120);
const RUN_TIMEOUT: Duration = Duration::from_secs(40 * 60);
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);
/// `read` without `limit` returns at most this many lines.
const READ_DEFAULT_LINES: u64 = 2000;
/// Larger first-request drift means the hydrated history is not the source context.
const MAX_CONTEXT_DRIFT: f64 = 0.15;
const PROGRESS_TOOLS: [&str; 5] = [
    "ask_question",
    "create_plan",
    "update_plan",
    "start_plan",
    "todos",
];
const CONTEXT_EVENTS: [&str; 4] = [
    "auto_compaction_start",
    "boundary_switched",
    "context_overflow_trim_start",
    "compaction_error",
];

struct Source {
    lines: Vec<String>,
    session_id: String,
    cwd: PathBuf,
    model_id: String,
    store_entry: Value,
    plan_files: Vec<PathBuf>,
    reference_prompt_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ReadWindow {
    path: PathBuf,
    offset: u64,
    limit: u64,
    file_lines: Option<u64>,
    whole_file: bool,
}

#[derive(Debug, Serialize)]
struct Acceptance {
    progress: bool,
    no_narrow_chain: bool,
    wide_first_reads: bool,
}

impl Acceptance {
    fn passed(&self) -> bool {
        self.progress && self.no_narrow_chain && self.wide_first_reads
    }
}

#[derive(Serialize)]
struct RunReport {
    run: usize,
    source_signatures_dropped: usize,
    stop: String,
    idle_outcome: String,
    steps: usize,
    tools_per_step: Vec<Vec<String>>,
    reads_per_step: Vec<Vec<ReadWindow>>,
    first_reads: Vec<ReadWindow>,
    first_read_median: f64,
    single_read_steps: usize,
    longest_single_narrow_streak: usize,
    first_progress: Option<(usize, String)>,
    first_prompt_tokens: Option<u64>,
    cache_read_tokens: u64,
    elapsed_secs: u64,
    context_events: BTreeMap<String, Vec<usize>>,
    acceptance: Acceptance,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"))
        })
        .unwrap_or(default)
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(rest),
        None => PathBuf::from(path),
    }
}

fn parse_line(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|err| panic!("transcript line is not JSON: {err}"))
}

fn assistant_message(entry: &Value) -> Option<&Value> {
    let message = &entry["message"];
    (entry["type"] == "message" && message["role"] == "assistant").then_some(message)
}

fn load_source() -> Source {
    let path = expand_home(&std::env::var(TRANSCRIPT_ENV).unwrap_or_else(|_| {
        panic!("set {TRANSCRIPT_ENV} to a session transcript inside its sessions directory")
    }));
    let text =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let all: Vec<&str> = text.lines().collect();
    let cut = env_usize(CUT_LINE_ENV, all.len());
    assert!(
        (2..=all.len()).contains(&cut),
        "{CUT_LINE_ENV} must be within 2..={}",
        all.len()
    );
    let entries: Vec<Value> = all[..cut].iter().map(|line| parse_line(line)).collect();

    let header = &entries[0];
    assert_eq!(
        header["type"], "session",
        "first line must be the session header"
    );
    let session_id = header["id"].as_str().expect("header id").to_string();
    let cwd = PathBuf::from(header["cwd"].as_str().expect("header cwd"));
    let model_id = entries
        .iter()
        .rev()
        .find(|entry| entry["type"] == "model_change")
        .and_then(|entry| entry["modelId"].as_str())
        .expect("the cut transcript records the active model")
        .to_string();
    assert_eq!(
        model_id, "fcodex/gemini-4-carbon",
        "only the authorized original route may replay source data"
    );
    let plan_files = entries
        .iter()
        .filter(|entry| entry["event"] == "plan.create")
        .filter_map(|entry| entry["path"].as_str())
        .map(expand_home)
        .collect();

    let store_path = path.with_file_name("sessions.json");
    let store: Value = serde_json::from_str(
        &fs::read_to_string(&store_path)
            .unwrap_or_else(|err| panic!("read {}: {err}", store_path.display())),
    )
    .expect("sessions.json");
    let store_entry = store["sessions"][&session_id].clone();
    assert!(
        store_entry.is_object(),
        "{} has no entry for {session_id}",
        store_path.display()
    );

    let reference_prompt_tokens = all[cut..]
        .iter()
        .map(|line| parse_line(line))
        .find_map(|entry| assistant_message(&entry).map(|m| m["usage"]["prompt_tokens"].as_u64()))
        .flatten();

    Source {
        lines: all[..cut].iter().map(|line| line.to_string()).collect(),
        session_id,
        cwd,
        model_id,
        store_entry,
        plan_files,
        reference_prompt_tokens,
    }
}

/// Only the disposable fixture is migrated; production deliberately does not read this format.
fn migrate_signature_dictionary(entry: &mut Value) -> bool {
    if entry["type"] != "message" || entry["message"]["role"] != "assistant" {
        return false;
    }
    let message = &mut entry["message"];
    let continuation = &message["reasoning_continuation"];
    if continuation["format"] != "gemini_thought_signatures" {
        return false;
    }
    let Some(signatures) = continuation["opaque_payload"].as_object() else {
        return false;
    };
    let mut parts = Vec::new();
    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
        parts.push(json!({"text": text}));
    }
    let calls = message["tool_calls"]
        .as_array()
        .expect("native fixture tool calls");
    let mut first_signature = None;
    for (index, call) in calls.iter().enumerate() {
        let id = call["id"].as_str().expect("internal call id");
        let args: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().expect("arguments"))
                .expect("valid arguments");
        let mut part = json!({"functionCall": {"name": call["function"]["name"], "args": args}});
        // stream.rs generated this exact prefix + UUID only when the upstream ID was absent.
        let synthetic = id.strip_prefix("gemini_call_").is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_hexdigit())
        });
        if !synthetic {
            part["functionCall"]["id"] = json!(id);
        }
        if let Some(sig) = signatures.get(id).and_then(Value::as_str) {
            part["thoughtSignature"] = json!(sig);
            if index == 0 {
                first_signature = Some(sig.to_string());
            }
        }
        parts.push(part);
    }
    let signature = first_signature.expect("every native fixture first call has a real signature");
    assert_eq!(
        parts
            .iter()
            .find(|p| p.get("functionCall").is_some())
            .unwrap()["thoughtSignature"],
        signature
    );
    message["reasoning_continuation"]["opaque_payload"] = json!(parts);
    true
}

/// Explicit cross-channel fixture migration, never applied to new model replies.
/// Keep every part and visible field; the existing wire builder supplies its first-call sentinel.
fn drop_source_gemini_signatures(entry: &mut Value) -> usize {
    if entry["type"] != "message" || entry["message"]["role"] != "assistant" {
        return 0;
    }
    let Some(continuation) = entry
        .get_mut("message")
        .and_then(|message| message.get_mut("reasoning_continuation"))
    else {
        return 0;
    };
    if continuation["format"] != "gemini_thought_signatures" {
        return 0;
    }
    continuation
        .get_mut("opaque_payload")
        .and_then(Value::as_array_mut)
        .map(|parts| {
            parts
                .iter_mut()
                .filter_map(Value::as_object_mut)
                .filter_map(|part| part.remove("thoughtSignature"))
                .count()
        })
        .unwrap_or(0)
}

/// Seed a disposable HOME with the cut transcript, its store entry and plan files.
fn prepare_home(source: &Source, drop_signatures: bool) -> (ServeFixture, String, PathBuf, usize) {
    let fixture = setup_serve_fixture("http://127.0.0.1:1");
    let config_path = fixture.home_path.join(".tomcat/tomcat.config.toml");
    let mut config = load_config_toml_file(&config_path).expect("load fixture config");
    config.llm.title_model = None;
    config.skills.enabled = true;
    config.primitive.bash_forbidden = vec![".*".to_string()];
    common::apply_models_toml_entry_app_config(
        &mut config,
        &common::real_models_toml_path(),
        &common::real_runtime_env_path(),
        &source.model_id,
    )
    .unwrap_or_else(|err| panic!("configure `{}`: {err}", source.model_id));
    fs::write(
        &config_path,
        toml::to_string_pretty(&config).expect("serialize config"),
    )
    .expect("write config");

    let models: toml::Value = fs::read_to_string(fixture.home_path.join(".tomcat/models.toml"))
        .expect("read fixture models")
        .parse()
        .expect("parse fixture models");
    let api_key_env = models["models"][0]["api_key_env"]
        .as_str()
        .expect("copied model credential mapping")
        .to_string();

    let sessions_dir = tomcat::resolve_sessions_dir(&config).expect("sessions dir");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let transcript = sessions_dir.join(format!("{}.jsonl", source.session_id));
    let mut source_signatures_dropped = 0;
    let migrated: Vec<String> = source
        .lines
        .iter()
        .map(|line| {
            let mut entry = parse_line(line);
            migrate_signature_dictionary(&mut entry);
            if drop_signatures {
                source_signatures_dropped += drop_source_gemini_signatures(&mut entry);
            }
            serde_json::to_string(&entry).unwrap()
        })
        .collect();
    fs::write(&transcript, format!("{}\n", migrated.join("\n"))).expect("seed transcript");
    let mut entry = source.store_entry.clone();
    entry["sessionFile"] = json!(transcript.display().to_string());
    let session_key = entry["sessionKey"]
        .as_str()
        .expect("sessionKey")
        .to_string();
    fs::write(
        sessions_dir.join("sessions.json"),
        serde_json::to_string_pretty(&json!({
            "sessions": { &source.session_id: entry },
            "current": { session_key: &source.session_id },
        }))
        .unwrap(),
    )
    .expect("seed session store");

    let plans_dir = fixture.home_path.join(".tomcat/plans");
    fs::create_dir_all(&plans_dir).expect("create plans dir");
    for plan in &source.plan_files {
        let name = plan.file_name().expect("plan file name");
        fs::copy(plan, plans_dir.join(name))
            .unwrap_or_else(|err| panic!("copy plan {}: {err}", plan.display()));
    }
    (fixture, api_key_env, transcript, source_signatures_dropped)
}

fn spawn_replay(fixture: &ServeFixture, binary: &Path, cwd: &Path, key_env: &str) -> ServeChild {
    let key = std::env::var(key_env).expect("credential injected from the runtime env");
    let mut command = isolated_serve_command_with_binary(&fixture.home_path, binary);
    command
        .env(key_env, key)
        .env("RUST_LOG", "tomcat=info")
        .current_dir(cwd)
        .args(["serve", "--stdio"]);
    spawn_serve_command(command)
}

/// Resume the seeded turn, stop after `steps` completed tool rounds or the first question.
fn drive(
    child: &mut ServeChild,
    session_id: &str,
    steps: usize,
) -> (String, String, BTreeMap<String, Vec<usize>>) {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "replay-init",
        "subtype": "initialize",
        "payload": {}
    }));
    let opened = child
        .recv_until(INIT_TIMEOUT, |frame| {
            frame["type"] == "control_response" && frame["requestId"] == "replay-init"
        })
        .pop()
        .expect("initialize response");
    assert_eq!(
        opened["payload"]["sessionId"], session_id,
        "serve must reopen the seeded session: {opened}"
    );
    child.send_value(&json!({"type": "resume", "id": "replay", "sessionId": session_id}));

    let deadline = Instant::now() + RUN_TIMEOUT;
    let mut accepted = false;
    let mut completed = 0;
    let mut stop: Option<&str> = None;
    let mut context_events = BTreeMap::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "replay exceeded {RUN_TIMEOUT:?}");
        let frame = child.recv_value(remaining);
        if frame["id"] == "replay" {
            assert_eq!(frame["success"], true, "resume rejected: {frame}");
            accepted = true;
            continue;
        }
        let own = frame["sessionId"] == session_id;
        let kind = frame["type"].as_str().unwrap_or_default();
        let mut interrupt = false;
        match kind {
            "turn_end" if own => {
                completed += 1;
                interrupt = completed >= steps && stop.is_none();
                if interrupt {
                    stop = Some("step_limit");
                }
            }
            "control_request" if frame["subtype"] == "ask_question" && stop.is_none() => {
                stop = Some("ask_question");
                interrupt = true;
            }
            "agent_idle" if own && accepted => {
                let outcome = frame["outcome"].as_str().unwrap_or_default().to_string();
                return (
                    stop.unwrap_or("finished").to_string(),
                    outcome,
                    context_events,
                );
            }
            _ if own && CONTEXT_EVENTS.contains(&kind) => {
                context_events
                    .entry(kind.to_string())
                    .or_insert_with(Vec::new)
                    .push(completed);
            }
            _ => {}
        }
        if interrupt {
            child.send_value(
                &json!({"type": "interrupt", "id": "replay-stop", "sessionId": session_id}),
            );
        }
    }
}

fn read_windows(arguments: &str, cwd: &Path, result: Option<&Value>) -> Vec<ReadWindow> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let entries = args["paths"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![args]);
    let output = result
        .and_then(|r| r["content"].as_str())
        .unwrap_or_default();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let name = entry["path"].as_str()?;
            let joined = cwd.join(name);
            let path = tomcat::infra::canonicalize(&joined).unwrap_or(joined);
            let offset = entry["offset"].as_u64().unwrap_or(1).max(1);
            let limit = entry["limit"].as_u64().unwrap_or(READ_DEFAULT_LINES).max(1);
            let file_lines = fs::read_to_string(&path)
                .ok()
                .map(|s| s.lines().count() as u64);
            // Batch sections are explicit; do not mistake a skipped/failed/truncated read for EOF.
            let section = if entries.len() > 1 {
                let marker = format!("=== [{}/{}]", index + 1, entries.len());
                output
                    .split_once(&marker)
                    .map(|(_, tail)| tail.split("\n\n=== [").next().unwrap_or(tail))
                    .unwrap_or_default()
            } else {
                output
            };
            let complete = !section.is_empty()
                && ![
                    "truncated",
                    "SKIPPED",
                    "FAILED",
                    "[Error]",
                    "[Tool result persisted:",
                ]
                .iter()
                .any(|marker| section.contains(marker));
            Some(ReadWindow {
                path,
                offset,
                limit,
                file_lines,
                whole_file: complete && offset == 1 && file_lines.is_some_and(|n| limit >= n),
            })
        })
        .collect()
}

fn median(mut values: Vec<u64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] as f64 + values[middle] as f64) / 2.0
    } else {
        values[middle] as f64
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze(
    run: usize,
    new_entries: &[Value],
    steps: usize,
    stop: String,
    idle_outcome: String,
    elapsed: Duration,
    context_events: BTreeMap<String, Vec<usize>>,
    cwd: &Path,
) -> RunReport {
    let messages: Vec<&Value> = new_entries
        .iter()
        .filter_map(assistant_message)
        .filter(|message| message["error_code"].is_null())
        .take(steps)
        .collect();
    let results: BTreeMap<&str, &Value> = new_entries
        .iter()
        .map(|e| &e["message"])
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["tool_call_id"].as_str().map(|id| (id, m)))
        .collect();
    let mut report = RunReport {
        run,
        source_signatures_dropped: 0,
        stop,
        idle_outcome,
        steps: messages.len(),
        tools_per_step: Vec::new(),
        reads_per_step: Vec::new(),
        first_reads: Vec::new(),
        first_read_median: 0.0,
        single_read_steps: 0,
        longest_single_narrow_streak: 0,
        first_progress: None,
        first_prompt_tokens: messages
            .first()
            .and_then(|m| m["usage"]["prompt_tokens"].as_u64()),
        cache_read_tokens: 0,
        elapsed_secs: elapsed.as_secs(),
        context_events,
        acceptance: Acceptance {
            progress: false,
            no_narrow_chain: false,
            wide_first_reads: false,
        },
    };
    let mut streak = 0;
    let mut first_files = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        let calls = message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let names: Vec<String> = calls
            .iter()
            .map(|c| {
                c["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let windows: Vec<ReadWindow> = calls
            .iter()
            .filter(|c| c["function"]["name"] == "read")
            .flat_map(|call| {
                read_windows(
                    call["function"]["arguments"].as_str().unwrap_or("{}"),
                    cwd,
                    call["id"].as_str().and_then(|id| results.get(id).copied()),
                )
            })
            .collect();
        let single_read = names.len() == 1 && windows.len() == 1;
        report.single_read_steps += usize::from(single_read);
        streak = if single_read && windows[0].limit < 300 {
            streak + 1
        } else {
            0
        };
        report.longest_single_narrow_streak = report.longest_single_narrow_streak.max(streak);
        if report.first_progress.is_none() {
            let progress = if names.is_empty()
                && message["content"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
            {
                Some("final_reply".to_string())
            } else {
                names
                    .iter()
                    .find(|name| PROGRESS_TOOLS.contains(&name.as_str()))
                    .cloned()
            };
            report.first_progress = progress.map(|name| (index + 1, name));
        }
        for window in &windows {
            if first_files.insert(window.path.clone()) {
                report.first_reads.push(ReadWindow {
                    path: window.path.clone(),
                    offset: window.offset,
                    limit: window.limit,
                    file_lines: window.file_lines,
                    whole_file: window.whole_file,
                });
            }
        }
        report.cache_read_tokens += message["usage"]["cache_read_tokens"].as_u64().unwrap_or(0);
        report.reads_per_step.push(windows);
        report.tools_per_step.push(names);
    }
    report.first_read_median = median(report.first_reads.iter().map(|w| w.limit).collect());
    // Whole-file reads of short files count as sufficient coverage (300), not narrow failures.
    let coverage_median = median(
        report
            .first_reads
            .iter()
            .map(|w| {
                if w.whole_file {
                    w.limit.max(300)
                } else {
                    w.limit
                }
            })
            .collect(),
    );
    report.acceptance = Acceptance {
        progress: report.first_progress.is_some(),
        no_narrow_chain: report.longest_single_narrow_streak <= 2,
        wide_first_reads: !report.first_reads.is_empty() && coverage_median >= 300.0,
    };
    report
}

#[test]
fn fixture_conversion_keeps_real_signatures_and_omits_only_generated_ids() {
    let synthetic = "gemini_call_0123456789abcdef0123456789abcdef";
    let mut entry = json!({"type":"message", "message": {"role":"assistant", "content":null,
        "tool_calls":[
            {"id":synthetic,"function":{"name":"read","arguments":"{\"path\":\"a\"}"}},
            {"id":"upstream","function":{"name":"read","arguments":"{}"}}],
        "reasoning_continuation":{"format":"gemini_thought_signatures", "opaque_payload":{synthetic:"REAL"}}}});
    assert!(migrate_signature_dictionary(&mut entry));
    let parts = &entry["message"]["reasoning_continuation"]["opaque_payload"];
    assert_eq!(parts[0]["thoughtSignature"], "REAL");
    assert!(parts[0]["functionCall"].get("id").is_none());
    assert_eq!(parts[1]["functionCall"]["id"], "upstream");
    assert!(!migrate_signature_dictionary(&mut entry));
}

#[test]
fn fixture_signature_drop_preserves_every_other_field() {
    let original = json!({"type":"message", "message": {
        "role":"assistant", "content":"visible answer", "thinking_text":"summary",
        "tool_calls":[{"id":"internal","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}],
        "reasoning_continuation":{"format":"gemini_thought_signatures", "opaque_payload":[
            {"text":"summary","thought":true,"thoughtSignature":"old-thought"},
            {"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"old-call","extra":42},
            {"text":"visible answer"}, {"text":"","thoughtSignature":"old-text"}
        ]}
    }});
    let mut expected = original.clone();
    for index in [0, 1, 3] {
        expected["message"]["reasoning_continuation"]["opaque_payload"][index]
            .as_object_mut()
            .unwrap()
            .remove("thoughtSignature");
    }
    let mut migrated = original.clone();
    assert_eq!(drop_source_gemini_signatures(&mut migrated), 3);
    assert_eq!(migrated, expected);
    assert_eq!(drop_source_gemini_signatures(&mut migrated), 0);
    assert_eq!(
        original["message"]["reasoning_continuation"]["opaque_payload"][1]["thoughtSignature"],
        "old-call"
    );

    for mut untouched in [
        json!({"type":"message","message":{"role":"tool","content":"source file content"}}),
        json!({"type":"message","message":{"role":"user","content":"original task"}}),
        json!({"type":"message","message":{"role":"assistant","content":"plain reply"}}),
        json!({"type":"message","message":{"role":"assistant","reasoning_continuation":{
            "format":"anthropic_thinking_blocks","opaque_payload":[{"thoughtSignature":"foreign"}]}}}),
    ] {
        let before = untouched.clone();
        assert_eq!(drop_source_gemini_signatures(&mut untouched), 0);
        assert_eq!(untouched, before);
    }
}

#[test]
fn metrics_distinguish_first_coverage_from_rereads_and_narrow_chains() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("long.rs"), "line\n".repeat(1000)).unwrap();
    fs::write(dir.path().join("short.rs"), "line\n".repeat(50)).unwrap();
    let read = |id: usize, path: &str, limit: u64| {
        json!({"type":"message", "message":{
        "role":"assistant", "tool_calls":[{"id":id.to_string(), "function":{"name":"read",
        "arguments":json!({"path":path,"limit":limit}).to_string()}}]}})
    };
    let result = |id: usize, content: &str| {
        json!({"type":"message", "message":{
        "role":"tool", "tool_call_id":id.to_string(), "content":content}})
    };
    let entries = vec![
        read(1, "long.rs", 500),
        result(1, "lines"),
        read(2, "long.rs", 80),
        result(2, "lines"),
        read(3, "long.rs", 90),
        result(3, "lines"),
        read(4, "short.rs", 50),
        result(4, "whole file"),
        json!({"type":"message","message":{"role":"assistant","content":"done"}}),
    ];
    let report = analyze(
        1,
        &entries,
        12,
        "finished".into(),
        "success".into(),
        Duration::ZERO,
        BTreeMap::new(),
        dir.path(),
    );
    assert_eq!(report.first_reads.len(), 2);
    assert_eq!(report.first_read_median, 275.0);
    assert!(report.acceptance.wide_first_reads);
    assert!(!report.acceptance.no_narrow_chain); // 80, 90, 50 is three single narrow steps.
    assert!(report.acceptance.progress);
    let truncated = read_windows(
        r#"{"path":"short.rs","limit":50}"#,
        dir.path(),
        Some(&json!({"content":"output truncated"})),
    );
    assert!(!truncated[0].whole_file);
    assert_eq!(median(vec![100, 500]), 300.0);
    assert_eq!(
        read_windows(
            r#"{"paths":[{"path":"short.rs"}],"limit":1}"#,
            dir.path(),
            None
        )[0]
        .limit,
        2000
    );
}

#[test]
#[ignore = "manual: replays authorized source code and signatures against the original model route"]
#[serial]
fn read_guidance_replay_from_real_transcript() {
    assert_eq!(
        std::env::var("TOMCAT_REAL_LLM_E2E").as_deref(),
        Ok("1"),
        "explicit real-model opt-in required"
    );
    common::setup_logging();
    let source = load_source();
    let runs = env_usize(RUNS_ENV, DEFAULT_RUNS);
    let steps = env_usize(STEPS_ENV, DEFAULT_STEPS);
    let drop_signatures = std::env::var(DROP_SOURCE_SIGNATURES_ENV).as_deref() == Ok("1");
    assert!(
        runs >= 3 && steps >= 12,
        "acceptance needs at least 3 runs of up to 12 steps"
    );
    let artifacts = std::env::var_os(ARTIFACTS_ENV).map(PathBuf::from);
    if let Some(dir) = &artifacts {
        fs::create_dir_all(dir).expect("create artifacts dir");
    }
    let mut reports = Vec::new();
    for run in 1..=runs {
        let (fixture, key_env, transcript, source_signatures_dropped) =
            prepare_home(&source, drop_signatures);
        let mut child = spawn_replay(&fixture, &cargo_bin_path(), &source.cwd, &key_env);
        let started = Instant::now();
        let (stop, idle_outcome, context_events) = drive(&mut child, &source.session_id, steps);
        let elapsed = started.elapsed();
        let exited = child.wait_for_exit(EXIT_TIMEOUT);
        assert!(
            exited.status.success(),
            "run {run} serve exit: {:?}",
            exited.status
        );
        let text = fs::read_to_string(&transcript).expect("read replayed transcript");
        let new_lines: Vec<&str> = text.lines().skip(source.lines.len()).collect();
        if let Some(dir) = &artifacts {
            fs::write(
                dir.join(format!("read-replay-{run}.jsonl")),
                format!("{}\n", new_lines.join("\n")),
            )
            .expect("write run transcript");
        }
        let new_entries: Vec<Value> = new_lines.iter().map(|line| parse_line(line)).collect();
        let mut report = analyze(
            run,
            &new_entries,
            steps,
            stop,
            idle_outcome,
            elapsed,
            context_events,
            &source.cwd,
        );
        report.source_signatures_dropped = source_signatures_dropped;
        println!(
            "READ_REPLAY_RUN {}",
            serde_json::to_string(&report).unwrap()
        );
        // Preserve all completed runs even when model behavior does not meet acceptance.
        reports.push(report);
        if let Some(dir) = &artifacts {
            fs::write(
                dir.join("read-replay-report.json"),
                serde_json::to_string_pretty(&reports).unwrap(),
            )
            .expect("write report");
        }
        let report = reports.last().unwrap();
        assert!(
            report.steps > 0,
            "run {run} completed no step; provider/environment blocked"
        );
        let actual = report
            .first_prompt_tokens
            .expect("first request usage missing");
        let expected = source
            .reference_prompt_tokens
            .filter(|n| *n > 0)
            .expect("source request usage missing");
        let drift = (actual as f64 - expected as f64).abs() / expected as f64;
        assert!(
            drift <= MAX_CONTEXT_DRIFT,
            "run {run}: {actual} vs source {expected} tokens; context not reproduced"
        );
    }
    let passed = reports.iter().filter(|r| r.acceptance.passed()).count();
    println!("READ_REPLAY_ACCEPTANCE {passed}/{runs} runs passed all three criteria");
    assert!(passed >= 2,"reading acceptance failed; inspect each run's progress, narrow chain and first-read coverage");
}
