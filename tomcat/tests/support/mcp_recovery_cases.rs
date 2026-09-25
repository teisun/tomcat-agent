//! Lifecycle cases reuse the same HTTP fixture and manager harness as T1/TC.
use super::manager_cases::Harness;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tomcat::core::connector::mcp::manager::ServerState;

async fn status(fixture: &Harness, predicate: impl Fn(&ServerState) -> bool) {
    timeout(Duration::from_secs(8), async {
        loop {
            if predicate(&fixture.manager.statuses()[0].state) {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("lifecycle status: {:?}", fixture.manager.statuses()));
}
fn posts(state: &Value) -> usize {
    state["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["method"] == "tools/call")
        .count()
}

#[tokio::test]
async fn hard_startup_deadline_covers_headers_and_catalog_and_reclaims_each_attempt() {
    for stage in ["initialize", "catalog"] {
        let fixture = Harness::configured_with_startup(None, 30000, Some(100)).await;
        fixture
            .control(if stage == "initialize" {
                json!({"initGate":"blocked"})
            } else {
                json!({"listGate":"blocked"})
            })
            .await;
        let result = timeout(
            Duration::from_secs(8),
            fixture.manager.connect_server("fault"),
        )
        .await
        .expect("three bounded attempts plus backoff/cleanup");
        assert!(result.is_err());
        assert_eq!(
            fixture.state().await["initialized"],
            3,
            "{stage}: every old transport must exit before the next one starts"
        );
        assert!(matches!(
            fixture.manager.statuses()[0].state,
            ServerState::Failed(_)
        ));
        fixture.control(json!({"release":"blocked"})).await;
        assert!(fixture.manager.connect_server("fault").await.is_err());
        assert_eq!(fixture.state().await["initialized"], 3);
        fixture.stop().await;
    }
}

#[tokio::test]
async fn logout_remove_and_filter_shrink_stop_queued_calls_at_the_grant_boundary() {
    for (limit, action) in [None, Some(1)].into_iter().flat_map(|limit| {
        ["logout", "remove", "filter"]
            .into_iter()
            .map(move |action| (limit, action))
    }) {
        let n = limit.unwrap_or(16);
        let fixture = Harness::start(limit, 30000).await;
        let active: Vec<_> = (0..n)
            .map(|i| fixture.call(json!({"label":format!("active-{i}"), "gate":"active"})))
            .collect();
        fixture
            .until(|state| state["calls"].as_array().unwrap().len() == n)
            .await;
        let queued = fixture.call(json!({"label":"queued-before-revocation"}));
        timeout(Duration::from_secs(2), async {
            while fixture.manager.call_debug_counts("fault") != Some((n, 0, n - 1)) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        match action {
            "remove" => {
                assert!(fixture
                    .manager
                    .remove_configured_server("fault", &fixture.cfg)
                    .unwrap());
            }
            "logout" => {
                assert!(!fixture.manager.logout_server("fault").unwrap());
            }
            _ => fixture
                .manager
                .set_configured_tool_filter(
                    "fault",
                    serde_json::from_value(json!({"include":["echo_other"]})).unwrap(),
                    &fixture.cfg,
                )
                .unwrap(),
        }
        fixture.control(json!({"release":"active"})).await;
        assert!(queued.await.unwrap().is_err());
        for call in active {
            assert_eq!(call.await.unwrap().is_ok(), action == "filter");
        }
        assert_eq!(
            posts(&fixture.state().await),
            n,
            "queued work must not reach the server"
        );
        if action == "remove" {
            assert!(fixture
                .manager
                .reload_configuration(&fixture.cfg)
                .unwrap()
                .is_empty());
            assert!(fixture.manager.statuses().is_empty());
            assert_eq!(fixture.state().await["initialized"], 1);
        } else if action == "filter" {
            assert_eq!(fixture.manager.statuses()[0].state, ServerState::Ready);
        } else {
            let _ = fixture.manager.connect_server("fault").await;
            assert_eq!(
                fixture.state().await["initialized"],
                1,
                "ordinary startup/polling cannot revive explicit retirement"
            );
            fixture.manager.reconnect_server("fault").await.unwrap();
            assert_eq!(
                fixture.state().await["initialized"],
                2,
                "explicit recovery waits for actual old-owner cleanup"
            );
        }
        fixture.stop().await;
    }
}

#[tokio::test]
async fn session_expiry_coalesces_reload_and_never_replays_the_original_call() {
    let fixture = Harness::start(None, 1000).await;
    let generation = fixture.manager.statuses()[0].generation.clone();
    fixture
        .control(json!({"expireSession":true, "initGate":"recovery", "listGate":"catalog"}))
        .await;
    assert!(fixture
        .call(json!({"label":"never-replay"}))
        .await
        .unwrap()
        .is_err());
    fixture.until(|state| state["initialized"] == 2).await;
    for _ in 0..16 {
        let receipt = fixture.manager.request_reconnect("fault").unwrap();
        assert!(receipt.accepted);
        assert_eq!(receipt.generation, generation);
        assert!(receipt.recovery_timeout_ms > 0);
    }
    assert_eq!(fixture.state().await["initialized"], 2);
    fixture.control(json!({"release":"recovery"})).await;
    fixture
        .until(|state| {
            state["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "tools/list")
                .count()
                == 2
        })
        .await;
    assert_eq!(fixture.manager.statuses()[0].state, ServerState::Connecting);
    assert_eq!(fixture.manager.statuses()[0].tool_count, 0);
    assert!(fixture
        .call(json!({"label":"not-ready"}))
        .await
        .unwrap()
        .is_err());
    fixture.control(json!({"release":"catalog"})).await;
    status(&fixture, |state| *state == ServerState::Ready).await;
    let state = fixture.state().await;
    assert_eq!(posts(&state), 1, "the expired POST was never retried");
    assert_eq!(state["calls"].as_array().unwrap().len(), 0);
    fixture
        .call(json!({"label":"new-explicit-call"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(posts(&fixture.state().await), 2);
    fixture.stop().await;
}

#[tokio::test]
async fn startup_budget_preserves_typed_http_failures_without_auth_guessing() {
    for (code, failures, expected, ready) in [
        (503, 2, 3, true),
        (503, 99, 3, false),
        (404, 99, 1, false),
        (403, 99, 1, false),
        (401, 99, 1, false),
    ] {
        let fixture = Harness::configured(None, 1000).await;
        fixture
            .control(json!({"initStatus":code, "initFailures":failures}))
            .await;
        let result = fixture.manager.connect_server("fault").await;
        assert_eq!(result.is_ok(), ready, "{code}: {result:?}");
        if let Err(error) = result {
            assert!(!error.to_string().contains("SECRET"));
        }
        assert_eq!(
            fixture.state().await["initialized"],
            expected,
            "HTTP {code}"
        );
        if code == 401 {
            assert_eq!(
                fixture.manager.statuses()[0].state,
                ServerState::NeedsAuthorization
            );
        } else if !ready {
            assert!(matches!(
                fixture.manager.statuses()[0].state,
                ServerState::Failed(_)
            ));
        }
        for _ in 0..3 {
            let _ = fixture.manager.connect_server("fault").await;
        }
        assert_eq!(
            fixture.state().await["initialized"],
            expected,
            "polling cannot replenish attempts"
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn directory_is_part_of_startup_and_test_connector_is_one_attempt() {
    let fixture = Harness::configured(None, 1000).await;
    fixture
        .control(json!({"listFailures":1, "listStatus":503}))
        .await;
    fixture.manager.connect_server("fault").await.unwrap();
    assert_eq!(fixture.state().await["initialized"], 2);
    assert_eq!(fixture.manager.statuses()[0].attempt, 2);
    fixture.control(json!({"initFailures":99})).await;
    assert!(fixture.manager.test_server("fault").await.is_err());
    assert_eq!(
        fixture.state().await["initialized"],
        3,
        "TestConnector gets only one new startup"
    );
    assert_eq!(fixture.manager.statuses()[0].attempt, 1);
    fixture.stop().await;
}

#[tokio::test]
async fn short_lived_connections_share_budget_until_explicit_reload() {
    let fixture = Harness::start(None, 1000).await;
    let generation = fixture.manager.statuses()[0].generation.clone();
    for initialized in [2, 3] {
        fixture.control(json!({"expireSession":true})).await;
        assert!(fixture
            .call(json!({"label":"expired"}))
            .await
            .unwrap()
            .is_err());
        status(&fixture, |state| *state == ServerState::Ready).await;
        assert_eq!(fixture.state().await["initialized"], initialized);
        assert_eq!(fixture.manager.statuses()[0].generation, generation);
    }
    fixture.control(json!({"expireSession":true})).await;
    assert!(fixture
        .call(json!({"label":"exhausted"}))
        .await
        .unwrap()
        .is_err());
    status(&fixture, |state| matches!(state, ServerState::Failed(_))).await;
    for _ in 0..3 {
        assert!(fixture.manager.connect_server("fault").await.is_err());
    }
    assert_eq!(fixture.state().await["initialized"], 3);
    let receipt = fixture.manager.request_reconnect("fault").unwrap();
    assert_ne!(receipt.generation, generation);
    status(&fixture, |state| *state == ServerState::Ready).await;
    assert_eq!(fixture.state().await["initialized"], 4);
    assert_eq!(fixture.state().await["calls"].as_array().unwrap().len(), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn reload_retires_sixteen_calls_and_does_not_send_the_admission_queue() {
    let fixture = Harness::start(None, 30000).await;
    let active: Vec<_> = (0..16)
        .map(|i| {
            fixture.call(
                json!({"label":format!("active-{i}"), "gate":"active", "ignoreCancellation":true}),
            )
        })
        .collect();
    fixture
        .until(|state| state["calls"].as_array().unwrap().len() == 16)
        .await;
    let queued: Vec<_> = (0..16)
        .map(|i| fixture.call(json!({"label":format!("queued-{i}")})))
        .collect();
    timeout(Duration::from_secs(2), async {
        while fixture.manager.call_debug_counts("fault") != Some((16, 0, 0)) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let old_generation = fixture.manager.statuses()[0].generation.clone();
    let receipt = fixture.manager.request_reconnect("fault").unwrap();
    assert_ne!(receipt.generation, old_generation);
    for task in active.into_iter().chain(queued) {
        assert!(task.await.unwrap().is_err());
    }
    status(&fixture, |state| *state == ServerState::Ready).await;
    assert_eq!(fixture.state().await["initialized"], 2);
    assert_eq!(posts(&fixture.state().await), 16);
    assert_eq!(
        fixture.manager.call_debug_counts("fault"),
        Some((0, 16, 32))
    );
    fixture.control(json!({"release":"active"})).await;
    fixture
        .call(json!({"label":"new-explicit"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(posts(&fixture.state().await), 17);
    fixture.stop().await;
}

#[tokio::test]
async fn removed_and_readded_source_rejects_late_startup_results() {
    let fixture = Harness::configured(None, 1000).await;
    let definition = fixture.manager.configured_server("fault").unwrap();
    fixture.control(json!({"initGate":"old"})).await;
    let manager = fixture.manager.clone();
    let old = tokio::spawn(async move { manager.connect_server("fault").await });
    fixture.until(|state| state["initialized"] == 1).await;
    fixture
        .manager
        .remove_configured_server("fault", &fixture.cfg)
        .unwrap();
    tomcat::core::connector::mcp::config::add_global_server(
        &fixture.cfg,
        "fault".into(),
        definition.config,
    )
    .unwrap();
    fixture.manager.reload_configuration(&fixture.cfg).unwrap();
    fixture.control(json!({"initGate":null})).await;
    fixture.manager.connect_server("fault").await.unwrap();
    assert!(old.await.unwrap().is_err());
    let generation = fixture.manager.statuses()[0].generation.clone();
    fixture.control(json!({"release":"old"})).await;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(fixture.manager.statuses()[0].generation, generation);
    assert_eq!(fixture.manager.statuses()[0].state, ServerState::Ready);
    assert_eq!(fixture.state().await["initialized"], 2);
    fixture
        .call(json!({"label":"new-source"}))
        .await
        .unwrap()
        .unwrap();
    fixture.stop().await;
}
