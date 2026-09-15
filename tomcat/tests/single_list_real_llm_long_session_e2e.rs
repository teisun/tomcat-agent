//! Opt-in production-path verification for the single-list context ownership model.
//!
//! This test deliberately uses `tomcat serve --stdio`: an `AgentLoop::run` seam cannot verify
//! the `run_loop` take/park handoff, timing-② application, transcript persistence, or serve's
//! `TurnStateLease`. It is ignored because it consumes real model tokens.

mod common;

use std::collections::HashSet;
use std::fs;
use std::time::Duration;

use common::serve::{setup_serve_fixture, spawn_serve_child, ServeChild, ServeFixture};
use serde_json::{json, Value};
use serial_test::serial;
use tomcat::load_config_toml_file;

const MODEL_ENV: &str = "TOMCAT_E2E_GUARD_REAL_MODEL";
const DEFAULT_MODEL: &str = "fcodex/gpt-5.6-terra";
const TURN_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_WARMUP_TURNS: usize = 8;

fn selected_model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn configure_real_model(fixture: &ServeFixture, model_id: &str) -> String {
    let models_toml = common::real_models_toml_path();
    let runtime_env = common::real_runtime_env_path();
    let config_path = fixture.home_path.join(".tomcat/tomcat.config.toml");
    let mut config = load_config_toml_file(&config_path).expect("load generated test config");
    config.storage.work_dir = Some(fixture.home_path.join(".tomcat").display().to_string());
    config.llm.default_model = model_id.to_string();
    config.llm.title_model = None;
    config.context.compaction_model = model_id.to_string();
    config.context.context_window_fallback = 40_000;
    config.context.output_reserve_tokens = Some(8_000);
    config.skills.enabled = false;

    let wire_model = common::apply_models_toml_entry_app_config(
        &mut config,
        &models_toml,
        &runtime_env,
        model_id,
    )
    .unwrap_or_else(|error| panic!("configure real model `{model_id}`: {error}"));

    // Keep the real endpoint, provider, capability and credential mapping verbatim, but make
    // the context small enough to exercise compaction within a bounded local test budget.
    let fixture_models = fixture.home_path.join(".tomcat/models.toml");
    let mut doc: toml::Value = fs::read_to_string(&fixture_models)
        .expect("read fixture models")
        .parse()
        .expect("parse fixture models");
    let model = doc["models"]
        .as_array_mut()
        .and_then(|models| models.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("one copied model entry");
    model.insert("context_window".to_string(), toml::Value::Integer(40_000));
    model.insert(
        "context_window_options".to_string(),
        toml::Value::Array(vec![toml::Value::Integer(40_000)]),
    );
    model.insert("max_output_tokens".to_string(), toml::Value::Integer(8_000));
    fs::write(
        &fixture_models,
        toml::to_string(&doc).expect("serialize constrained model"),
    )
    .expect("write constrained model");
    fs::write(
        config_path,
        toml::to_string_pretty(&config).expect("serialize constrained config"),
    )
    .expect("write constrained config");
    wire_model
}

fn initialize(child: &mut ServeChild) -> String {
    child.send_value(&json!({
        "type": "control_request",
        "requestId": "single-list-init",
        "subtype": "initialize",
        "payload": {}
    }));
    child
        .recv_until(TURN_TIMEOUT, |frame| {
            frame["type"].as_str() == Some("control_response")
                && frame["requestId"].as_str() == Some("single-list-init")
        })
        .last()
        .expect("initialize response")["payload"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string()
}

fn run_prompt(
    child: &mut ServeChild,
    session_id: &str,
    id: &str,
    text: impl Into<String>,
) -> Vec<Value> {
    child.send_value(&json!({
        "type": "prompt",
        "id": id,
        "sessionId": session_id,
        "text": text.into(),
        "params": {}
    }));
    child.recv_until(TURN_TIMEOUT, |frame| {
        frame["type"].as_str() == Some("agent_idle")
            && frame["sessionId"].as_str() == Some(session_id)
    })
}

fn event_count(frames: &[Value], event: &str) -> usize {
    frames
        .iter()
        .filter(|frame| frame["type"].as_str() == Some(event))
        .count()
}

fn transcript_entries(fixture: &ServeFixture, session_id: &str) -> Vec<Value> {
    let config = load_config_toml_file(&fixture.home_path.join(".tomcat/tomcat.config.toml"))
        .expect("read fixture config");
    let path = tomcat::resolve_sessions_dir(&config)
        .expect("resolve sessions directory")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .expect("read session transcript")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse transcript row"))
        .collect()
}

fn get_messages(child: &mut ServeChild, session_id: &str) -> Vec<Value> {
    child.send_value(&json!({
        "type": "get_messages",
        "id": "single-list-messages",
        "sessionId": session_id,
        "params": { "limit": 512 }
    }));
    child
        .recv_until(Duration::from_secs(30), |frame| {
            frame["id"].as_str() == Some("single-list-messages")
        })
        .into_iter()
        .find(|frame| frame["id"].as_str() == Some("single-list-messages"))
        .and_then(|frame| frame["payload"]["messages"].as_array().cloned())
        .expect("get_messages payload")
}

#[test]
#[ignore = "manual: real 40K serve session validates the single-list refactor"]
#[serial]
fn real_terra_long_session_exercises_single_list_handoffs() {
    assert_eq!(
        std::env::var("TOMCAT_REAL_LLM_E2E").as_deref(),
        Ok("1"),
        "set TOMCAT_REAL_LLM_E2E=1 to acknowledge real-model cost"
    );
    common::setup_logging();
    let model_id = selected_model();
    let fixture = setup_serve_fixture("http://127.0.0.1:1");
    let wire_model = configure_real_model(&fixture, &model_id);

    for index in 1..=12 {
        let line = format!("SENTINEL-{index:02} durable file index\n");
        fs::write(
            fixture.workspace.join(format!("big-{index:02}.txt")),
            line.repeat(1_100),
        )
        .expect("write long context fixture");
    }

    let mut child = spawn_serve_child(&fixture);
    let session_id = initialize(&mut child);
    let mut all_frames = Vec::new();
    let mut marker_seen = false;
    let mut switched_seen = false;
    let mut turns = 0usize;

    // A/B: let the model create enough tool-result context to start preheat, then keep driving
    // until the ready summary is applied on either timing-② or mid-turn.
    for index in 1..=MAX_WARMUP_TURNS {
        let frames = run_prompt(
            &mut child,
            &session_id,
            &format!("single-list-read-{index}"),
            format!(
                "Read big-{index:02}.txt with exactly one read tool call. Give one short sentence \
                 containing SENTINEL-{index:02}; do not read another file."
            ),
        );
        marker_seen |= event_count(&frames, "auto_compaction_start") > 0;
        switched_seen |= event_count(&frames, "boundary_switched") > 0;
        turns += 1;
        all_frames.extend(frames);
        if marker_seen && switched_seen {
            break;
        }
    }
    assert!(
        marker_seen,
        "expected the 0.50 preheat watermark within {MAX_WARMUP_TURNS} turns"
    );

    if !switched_seen {
        let frames = run_prompt(
            &mut child,
            &session_id,
            "single-list-apply",
            "Read big-09.txt with exactly one read tool call and answer in one sentence.",
        );
        switched_seen = event_count(&frames, "boundary_switched") > 0;
        turns += 1;
        all_frames.extend(frames);
    }
    assert!(
        switched_seen,
        "a ready preheat must switch without apply_boundary_stale"
    );

    // C: original incident shape — several tool results in the same turn.
    let frames = run_prompt(
        &mut child,
        &session_id,
        "single-list-midturn",
        "Read big-09.txt, big-10.txt, big-11.txt, then big-12.txt in that order. Use exactly \
         one read call per assistant response, do not batch them, then provide a brief list of \
         their SENTINEL labels.",
    );
    turns += 1;
    all_frames.extend(frames);

    // D: command path must reload from the parked list and leave the next prompt usable.
    child.send_value(&json!({
        "type": "compact",
        "id": "single-list-compact",
        "sessionId": session_id,
    }));
    let compact_frames = child.recv_until(TURN_TIMEOUT, |frame| {
        frame["id"].as_str() == Some("single-list-compact")
    });
    let compact_response = compact_frames
        .iter()
        .find(|frame| frame["success"].as_bool() == Some(true))
        .unwrap_or_else(|| panic!("manual compact must respond successfully: {compact_frames:?}"));
    let before_ratio = compact_response["payload"]["beforeUsageRatio"]
        .as_f64()
        .expect("compact before ratio");
    let after_ratio = compact_response["payload"]["afterUsageRatio"]
        .as_f64()
        .expect("compact after ratio");
    assert!(
        after_ratio < before_ratio,
        "manual compact must lower the rehydrated ratio: {before_ratio} -> {after_ratio}"
    );
    all_frames.extend(compact_frames);
    let frames = run_prompt(
        &mut child,
        &session_id,
        "single-list-after-compact",
        "Which SENTINEL file labels have you read? Answer briefly.",
    );
    turns += 1;
    all_frames.extend(frames);

    // E: send a real interrupt only after streaming began, then prove the parked list is usable.
    child.send_value(&json!({
        "type": "prompt",
        "id": "single-list-interrupt-prompt",
        "sessionId": session_id,
        "text": "Count from 1 to 2000, one number per line."
    }));
    let mut interrupted_frames = child.recv_until(TURN_TIMEOUT, |frame| {
        frame["type"].as_str() == Some("message_update")
    });
    child.send_value(&json!({
        "type": "interrupt",
        "id": "single-list-interrupt",
        "sessionId": session_id,
    }));
    interrupted_frames.extend(child.recv_until(TURN_TIMEOUT, |frame| {
        frame["type"].as_str() == Some("agent_idle")
            && frame["sessionId"].as_str() == Some(&session_id)
    }));
    assert!(
        event_count(&interrupted_frames, "agent_interrupted") == 1,
        "streaming interrupt must settle exactly once"
    );
    all_frames.extend(interrupted_frames);
    let frames = run_prompt(
        &mut child,
        &session_id,
        "single-list-after-interrupt",
        "Name at least one big-NN.txt file you read earlier.",
    );
    turns += 1;
    all_frames.extend(frames);

    // F: verify durable shape independently of model prose.
    assert_eq!(
        event_count(&all_frames, "compaction_error"),
        0,
        "single-list path must not surface a compaction error"
    );
    assert!(
        !child.stderr().contains("apply_boundary_stale"),
        "no stale boundary is permitted in production serve stderr: {}",
        child.stderr()
    );
    let entries = transcript_entries(&fixture, &session_id);
    let marker_ids: HashSet<_> = entries
        .iter()
        .filter(|entry| entry["type"].as_str() == Some("branch_summary"))
        .filter(|entry| entry["isBoundary"].as_bool() == Some(true))
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect();
    let body_ids: HashSet<_> = entries
        .iter()
        .filter(|entry| entry["type"].as_str() == Some("branch_summary_text"))
        .filter_map(|entry| entry["forId"].as_str().map(str::to_owned))
        .collect();
    assert!(
        !marker_ids.is_empty(),
        "the preheat start event must have created a durable boundary marker"
    );
    let unmatched_markers = marker_ids
        .iter()
        .filter(|id| !body_ids.contains(*id))
        .count();
    assert!(
        unmatched_markers <= 1,
        "every marker except at most the final pending one needs one body: markers={marker_ids:?}, bodies={body_ids:?}"
    );
    let messages = get_messages(&mut child, &session_id);
    let ids: Vec<_> = messages
        .iter()
        .filter_map(|message| message["id"].as_str())
        .collect();
    assert_eq!(
        ids.len(),
        ids.iter().copied().collect::<HashSet<_>>().len(),
        "serve message list must not contain duplicate durable ids"
    );
    println!(
        "phase=\"single_list_real_llm\" model={model_id} wire_model={wire_model} \
         turns={turns} boundary_switched={} markers={} bodies={}",
        event_count(&all_frames, "boundary_switched"),
        marker_ids.len(),
        body_ids.len(),
    );
}
