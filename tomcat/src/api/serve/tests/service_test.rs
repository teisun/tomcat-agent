use std::ffi::OsStr;

use crate::api::serve::service::service_command;
use crate::AppConfig;

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
