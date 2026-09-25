#[path = "support/mcp_http_probe.rs"]
mod http_probe;
#[path = "support/mcp_oauth_recovery_cases.rs"]
mod oauth_recovery_cases;
#[path = "support/mcp_recovery_cases.rs"]
mod recovery_cases;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use rmcp::service::PeerRequestOptions;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::sleep;
use tomcat::core::connector::mcp::config::{
    ConfiguredMcpServer, McpConfigSource, McpOAuthConfig, McpServerConfig, ToolFilter,
};
use tomcat::core::connector::mcp::oauth::{authorize, OAuthTokenStore, StoredOAuthToken};
use tomcat::core::connector::mcp::transport::{HttpTransport, McpTransport};
use tomcat::AppConfig;

async fn start_fake_server(
    temp_dir: &Path,
    refresh_count_file: Option<&Path>,
    discovery_unavailable: bool,
) -> (tokio::process::Child, String) {
    start_fixture(temp_dir, refresh_count_file, discovery_unavailable, false).await
}

async fn start_fixture(
    temp_dir: &Path,
    refresh_count_file: Option<&Path>,
    discovery_unavailable: bool,
    faults: bool,
) -> (tokio::process::Child, String) {
    start_fixture_with_oauth_failure(
        temp_dir,
        refresh_count_file,
        discovery_unavailable,
        faults,
        false,
    )
    .await
}

async fn start_fixture_with_oauth_failure(
    temp_dir: &Path,
    refresh_count_file: Option<&Path>,
    discovery_unavailable: bool,
    faults: bool,
    fail_authenticated_startup: bool,
) -> (tokio::process::Child, String) {
    let address_file = temp_dir.join("bound-address");
    let mut command = Command::new(env!("CARGO_BIN_EXE_test_streamable_http_server"));
    command
        .args(["--port", "0"])
        .env("MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE", &address_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    if faults {
        command.arg("--faults");
    }
    if let Some(refresh_count_file) = refresh_count_file {
        command.env("MCP_STREAMABLE_HTTP_REFRESH_COUNT_FILE", refresh_count_file);
        command.env(
            "MCP_STREAMABLE_HTTP_INIT_COUNT_FILE",
            refresh_count_file.with_extension("initializations"),
        );
    }
    if discovery_unavailable {
        command.env("MCP_STREAMABLE_HTTP_DISCOVERY_UNAVAILABLE", "1");
    }
    if fail_authenticated_startup {
        command.env("MCP_STREAMABLE_HTTP_FAIL_AUTHENTICATED_STARTUP", "1");
    }
    let mut child = command.spawn().expect("spawn fake HTTP MCP server");
    let address = match tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(address) = tokio::fs::read_to_string(&address_file).await {
                // File creation/truncation precedes its write. Accept only a
                // complete address, never turn an empty file into http://mcp/.
                if let Ok(address) = address.trim().parse::<std::net::SocketAddr>() {
                    if address.ip().is_loopback() && address.port() != 0 {
                        break address.to_string();
                    }
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    {
        Ok(address) => address,
        Err(_) => {
            let _ = child.kill().await;
            panic!("fake server bound");
        }
    };
    (child, address)
}

#[tokio::test]
async fn fake_streamable_http_oauth_round_trips_without_a_human() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("tomcat").to_string_lossy().into_owned());
    std::fs::create_dir_all(temp.path().join("tomcat")).expect("work dir");
    let refresh_count_file = temp.path().join("refresh-count");
    let (mut child, address) =
        start_fake_server(temp.path(), Some(&refresh_count_file), false).await;
    let mcp_url = format!("http://{address}/mcp");
    let store = OAuthTokenStore::open(&cfg).expect("token store");
    let oauth = McpOAuthConfig::default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("local HTTP test client");
    let token = authorize(&client, &store, "fake", &mcp_url, &oauth, false)
        .await
        .expect("unattended OAuth login");
    assert_eq!(token.access_token, "fake-access-token");
    assert!(store.load("fake").expect("load token").is_some());
    let mut expired = token.clone();
    expired.expires_at = Some(0);
    store
        .save("fake", expired)
        .expect("expire stored token for refresh");

    let server = ConfiguredMcpServer {
        config_key: "fake".to_string(),
        name: "fake".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some(mcp_url),
            auth: Some("oauth".to_string()),
            env: Default::default(),
            headers: Default::default(),
            oauth: Some(oauth),
            cwd: None,
            tool_filter: ToolFilter::default(),
        },
    };
    let client = HttpTransport::new(store)
        .connect(&server)
        .await
        .expect("HTTP MCP connect");
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(tools.tools.len(), 1);
    let result = client
        .peer()
        .call_tool(
            rmcp::model::CallToolRequestParams::new("echo").with_arguments(
                serde_json::json!({"message": "hello"})
                    .as_object()
                    .expect("arguments")
                    .clone(),
            ),
        )
        .await
        .expect("tools/call");
    let result = serde_json::to_value(result).expect("serialize result");
    assert!(result.to_string().contains("fake echo: hello"));

    let refresh_count = tokio::fs::read_to_string(&refresh_count_file)
        .await
        .expect("fake OAuth refresh count");
    assert!(refresh_count.parse::<usize>().expect("refresh count") >= 1);
    child.kill().await.expect("stop fake server");
}

#[tokio::test]
async fn valid_oauth_token_survives_transient_discovery_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("tomcat").to_string_lossy().into_owned());
    std::fs::create_dir_all(temp.path().join("tomcat")).expect("work dir");
    let (mut child, address) = start_fake_server(temp.path(), None, true).await;
    let mcp_url = format!("http://{address}/mcp");
    let store = OAuthTokenStore::open(&cfg).expect("token store");
    store
        .save(
            "fake",
            StoredOAuthToken {
                access_token: "fake-access-token".to_string(),
                refresh_token: Some("fake-refresh-token".to_string()),
                expires_at: Some(u64::MAX),
                token_endpoint: format!("http://{address}/token"),
                issuer: Some(format!("http://{address}")),
                resource: Some(mcp_url.clone()),
                mcp_url: Some(mcp_url.clone()),
                client_metadata_url: None,
                scopes: Vec::new(),
                client_id: "fake-client".to_string(),
                client_secret: None,
            },
        )
        .expect("save valid token");
    let server = ConfiguredMcpServer {
        config_key: "fake".to_string(),
        name: "fake".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some(mcp_url),
            auth: Some("oauth".to_string()),
            env: Default::default(),
            headers: Default::default(),
            oauth: Some(McpOAuthConfig::default()),
            cwd: None,
            tool_filter: ToolFilter::default(),
        },
    };

    let client = HttpTransport::new(store)
        .connect(&server)
        .await
        .expect("valid token remains usable when discovery is unavailable");
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(tools.tools.len(), 1);
    child.kill().await.expect("stop fake server");
}

/// The same server binary as the OAuth fixtures, with opt-in fault routes.
/// Every child is killed on Drop, including assertion failures.
struct FaultHarness {
    _temp: tempfile::TempDir,
    child: tokio::process::Child,
    address: String,
    http: reqwest::Client,
    probe: http_probe::ProbeClient,
    client: tomcat::core::connector::mcp::transport::McpClient,
}

impl FaultHarness {
    async fn start(limit: usize) -> Self {
        Self::with_mode(limit, false, false).await
    }

    async fn with_mode(limit: usize, bounded: bool, modern: bool) -> Self {
        use rmcp::service::{serve_client_with_lifecycle, ClientLifecycleMode};
        use tomcat::core::connector::mcp::transport::CancellationBoundedTransport;
        let temp = tempfile::tempdir().expect("fixture directory");
        let (child, address) = start_fixture(temp.path(), None, false, true).await;
        let http = reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .expect("HTTP client");
        let config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                format!("http://{address}/mcp"),
            )
            .reinit_on_expired_session(false)
            .max_concurrent_requests(limit);
        let probe = http_probe::ProbeClient::new(http.clone());
        let transport =
            rmcp::transport::StreamableHttpClientTransport::with_client(probe.clone(), config);
        let lifecycle = if modern {
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            }
        } else {
            ClientLifecycleMode::Initialize
        };
        let client = tokio::time::timeout(Duration::from_secs(5), async {
            if bounded {
                serve_client_with_lifecycle(
                    (),
                    CancellationBoundedTransport::new(transport),
                    lifecycle,
                )
                .await
            } else {
                serve_client_with_lifecycle((), transport, lifecycle).await
            }
        })
        .await
        .expect("initialize deadline")
        .expect("initialize fault fixture");
        Self {
            _temp: temp,
            child,
            address,
            http,
            probe,
            client,
        }
    }

    async fn control(&self, command: Value) {
        self.http
            .post(format!("http://{}/__test/control", self.address))
            .json(&command)
            .send()
            .await
            .expect("control request")
            .error_for_status()
            .expect("control accepted");
    }

    async fn snapshot(&self) -> Value {
        self.http
            .get(format!("http://{}/__test/state", self.address))
            .send()
            .await
            .expect("snapshot request")
            .json()
            .await
            .expect("snapshot JSON")
    }

    async fn until(&self, condition: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = self.snapshot().await;
                if condition(&snapshot) {
                    return snapshot;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture observation deadline")
    }

    async fn request(
        &self,
        name: &str,
        args: Value,
        timeout_ms: u64,
    ) -> rmcp::service::RequestHandle<rmcp::RoleClient> {
        self.client
            .peer()
            .send_request_with_option(
                rmcp::model::ClientRequest::CallToolRequest(rmcp::model::Request::new(
                    rmcp::model::CallToolRequestParams::new(name.to_owned())
                        .with_arguments(args.as_object().expect("arguments").clone()),
                )),
                PeerRequestOptions::with_timeout(Duration::from_millis(timeout_ms))
                    .reset_timeout_on_progress(),
            )
            .await
            .expect("submit SDK request")
    }

    async fn stop(mut self) {
        assert!(
            self.client
                .close_with_timeout(Duration::from_secs(2))
                .await
                .expect("close client")
                .is_some(),
            "SDK service must actually finish"
        );
        self.child.kill().await.expect("stop fixture");
        self.child.wait().await.expect("reap fixture");
    }
}

#[tokio::test]
async fn sdk_m0_requests_overlap_before_delayed_headers_json_and_sse_finish() {
    for shape in ["headers", "json", "sse"] {
        for other_tool in ["echo", "echo_other"] {
            let fixture = FaultHarness::start(16).await;
            let first = fixture
                .request(
                    "echo",
                    json!({"label":"A", "response":shape, "gate":"A"}),
                    5000,
                )
                .await;
            let first_id = first.id.clone();
            let a = tokio::spawn(first.await_response());
            fixture
                .until(|s| s["calls"].as_array().unwrap().len() == 1)
                .await;
            let second = fixture
                .request(other_tool, json!({"label":"B"}), 1000)
                .await;
            assert_ne!(
                first_id, second.id,
                "same-tool calls need distinct wire IDs"
            );
            let b = second
                .await_response()
                .await
                .expect("B completes while A is gated");
            assert!(serde_json::to_string(&b)
                .unwrap()
                .contains("fixture result: B"));
            assert!(
                !a.is_finished(),
                "A must remain blocked until explicitly released"
            );
            let snapshot = fixture.snapshot().await;
            assert_eq!(snapshot["initialized"], 1);
            assert_eq!(
                snapshot["calls"][0]["session"],
                snapshot["calls"][1]["session"]
            );
            fixture.control(json!({"release":"A"})).await;
            let a = a.await.unwrap().expect("A result");
            assert!(serde_json::to_string(&a)
                .unwrap()
                .contains("fixture result: A"));
            fixture.until(|s| s["activeResponses"] == 0).await;
            fixture.stop().await;
        }
    }
}

#[tokio::test]
async fn sdk_m0_unit_handler_resets_only_matching_progress() {
    let fixture = FaultHarness::start(16).await;
    let start = tokio::time::Instant::now();
    let result = fixture
        .request(
            "echo",
            json!({
                "label":"progress", "delayMs":900, "progressMs":50,
            }),
            300,
        )
        .await
        .await_response()
        .await;
    assert!(
        result.is_ok(),
        "matching progress must extend the original window: {result:?}"
    );
    assert!(start.elapsed() >= Duration::from_millis(900));
    for (label, extra) in [
        ("wrong", json!({"wrongToken":true})),
        ("heartbeat", json!({"heartbeat":true})),
    ] {
        let mut args = json!({"label":label, "delayMs":900, "progressMs":50});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let result = fixture
            .request("echo", args, 300)
            .await
            .await_response()
            .await;
        assert!(
            matches!(result, Err(rmcp::ServiceError::Timeout { .. })),
            "{label}: {result:?}"
        );
    }
    let snapshot = fixture.snapshot().await;
    assert_eq!(snapshot["initialized"], 1);
    assert_eq!(
        snapshot["calls"].as_array().unwrap().len(),
        3,
        "renewal cannot resend tools"
    );
    assert!(snapshot["calls"][0]["progress"].as_u64().unwrap() > 1);
    fixture.stop().await;
}

#[tokio::test]
async fn sdk_m0_expired_session_is_not_reinitialized_or_replayed() {
    let fixture = FaultHarness::start(16).await;
    fixture.control(json!({"expireSession":true})).await;
    let result = fixture
        .request("echo", json!({"label":"expired"}), 1000)
        .await
        .await_response()
        .await;
    assert!(result.is_err());
    let snapshot = fixture.snapshot().await;
    assert_eq!(
        snapshot["initialized"], 1,
        "SDK reinitialization must be disabled"
    );
    assert!(
        snapshot["calls"].as_array().unwrap().is_empty(),
        "expired request must not execute"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn sdk_m0_lost_response_is_not_a_closed_shared_channel() {
    let fixture = FaultHarness::start(16).await;
    let result = fixture
        .request("echo", json!({"label":"lost", "truncate":true}), 1000)
        .await
        .await_response()
        .await;
    assert!(
        matches!(result, Err(rmcp::ServiceError::TransportClosed)),
        "{result:?}"
    );
    assert!(
        !fixture.client.peer().is_transport_closed(),
        "a lost response is not a dead client"
    );
    fixture
        .request("echo_other", json!({"label":"healthy"}), 1000)
        .await
        .await_response()
        .await
        .expect("healthy call");
    let snapshot = fixture.snapshot().await;
    assert_eq!(snapshot["initialized"], 1);
    assert_eq!(snapshot["calls"].as_array().unwrap().len(), 2);
    fixture.stop().await;
}

/// Characterization, not acceptance of the delayed timeout: M1 must provide
/// timely caller completion while owning the SDK cancellation/cleanup work.
#[tokio::test]
async fn sdk_m0_legacy_idle_timeout_waits_for_cancel_post_response() {
    let fixture = FaultHarness::start(16).await;
    fixture.control(json!({"cancelGate":"cancel"})).await;
    let request = fixture
        .request(
            "echo",
            json!({
                "label":"timed", "gate":"never", "ignoreCancellation":true,
            }),
            150,
        )
        .await;
    let result = tokio::spawn(request.await_response());
    fixture.until(|s| s["calls"][0]["cancellations"] == 1).await;
    assert!(
        !result.is_finished(),
        "SDK awaits the remote cancellation POST after its idle deadline"
    );
    fixture.control(json!({"release":"cancel"})).await;
    assert!(matches!(
        result.await.unwrap(),
        Err(rmcp::ServiceError::Timeout { .. })
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn bounded_transport_reclaims_cancelled_io_without_closing_other_requests() {
    for modern in [false, true] {
        for shape in ["headers", "json", "sse"] {
            let fixture = FaultHarness::with_mode(16, true, modern).await;
            fixture
                .control(json!({"cancelGate":"blocked-control"}))
                .await;
            let a = fixture
                .request(
                    "echo",
                    json!({
                        "label":"A", "gate":"A", "response":shape, "ignoreCancellation":true,
                    }),
                    30000,
                )
                .await;
            let b = fixture
                .request(
                    "echo",
                    json!({
                        "label":"B", "gate":"B", "response":shape, "ignoreCancellation":true,
                    }),
                    30000,
                )
                .await;
            let c = fixture
                .request("echo_other", json!({"label":"C", "gate":"C"}), 30000)
                .await;
            fixture
                .until(|s| s["calls"].as_array().unwrap().len() == 3)
                .await;
            assert_eq!(fixture.probe.active(), 3);
            let cancel_a = tokio::spawn(a.cancel(Some("stop A".into())));
            let cancel_b = tokio::spawn(b.cancel(Some("stop B".into())));
            tokio::time::timeout(Duration::from_secs(5), async {
                // Delivery failure is allowed; local ownership must still end.
                let _ = cancel_a.await.unwrap();
                let _ = cancel_b.await.unwrap();
                while fixture.probe.active() != 1 {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("cancelled POST/SSE ownership reclaimed within K");
            assert!(!fixture.client.peer().is_transport_closed());
            fixture.control(json!({"release":"C"})).await;
            c.await_response()
                .await
                .expect("unrelated C completes on original client");
            assert_eq!(fixture.snapshot().await["initialized"], 1);
            fixture.control(json!({"release":"blocked-control"})).await;
            fixture.stop().await;
        }
    }
}

#[tokio::test]
async fn sdk_m0_modern_unit_handler_progress_extends_wait() {
    let fixture = FaultHarness::with_mode(16, true, true).await;
    fixture
        .request(
            "echo",
            json!({"label":"modern-progress", "delayMs":900, "progressMs":50}),
            300,
        )
        .await
        .await_response()
        .await
        .expect("modern progress extends native SDK window");
    assert!(
        fixture.snapshot().await["calls"][0]["progress"]
            .as_u64()
            .unwrap()
            > 1
    );
    fixture.stop().await;
}

mod manager_cases {
    use super::*;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    use tomcat::core::connector::mcp::manager::{McpCallContext, McpManager, ServerState};

    pub(super) struct Harness {
        _temp: tempfile::TempDir,
        child: tokio::process::Child,
        address: String,
        http: reqwest::Client,
        pub(super) manager: Arc<McpManager>,
        pub(super) cfg: AppConfig,
        limit: usize,
    }

    impl Harness {
        pub(super) async fn start(limit: Option<usize>, timeout_ms: u64) -> Self {
            let fixture = Self::configured(limit, timeout_ms).await;
            fixture.manager.connect_server("fault").await.unwrap();
            fixture
        }
        pub(super) async fn configured(limit: Option<usize>, timeout_ms: u64) -> Self {
            Self::configured_with_startup(limit, timeout_ms, None).await
        }
        pub(super) async fn configured_with_startup(
            limit: Option<usize>,
            timeout_ms: u64,
            startup_ms: Option<u64>,
        ) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let (child, address) = start_fixture(temp.path(), None, false, true).await;
            let mut cfg = AppConfig::default();
            cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
            cfg.connector.mcp.call_timeout_ms = timeout_ms;
            if let Some(startup_ms) = startup_ms {
                cfg.connector.mcp.startup_timeout_ms = startup_ms;
            }
            if let Some(limit) = limit {
                cfg.connector.mcp.max_concurrent_calls = limit;
            }
            let config = json!({"url":format!("http://{address}/mcp"), "auth":"none"});
            tomcat::core::connector::mcp::config::add_global_server(
                &cfg,
                "fault".into(),
                serde_json::from_value(config).unwrap(),
            )
            .unwrap();
            let manager = McpManager::new(&cfg, None).unwrap();
            let actual = cfg.connector.mcp.max_concurrent_calls;
            assert_eq!(actual, limit.unwrap_or(16));
            Self {
                _temp: temp,
                child,
                address,
                manager,
                cfg,
                limit: actual,
                http: reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap(),
            }
        }
        pub(super) async fn state(&self) -> Value {
            self.http
                .get(format!("http://{}/__test/state", self.address))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        }
        pub(super) async fn control(&self, command: Value) {
            self.http
                .post(format!("http://{}/__test/control", self.address))
                .json(&command)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }
        pub(super) async fn until(&self, test: impl Fn(&Value) -> bool) {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if test(&self.state().await) {
                        return;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("manager fixture condition");
        }
        async fn counts(&self, expected: (usize, usize, usize)) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while self.manager.call_debug_counts("fault") != Some(expected) {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("managed tasks and permits must return to expected counts");
        }
        pub(super) fn call(&self, args: Value) -> tokio::task::JoinHandle<Result<Value, String>> {
            self.call_context(args, McpCallContext::default())
        }
        fn call_context(
            &self,
            args: Value,
            context: McpCallContext,
        ) -> tokio::task::JoinHandle<Result<Value, String>> {
            let manager = self.manager.clone();
            tokio::spawn(async move {
                manager
                    .call_model_tool_with_context("mcp__fault__echo", args, context)
                    .await
                    .map_err(|error| error.to_string())
            })
        }
        pub(super) async fn stop(mut self) {
            if self.manager.configured_server("fault").is_some() {
                self.manager
                    .remove_configured_server("fault", &self.cfg)
                    .unwrap();
            }
            self.child.kill().await.unwrap();
            self.child.wait().await.unwrap();
        }
    }

    #[tokio::test]
    async fn complete_call_quota_defaults_to_sixteen_and_preserves_explicit_overrides() {
        for limit in [None, Some(1), Some(32)] {
            for shape in ["headers", "json", "sse"] {
                let fixture = Harness::start(limit, 30000).await;
                let n = fixture.limit;
                let mut active = Vec::new();
                for index in 0..n {
                    let context = McpCallContext::new(
                        format!("agent-session-{index}"),
                        "same-outer-id",
                        &CancellationToken::new(),
                    );
                    active.push(fixture.call_context(json!({"label":format!("active-{index}"), "gate":"active", "response":shape}), context));
                }
                fixture
                    .until(|s| s["calls"].as_array().unwrap().len() == n)
                    .await;
                let queued: Vec<_> = (0..n)
                    .map(|i| fixture.call(json!({"label":format!("queued-{i}")})))
                    .collect();
                fixture.counts((n, 0, 0)).await;
                assert_eq!(fixture.state().await["calls"].as_array().unwrap().len(), n);
                let busy = fixture
                    .call(json!({"label":"busy"}))
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string();
                assert!(busy.contains("busy") && busy.contains("not sent"));
                fixture.control(json!({"release":"active"})).await;
                for (i, task) in active.into_iter().enumerate() {
                    let result = task.await.unwrap().unwrap();
                    assert_eq!(
                        result["content"][0]["text"],
                        format!("fixture result: active-{i}")
                    );
                }
                for task in queued {
                    task.await.unwrap().unwrap();
                }
                fixture.counts((0, n, 2 * n)).await;
                let state = fixture.state().await;
                assert_eq!(state["calls"].as_array().unwrap().len(), 2 * n);
                assert_eq!(state["initialized"], 1);
                fixture.stop().await;
            }
        }
    }

    #[tokio::test]
    async fn request_drop_cancel_and_idle_timeout_do_not_retire_shared_source() {
        for shape in ["headers", "json", "sse"] {
            let fixture = Harness::start(None, 300).await;
            fixture.control(json!({"cancelGate":"cancel"})).await;
            for index in 0..10 {
                let cancel = CancellationToken::new();
                let context = McpCallContext::new("session-A", "same-id", &cancel);
                let a = fixture.call_context(json!({"label":format!("A-{index}"), "gate":"A", "response":shape, "ignoreCancellation":true}), context);
                fixture
                    .until(|s| {
                        s["calls"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["label"] == format!("A-{index}"))
                    })
                    .await;
                if index % 3 == 0 {
                    a.abort();
                    let _ = a.await;
                } else if index % 3 == 1 {
                    cancel.cancel();
                    assert!(a.await.unwrap().is_err());
                } else {
                    assert!(a
                        .await
                        .unwrap()
                        .unwrap_err()
                        .to_string()
                        .contains("idle wait timed out"));
                }
                let b = fixture.call_context(
                    json!({"label":"B"}),
                    McpCallContext::new("session-B", "same-id", &CancellationToken::new()),
                );
                assert_eq!(
                    b.await.unwrap().unwrap()["content"][0]["text"],
                    "fixture result: B"
                );
                fixture.counts((0, 16, 32)).await;
                assert_eq!(fixture.manager.statuses()[0].state, ServerState::Ready);
            }
            assert_eq!(fixture.state().await["initialized"], 1);
            fixture.control(json!({"release":"cancel"})).await;
            fixture.stop().await;
        }
    }

    #[tokio::test]
    async fn progress_resets_only_its_call_and_not_admission_queue() {
        let fixture = Harness::start(Some(1), 300).await;
        let a = fixture.call(json!({"label":"progress", "gate":"progress", "progressMs":40}));
        fixture
            .until(|s| s["calls"].as_array().unwrap().len() == 1)
            .await;
        let queued = fixture.call(json!({"label":"queued"}));
        assert!(queued
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("admission queue timed out"));
        sleep(Duration::from_millis(400)).await;
        assert!(!a.is_finished());
        assert_eq!(fixture.state().await["calls"].as_array().unwrap().len(), 1);
        fixture.control(json!({"release":"progress"})).await;
        a.await.unwrap().unwrap();
        fixture.counts((0, 1, 2)).await;
        fixture.stop().await;
    }

    #[tokio::test]
    async fn http_status_is_observed_without_replay_or_remote_body_disclosure() {
        let fixture = Harness::start(None, 1000).await;
        for status in [400, 429, 503] {
            let message = fixture
                .call(
                    json!({"label":format!("status-{status}"), "httpStatus":status,
                "responseBody":"SECRET_RESPONSE_BODY https://secret.invalid/?token=SECRET_QUERY",
                "privateArgument":"SECRET_ARGUMENT"}),
                )
                .await
                .unwrap()
                .unwrap_err();
            assert!(message.contains(&format!("HTTP {status}")), "{message}");
            assert!(!message.contains("SECRET"), "{message}");
            assert_eq!(fixture.manager.statuses()[0].state, ServerState::Ready);
        }
        let message = fixture
            .call(json!({"label":"malformed", "malformed":true}))
            .await
            .unwrap()
            .unwrap_err();
        assert!(!message.contains("SECRET"));
        fixture
            .call(json!({"label":"healthy"}))
            .await
            .unwrap()
            .unwrap();
        let state = fixture.state().await;
        assert_eq!(state["initialized"], 1);
        assert_eq!(
            state["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "tools/call")
                .count(),
            5
        );
        assert_eq!(state["calls"].as_array().unwrap().len(), 5);
        fixture.counts((0, 16, 32)).await;
        fixture.stop().await;
    }

    #[tokio::test]
    async fn business_errors_and_truncated_results_keep_other_calls_usable_without_replay() {
        let fixture = Harness::start(None, 1000).await;
        assert_eq!(
            fixture
                .call(json!({"label":"business", "businessError":true}))
                .await
                .unwrap()
                .unwrap()["isError"],
            true
        );
        for args in [
            json!({"label":"rpc", "rpcError":true}),
            json!({"label":"stream", "truncate":true}),
        ] {
            assert!(fixture.call(args).await.unwrap().is_err());
            assert_eq!(fixture.manager.statuses()[0].state, ServerState::Ready);
        }
        fixture
            .call(json!({"label":"healthy"}))
            .await
            .unwrap()
            .unwrap();
        fixture.counts((0, 16, 32)).await;
        let state = fixture.state().await;
        assert_eq!(state["initialized"], 1);
        assert_eq!(state["calls"].as_array().unwrap().len(), 4);
        fixture.stop().await;
    }

    #[tokio::test]
    async fn different_sources_do_not_share_complete_call_quota() {
        let fixture = Harness::start(Some(1), 30000).await;
        let other_dir = fixture._temp.path().join("other-server");
        std::fs::create_dir_all(&other_dir).unwrap();
        let (mut other_child, other_address) = start_fixture(&other_dir, None, false, true).await;
        tomcat::core::connector::mcp::config::add_global_server(
            &fixture.cfg,
            "other".into(),
            serde_json::from_value(
                json!({"url":format!("http://{other_address}/mcp"), "auth":"none"}),
            )
            .unwrap(),
        )
        .unwrap();
        fixture.manager.reload_configuration(&fixture.cfg).unwrap();
        fixture.manager.connect_server("other").await.unwrap();
        let a = fixture.call(json!({"label":"full-source-A", "gate":"A"}));
        fixture
            .until(|s| s["calls"].as_array().unwrap().len() == 1)
            .await;
        let b = tokio::time::timeout(
            Duration::from_secs(2),
            fixture
                .manager
                .call_model_tool("mcp__other__echo", json!({"label":"independent-B"})),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(b["content"][0]["text"], "fixture result: independent-B");
        assert!(!a.is_finished());
        assert_eq!(
            fixture.state().await["initialized"],
            1,
            "adding another source keeps A's connection"
        );
        fixture.control(json!({"release":"A"})).await;
        a.await.unwrap().unwrap();
        fixture
            .manager
            .remove_configured_server("other", &fixture.cfg)
            .unwrap();
        other_child.kill().await.unwrap();
        other_child.wait().await.unwrap();
        fixture.stop().await;
    }

    #[tokio::test]
    async fn real_thirty_five_second_sse_body_is_not_cut_off_by_http_client() {
        let fixture = Harness::configured(None, 40000).await;
        fixture.control(json!({"notificationStream":true})).await;
        fixture.manager.connect_server("fault").await.unwrap();
        fixture
            .until(|state| state["activeNotificationStreams"] == 1)
            .await;
        let began = tokio::time::Instant::now();
        fixture
            .call(json!({"label":"35-seconds", "delayMs":35000}))
            .await
            .unwrap()
            .unwrap();
        assert!(began.elapsed() >= Duration::from_secs(35));
        assert_eq!(fixture.state().await["initialized"], 1);
        assert_eq!(
            fixture.state().await["activeNotificationStreams"],
            1,
            "idle GET must survive alongside the long answer"
        );
        fixture.counts((0, 16, 32)).await;
        fixture.manager.logout_server("fault").unwrap();
        fixture
            .until(|state| state["activeNotificationStreams"] == 0)
            .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.manager.retirement_debug_count("fault") != Some(0) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned SDK service and transport must actually exit");
        fixture.stop().await;
    }
}

/// Explicit capability gate: do not hide this behind the manager's old lock.
/// Run with --ignored while evaluating the SDK's residual lifecycle gap.
#[tokio::test]
#[ignore = "M0 capability gate: legacy cancellation queue must satisfy the plan's K=5s"]
async fn sdk_m0_saturated_legacy_cancellation_meets_five_second_cleanup() {
    let fixture = FaultHarness::start(16).await;
    fixture
        .control(json!({"cancelGate":"blocked-control"}))
        .await;
    let a = fixture
        .request(
            "echo",
            json!({"label":"A", "gate":"A", "ignoreCancellation":true}),
            30000,
        )
        .await;
    let b = fixture
        .request(
            "echo",
            json!({"label":"B", "gate":"B", "ignoreCancellation":true}),
            30000,
        )
        .await;
    fixture
        .until(|s| s["calls"].as_array().unwrap().len() == 2)
        .await;
    let cancel_a = tokio::spawn(a.cancel(Some("test stop A".into())));
    fixture.until(|s| s["calls"][0]["cancellations"] == 1).await;
    let mut cancel_b = tokio::spawn(b.cancel(Some("test stop B".into())));
    let completed = tokio::time::timeout(Duration::from_secs(5), &mut cancel_b)
        .await
        .is_ok();
    let retained_io = fixture.probe.active();
    eprintln!("raw SDK after K: cancellation finished={completed}, retained tool HTTP/SSE owners={retained_io}");
    // Always release and reap before asserting, so a failed capability probe
    // cannot strand the fixture or either cancellation task.
    fixture.control(json!({"release":"blocked-control"})).await;
    let _ = cancel_a.await;
    if !completed {
        let _ = cancel_b.await;
    }
    fixture.stop().await;
    assert!(
        completed && retained_io == 0,
        "rmcp legacy cancellation exceeded K=5s; remaining local HTTP/SSE owners: {retained_io}"
    );
}
