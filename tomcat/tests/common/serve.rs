use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tomcat::load_config_toml_file;

#[allow(deprecated)]
pub fn cargo_bin_path() -> PathBuf {
    assert_cmd::cargo::cargo_bin("tomcat")
}

#[derive(Debug)]
pub struct ServeFixture {
    _home: tempfile::TempDir,
    pub home_path: PathBuf,
    pub workspace: PathBuf,
}

pub fn setup_serve_fixture(base_url: &str) -> ServeFixture {
    let home = tempfile::tempdir().expect("temp home");
    let home_path = home.path().to_path_buf();
    let workspace = home_path.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");

    let init_output = StdCommand::new(cargo_bin_path())
        .env_remove("TOMCAT__LLM__DEFAULT_MODEL")
        .args(["init"])
        .env("HOME", &home_path)
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("run tomcat init");
    assert!(
        init_output.status.success(),
        "tomcat init should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&init_output.stdout),
        String::from_utf8_lossy(&init_output.stderr)
    );

    let config_path = home_path.join(".tomcat").join("tomcat.config.toml");
    let mut cfg = load_config_toml_file(&config_path).expect("load config");
    cfg.storage.work_dir = Some(home_path.join(".tomcat").to_string_lossy().to_string());
    cfg.llm.default_model = "gpt-5.4".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    cfg.llm.title_model = Some("utility-flash".to_string());
    cfg.skills.enabled = false;
    fs::write(
        &config_path,
        toml::to_string_pretty(&cfg).expect("serialize serve config"),
    )
    .expect("persist serve config");

    fs::write(
        home_path.join(".tomcat").join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4"
api = "openai"
provider = "openai"
base_url = "{base_url}"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true, web_search = false }}

[[models]]
id = "utility-flash"
api = "openai"
provider = "openai"
base_url = "http://127.0.0.1:1"
capabilities = {{ vision = false, files = false, tools = false, reasoning = false, web_search = false }}
"#
        ),
    )
    .expect("write models override");

    ServeFixture {
        _home: home,
        home_path,
        workspace,
    }
}

pub struct ServeChild {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_rx: mpsc::Receiver<String>,
    stderr_buf: Arc<Mutex<String>>,
}

impl ServeChild {
    pub fn send_value(&mut self, value: &Value) {
        self.send_raw(&value.to_string());
    }

    pub fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("serve stdin still available");
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .expect("write serve stdin");
        stdin.flush().expect("flush serve stdin");
    }

    pub fn recv_value(&self, timeout: Duration) -> Value {
        let line = self
            .stdout_rx
            .recv_timeout(timeout)
            .unwrap_or_else(|err| panic!("timed out waiting for serve stdout: {err}; stderr={}", self.stderr()));
        serde_json::from_str(&line).unwrap_or_else(|err| {
            panic!("stdout line should be json: {err}; line={line}");
        })
    }

    pub fn recv_until<F>(&self, timeout: Duration, predicate: F) -> Vec<Value>
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let now = Instant::now();
            assert!(
                now < deadline,
                "timed out waiting for matching serve stdout"
            );
            let line = self
                .stdout_rx
                .recv_timeout(deadline.saturating_duration_since(now))
                .unwrap_or_else(|err| {
                    panic!(
                        "timed out waiting for matching serve stdout: {err}; seen={out:?}; stderr={}",
                        self.stderr()
                    )
                });
            let value = serde_json::from_str(&line).unwrap_or_else(|err| {
                panic!("stdout line should be json: {err}; line={line}");
            });
            let matched = predicate(&value);
            out.push(value);
            if matched {
                return out;
            }
        }
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn wait_for_exit(mut self, timeout: Duration) -> std::process::Output {
        self.close_stdin();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait serve child") {
                let mut stdout = Vec::new();
                while let Ok(line) = self.stdout_rx.try_recv() {
                    stdout.extend_from_slice(line.as_bytes());
                    stdout.push(b'\n');
                }
                let stderr = self
                    .stderr_buf
                    .lock()
                    .expect("stderr lock")
                    .clone()
                    .into_bytes();
                return std::process::Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for serve exit"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn stderr(&self) -> String {
        self.stderr_buf.lock().expect("stderr lock").clone()
    }
}

impl Drop for ServeChild {
    fn drop(&mut self) {
        let _ = self.stdin.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn spawn_serve_child(fx: &ServeFixture) -> ServeChild {
    let mut child = StdCommand::new(cargo_bin_path())
        .env_remove("TOMCAT__LLM__DEFAULT_MODEL")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("OPENAI_API_KEY", "dummy-key")
        .env("RUST_LOG", "tomcat=debug,info")
        .current_dir(&fx.workspace)
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tomcat serve");

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("serve stdout");
    let stderr = child.stderr.take().expect("serve stderr");
    let (tx, rx) = mpsc::channel();
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf_thread = Arc::clone(&stderr_buf);

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        let _ = reader.read_to_string(&mut buf);
        *stderr_buf_thread.lock().expect("stderr lock") = buf;
    });

    ServeChild {
        child,
        stdin,
        stdout_rx: rx,
        stderr_buf,
    }
}

#[derive(Debug, Clone)]
pub struct ScriptedPart {
    pub delay_ms: u64,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ScriptedResponse {
    pub parts: Vec<ScriptedPart>,
}

pub struct ScriptedOpenAiServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    join: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl ScriptedOpenAiServer {
    pub fn captured_requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for ScriptedOpenAiServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub fn spawn_scripted_openai_stream_server(
    responses: Vec<ScriptedResponse>,
) -> ScriptedOpenAiServer {
    spawn_scripted_openai_stream_server_internal(responses, false)
}

pub fn spawn_scripted_openai_stream_server_with_auto_title(
    responses: Vec<ScriptedResponse>,
) -> ScriptedOpenAiServer {
    spawn_scripted_openai_stream_server_internal(responses, true)
}

fn spawn_scripted_openai_stream_server_internal(
    responses: Vec<ScriptedResponse>,
    auto_title_response: bool,
) -> ScriptedOpenAiServer {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock llm server");
    listener
        .set_nonblocking(true)
        .expect("mock llm listener nonblocking");
    let addr = listener.local_addr().expect("local addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_thread = Arc::clone(&requests);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_thread = Arc::clone(&shutdown);
    let join = thread::spawn(move || {
        let mut scripted = std::collections::VecDeque::from(responses);
        loop {
            if shutdown_thread.load(Ordering::Relaxed) {
                break;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(err) => panic!("accept mock request: {err}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("stream read timeout");
            let request = read_http_request(&mut stream);
            if request.trim().is_empty() {
                continue;
            }
            requests_thread
                .lock()
                .expect("requests lock")
                .push(request.clone());
            let handled_title = auto_title_response && request_is_session_title_request(&request);
            let (headers, parts) = if handled_title {
                (
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                    vec![ScriptedPart {
                        delay_ms: 0,
                        body: session_title_response_json(&request, "Generated title"),
                    }],
                )
            } else {
                let scripted = scripted
                    .pop_front()
                    .expect("missing scripted response for streamed request");
                (
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                    scripted.parts,
                )
            };
            stream.write_all(headers.as_bytes()).expect("write headers");
            for part in parts {
                if part.delay_ms > 0 {
                    thread::sleep(Duration::from_millis(part.delay_ms));
                }
                stream
                    .write_all(part.body.as_bytes())
                    .expect("write response part");
                stream.flush().expect("flush response part");
            }
            if scripted.is_empty() && handled_title {
                break;
            }
            if !handled_title && scripted.is_empty() {
                break;
            }
        }
    });
    ScriptedOpenAiServer {
        base_url: format!("http://{addr}"),
        requests,
        join: Some(join),
        shutdown,
    }
}

pub fn captured_non_title_requests(server: &ScriptedOpenAiServer) -> Vec<String> {
    server
        .captured_requests()
        .into_iter()
        .filter(|request| !request_is_session_title_request(request))
        .collect()
}

fn request_is_session_title_request(request: &str) -> bool {
    let Some((_, body)) = request.split_once("\r\n\r\n") else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    if value.get("stream").and_then(Value::as_bool) != Some(false) {
        return false;
    }
    body.contains("Generate a short chat title from the user's first message.\\n")
}

fn session_title_response_json(request: &str, title: &str) -> String {
    if request.starts_with("POST /v1/responses ") {
        return serde_json::json!({
            "id": "title-mock",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": title,
                }],
            }],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "total_tokens": 2,
            },
        })
        .to_string();
    }

    serde_json::json!({
        "id": "title-mock",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": title,
            },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2,
        },
    })
    .to_string()
}

pub fn sse_delta(content: &str) -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\n"),
    }
}

pub fn sse_finish(reason: &str) -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: format!("data: {{\"choices\":[{{\"finish_reason\":\"{reason}\"}}]}}\n\n"),
    }
}

pub fn sse_done() -> ScriptedPart {
    ScriptedPart {
        delay_ms: 0,
        body: "data: [DONE]\n\n".to_string(),
    }
}

pub fn sse_tool_call(id: &str, name: &str, args_json: &str) -> ScriptedPart {
    let arguments = serde_json::to_string(args_json).expect("serialize tool call args");
    ScriptedPart {
        delay_ms: 0,
        body: format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"{id}\",\"function\":{{\"name\":\"{name}\",\"arguments\":{arguments}}}}}]}}}}]}}\n\n"
        ),
    }
}

pub fn delayed(part: ScriptedPart, delay_ms: u64) -> ScriptedResponse {
    ScriptedResponse {
        parts: vec![ScriptedPart {
            delay_ms,
            body: part.body,
        }],
    }
}

pub fn response(parts: Vec<ScriptedPart>) -> ScriptedResponse {
    ScriptedResponse { parts }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut body_start = None;
    let mut content_len = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if body_start.is_none() {
                    if let Some(end) = find_header_end(&raw) {
                        body_start = Some(end + 4);
                        let headers = String::from_utf8_lossy(&raw[..end]);
                        content_len = headers.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        });
                    }
                }
                if let (Some(start), Some(len)) = (body_start, content_len) {
                    if raw.len() >= start + len {
                        break;
                    }
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if let (Some(start), Some(len)) = (body_start, content_len) {
                    let body_len = raw.len().saturating_sub(start);
                    if body_len < len {
                        assert!(
                            Instant::now() < deadline,
                            "timed out reading mock request body: got {body_len} of {len} bytes"
                        );
                        continue;
                    }
                }
                if raw.is_empty() && Instant::now() < deadline {
                    continue;
                }
                break;
            }
            Err(err) => panic!("read mock request: {err}"),
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}

pub fn try_extract_json_body(request: &str) -> Option<Value> {
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    serde_json::from_str(body).ok()
}

pub fn extract_json_body(request: &str) -> Value {
    try_extract_json_body(request)
        .unwrap_or_else(|| panic!("request body should be json: {request}"))
}

pub fn assert_ndjson_line(value: &Value) {
    assert!(
        value.get("type").is_some(),
        "expected typed NDJSON frame: {value}"
    );
}

pub fn fixture_path(parts: &[&str]) -> PathBuf {
    let mut path = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}
