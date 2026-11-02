use super::*;
use serde_json::json;

#[test]
fn session_mcp_setup_maps_stdio_http_and_redacts_private_values() {
    let wire = json!([
        {"name":"local","command":"/fixture/bin","args":["secret-arg"],"env":[{"name":"TOKEN","value":"secret-env"}]},
        {"type":"http","name":"remote","url":"https://example.test/mcp","headers":[{"name":"Authorization","value":"Bearer secret-header"}]}
    ]);
    let servers = serde_json::from_value::<Vec<acp::McpServer>>(wire).unwrap();
    let config = validate_setup(&servers, &[]).unwrap();
    let local = &config.0["local"];
    assert_eq!(local.command, "/fixture/bin");
    assert_eq!(local.args, ["secret-arg"]);
    assert_eq!(local.env["TOKEN"], "secret-env");
    assert!(
        local.cwd.is_none(),
        "cwd is assigned by the session, not protocol decoding"
    );
    let remote = &config.0["remote"];
    assert_eq!(remote.url.as_deref(), Some("https://example.test/mcp"));
    assert_eq!(remote.headers["Authorization"], "Bearer secret-header");
    assert!(!format!("{config:?}").contains("secret"));
}

#[test]
fn session_mcp_setup_rejects_invalid_config_before_native_session_creation() {
    for (case, wire) in [
        json!([{"name":"","command":"/fixture","args":[],"env":[]}]),
        json!([{"name":"dup","command":"/fixture","args":[],"env":[]}, {"name":"dup","command":"/other","args":[],"env":[]}]),
        json!([{"name":"env","command":"/fixture","args":[],"env":[{"name":"INVALID=KEY","value":"x"}]}]),
        json!([{"type":"sse","name":"old","url":"https://example.test/sse","headers":[]}]),
        json!([{"type":"http","name":"bad","url":"file:///etc/passwd","headers":[]}]),
        json!([{"type":"http","name":"bad","url":"https://example.test/mcp","headers":[{"name":"Authorization","value":"token\r\ninjected: bad"}]}]),
    ].into_iter().enumerate() {
        let servers = serde_json::from_value::<Vec<acp::McpServer>>(wire).unwrap();
        assert!(validate_setup(&servers, &[]).is_err(), "accepted invalid case {case}");
    }
    assert!(validate_setup(&[], &["/unsupported".into()]).is_err());
    assert!(validate_setup(&[], &[]).unwrap().0.is_empty());
}
