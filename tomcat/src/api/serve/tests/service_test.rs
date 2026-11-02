use std::ffi::OsStr;

use crate::api::serve::service::service_command;
use crate::AppConfig;

use super::{
    handle_connection, read_json, write_json, ConnectionHello, ConnectionProtocol, ServiceInfo,
    CONNECT_TIMEOUT, LOCAL_PROTOCOL_VERSION,
};
use crate::api::serve::{
    runtime::ServeRuntime,
    test_support::{serve_test_config, write_test_config},
};
use crate::AppError;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};

struct HandshakeFixture {
    _root: tempfile::TempDir,
    runtime: Arc<ServeRuntime>,
    client: BufReader<DuplexStream>,
    task: tokio::task::JoinHandle<Result<(), AppError>>,
    finished: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl HandshakeFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cfg = serve_test_config(root.path(), "http://127.0.0.1:1");
        let runtime = ServeRuntime::new(
            cfg.clone(),
            write_test_config(&cfg),
            crate::api::serve::build_shared_model_prefs(&cfg).unwrap(),
            true,
        )
        .unwrap();
        let info = ServiceInfo {
            service: "tomcat".into(),
            protocol_version: LOCAL_PROTOCOL_VERSION,
            server_version: env!("CARGO_PKG_VERSION").into(),
            instance_id: "handshake-instance".into(),
            pid: std::process::id(),
            agent_id: cfg.agent.id,
            endpoint: root.path().join("unused"),
            running: true,
        };
        let (server, client) = tokio::io::duplex(8192);
        let (finished, receiver) = tokio::sync::watch::channel(None);
        let task = tokio::spawn(handle_connection(
            runtime.clone(),
            Box::new(server),
            info,
            receiver,
        ));
        Self {
            _root: root,
            runtime,
            client: BufReader::new(client),
            task,
            finished,
        }
    }

    async fn send(&mut self, record: &Value) {
        write_json(&mut self.client, record).await.unwrap();
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(CONNECT_TIMEOUT, read_json(&mut self.client))
            .await
            .unwrap()
            .unwrap()
    }

    async fn hello(&mut self) {
        self.send(&json!({"service":"tomcat", "protocolVersion":1,
            "clientVersion":env!("CARGO_PKG_VERSION"), "protocol":"native"}))
            .await;
        let reply = self.next().await;
        assert_eq!(reply["instanceId"], "handshake-instance");
        assert!(reply.get("error").is_none(), "{reply}");
    }
}

#[tokio::test]
async fn native_waits_for_environment_then_acknowledges_before_initialize() {
    let mut fx = HandshakeFixture::new();
    fx.hello().await;
    assert!(
        fx.runtime.connections.read().is_empty(),
        "hello must not admit a native connection"
    );
    fx.send(&json!({"environment":{"TOMCAT_HOST_LOCALE":"zh-CN"}}))
        .await;
    assert_eq!(fx.next().await, json!({"ready":true}));
    fx.send(&json!({"type":"control_request", "requestId":"init", "subtype":"initialize", "payload":{}})).await;
    let initialized = fx.next().await;
    assert_eq!(initialized["type"], "control_response");
    assert_eq!(
        initialized["payload"]["uiPreferences"]["effective"],
        "zh-CN"
    );
    fx.client.shutdown().await.unwrap();
    fx.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn native_rejects_invalid_environment_without_creating_a_connection() {
    for record in [
        json!({}),
        json!({"environment":null}),
        json!({"environment":[]}),
        json!({"environment":{"BAD=KEY":"value"}}),
        json!({"environment":{"KEY":"bad\0value"}}),
    ] {
        let mut fx = HandshakeFixture::new();
        fx.hello().await;
        fx.send(&record).await;
        let reply = fx.next().await;
        assert_eq!(reply["error"], "invalid_connection_environment");
        assert!(fx.runtime.connections.read().is_empty());
        fx.task.await.unwrap().unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn native_environment_phase_keeps_the_handshake_deadline() {
    let mut fx = HandshakeFixture::new();
    fx.hello().await;
    assert!(fx.runtime.connections.read().is_empty());
    tokio::time::advance(CONNECT_TIMEOUT + std::time::Duration::from_millis(1)).await;
    let error = fx
        .task
        .await
        .unwrap()
        .expect_err("idle environment phase must time out");
    assert!(error.to_string().contains("timed out"));
    assert!(fx.runtime.connections.read().is_empty());
}

#[tokio::test]
async fn native_rejects_old_hello_including_an_empty_environment() {
    for environment in [json!({}), json!({"OLD":"value"})] {
        let mut fx = HandshakeFixture::new();
        fx.send(&json!({"service":"tomcat", "protocolVersion":1,
            "clientVersion":env!("CARGO_PKG_VERSION"), "protocol":"native", "environment":environment})).await;
        assert_eq!(fx.next().await["error"], "native_handshake_outdated");
        assert!(fx.runtime.connections.read().is_empty());
        fx.task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn unchanged_management_and_acp_hellos_keep_their_original_protocol() {
    for protocol in [
        ConnectionProtocol::Status,
        ConnectionProtocol::Stop,
        ConnectionProtocol::Acp,
    ] {
        let mut fx = HandshakeFixture::new();
        let mut hello = serde_json::to_value(ConnectionHello::new(protocol)).unwrap();
        hello["environment"] = json!({"ACP_ENV":"still-supported"});
        fx.send(&hello).await;
        let info = fx.next().await;
        assert!(info.get("error").is_none(), "{info}");
        assert_eq!(info["protocolVersion"], 1);
        if protocol == ConnectionProtocol::Stop {
            fx.finished.send_replace(Some(Ok(())));
            assert_eq!(fx.next().await, json!({"stopped":true}));
            assert!(fx.runtime.shutdown.is_cancelled());
        } else if protocol == ConnectionProtocol::Acp {
            fx.send(
                &json!({"jsonrpc":"2.0", "id":"acp-init", "method":"initialize",
                "params":{"protocolVersion":1,"clientCapabilities":{}}}),
            )
            .await;
            assert_eq!(fx.next().await["result"]["protocolVersion"], 1);
            fx.client.shutdown().await.unwrap();
        }
        fx.task.await.unwrap().unwrap();
    }
}

#[test]
fn local_hop_wire_is_pinned_to_protocol_version() {
    use crate::api::acp::{CLOSE, CLOSED, CLOSE_FAILED};
    use crate::api::serve::service::{
        ConnectionHello, ConnectionProtocol, ServiceInfo, LOCAL_PROTOCOL_VERSION,
    };
    use std::collections::BTreeSet;
    let notice = concat!(
        "local wire changed: confirm old/new peers still interoperate ",
        "(optional fields with defaults may); otherwise bump LOCAL_PROTOCOL_VERSION"
    );
    let keys = |value: serde_json::Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let expected = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(LOCAL_PROTOCOL_VERSION, 1, "{notice}");
    let hello = serde_json::to_value(ConnectionHello::new(ConnectionProtocol::Acp)).unwrap();
    assert_eq!(hello["protocol"], "acp", "{notice}");
    assert_eq!(
        keys(hello),
        expected(&[
            "service",
            "protocolVersion",
            "clientVersion",
            "protocol",
            "environment",
            "skillsDir"
        ]),
        "{notice}"
    );
    let info = ServiceInfo {
        service: "tomcat".into(),
        protocol_version: LOCAL_PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").into(),
        instance_id: "wire-fixture".into(),
        pid: 7,
        agent_id: "main".into(),
        endpoint: "fixture-endpoint".into(),
        running: true,
    };
    assert_eq!(
        keys(serde_json::to_value(info).unwrap()),
        expected(&[
            "service",
            "protocolVersion",
            "serverVersion",
            "instanceId",
            "pid",
            "agentId",
            "endpoint",
            "running"
        ]),
        "{notice}"
    );
    assert_eq!(
        [CLOSE, CLOSED, CLOSE_FAILED],
        ["\0tomcat.close", "\0tomcat.closed", "\0tomcat.close_failed"],
        "{notice}"
    );
}

#[test]
fn service_command_removes_starter_proxy_variables() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(root.path().to_string_lossy().into_owned());
    cfg.agent.id = "proxy-service-fixture".into();
    let command = service_command(&cfg).unwrap();
    let environment = command.get_envs().collect::<Vec<_>>();
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        assert!(
            environment.iter().any(|(name, value)| {
                let matches = if cfg!(windows) {
                    name.to_string_lossy().eq_ignore_ascii_case(key)
                } else {
                    *name == OsStr::new(key)
                };
                matches && value.is_none()
            }),
            "{key} must be explicitly removed"
        );
    }
    let work_dir = crate::get_work_dir(&cfg).unwrap();
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            OsStr::new("serve"),
            OsStr::new("run"),
            OsStr::new("--work-dir"),
            work_dir.as_os_str(),
            OsStr::new("--agent-id"),
            OsStr::new(&cfg.agent.id),
        ]
    );
}
