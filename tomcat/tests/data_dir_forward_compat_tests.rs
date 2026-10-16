//! Immutable old-version data, produced through the tagged executable's real persistence paths.
//!
//! Generation (one time, not part of ordinary test runs):
//! 1. `git worktree add --detach /tmp/tomcat-upgrade-baseline-20261015 ext-v0.1.90`
//! 2. Build its tomcat/Cargo.toml with `cargo build --locked --release --bin tomcat`.
//! 3. Set TOMCAT_LEGACY_FIXTURE_BINARY to that absolute binary and run
//!    `cargo test --test data_dir_forward_compat_tests generate_legacy_data -- --ignored`.
//!
//! The tag is d00ebe085e2bb23b8dc9e6f395439338e730a0c4 (CLI 0.1.74). Only fake keys
//! and loopback HTTP are used. Existing captures are never overwritten. No current AppConfig
//! serialization participates in generation. Tests relocate HOME strings and the mock port only
//! in disposable copies; checked-in bytes remain the original CLI output.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::serve::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const WAIT: Duration = Duration::from_secs(20);
const MODEL: &str = "gpt-5.4";
const LEGACY_REPLY: &str = "legacy fixture conversation";
const RETIRED_BUILTINS: [&str; 9] = [
    "gpt-5.2",
    "gpt-5.4",
    "gpt-5.5",
    "utility-flash",
    "mimo-v2.5-pro",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
];
const NEW_BUILTINS: [&str; 4] = [
    "openai/gpt-6-astra",
    "openai/gpt-6.1-sol",
    "mimo/mimo-v2.6-pro",
    "anthropic/claude-opus-5-5",
];
const ORPHAN_OVERRIDE: &str = "claude-opus-4-1";

fn model_ids(models: &Value) -> Vec<String> {
    models["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_string())
        .collect()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/data-dir-v0.1.90")
}

fn isolated_command(home: &Path, workspace: &Path, binary: &Path) -> Command {
    let mut command = isolated_serve_command_with_binary(home, binary);
    command
        .current_dir(workspace)
        .env("TOMCAT_SESSION_MODE", "claw")
        .env("TOMCAT__CONNECTOR__ENABLED", "false")
        .env("TOMCAT__LLM__TITLE_MODEL", MODEL)
        .env("TOMCAT__CONTEXT__COMPACTION_MODEL", MODEL);
    command
}

fn initialize(child: &mut ServeChild) -> Value {
    child.send_value(&json!({"type":"control_request", "requestId":"initialize", "subtype":"initialize", "payload":{}}));
    child
        .recv_until(WAIT, |v| v["requestId"] == "initialize")
        .last()
        .unwrap()["payload"]
        .clone()
}

fn request(child: &mut ServeChild, mut command: Value) -> Value {
    command["id"] = json!("operation");
    child.send_value(&command);
    let frames = child.recv_until(WAIT, |v| v["id"] == "operation");
    let reply = frames.last().unwrap();
    assert_eq!(reply["success"], true, "{reply}");
    reply["payload"].clone()
}

fn local_model(child: &mut ServeChild, url: &str) {
    request(
        child,
        json!({"type":"upsert_model", "model":{
            "id":MODEL, "modelName":MODEL, "api":"openai", "provider":"openai",
            "baseUrl":url, "apiKeyEnv":"OPENAI_API_KEY",
            "capabilities":{"vision":false,"files":false,"tools":true,"reasoning":false,"web_search":false}
        }}),
    );
}

fn prompt(child: &mut ServeChild, session: &str, text: &str) {
    child.send_value(
        &json!({"type":"prompt", "id":"prompt", "sessionId":session, "text":text, "params":{}}),
    );
}

fn copy_capture(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            copy_capture(&path, &target.join(path.file_name().unwrap()));
        } else if !path.to_string_lossy().ends_with(".lock") {
            fs::copy(&path, target.join(path.file_name().unwrap())).unwrap();
        }
    }
}

#[test]
#[ignore = "one-time capture requires an explicitly supplied binary built from ext-v0.1.90"]
fn generate_legacy_data() {
    let binary = PathBuf::from(
        std::env::var_os("TOMCAT_LEGACY_FIXTURE_BINARY").expect("historical binary required"),
    );
    assert!(binary.is_absolute());
    let binary_hash = format!("{:x}", Sha256::digest(fs::read(&binary).unwrap()));
    for variant in ["cli-init", "plugin-only"] {
        let output = fixtures().join(variant);
        assert!(
            !output.exists(),
            "never replace an immutable fixture: {}",
            output.display()
        );
        let fx = setup_cold_serve_fixture(false);
        let old_version = isolated_command(&fx.home_path, &fx.workspace, &binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(old_version.status.success());
        assert!(String::from_utf8_lossy(&old_version.stdout).contains("0.1.74"));
        if variant == "cli-init" {
            let init = isolated_command(&fx.home_path, &fx.workspace, &binary)
                .arg("init")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(init.status.success(), "{init:?}");
        }
        let create = json!({"goal":"Legacy upgrade fixture plan", "draft":"Preserve old user data after an upgrade.", "todos":[{"id":"check-old-data","content":"Check old user data"}]}).to_string();
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![
            response(vec![
                sse_delta(LEGACY_REPLY),
                sse_finish("stop"),
                sse_done(),
            ]),
            response(vec![
                sse_tool_call("legacy-plan", "create_plan", &create),
                sse_finish("tool_calls"),
                sse_done(),
            ]),
            response(vec![
                sse_delta("legacy plan saved"),
                sse_finish("stop"),
                sse_done(),
            ]),
        ]);
        let mut command = isolated_command(&fx.home_path, &fx.workspace, &binary);
        command.args(["serve", "--stdio"]);
        let mut child = spawn_serve_command(command);
        let init = initialize(&mut child);
        let session = init["sessionId"].as_str().unwrap().to_string();
        request(
            &mut child,
            json!({"type":"set_provider_key", "envName":"OPENAI_API_KEY", "value":"legacy-fixture-not-real"}),
        );
        request(
            &mut child,
            json!({"type":"set_default_model", "modelId":MODEL}),
        );
        let mut plan_id = None;
        if variant == "cli-init" {
            local_model(&mut child, &server.base_url);
            prompt(&mut child, &session, "Record a local fixture conversation.");
            let frames = child.recv_until(WAIT, |v| v["type"] == "agent_idle");
            assert!(
                frames
                    .iter()
                    .any(|v| v["assistantMessageEvent"]["delta"] == LEGACY_REPLY),
                "{frames:?}"
            );
            request(
                &mut child,
                json!({"type":"set_plan_mode", "sessionId":session, "action":"enter"}),
            );
            prompt(
                &mut child,
                &session,
                "Save a fixture plan; do not implement it.",
            );
            let frames = child.recv_until(WAIT, |v| {
                v["type"] == "control_request" && v["subtype"] == "ask_question"
            });
            let question = frames.last().unwrap();
            child.send_value(&json!({"type":"control_response", "requestId":question["requestId"], "sessionId":session, "payload":{
                "answers":[{"question_id":"plan-review","option_ids":["skip"],"skipped":false,"picked_recommended":false}],
                "outcome":"answered", "cancelled":false
            }}));
            child.recv_until(WAIT, |v| v["type"] == "agent_idle");
            let plans = fs::read_dir(fx.home_path.join(".tomcat/plans"))
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".plan.md"))
                .collect::<Vec<_>>();
            assert_eq!(plans.len(), 1);
            plan_id = Some(
                plans[0]
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim_end_matches(".plan.md")
                    .to_string(),
            );
        }
        assert!(child.wait_for_exit(WAIT).status.success());
        let data = fx.home_path.join(".tomcat");
        fs::create_dir_all(&output).unwrap();
        for name in ["tomcat.config.toml", "models.toml"] {
            if data.join(name).exists() {
                fs::copy(data.join(name), output.join(name)).unwrap();
            }
        }
        for name in ["assets", "agents/main/sessions", "plans"] {
            if data.join(name).exists() {
                copy_capture(&data.join(name), &output.join(name));
            }
        }
        if variant == "cli-init" {
            let models: toml::Value = fs::read_to_string(output.join("models.toml"))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(models["models"].as_array().unwrap().len(), 18);
        } else {
            assert!(!output.join("models.toml").exists());
        }
        fs::write(output.join("fixture-info.json"), serde_json::to_string_pretty(&json!({
            "tag":"ext-v0.1.90", "commit":"d00ebe085e2bb23b8dc9e6f395439338e730a0c4", "binarySha256":binary_hash,
            "home":fx.home_path, "baseUrl":server.base_url, "sessionId":session, "planId":plan_id
        })).unwrap()).unwrap();
    }
}

fn copy_relocated(source: &Path, destination: &Path, home: &str, info: &Value, url: &str) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap();
        if name == "fixture-info.json" {
            continue;
        }
        if path.is_dir() {
            copy_relocated(&path, &destination.join(name), home, info, url);
        } else {
            let raw = fs::read_to_string(&path).unwrap();
            let relocated = raw
                .replace(info["home"].as_str().unwrap(), home)
                .replace(info["baseUrl"].as_str().unwrap(), url);
            fs::write(destination.join(name), relocated).unwrap();
        }
    }
}

#[test]
fn legacy_data_directories_remain_usable_after_upgrade() {
    for variant in ["cli-init", "plugin-only"] {
        let source = fixtures().join(variant);
        let info: Value = serde_json::from_slice(
            &fs::read(source.join("fixture-info.json")).expect("checked-in legacy fixture"),
        )
        .unwrap();
        let fx = setup_cold_serve_fixture(false);
        let server = spawn_scripted_openai_stream_server_with_auto_title(vec![response(vec![
            sse_delta("upgraded local chat"),
            sse_finish("stop"),
            sse_done(),
        ])]);
        let data = fx.home_path.join(".tomcat");
        copy_relocated(
            &source,
            &data,
            fx.home_path.to_str().unwrap(),
            &info,
            &server.base_url,
        );
        if variant == "cli-init" {
            let mut models = fs::read_to_string(data.join("models.toml")).unwrap();
            models.push_str(&format!(
                "\n[[models]]\nid = \"{ORPHAN_OVERRIDE}\"\nbase_url = \"https://gateway.example.test\"\n"
            ));
            fs::write(data.join("models.toml"), models).unwrap();
        }
        let config_before = fs::read(data.join("tomcat.config.toml")).unwrap();
        let models_before = fs::read(data.join("models.toml")).ok();
        let mut command = isolated_command(&fx.home_path, &fx.workspace, &cargo_bin_path());
        command.args(["serve", "--stdio"]);
        let mut child = spawn_serve_command(command);
        let session = initialize(&mut child)["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(session, info["sessionId"].as_str().unwrap());
        let models = request(&mut child, json!({"type":"list_models"}));
        assert_eq!(models["defaultModelId"], MODEL);
        assert_eq!(
            fs::read(data.join("tomcat.config.toml")).unwrap(),
            config_before
        );
        assert_eq!(fs::read(data.join("models.toml")).ok(), models_before);
        if variant == "cli-init" {
            let persisted: toml::Value = fs::read_to_string(data.join("models.toml"))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(persisted["models"].as_array().unwrap().len(), 19);
            let ids = model_ids(&models);
            assert!(ids.iter().any(|id| id == MODEL), "{ids:?}");
            assert_eq!(ids.iter().filter(|id| *id == "gpt-5.6").count(), 1);
            for id in NEW_BUILTINS {
                assert!(ids.iter().any(|known| known == id), "{id} missing: {ids:?}");
            }
            assert!(!ids.iter().any(|id| id == ORPHAN_OVERRIDE));
            let warnings = models["warnings"].as_array().expect("orphan diagnostics");
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].as_str().unwrap().contains(ORPHAN_OVERRIDE));
            let messages = request(
                &mut child,
                json!({"type":"get_messages", "sessionId":session}),
            );
            assert!(messages.to_string().contains(LEGACY_REPLY), "{messages}");
            let plans = request(&mut child, json!({"type":"get_state", "sessionId":session}));
            assert!(
                plans.to_string().contains(info["planId"].as_str().unwrap()),
                "{plans}"
            );
        } else {
            let ids = model_ids(&models);
            assert_eq!(ids.len(), 13, "{ids:?}");
            for retired in RETIRED_BUILTINS {
                assert!(!ids.iter().any(|id| id == retired), "{retired}: {ids:?}");
            }
            let saved = models["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|model| model["id"] == "gpt-5.6")
                .unwrap();
            assert_eq!(saved["keySource"], "saved", "{saved}");
            let state = request(&mut child, json!({"type":"get_state", "sessionId":session}));
            assert_eq!(state["modelReady"], false, "a retired default asks to reselect: {state}");
            // Only after proving unmodified cold loading, route the builtin through the local
            // mock using the same Settings command; never send the stored fake key off-host.
            local_model(&mut child, &server.base_url);
            let state = request(&mut child, json!({"type":"get_state", "sessionId":session}));
            assert_eq!(state["modelReady"], true, "{state}");
        }
        let models_before_chat = fs::read(data.join("models.toml")).unwrap();
        prompt(&mut child, &session, "Continue locally after upgrading.");
        let frames = child.recv_until(WAIT, |v| v["type"] == "agent_idle");
        assert!(
            frames
                .iter()
                .any(|v| v["assistantMessageEvent"]["delta"] == "upgraded local chat"),
            "{frames:?}"
        );
        let captured = captured_non_title_requests(&server);
        assert_eq!(captured.len(), 1);
        assert_eq!(extract_json_body(&captured[0])["model"], MODEL);
        assert_eq!(
            fs::read(data.join("tomcat.config.toml")).unwrap(),
            config_before
        );
        assert_eq!(
            fs::read(data.join("models.toml")).unwrap(),
            models_before_chat
        );
        assert!(child.wait_for_exit(WAIT).status.success());
    }
}
