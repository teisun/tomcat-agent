//! 集成测试公共模块：日志初始化、`.env` 加载与共享 fixture。
//! 使用 Once 保证并行测试下只初始化一次，避免重复 init 导致 panic。

#![allow(dead_code)]

pub mod serve;

use rcgen::generate_simple_self_signed;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;
use tomcat::{AppConfig, DefaultLlmResolver, LlmProvider, LlmResolver, LlmScene, ModelCatalog};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

pub const DEEPSEEK_TEST_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEEPSEEK_TEST_API_BASE: &str = "https://api.deepseek.com";
pub const DEEPSEEK_TEST_MODEL_ENV: &str = "TOMCAT_E2E_DEEPSEEK_MODEL";
pub const DEEPSEEK_TEST_DEFAULT_MODEL: &str = "deepseek-v4-pro";
pub const OPENAI_TEST_MODEL_ENV: &str = "TOMCAT_E2E_OPENAI_TARGET";
pub const OPENAI_TEST_DEFAULT_MODEL: &str = "gpt-5.4_litellm-sunmi";
pub const OPENAI_GATEWAY_TEST_API_KEY_ENV: &str = "LITELLM_SUNMI_API_KEY";
pub const OPENAI_GATEWAY_TEST_BASE_URL: &str = "https://aigateway.sunmi.com";
pub const MIMO_TEST_MODEL_ENV: &str = "TOMCAT_E2E_MIMO_MODEL";
pub const MIMO_TEST_DEFAULT_MODEL: &str = "mimo-v2.5-pro";
pub const MIMO_TEST_BASE_URL_ENV: &str = "TOMCAT_E2E_MIMO_BASE_URL";
pub const MIMO_TEST_DEFAULT_BASE_URL: &str = "https://token-plan-cn.xiaomimimo.com";
pub const MIMO_TEST_API_KEY_ENV: &str = "MIMO_API_KEY";

/// 为依赖真实 LLM 凭证的集成测试加载环境变量（与 `UNIT_TEST_SPEC` / `INTEGRATION_TEST_SPEC` 对齐）。
///
/// 顺序（`dotenvy` 默认不覆盖已存在的环境变量）：
/// 1. `tomcat/.env`（`CARGO_MANIFEST_DIR`，与 `src/core/llm/tests/mocks.rs::load_dotenv` 一致）
/// 2. `dotenvy::dotenv()`：从当前工作目录向上查找 `.env`（`cargo test` 在 crate 根执行时通常同上）
pub fn load_openai_test_env() {
    let manifest_env = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(&manifest_env);
    let _ = dotenvy::dotenv();
}

/// 为通用 real-LLM / E2E 测试加载环境变量；当前统一走 DeepSeek。
pub fn load_deepseek_test_env() {
    load_openai_test_env();
}

pub fn deepseek_test_model() -> String {
    std::env::var(DEEPSEEK_TEST_MODEL_ENV)
        .unwrap_or_else(|_| DEEPSEEK_TEST_DEFAULT_MODEL.to_string())
}

pub fn e2e_openai_model() -> String {
    std::env::var(OPENAI_TEST_MODEL_ENV).unwrap_or_else(|_| OPENAI_TEST_DEFAULT_MODEL.to_string())
}

pub fn mimo_test_model() -> String {
    std::env::var(MIMO_TEST_MODEL_ENV).unwrap_or_else(|_| MIMO_TEST_DEFAULT_MODEL.to_string())
}

pub fn mimo_test_base_url() -> String {
    std::env::var(MIMO_TEST_BASE_URL_ENV).unwrap_or_else(|_| MIMO_TEST_DEFAULT_BASE_URL.to_string())
}

pub fn require_deepseek_api_key(test_name: &str) -> String {
    setup_logging();
    load_deepseek_test_env();
    std::env::var(DEEPSEEK_TEST_API_KEY_ENV).unwrap_or_else(|_| {
        panic!("{test_name} 必须设置 {DEEPSEEK_TEST_API_KEY_ENV}（环境变量或 tomcat/.env）")
    })
}

pub fn apply_deepseek_llm_config(cfg: &mut tomcat::LlmConfig) {
    cfg.default_model = deepseek_test_model();
    cfg.thinking.enabled = true;
    cfg.thinking.level = "high".to_string();
}

pub fn apply_deepseek_app_config(cfg: &mut tomcat::AppConfig) {
    apply_deepseek_llm_config(&mut cfg.llm);
    cfg.context.compaction_model = deepseek_test_model();
    maybe_write_test_models(cfg);
}

pub fn apply_openai_app_config(cfg: &mut AppConfig) {
    cfg.llm.default_model = e2e_openai_model();
    cfg.context.compaction_model = "gpt-5.4".to_string();
    maybe_write_test_models(cfg);
}

pub fn apply_openai_responses_test_config(
    cfg: &mut AppConfig,
    env_key: &str,
    base_url: Option<&str>,
) {
    if cfg.llm.default_model.trim().is_empty() {
        cfg.llm.default_model = "gpt-5.4".to_string();
    }
    if cfg.context.compaction_model.trim().is_empty() {
        cfg.context.compaction_model = cfg.llm.default_model.clone();
    }
    let model_id = cfg.llm.default_model.clone();
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id: &model_id,
            api: "openai-responses",
            provider: "openai",
            env_key,
            base_url: base_url.or(Some("https://api.openai.com")),
            model_name: None,
            thinking_format: Some("openai"),
            supports_files: true,
            supports_reasoning: true,
        },
    );
}

pub fn apply_openai_compatible_test_config(
    cfg: &mut AppConfig,
    model_id: &str,
    provider: &str,
    env_key: &str,
    base_url: &str,
    thinking_format: Option<&str>,
) {
    cfg.llm.default_model = model_id.to_string();
    cfg.context.compaction_model = model_id.to_string();
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id,
            api: "openai",
            provider,
            env_key,
            base_url: Some(base_url),
            model_name: None,
            thinking_format,
            supports_files: false,
            supports_reasoning: true,
        },
    );
}

pub fn resolve_main_provider(cfg: &AppConfig) -> Arc<dyn LlmProvider> {
    resolve_main_call(cfg).provider_impl
}

pub fn resolve_main_call(cfg: &AppConfig) -> tomcat::ResolvedCall {
    let catalog = Arc::new(ModelCatalog::load(cfg).expect("load model catalog for test"));
    let resolver = DefaultLlmResolver::new(cfg.clone(), catalog);
    resolver
        .resolve(LlmScene::Main, None)
        .expect("resolve main provider for test")
}

fn maybe_write_test_models(cfg: &AppConfig) {
    let Some(work_dir) = cfg.storage.work_dir.as_deref().map(Path::new) else {
        return;
    };
    let mut entries = vec![format!(
        r#"[[models]]
id = "{model_id}"
api = "openai"
provider = "deepseek"
api_key_env = "{env_name}"
base_url = "{base_url}"
thinking_format = "deepseek"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true }}
"#,
        model_id = deepseek_test_model(),
        env_name = DEEPSEEK_TEST_API_KEY_ENV,
        base_url = DEEPSEEK_TEST_API_BASE,
    )];
    entries.push(format!(
        r#"[[models]]
id = "gpt-5.4_litellm-sunmi"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "litellm-sunmi"
api_key_env = "{env_name}"
base_url = "{base_url}"
thinking_format = "openai"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true }}
"#,
        env_name = OPENAI_GATEWAY_TEST_API_KEY_ENV,
        base_url = OPENAI_GATEWAY_TEST_BASE_URL,
    ));
    entries.push(format!(
        r#"[[models]]
id = "{model_id}"
api = "openai"
provider = "mimo"
api_key_env = "{env_name}"
base_url = "{base_url}"
thinking_format = "doubao"
capabilities = {{ vision = false, files = false, tools = true, reasoning = true }}
"#,
        model_id = mimo_test_model(),
        env_name = MIMO_TEST_API_KEY_ENV,
        base_url = mimo_test_base_url(),
    ));
    let path = work_dir.join("models.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create test models.toml parent");
    }
    std::fs::write(path, entries.join("\n")).expect("write test models.toml");
}

struct ModelOverrideSpec<'a> {
    model_id: &'a str,
    api: &'a str,
    provider: &'a str,
    env_key: &'a str,
    base_url: Option<&'a str>,
    model_name: Option<&'a str>,
    thinking_format: Option<&'a str>,
    supports_files: bool,
    supports_reasoning: bool,
}

fn write_model_override(cfg: &AppConfig, spec: ModelOverrideSpec<'_>) {
    let Some(work_dir) = cfg.storage.work_dir.as_deref().map(Path::new) else {
        return;
    };
    let mut lines = vec![
        "[[models]]".to_string(),
        format!("id = \"{}\"", spec.model_id),
    ];
    if let Some(model_name) = spec.model_name {
        lines.push(format!("model_name = \"{model_name}\""));
    }
    lines.push(format!("api = \"{}\"", spec.api));
    lines.push(format!("provider = \"{}\"", spec.provider));
    lines.push(format!("api_key_env = \"{}\"", spec.env_key));
    if let Some(base_url) = spec.base_url {
        lines.push(format!("base_url = \"{base_url}\""));
    }
    if let Some(thinking_format) = spec.thinking_format {
        lines.push(format!("thinking_format = \"{thinking_format}\""));
    }
    lines.push(format!(
        "capabilities = {{ vision = true, files = {}, tools = true, reasoning = {}, web_search = false }}",
        spec.supports_files, spec.supports_reasoning
    ));
    let path = work_dir.join("models.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create model override parent");
    }
    std::fs::write(path, format!("{}\n", lines.join("\n"))).expect("write model override");
}

/// 初始化日志，供各集成测试在入口调用；使用 test_writer 以便 cargo test 捕获输出。
pub fn setup_logging() {
    INIT.call_once(|| {
        tracing_subscriber::registry()
            .with(fmt::layer().with_test_writer())
            .with(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
            .init();
    });
}

/// 在 `~/.tomcat/temp/` 下创建本次 E2E 专用子目录（已默认在 workspace_roots 内）。
pub fn dot_tomcat_e2e_workdir(label: &str) -> std::path::PathBuf {
    let base = tomcat::resolve_dot_tomcat_temp_dir().expect("resolve ~/.tomcat/temp");
    let dir = base.join(format!(
        "{label}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create e2e workdir under ~/.tomcat/temp");
    dir
}

/// 仓库内约定的 scratch 根：`tomcat/workspace-temp/`。
pub fn repo_workspace_temp_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("workspace-temp")
}

/// 仓库内约定的 E2E 诊断日志目录：`tomcat/workspace-temp/logs/`。
pub fn repo_workspace_temp_logs_dir() -> std::path::PathBuf {
    let dir = repo_workspace_temp_dir().join("logs");
    std::fs::create_dir_all(&dir).expect("create workspace-temp/logs for e2e");
    dir
}

/// 生成适合文件名的时间戳。
pub fn filename_timestamp() -> String {
    chrono::Local::now().format("%Y%m%d_%H%M%S_%3f").to_string()
}

/// 把任意文本收敛成低噪音 ASCII 文件名片段。
pub fn slugify_filename(input: &str, fallback: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= max_len {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// 复用固定 DEFAULT_SESSION_KEY，新建一个 fresh session 并返回对应 entry。
pub fn begin_fresh_default_session(
    sessions_dir: &Path,
    cwd: Option<&Path>,
) -> tomcat::SessionEntry {
    std::fs::create_dir_all(sessions_dir).expect("create sessions dir for e2e");
    let mgr = tomcat::SessionManager::new(sessions_dir.to_path_buf());
    mgr.new_current_session(cwd.map(|p| p.to_string_lossy().to_string()))
        .expect("create fresh default session for e2e")
}

/// 把固定 DEFAULT_SESSION_KEY 回切到指定 session_id。
pub fn switch_default_session(sessions_dir: &Path, session_id: &str) -> tomcat::SessionEntry {
    let mgr = tomcat::SessionManager::new(sessions_dir.to_path_buf());
    mgr.switch_current_to_session_id(session_id)
        .expect("switch default session for e2e")
}

#[derive(Debug, Clone)]
pub struct CreatedPlanRef {
    pub plan_id: String,
    pub path: std::path::PathBuf,
}

fn expand_home_path(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    std::path::PathBuf::from(path)
}

fn parse_created_plan_json(text: &str) -> Option<CreatedPlanRef> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let plan_id = value.get("plan_id")?.as_str()?.to_string();
    let path = expand_home_path(value.get("path")?.as_str()?);
    Some(CreatedPlanRef { plan_id, path })
}

pub fn extract_created_plan_from_messages(
    messages: &[tomcat::ChatMessage],
) -> Option<CreatedPlanRef> {
    messages.iter().rev().find_map(|msg| {
        if msg.role != tomcat::core::llm::ChatMessageRole::Tool {
            return None;
        }
        let text = match msg.content.as_ref()? {
            tomcat::core::llm::ChatMessageContent::Text(text) => text.as_str(),
            _ => return None,
        };
        parse_created_plan_json(text)
    })
}

pub fn extract_created_plan_from_transcript_path(transcript_path: &Path) -> Option<CreatedPlanRef> {
    let content = std::fs::read_to_string(transcript_path).ok()?;
    content.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let message = value.get("message")?;
        if message.get("role").and_then(|v| v.as_str()) != Some("tool") {
            return None;
        }
        let text = message.get("content")?.as_str()?;
        parse_created_plan_json(text)
    })
}

/// 测试期间把进程 cwd 切到 `path`，Drop 时还原。
pub struct CwdGuard {
    orig: Option<std::path::PathBuf>,
}

impl CwdGuard {
    pub fn set(path: &std::path::Path) -> Self {
        let orig = std::env::current_dir().ok();
        std::env::set_current_dir(path).expect("set_current_dir for e2e");
        Self { orig }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.orig {
            let _ = std::env::set_current_dir(p);
        }
    }
}

pub struct HttpsTestServer {
    addr: std::net::SocketAddr,
    max_concurrency: Arc<AtomicUsize>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpsTestServer {
    pub async fn start(
        hostname: &str,
        status_line: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
    ) -> Self {
        let certified = generate_simple_self_signed(vec![hostname.to_string()])
            .expect("generate self-signed cert");
        let cert_chain = vec![CertificateDer::from(certified.cert.der().to_vec())];
        let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let server_config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .expect("build rustls server config");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind https test server");
        let addr = listener.local_addr().expect("listener addr");
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let current_concurrency = Arc::new(AtomicUsize::new(0));
        let max_concurrency = Arc::new(AtomicUsize::new(0));
        let task_current = Arc::clone(&current_concurrency);
        let task_max = Arc::clone(&max_concurrency);
        let status_line = status_line.to_string();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let Ok((stream, _)) = accept else { break; };
                        let acceptor = acceptor.clone();
                        let headers = headers.clone();
                        let body = body.clone();
                        let status_line = status_line.clone();
                        let task_current = Arc::clone(&task_current);
                        let task_max = Arc::clone(&task_max);
                        tokio::spawn(async move {
                            let Ok(mut tls_stream) = acceptor.accept(stream).await else {
                                return;
                            };
                            let mut request_buf = vec![0u8; 4096];
                            let _ = tls_stream.read(&mut request_buf).await;
                            let in_flight = task_current.fetch_add(1, Ordering::SeqCst) + 1;
                            task_max.fetch_max(in_flight, Ordering::SeqCst);
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            let mut response = format!("HTTP/1.1 {status_line}\r\n");
                            for (name, value) in &headers {
                                response.push_str(name);
                                response.push_str(": ");
                                response.push_str(value);
                                response.push_str("\r\n");
                            }
                            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
                            response.push_str("Connection: close\r\n\r\n");
                            let _ = tls_stream.write_all(response.as_bytes()).await;
                            let _ = tls_stream.write_all(&body).await;
                            let _ = tls_stream.flush().await;
                            let _ = tls_stream.shutdown().await;
                            task_current.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                }
            }
        });
        Self {
            addr,
            max_concurrency,
            shutdown_tx: Some(shutdown_tx),
            task,
        }
    }

    pub fn client_for(&self, hostname: &str, timeout: Duration) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .resolve(hostname, self.addr)
            .build()
            .expect("build https test client")
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency.load(Ordering::SeqCst)
    }

    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }
}

impl Drop for HttpsTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}

pub struct ProxyTestServer {
    addr: std::net::SocketAddr,
    seen_hosts: Arc<Mutex<Vec<String>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyTestServer {
    pub async fn start(routes: Vec<(String, std::net::SocketAddr)>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind proxy test server");
        let addr = listener.local_addr().expect("proxy listener addr");
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let routes = Arc::new(
            routes
                .into_iter()
                .map(|(host, addr)| (host.to_ascii_lowercase(), addr))
                .collect::<HashMap<_, _>>(),
        );
        let seen_hosts = Arc::new(Mutex::new(Vec::new()));
        let task_routes = Arc::clone(&routes);
        let task_seen_hosts = Arc::clone(&seen_hosts);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let Ok((stream, _)) = accept else { break; };
                        let routes = Arc::clone(&task_routes);
                        let seen_hosts = Arc::clone(&task_seen_hosts);
                        tokio::spawn(async move {
                            handle_proxy_connection(stream, routes, seen_hosts).await;
                        });
                    }
                }
            }
        });
        Self {
            addr,
            seen_hosts,
            shutdown_tx: Some(shutdown_tx),
            task,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn saw_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.seen_hosts
            .lock()
            .expect("proxy seen_hosts mutex")
            .iter()
            .any(|recorded| recorded == &host)
    }

    pub fn seen_hosts(&self) -> Vec<String> {
        self.seen_hosts
            .lock()
            .expect("proxy seen_hosts mutex")
            .clone()
    }
}

impl Drop for ProxyTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}

async fn handle_proxy_connection(
    mut stream: TcpStream,
    routes: Arc<HashMap<String, std::net::SocketAddr>>,
    seen_hosts: Arc<Mutex<Vec<String>>>,
) {
    let Some(authority) = read_connect_authority(&mut stream).await else {
        let _ = stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority.as_str())
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    seen_hosts
        .lock()
        .expect("proxy seen_hosts mutex")
        .push(host.clone());
    let Some(target_addr) = routes.get(&host).copied() else {
        let _ = stream
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let Ok(mut upstream) = TcpStream::connect(target_addr).await else {
        let _ = stream
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    if stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    let _ = copy_bidirectional(&mut stream, &mut upstream).await;
}

async fn read_connect_authority(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 8192 {
            break;
        }
    }
    let header_end = buffer.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let header = String::from_utf8_lossy(&buffer[..header_end]);
    let request_line = header.lines().next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?;
    let authority = parts.next()?;
    if !method.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    Some(authority.to_string())
}
