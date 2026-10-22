//! Standard ACP helpers shared by protocol and native/ACP boundary journeys.
use super::serve::{spawn_serve_command, ServeChild};
use super::shared_serve::ServiceFixture;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub const WAIT: Duration = Duration::from_secs(20);

pub fn spawn(fx: &ServiceFixture) -> ServeChild {
    let mut command = fx.command();
    command.arg("acp");
    spawn_serve_command(command)
}
pub fn request(client: &mut ServeChild, id: Value, method: &str, params: Value) -> Vec<Value> {
    client.send_value(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
    let frames = client.recv_until(WAIT, |v| v["id"] == id);
    assert!(
        frames.iter().all(|v| v["jsonrpc"] == "2.0"),
        "non-ACP stdout: {frames:?}"
    );
    frames
}
pub fn ok(frames: &[Value]) -> &Value {
    let reply = frames.last().unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    &reply["result"]
}
pub fn initialize(client: &mut ServeChild) {
    let frames = request(
        client,
        json!(1),
        "initialize",
        json!({
            "protocolVersion":1, "clientCapabilities":{}, "clientInfo":{"name":"acp-test-host", "version":"1"}
        }),
    );
    assert_eq!(ok(&frames)["protocolVersion"], 1);
}
pub fn new_session(client: &mut ServeChild, cwd: &Path) -> String {
    let frames = request(
        client,
        json!("new"),
        "session/new",
        json!({"cwd":cwd, "mcpServers":[]}),
    );
    ok(&frames)["sessionId"].as_str().unwrap().to_owned()
}
pub fn prompt(client: &mut ServeChild, id: &str, session: &str, text: &str) {
    client.send_value(
        &json!({"jsonrpc":"2.0", "id":id, "method":"session/prompt", "params":{
            "sessionId":session, "prompt":[{"type":"text","text":text}]
        }}),
    );
}
pub fn close(client: ServeChild) {
    let output = client.wait_for_exit(WAIT);
    assert!(output.status.success(), "ACP exit: {output:?}");
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let frame: Value = serde_json::from_str(line)
            .expect("stdout must not contain private close acknowledgements");
        assert_eq!(frame["jsonrpc"], "2.0");
    }
}
