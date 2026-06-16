use crate::ext::{PluginEngineConfig, PluginVmInstance};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn ok_response(data: serde_json::Value) -> String {
    json!({ "ok": true, "data": data }).to_string()
}

#[test]
fn dispatch_event_isolates_non_fatal_handler_errors() {
    let mut instance =
        PluginVmInstance::new(PluginEngineConfig::default(), "event-isolation".to_string())
            .expect("create quickjs instance");
    instance
        .register_host_binding(|_request_json| Ok(ok_response(serde_json::Value::Null)))
        .expect("register host binding");

    instance
        .run_script(
            r#"
let seen = [];
pi.on("demo", function () {
  seen.push("first");
  throw new Error("boom");
});
pi.on("demo", function () {
  seen.push("second");
});
__pi_dispatch_event(JSON.stringify({
  type: "demo",
  data: {},
  context: { cwd: "/tmp" }
}));
if (seen.length !== 2 || seen[0] !== "first" || seen[1] !== "second") {
  throw new Error("later handlers should still run after earlier error: " + JSON.stringify(seen));
}
"#,
        )
        .expect("non-fatal handler error should not abort dispatch");
}

#[test]
fn async_poll_refreshes_budget_before_each_host_poll() {
    let poll_count = Arc::new(AtomicUsize::new(0));
    let poll_count_for_binding = Arc::clone(&poll_count);
    let mut instance = PluginVmInstance::new(
        PluginEngineConfig {
            call_timeout_ms: 5_000,
            interrupt_budget: 5_000_000,
            ..Default::default()
        },
        "async-budget-reset".to_string(),
    )
    .expect("create quickjs instance");
    instance
        .register_host_binding(move |request_json| {
            let req: serde_json::Value =
                serde_json::from_str(request_json).expect("parse host request");
            let module = req
                .get("module")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let method = req
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();

            match (module, method) {
                ("llm", "setModel") => Ok(ok_response(json!({ "pending": true }))),
                ("__async", "poll") => {
                    let call_id = req
                        .pointer("/params/callId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        !call_id.is_empty(),
                        "poll request should keep the async call id"
                    );
                    let poll_no = poll_count_for_binding.fetch_add(1, Ordering::SeqCst) + 1;
                    if poll_no == 1 {
                        Ok(ok_response(json!({ "ready": false })))
                    } else {
                        Ok(ok_response(json!({
                            "ready": true,
                            "response": {
                                "ok": true,
                                "data": { "model": "gpt-5.4" }
                            }
                        })))
                    }
                }
                _ => Ok(ok_response(serde_json::Value::Null)),
            }
        })
        .expect("register host binding");

    instance
        .run_script(
            r#"
const originalReset = globalThis.__pi_budget_reset;
let resetCount = 0;
globalThis.__pi_budget_reset = function () {
  resetCount += 1;
  return originalReset();
};

(async function () {
  const result = await pi.setModel("gpt-5.4");
  if (!result || result.model !== "gpt-5.4") {
    throw new Error("async hostcall should resolve the ready payload");
  }
  if (resetCount < 4) {
    throw new Error("expected poll loop to refresh interrupt budget before each timer callback and host poll, got " + resetCount);
  }
})();
"#,
        )
        .expect("async poll loop should refresh the budget and resolve");

    assert_eq!(
        poll_count.load(Ordering::SeqCst),
        2,
        "fixture should exercise one pending poll and one ready poll"
    );
}
