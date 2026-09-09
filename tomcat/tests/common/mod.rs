//! 集成测试公共模块：日志初始化、`.env` 加载与共享 fixture。
//! 使用 Once 保证并行测试下只初始化一次，避免重复 init 导致 panic。

#![allow(dead_code)]

pub mod serve;

use base64::Engine as _;
use native_tls::{Identity, TlsAcceptor as NativeTlsAcceptor};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tokio::io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_native_tls::TlsAcceptor;
use tomcat::{
    AppConfig, DefaultLlmResolver, LlmProvider, LlmResolver, LlmScene, ModelCatalog,
    ModelPrefsStore, ThinkingLevel,
};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();
static TEMP_HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub const DEEPSEEK_TEST_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEEPSEEK_TEST_API_BASE: &str = "https://api.deepseek.com";
pub const DEEPSEEK_TEST_MODEL_ENV: &str = "TOMCAT_E2E_DEEPSEEK_MODEL";
pub const DEEPSEEK_TEST_DEFAULT_MODEL: &str = "deepseek-v4-pro";
pub const OPENAI_TEST_MODEL_ENV: &str = "TOMCAT_E2E_OPENAI_TARGET";
pub const OPENAI_TEST_DEFAULT_MODEL: &str = "gpt-5.4_litellm-sunmi";
pub const OPENAI_GATEWAY_TEST_API_KEY_ENV: &str = "LITELLM_SUNMI_API_KEY";
pub const OPENAI_GATEWAY_TEST_BASE_URL: &str = "https://aigateway.sunmi.com";
pub const FCODEX_TEST_MODEL_ENV: &str = "TOMCAT_E2E_FCODEX_MODEL";
pub const FCODEX_TEST_DEFAULT_MODEL: &str = "fcodex/gpt-5.6-sol";
pub const FCODEX_TEST_API_KEY_ENV: &str = "FCODEX_OPENAI_API_KEY";
/// Keep this aligned with the user-facing fcodex Anthropic model entry in
/// `~/.tomcat/models.toml`; a different test-only key name silently skips
/// real gateway coverage.
pub const FCODEX_ANTHROPIC_TEST_API_KEY_ENV: &str = "FCODEX_ANTHROPIC_API_KEY";
pub const FCODEX_ANTHROPIC_TEST_MODEL: &str = "fcodex/claude-opus-5";
pub const FCODEX_TEST_BASE_URL_ENV: &str = "TOMCAT_E2E_FCODEX_BASE_URL";
pub const FCODEX_TEST_DEFAULT_BASE_URL: &str = "https://fcodex.top";
pub const MIMO_TEST_MODEL_ENV: &str = "TOMCAT_E2E_MIMO_MODEL";
pub const MIMO_TEST_DEFAULT_MODEL: &str = "mimo-v2.5-pro";
pub const MIMO_TEST_BASE_URL_ENV: &str = "TOMCAT_E2E_MIMO_BASE_URL";
pub const MIMO_TEST_DEFAULT_BASE_URL: &str = "https://token-plan-cn.xiaomimimo.com";
pub const MIMO_TEST_API_KEY_ENV: &str = "MIMO_API_KEY";
pub const KIMI_TEST_MODEL_ENV: &str = "TOMCAT_E2E_KIMI_MODEL";
pub const KIMI_TEST_DEFAULT_MODEL: &str = "kimi-k3";
pub const KIMI_TEST_API_KEY_ENV: &str = "MOONSHOT_API_KEY";
pub const KIMI_TEST_BASE_URL_ENV: &str = "TOMCAT_E2E_KIMI_BASE_URL";
pub const KIMI_TEST_DEFAULT_BASE_URL: &str = "https://api.moonshot.cn";
pub const ANTHROPIC_TEST_MODEL_ENV: &str = "TOMCAT_E2E_ANTHROPIC_MODEL";
pub const ANTHROPIC_TEST_DEFAULT_MODEL: &str = "claude-opus-4-6";
pub const ANTHROPIC_TEST_BASE_URL_ENV: &str = "TOMCAT_E2E_ANTHROPIC_BASE_URL";
pub const ANTHROPIC_TEST_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_TEST_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// 为依赖真实 LLM 凭证的集成测试加载环境变量（与 `UNIT_TEST_SPEC` / `INTEGRATION_TEST_SPEC` 对齐）。
///
/// 顺序（`dotenvy` 默认不覆盖已存在的环境变量）：
/// 1. `tomcat/.env`（`CARGO_MANIFEST_DIR`，与 `src/core/llm/tests/mocks.rs::load_dotenv` 一致）
/// 2. `dotenvy::dotenv()`：从当前工作目录向上查找 `.env`（`cargo test` 在 crate 根执行时通常同上）
pub fn load_openai_test_env() {
    if std::env::var("TOMCAT_TEST_EXPORTED_ENV_ONLY")
        .ok()
        .as_deref()
        == Some("1")
    {
        return;
    }
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
    std::env::var(OPENAI_TEST_MODEL_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OPENAI_TEST_DEFAULT_MODEL.to_string())
}

pub fn mimo_test_model() -> String {
    std::env::var(MIMO_TEST_MODEL_ENV).unwrap_or_else(|_| MIMO_TEST_DEFAULT_MODEL.to_string())
}

pub fn fcodex_test_model() -> String {
    std::env::var(FCODEX_TEST_MODEL_ENV).unwrap_or_else(|_| FCODEX_TEST_DEFAULT_MODEL.to_string())
}

pub fn fcodex_test_base_url() -> String {
    std::env::var(FCODEX_TEST_BASE_URL_ENV)
        .unwrap_or_else(|_| FCODEX_TEST_DEFAULT_BASE_URL.to_string())
}

pub fn mimo_test_base_url() -> String {
    std::env::var(MIMO_TEST_BASE_URL_ENV).unwrap_or_else(|_| MIMO_TEST_DEFAULT_BASE_URL.to_string())
}

pub fn kimi_test_model() -> String {
    std::env::var(KIMI_TEST_MODEL_ENV).unwrap_or_else(|_| KIMI_TEST_DEFAULT_MODEL.to_string())
}

pub fn kimi_test_base_url() -> String {
    std::env::var(KIMI_TEST_BASE_URL_ENV).unwrap_or_else(|_| KIMI_TEST_DEFAULT_BASE_URL.to_string())
}

pub fn anthropic_test_model() -> String {
    std::env::var(ANTHROPIC_TEST_MODEL_ENV)
        .unwrap_or_else(|_| ANTHROPIC_TEST_DEFAULT_MODEL.to_string())
}

pub fn anthropic_test_base_url() -> String {
    std::env::var(ANTHROPIC_TEST_BASE_URL_ENV)
        .unwrap_or_else(|_| ANTHROPIC_TEST_DEFAULT_BASE_URL.to_string())
}

pub fn openai_target_uses_builtin_responses_key(target: &str) -> bool {
    matches!(target, "gpt-5.2" | "gpt-5.4" | "gpt-5.5" | "gpt-5.6")
}

pub fn openai_test_api_key_env_for_model(target: &str) -> &'static str {
    if openai_target_uses_builtin_responses_key(target) {
        "OPENAI_API_KEY"
    } else {
        OPENAI_GATEWAY_TEST_API_KEY_ENV
    }
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
    cfg.context.compaction_model = cfg.llm.default_model.clone();
    maybe_write_test_models(cfg);
}

pub fn apply_fcodex_app_config(cfg: &mut AppConfig) {
    let model_id = fcodex_test_model();
    apply_fcodex_responses_app_config(cfg, &model_id);
}

pub fn apply_fcodex_responses_app_config(cfg: &mut AppConfig, model_id: &str) {
    let base_url = fcodex_test_base_url();
    let model_name = model_id.strip_prefix("fcodex/").unwrap_or(model_id);
    cfg.llm.default_model = model_id.to_string();
    cfg.context.compaction_model = model_id.to_string();
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id,
            api: "openai-responses",
            provider: "fcodex",
            env_key: FCODEX_TEST_API_KEY_ENV,
            base_url: Some(base_url.as_str()),
            model_name: Some(model_name),
            thinking_format: Some("openai"),
            supports_files: true,
            supports_reasoning: true,
        },
    );
}

pub fn apply_fcodex_anthropic_app_config(cfg: &mut AppConfig) {
    let base_url = fcodex_test_base_url();
    cfg.llm.default_model = FCODEX_ANTHROPIC_TEST_MODEL.to_string();
    cfg.context.compaction_model = FCODEX_ANTHROPIC_TEST_MODEL.to_string();
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id: FCODEX_ANTHROPIC_TEST_MODEL,
            api: "anthropic-messages",
            provider: "fcodex",
            env_key: FCODEX_ANTHROPIC_TEST_API_KEY_ENV,
            base_url: Some(base_url.as_str()),
            model_name: Some("claude-opus-5"),
            thinking_format: Some("anthropic-adaptive"),
            supports_files: true,
            supports_reasoning: true,
        },
    );
    let path = cfg
        .storage
        .work_dir
        .as_deref()
        .map(Path::new)
        .expect("fcodex Anthropic probe has a work directory")
        .join("models.toml");
    let mut model_toml =
        std::fs::read_to_string(&path).expect("read fcodex Anthropic probe model override");
    model_toml.push_str("context_window = 1000000\nmax_output_tokens = 128000\n");
    std::fs::write(path, model_toml).expect("write fcodex Anthropic probe capabilities");
}

pub fn apply_kimi_app_config(cfg: &mut AppConfig) {
    let model_id = kimi_test_model();
    let base_url = kimi_test_base_url();
    let is_kimi_k3 = model_id.eq_ignore_ascii_case("kimi-k3");
    cfg.llm.default_model = model_id.clone();
    cfg.context.compaction_model = model_id;
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id: cfg.llm.default_model.as_str(),
            api: "openai",
            provider: "moonshot",
            env_key: KIMI_TEST_API_KEY_ENV,
            base_url: Some(base_url.as_str()),
            model_name: None,
            thinking_format: Some(if is_kimi_k3 { "openai" } else { "doubao" }),
            supports_files: true,
            supports_reasoning: true,
        },
    );
}

pub fn apply_anthropic_app_config(cfg: &mut AppConfig) {
    let model_id = anthropic_test_model();
    let base_url = anthropic_test_base_url();
    cfg.llm.default_model = model_id.clone();
    cfg.context.compaction_model = model_id.clone();
    write_model_override(
        cfg,
        ModelOverrideSpec {
            model_id: &model_id,
            api: "anthropic-messages",
            provider: "anthropic",
            env_key: ANTHROPIC_TEST_API_KEY_ENV,
            base_url: Some(base_url.as_str()),
            model_name: None,
            thinking_format: Some("anthropic"),
            supports_files: true,
            supports_reasoning: true,
        },
    );
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

pub fn model_prefs_for(cfg: &AppConfig) -> Arc<ModelPrefsStore> {
    Arc::new(
        ModelPrefsStore::load(
            tomcat::resolve_model_thinking_path(cfg).expect("model preference path"),
            ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0,
        )
        .expect("model preferences"),
    )
}

pub fn resolve_main_call(cfg: &AppConfig) -> tomcat::ResolvedCall {
    let catalog = Arc::new(ModelCatalog::load(cfg).expect("load model catalog for test"));
    let resolver = DefaultLlmResolver::new(cfg.clone(), catalog, model_prefs_for(cfg));
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

/// 手动缓存探针要测的 `~/.tomcat/models.toml` model id（例：`idatatlas/gpt-5.6-terra`）。
pub const CACHE_PROBE_MODEL_ENV: &str = "TOMCAT_E2E_CACHE_PROBE_MODEL";
/// 覆盖真实 models.toml 路径（默认 `$HOME/.tomcat/models.toml`）。
pub const MODELS_TOML_PATH_ENV: &str = "TOMCAT_E2E_MODELS_TOML";

/// 真实 `~/.tomcat/models.toml` 路径。**必须在 `TempHomeGuard::new()` 之前调用**：guard 会把 HOME 切到 tempdir。
pub fn real_models_toml_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os(MODELS_TOML_PATH_ENV) {
        return std::path::PathBuf::from(path);
    }
    real_home().join(".tomcat").join("models.toml")
}

/// 真实运行时凭证文件 `~/.tomcat/assets/.env`（`tomcat init` 生成）。同样必须在切 HOME 之前调用。
pub fn real_runtime_env_path() -> std::path::PathBuf {
    real_home().join(".tomcat").join("assets").join(".env")
}

fn real_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

/// 把真实 models.toml 里 `id == model_id` 的那条 `[[models]]` **原样**搬进测试 work_dir 的 models.toml，
/// 让测试用与用户线上完全相同的 api / provider / base_url / 能力配置访问同一网关；并把该条目
/// `api_key_env` 指向的凭证从 `runtime_env` 注入进程环境（进程里已有则不覆盖）。
///
/// 返回 wire model name（`model_name`，缺省为 id 去掉 `provider/` 前缀），供日志标注。
pub fn apply_models_toml_entry_app_config(
    cfg: &mut AppConfig,
    models_toml: &Path,
    runtime_env: &Path,
    model_id: &str,
) -> Result<String, String> {
    let text = std::fs::read_to_string(models_toml)
        .map_err(|err| format!("read {}: {err}", models_toml.display()))?;
    let doc: toml::Value =
        toml::from_str(&text).map_err(|err| format!("parse {}: {err}", models_toml.display()))?;
    let entry = doc
        .get("models")
        .and_then(toml::Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|m| m.get("id").and_then(toml::Value::as_str) == Some(model_id))
        })
        .cloned()
        .ok_or_else(|| format!("model `{model_id}` not found in {}", models_toml.display()))?;

    let api_key_env = entry
        .get("api_key_env")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("model `{model_id}` has no api_key_env"))?
        .to_string();
    if std::env::var(&api_key_env)
        .ok()
        .is_none_or(|value| value.trim().is_empty())
    {
        // 与运行时 `infra::config::read_env_entries` 同源（dotenvy），这里只读不写。
        let value = dotenvy::from_path_iter(runtime_env)
            .map_err(|err| format!("read {}: {err}", runtime_env.display()))?
            .filter_map(Result::ok)
            .find_map(|(key, value)| (key == api_key_env).then_some(value));
        match value.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => std::env::set_var(&api_key_env, value),
            _ => {
                return Err(format!(
                    "`{api_key_env}` is neither exported nor present in {}",
                    runtime_env.display()
                ))
            }
        }
    }

    let wire_model = entry
        .get("model_name")
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            model_id
                .rsplit_once('/')
                .map_or(model_id, |(_, name)| name)
                .to_string()
        });

    let mut root = toml::map::Map::new();
    root.insert("models".to_string(), toml::Value::Array(vec![entry]));
    let rendered = toml::to_string(&toml::Value::Table(root))
        .map_err(|err| format!("serialize models.toml entry: {err}"))?;
    let work_dir = cfg
        .storage
        .work_dir
        .as_deref()
        .map(Path::new)
        .ok_or("cfg.storage.work_dir must be set before applying a models.toml entry")?;
    std::fs::create_dir_all(work_dir).map_err(|err| format!("create work_dir: {err}"))?;
    std::fs::write(work_dir.join("models.toml"), rendered)
        .map_err(|err| format!("write test models.toml: {err}"))?;

    cfg.llm.default_model = model_id.to_string();
    cfg.context.compaction_model = model_id.to_string();
    Ok(wire_model)
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

/// 把当前测试进程的 HOME 切到独立 tempdir，隔离 `~/.tomcat/*` 盘状态。
///
/// nextest 模式下是一用例一进程，因此一个 guard 对应一个测试进程，最适合 real-llm
/// / CLI 子进程这类会落盘到 `~/.tomcat` 的场景。
pub struct TempHomeGuard {
    _home_lock: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    old_home: Option<OsString>,
}

impl TempHomeGuard {
    pub fn new() -> Self {
        let home_lock = TEMP_HOME_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("lock TEMP_HOME for test");
        let home = tempfile::tempdir().expect("create temp HOME for test");
        let dot_tomcat = home.path().join(".tomcat");
        std::fs::create_dir_all(dot_tomcat.join("plans")).expect("create ~/.tomcat/plans for test");
        std::fs::create_dir_all(dot_tomcat.join("temp")).expect("create ~/.tomcat/temp for test");
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        Self {
            _home_lock: home_lock,
            home,
            old_home,
        }
    }

    pub fn home_path(&self) -> &Path {
        self.home.path()
    }

    pub fn dot_tomcat_path(&self) -> std::path::PathBuf {
        self.home.path().join(".tomcat")
    }
}

impl Drop for TempHomeGuard {
    fn drop(&mut self) {
        if let Some(old_home) = &self.old_home {
            std::env::set_var("HOME", old_home);
        } else {
            std::env::remove_var("HOME");
        }
    }
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
        _hostname: &str,
        status_line: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
    ) -> Self {
        let encoded = include_str!("../fixtures/native-tls-test-identity.p12.b64")
            .split_whitespace()
            .collect::<String>();
        let pkcs12 = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("decode native TLS test identity");
        let identity =
            Identity::from_pkcs12(&pkcs12, "tomcat-test").expect("load native TLS test identity");
        let acceptor = TlsAcceptor::from(
            NativeTlsAcceptor::new(identity).expect("build native TLS server acceptor"),
        );
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
