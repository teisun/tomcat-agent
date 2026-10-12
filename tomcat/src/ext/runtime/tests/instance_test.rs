use super::{PluginEngineConfig, PluginVmInstance};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn run_script_file_reports_missing_path() {
    let mut instance =
        PluginVmInstance::new(PluginEngineConfig::default(), "missing-script".to_string())
            .expect("create quickjs instance");
    let missing = std::path::Path::new("/definitely/missing/plugin.js");
    let err = instance
        .run_script_file(missing)
        .expect_err("missing script should fail");
    assert!(
        err.to_string().contains("script file not found"),
        "missing script error should mention path resolution: {err}"
    );
}

#[test]
fn run_agent_code_returns_structured_final_value() {
    let mut instance =
        PluginVmInstance::new(PluginEngineConfig::default(), "agent-code".to_string())
            .expect("create quickjs instance");
    let result = instance
        .run_agent_code("return { kept: [1, 2], discarded: null };")
        .expect("agent code should return JSON");
    assert_eq!(result["kept"], serde_json::json!([1, 2]));
    assert!(result["discarded"].is_null());
}

#[test]
fn build_combined_script_appends_main_loop_for_session_vm() {
    let instance =
        PluginVmInstance::new(PluginEngineConfig::default(), "combined-script".to_string())
            .expect("create quickjs instance");
    let dir = tempfile::tempdir().expect("create tempdir");
    let script_path = dir.path().join("main.js");
    std::fs::write(&script_path, "pi.log('ready');\n").expect("write user script");
    let combined = instance
        .build_combined_script(&script_path, true)
        .expect("build combined script");
    assert!(
        combined.contains("// --- pi_main_loop.js ---"),
        "session VM should inject main loop when plugin does not start it explicitly"
    );
    assert!(
        combined.contains("__pi_start_event_loop"),
        "combined script should include event loop bootstrap"
    );
    assert!(
        combined.contains("__pi_resume_timeout"),
        "combined script should resume the timeout budget at the user-code boundary"
    );
}

#[test]
fn agent_result_does_not_wait_for_unawaited_timers() {
    let mut vm =
        PluginVmInstance::new(PluginEngineConfig::default(), "agent-final".into()).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        vm.run_agent_code("setTimeout(() => {}, 60000); return 42;")
            .unwrap(),
        42
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn agent_external_cancel_stops_busy_and_idle_execution() {
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;
    for body in ["while (true) {}", "await new Promise(() => {});"] {
        let mut vm = PluginVmInstance::new(
            PluginEngineConfig {
                call_timeout_ms: 500,
                interrupt_budget: 0,
                ..Default::default()
            },
            "agent-stop".into(),
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let (ready, seen) = std::sync::mpsc::channel();
        vm.register_host_binding(move |_| {
            ready.send(()).unwrap();
            Ok(serde_json::json!({"ok":true,"data":null}).to_string())
        })
        .unwrap();
        let stopper = std::thread::spawn(move || {
            seen.recv_timeout(Duration::from_secs(2)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            stop.cancel();
        });
        let start = Instant::now();
        let error = vm
            .run_agent_code_with_cancel(
                &format!("__pi_host_call('{{}}'); {body}"),
                cancel,
                move || start.elapsed() < Duration::from_secs(2),
            )
            .unwrap_err();
        stopper.join().unwrap();
        assert!(error.to_string().contains("cancelled"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn agent_host_wait_exceeds_short_vm_window_but_local_work_still_times_out() {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;
    for tail in ["return result;", "while (true) {}"] {
        let mut vm = PluginVmInstance::new(
            PluginEngineConfig {
                call_timeout_ms: 20,
                interrupt_budget: 0,
                ..Default::default()
            },
            "agent-host-wait".into(),
        )
        .unwrap();
        let pending = Arc::new(AtomicBool::new(false));
        let binding_pending = pending.clone();
        let started = parking_lot::Mutex::new(None::<Instant>);
        vm.register_host_binding(move |request| {
            let request: serde_json::Value = serde_json::from_str(request).unwrap();
            let response = if request["module"] == "connector" {
                binding_pending.store(true, Ordering::SeqCst);
                *started.lock() = Some(Instant::now());
                serde_json::json!({"ok":true,"data":{"pending":true},"callId":request["callId"]})
            } else if started.lock().unwrap().elapsed() >= Duration::from_millis(350) {
                binding_pending.store(false, Ordering::SeqCst);
                serde_json::json!({"ok":true,"data":{"ready":true,"response":{"ok":true,"data":42}}})
            } else { serde_json::json!({"ok":true,"data":{"ready":false}}) };
            Ok(response.to_string())
        }).unwrap();
        let value = vm.run_agent_code_with_cancel(
            &format!("const result = await callTool('fixture', {{}}); {tail}"),
            CancellationToken::new(),
            move || pending.load(Ordering::SeqCst),
        );
        if tail.starts_with("return") {
            assert_eq!(value.unwrap(), 42);
        } else {
            assert!(value.unwrap_err().to_string().contains("20ms timeout"));
        }
    }
    let mut vm = PluginVmInstance::new(
        PluginEngineConfig {
            call_timeout_ms: 50,
            interrupt_budget: 0,
            ..Default::default()
        },
        "agent-empty-promise".into(),
    )
    .unwrap();
    assert!(vm
        .run_agent_code("await new Promise(() => {});")
        .unwrap_err()
        .to_string()
        .contains("50ms timeout"));
}

#[test]
fn run_script_reaches_registered_host_binding() {
    let mut instance =
        PluginVmInstance::new(PluginEngineConfig::default(), "host-binding".to_string())
            .expect("create quickjs instance");
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&call_count);
    instance
        .register_host_binding(move |_request_json| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
        })
        .expect("register host binding");
    instance
        .run_script("pi.log('hello from inline test');")
        .expect("run inline script");
    assert!(
        call_count.load(Ordering::SeqCst) >= 1,
        "pi.log should route through the host binding"
    );
}

#[test]
fn bridge_exec_keeps_complete_command_and_existing_options_position() {
    let mut vm =
        PluginVmInstance::new(PluginEngineConfig::default(), "exec-command".into()).unwrap();
    let requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let captured = requests.clone();
    vm.register_host_binding(move |request| {
        let request: serde_json::Value = serde_json::from_str(request).unwrap();
        captured.lock().push(request);
        Ok(serde_json::json!({"ok":true,"data":{"exitCode":0}}).to_string())
    })
    .unwrap();
    assert_eq!(
        vm.run_agent_code(
            r#"
        const command = "printf 'two words' | cat";
        await pi.exec(command);
        await tomcat.exec(command, [], {cwd: '/fixture', foregroundWaitMs: 9000});
        return (await pi.exec(command, undefined, {cwd: '/fixture'})).exitCode;
    "#
        )
        .unwrap(),
        0
    );
    let requests = requests.lock();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["module"], "fs");
        assert_eq!(request["method"], "executeBash");
        assert_eq!(request["params"]["command"], "printf 'two words' | cat");
        assert!(
            request["params"].get("args").is_none(),
            "obsolete argv must not be advertised: {request}"
        );
    }
    assert_eq!(requests[1]["params"]["cwd"], "/fixture");
    assert_eq!(requests[1]["params"]["foreground_wait_ms"], 9000);
    assert_eq!(requests[2]["params"]["cwd"], "/fixture");
}

#[test]
fn bridge_exec_rejects_obsolete_argv_before_submitting_any_command() {
    let mut vm =
        PluginVmInstance::new(PluginEngineConfig::default(), "exec-obsolete".into()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    vm.register_host_binding(move |_| {
        captured.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"ok":true,"data":{"exitCode":0}}).to_string())
    })
    .unwrap();
    let result = vm
        .run_agent_code(
            r#"
        const errors = [];
        for (const args of [['status'], 'status', {cwd: '/wrong-position'}]) {
            errors.push(await pi.exec('git', args).then(() => null, error => error.message));
        }
        return errors;
    "#,
        )
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "unsupported argv must not execute the bare program"
    );
    for error in result.as_array().unwrap() {
        assert!(
            error
                .as_str()
                .is_some_and(|message| message.contains("complete shell command")),
            "{error}"
        );
    }
}

#[test]
fn heap_limit_rejects_large_allocation() {
    let script = r#"
globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
"#;
    let mut tight = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 1,
            call_timeout_ms: 1_000,
            interrupt_budget: 1_000_000,
            ..Default::default()
        },
        "heap-limit".to_string(),
    )
    .expect("create quickjs instance");
    let err = tight
        .run_script(script)
        .expect_err("allocation above heap limit should fail");
    let mut roomy = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 8,
            call_timeout_ms: 1_000,
            interrupt_budget: 1_000_000,
            ..Default::default()
        },
        "heap-roomy".to_string(),
    )
    .expect("create roomy quickjs instance");
    roomy
        .run_script(script)
        .expect("same allocation should fit once heap budget is raised");
    assert!(
        matches!(err, crate::infra::AppError::QuickJS(_)),
        "heap guard should surface a QuickJS-side failure, got: {err}"
    );
}

#[test]
fn heap_limit_zero_allows_large_allocation() {
    let script = r#"
globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
"#;
    let mut unbounded = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 0,
            call_timeout_ms: 1_000,
            interrupt_budget: 1_000_000,
            ..Default::default()
        },
        "heap-unbounded".to_string(),
    )
    .expect("create unbounded quickjs instance");
    unbounded
        .run_script(script)
        .expect("heap_mb=0 should skip the explicit memory limit");
}

#[test]
fn call_timeout_interrupts_long_sync_script() {
    let mut instance = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 8,
            call_timeout_ms: 50,
            interrupt_budget: 0,
            ..Default::default()
        },
        "call-timeout".to_string(),
    )
    .expect("create timeout-scoped quickjs instance");
    let err = instance
        .run_script("while (true) {}")
        .expect_err("call_timeout_ms should interrupt a runaway synchronous loop");
    assert!(
        err.to_string().contains("50ms timeout"),
        "timeout error should mention the configured 50ms budget, got: {err}"
    );
}

#[test]
fn wait_for_event_refreshes_timeout_budget_between_idle_ticks() {
    let mut instance = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 8,
            // Four 100ms waits exceed the total budget; per-tick headroom tolerates scheduler jitter.
            call_timeout_ms: 200,
            interrupt_budget: 0,
            ..Default::default()
        },
        "idle-budget-reset".to_string(),
    )
    .expect("create quickjs instance");
    instance
        .register_host_binding(|request_json| {
            let request: serde_json::Value =
                serde_json::from_str(request_json).expect("host request should be JSON");
            if request["module"] == "__session" && request["method"] == "waitForEvent" {
                std::thread::sleep(std::time::Duration::from_millis(100));
                return Ok(
                    serde_json::json!({ "ok": true, "data": { "type": "__tick" } }).to_string(),
                );
            }
            Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
        })
        .expect("register host binding");
    instance
        .run_script(
            r#"
(async function () {
  globalThis.__idleBudgetSink = 0;
  for (var i = 0; i < 4; i += 1) {
    await __pi_wait_for_event(100);
    for (var j = 0; j < 5000; j += 1) {
      globalThis.__idleBudgetSink += j;
    }
  }
})();
"#,
        )
        .expect("idle wait ticks should reset timeout budget instead of tripping it");
}

#[test]
fn wait_for_event_can_block_past_call_timeout_without_tripping_idle_vm() {
    let mut instance = PluginVmInstance::new(
        PluginEngineConfig {
            quickjs_heap_mb: 8,
            call_timeout_ms: 50,
            interrupt_budget: 0,
            ..Default::default()
        },
        "idle-timeout-pause".to_string(),
    )
    .expect("create quickjs instance");
    instance
        .register_host_binding(|request_json| {
            let request: serde_json::Value =
                serde_json::from_str(request_json).expect("host request should be JSON");
            if request["module"] == "__session" && request["method"] == "waitForEvent" {
                std::thread::sleep(std::time::Duration::from_millis(80));
                return Ok(
                    serde_json::json!({ "ok": true, "data": { "type": "__tick" } }).to_string(),
                );
            }
            Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
        })
        .expect("register host binding");
    instance
        .run_script(
            r#"
(async function () {
  await __pi_wait_for_event(80);
  globalThis.__idleWaitCompleted = true;
})();
"#,
        )
        .expect("idle wait should not spend call_timeout_ms while no plugin code is running");
}

#[test]
fn destroy_consumes_instance_without_side_effects() {
    let instance = PluginVmInstance::new(PluginEngineConfig::default(), "destroy".to_string())
        .expect("create quickjs instance");
    instance.destroy();
}

#[test]
fn agent_code_cannot_reset_its_execution_guard() {
    let mut results = Vec::new();
    for (timeout, budget, expected) in [(50, 0, "50ms timeout"), (0, 8, "interrupt budget 8")] {
        let mut vm = PluginVmInstance::new(
            PluginEngineConfig {
                call_timeout_ms: timeout,
                interrupt_budget: budget,
                ..Default::default()
            },
            "agent-budget-owner".into(),
        )
        .unwrap();
        let result = vm.run_agent_code(
            r#"
const until = Date.now() + 200;
while (Date.now() < until) {
  if (typeof __pi_budget_reset === 'function') __pi_budget_reset();
  if (typeof __pi_resume_timeout === 'function') __pi_resume_timeout();
}
return 'escaped';
"#,
        );
        results.push((expected, result));
    }
    for (expected, result) in results {
        let error = result.expect_err("guest code must not refresh its own budget");
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn agent_code_does_not_receive_plugin_budget_or_idle_controls() {
    let mut vm =
        PluginVmInstance::new(PluginEngineConfig::default(), "agent-capabilities".into()).unwrap();
    let value = vm.run_agent_code("return [typeof __pi_budget_reset, typeof __pi_resume_timeout, typeof __pi_wait_for_event];").unwrap();
    assert_eq!(
        value,
        serde_json::json!(["undefined", "undefined", "undefined"])
    );
}
