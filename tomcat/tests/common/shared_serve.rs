//! Disposable shared-service fixture, used by native IPC and ACP process tests.
use super::serve::{isolated_serve_command, setup_serve_fixture_with_home, ServeFixture};
use serde_json::{json, Value};
use std::process::{Command, Output};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub struct ServiceFixture(pub ServeFixture);
impl ServiceFixture {
    pub fn new(base_url: &str) -> Self {
        // macOS's default temp path can exceed sockaddr_un's limit.
        #[cfg(unix)]
        let home = tempfile::Builder::new()
            .prefix("tc-")
            .tempdir_in("/tmp")
            .unwrap();
        #[cfg(windows)]
        let home = tempfile::tempdir().unwrap();
        let fixture = Self(setup_serve_fixture_with_home(base_url, home));
        let output = fixture
            .command()
            .args(["model", "key", "set", "openai", "dummy-key"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "save isolated dummy key: {output:?}"
        );
        fixture
    }
    pub fn command(&self) -> Command {
        let mut command = isolated_serve_command(&self.0.home_path);
        command.current_dir(&self.0.workspace);
        command
    }
    pub fn action(&self, action: &str) -> Value {
        output_json(self.command().args(["serve", action]).output().unwrap())
    }
    pub fn configure(&self, update: impl FnOnce(&mut tomcat::AppConfig)) {
        let path = self.0.home_path.join(".tomcat/tomcat.config.toml");
        let mut cfg = tomcat::load_config_toml_file(&path).unwrap();
        update(&mut cfg);
        std::fs::write(path, toml::to_string_pretty(&cfg).unwrap()).unwrap();
    }
}
impl Drop for ServiceFixture {
    fn drop(&mut self) {
        let result = self.command().args(["serve", "stop"]).output();
        if !matches!(&result, Ok(output) if output.status.success()) {
            eprintln!(
                "isolated service cleanup failed at {}: {result:?}",
                self.0.home_path.display()
            );
        }
    }
}

const WAIT: Duration = Duration::from_secs(20);
pub trait TestIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> TestIo for T {}

/// The same native IPC client used by shared-service and ACP observation tests.
pub struct NativeClient(pub BufReader<Box<dyn TestIo>>);
impl NativeClient {
    pub async fn open(info: &Value, version: &str) -> Self {
        Self::open_with_environment(info, version, json!({})).await
    }
    pub async fn open_with_environment(info: &Value, version: &str, environment: Value) -> Self {
        let endpoint = info["endpoint"].as_str().unwrap();
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(endpoint).await.unwrap();
        #[cfg(windows)]
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(endpoint)
            .unwrap();
        let mut client = Self(BufReader::new(Box::new(stream)));
        client.send(json!({"service":"tomcat","protocolVersion":1,"clientVersion":version,"protocol":"native","environment":environment})).await;
        client
    }
    pub async fn connect(info: &Value) -> Self {
        let mut client = Self::open(info, env!("CARGO_PKG_VERSION")).await;
        let hello = client.next().await;
        assert_eq!(hello["instanceId"], info["instanceId"]);
        assert!(hello.get("error").is_none(), "{hello}");
        client.send(json!({"type":"control_request", "requestId":"init", "subtype":"initialize", "payload":{}})).await;
        client
            .until(|v| v["type"] == "control_response" && v["requestId"] == "init")
            .await;
        client
    }
    pub async fn send(&mut self, value: Value) {
        self.0
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        self.0.flush().await.unwrap();
    }
    pub async fn next(&mut self) -> Value {
        let mut line = String::new();
        let count = tokio::time::timeout(WAIT, self.0.read_line(&mut line))
            .await
            .expect("socket frame deadline")
            .unwrap();
        assert!(count > 0, "unexpected service EOF");
        serde_json::from_str(&line).unwrap()
    }
    pub async fn until(&mut self, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        tokio::time::timeout(WAIT, async {
            let mut frames = Vec::new();
            loop {
                let frame = self.next().await;
                let matched = predicate(&frame);
                frames.push(frame);
                if matched {
                    return frames;
                }
            }
        })
        .await
        .expect("matching frame deadline")
    }
    pub async fn request(&mut self, request: Value) -> Value {
        let id = request["id"].clone();
        self.send(request).await;
        self.until(|v| v["id"] == id).await.pop().unwrap()
    }
    pub async fn new_session(&mut self, cwd: &Path) -> String {
        let reply = self
            .request(json!({"type":"new_session","id":"same-id","params":{"cwd":cwd}}))
            .await;
        assert_eq!(reply["success"], true, "{reply}");
        reply["payload"]["sessionId"].as_str().unwrap().to_owned()
    }
}

pub fn output_json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("invalid service JSON: {e}; {output:?}"))
}
