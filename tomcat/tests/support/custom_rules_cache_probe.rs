//! Bounded manual probe: real project rules -> production prompt snapshot -> provider usage.
//! Reuses the parent probe's transport helpers, but never executes returned tools.
//! Run only this paid case with TOMCAT_E2E_CACHE_PROBE_MODEL set to a models.toml ID:
//! cargo test --test prompt_cache_real_llm_tests custom_rules_cache_probe::project_rules_cache_ab -- --ignored --exact --nocapture

use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tomcat::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};
use tomcat::core::project_instructions::{discover, read_body, render_rules};

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;
const BUDGET: usize = 400_000;
const STABLE_ROUNDS: usize = 4;
const CHANGED_ROUNDS: usize = 3;

fn gate(root: &Path) -> DefaultPermissionGate {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: root.to_path_buf(),
            workspace_roots: vec![root.to_path_buf()],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
}

fn rendered_rules(root: &Path) -> ProbeResult<String> {
    let permission = gate(root);
    let discovery = discover(root, ".agents", true, &permission);
    let (text, diagnostics, loaded) = render_rules(&discovery, root, &permission, BUDGET);
    if loaded == 0 || !diagnostics.is_empty() {
        return Err(format!("rules probe requires all fixture rules to load: loaded={loaded}, diagnostics={diagnostics:?}").into());
    }
    Ok(text)
}

fn copy_project_rules(source: &Path, target: &Path) -> ProbeResult<Vec<PathBuf>> {
    let permission = gate(source);
    let discovery = discover(source, ".agents", true, &permission);
    if !discovery.diagnostics.is_empty() {
        return Err(format!("rule discovery failed: {:?}", discovery.diagnostics).into());
    }
    let mut paths = Vec::new();
    for file in discovery.files.iter().filter(|file| file.always_apply) {
        let body = read_body(source, &file.file_path, &permission)?;
        let header = serde_yaml::to_string(&serde_json::json!({
            "description": file.card.description, "alwaysApply": true
        }))?;
        let target_file = target.join(&file.card.path);
        fs::create_dir_all(target_file.parent().ok_or("rule has no parent")?)?;
        fs::write(&target_file, format!("---\n{header}---\n{body}"))?;
        paths.push(target_file);
    }
    if paths.is_empty() {
        return Err("no active project rules; refusing to measure an empty rules section".into());
    }
    let (expected, _, loaded) = render_rules(&discovery, source, &permission, BUDGET);
    assert_eq!(
        loaded,
        paths.len(),
        "active project rules must fit the probe budget"
    );
    assert_eq!(
        rendered_rules(target)?,
        expected,
        "copied rules must render byte-for-byte identically"
    );
    Ok(paths)
}

fn fingerprint(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn append_history(history: &mut Vec<ChatMessage>, round: usize) {
    // Identical append-only history in both arms, not random model output or real tool IO.
    let id = format!("rules-cache-call-{round}");
    history.push(ChatMessage::assistant_with_tool_calls(
        None,
        vec![serde_json::json!({
            "id": id, "type": "function",
            "function": {"name": "config_get", "arguments": "{\"key\":\"agent.id\"}"}
        })],
    ));
    history.push(ChatMessage::tool(&id, "\"cache-probe\""));
    history.push(ChatMessage::user(format!(
        "Measurement round {}. Reply with exactly OK, without any tool call.",
        round + 1
    )));
}

async fn measure(
    resolved: &tomcat::ResolvedCall,
    snapshot: &SystemPromptSnapshot,
    history: &[ChatMessage],
    cohort: &str,
    phase: &str,
    round: usize,
) -> ProbeResult<TokenUsage> {
    // A fixed per-arm early marker reduces cross-warming. Shared tool prefixes can still hit;
    // the first request is not asserted to be a guaranteed cold miss.
    let system = format!(
        "Cache measurement cohort: {cohort}\n\n{}",
        snapshot.system_text()
    );
    let mut messages = vec![ChatMessage::system(system)];
    messages.extend_from_slice(history);
    let mut tail =
        ChatMessage::user("Cache measurement only: reply exactly OK; do not invoke tools.");
    tail.kind = MessageKind::EphemeralTail;
    messages.push(tail);
    let mut probe = request(messages, &resolved.model, cohort);
    probe.stream = Some(true);
    probe.temperature = None;
    probe.max_tokens = if resolved.api == "openai-responses" {
        None
    } else {
        Some(64)
    };
    probe.tools = Some(snapshot.tool_definitions().to_vec());
    let started = std::time::Instant::now();
    let captured = tokio::time::timeout(
        REQUEST_TIMEOUT,
        capture_stream_turn(resolved.provider_impl.as_ref(), probe),
    )
    .await
    .map_err(|_| "complete rules-cache request exceeded 120 seconds")??;
    let usage = captured.usage;
    let cached = usage
        .cache_read_tokens
        .ok_or("provider omitted cache_read_tokens; this is not a measured zero")?;
    if usage.prompt_tokens == 0 || cached > usage.prompt_tokens {
        return Err(format!("invalid cache accounting: {usage:?}").into());
    }
    eprintln!(
        "RULES_CACHE_ROW {}",
        serde_json::json!({
            "model": resolved.catalog_id, "api": resolved.api, "phase": phase, "round": round,
            "prompt_sha256": fingerprint(snapshot.system_text()),
            "prompt_tokens": usage.prompt_tokens, "cache_read_tokens": cached,
            "cache_write_tokens": usage.cache_write_tokens, "completion_tokens": usage.completion_tokens,
            "hit_rate": cached as f64 / usage.prompt_tokens as f64,
            "elapsed_ms": started.elapsed().as_millis(),
            "returned_tool_calls": captured.assistant.tool_calls.as_ref().map_or(0, Vec::len)
        })
    );
    Ok(usage)
}

#[test]
fn rendered_rules_keep_snapshot_stable_until_body_changes() -> ProbeResult<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let rules = root.join(".cursor/rules/example.mdc");
    fs::create_dir_all(rules.parent().unwrap())?;
    let original =
        "---\nalwaysApply: true\ndescription: cache regression\n---\nUse concise answers.\n";
    fs::write(&rules, original)?;
    let context = main_agent_context();
    let surface = ToolSurface::from_plugin_tools(false, &[]);
    let initial = rendered_rules(&root)?;
    let mut snapshot = SystemPromptSnapshot::new(&context, &surface, None, None, BUDGET, &initial);
    let before = snapshot.system_text().to_owned();
    fs::write(&rules, original)?; // mtime changes must not change rendered bytes or cache identity.
    assert!(!snapshot.refresh(
        &context,
        &surface,
        None,
        None,
        BUDGET,
        &rendered_rules(&root)?
    ));
    assert_eq!(snapshot.system_text(), before);
    fs::write(&rules, original.replace("concise", "precise"))?;
    assert!(snapshot.refresh(
        &context,
        &surface,
        None,
        None,
        BUDGET,
        &rendered_rules(&root)?
    ));
    assert!(snapshot.system_text().contains("Use precise answers."));
    assert!(!snapshot.system_text().contains("Use concise answers."));
    assert!(!snapshot.refresh(
        &context,
        &surface,
        None,
        None,
        BUDGET,
        &rendered_rules(&root)?
    ));
    assert!(snapshot.refresh(&context, &surface, None, None, BUDGET, ""));
    assert!(!snapshot.system_text().contains("User Custom Instructions"));
    Ok(())
}

#[tokio::test]
#[ignore = "manual paid probe: 4 baseline + 4 real-rule + 3 changed-rule requests; set TOMCAT_E2E_CACHE_PROBE_MODEL"]
#[serial]
async fn project_rules_cache_ab() -> ProbeResult<()> {
    common::load_openai_test_env();
    let model = std::env::var(common::CACHE_PROBE_MODEL_ENV)
        .map_err(|_| "set TOMCAT_E2E_CACHE_PROBE_MODEL to a models.toml model ID")?;
    let models_path = common::real_models_toml_path();
    let env_path = common::real_runtime_env_path();
    let _home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("rules-cache")
            .display()
            .to_string(),
    );
    common::apply_models_toml_entry_app_config(&mut cfg, &models_path, &env_path, &model)?;
    cfg.llm.retry_count = 0;
    let resolved = common::resolve_main_call(&cfg);
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("missing repo root")?
        .canonicalize()?;
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let paths = copy_project_rules(&source, &root)?;
    let rules = rendered_rules(&root)?;
    let context = main_agent_context();
    let surface = ToolSurface::from_plugin_tools(false, &[]);
    let mut snapshots = [
        SystemPromptSnapshot::new(&context, &surface, None, None, BUDGET, ""),
        SystemPromptSnapshot::new(&context, &surface, None, None, BUDGET, &rules),
    ];
    assert_eq!(
        snapshots[0].tool_definitions(),
        snapshots[1].tool_definitions()
    );
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let cohorts = [
        format!("rules-cache:{nonce}:off"),
        format!("rules-cache:{nonce}:on"),
    ];
    let mut history = vec![ChatMessage::user(
        "This is a cache measurement, not a coding task. Reply exactly OK; do not invoke tools.",
    )];
    eprintln!(
        "RULES_CACHE_SETUP {}",
        serde_json::json!({
            "model": model, "api": resolved.api, "nonce": nonce,
            "rule_sources": paths.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string()).collect::<Vec<_>>(),
            "rules_chars": rules.chars().count(), "rules_sha256": fingerprint(&rules),
            "system_chars_off": snapshots[0].system_text().chars().count(),
            "system_chars_on": snapshots[1].system_text().chars().count(),
            "tools": snapshots[0].tool_definitions().len(), "requests": 11, "inter_request_delay_ms": 8000
        })
    );
    let mut rows = [Vec::new(), Vec::new(), Vec::new()];
    for round in 1..=STABLE_ROUNDS {
        // Alternate first arm, holding durable history, tools and runtime tail fixed.
        let order = if round % 2 == 1 { [0, 1] } else { [1, 0] };
        for arm in order {
            let current_rules = if arm == 0 {
                String::new()
            } else {
                rendered_rules(&root)?
            };
            assert!(!snapshots[arm].refresh(
                &context,
                &surface,
                None,
                None,
                BUDGET,
                &current_rules
            ));
            let phase = if arm == 0 { "rules_off" } else { "rules_on" };
            rows[arm].push(
                measure(
                    &resolved,
                    &snapshots[arm],
                    &history,
                    &cohorts[arm],
                    phase,
                    round,
                )
                .await?,
            );
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
        append_history(&mut history, round);
    }
    // Change only a temporary rule copy, preserving this arm's routing key and cohort marker.
    let first = &paths[0];
    let text = fs::read_to_string(first)?;
    fs::write(
        first,
        format!("{text}\nCache diagnostic revision: {nonce}.\n"),
    )?;
    assert!(snapshots[1].refresh(
        &context,
        &surface,
        None,
        None,
        BUDGET,
        &rendered_rules(&root)?
    ));
    for round in 1..=CHANGED_ROUNDS {
        assert!(!snapshots[1].refresh(
            &context,
            &surface,
            None,
            None,
            BUDGET,
            &rendered_rules(&root)?
        ));
        rows[2].push(
            measure(
                &resolved,
                &snapshots[1],
                &history,
                &cohorts[1],
                "rules_changed",
                round,
            )
            .await?,
        );
        if round < CHANGED_ROUNDS {
            append_history(&mut history, STABLE_ROUNDS + round);
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
    }
    for (phase, usages) in ["rules_off", "rules_on", "rules_changed"].iter().zip(&rows) {
        for (scope, skip) in [("all", 0), ("after_first", 1)] {
            let prompt: u64 = usages
                .iter()
                .skip(skip)
                .map(|u| u64::from(u.prompt_tokens))
                .sum();
            let cached: u64 = usages
                .iter()
                .skip(skip)
                .map(|u| u64::from(u.cache_read_tokens.unwrap()))
                .sum();
            eprintln!(
                "RULES_CACHE_SUMMARY {}",
                serde_json::json!({
                    "phase": phase, "scope": scope, "requests": usages.len() - skip,
                    "prompt_tokens": prompt, "cache_read_tokens": cached,
                    "weighted_hit_rate": cached as f64 / prompt.max(1) as f64
                })
            );
        }
    }
    // This is a diagnostic, not a fixed gateway SLA. Zero hits remain visible, not silently skipped.
    Ok(())
}
