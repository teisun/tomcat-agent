//! 集成测试公共模块：日志初始化、`.env` 加载与共享 fixture。
//! 使用 Once 保证并行测试下只初始化一次，避免重复 init 导致 panic。

#![allow(dead_code)]

use std::path::Path;
use std::sync::Once;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

pub const DEEPSEEK_TEST_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEEPSEEK_TEST_API_BASE: &str = "https://api.deepseek.com";
pub const DEEPSEEK_TEST_MODEL_ENV: &str = "TOMCAT_E2E_DEEPSEEK_MODEL";
pub const DEEPSEEK_TEST_DEFAULT_MODEL: &str = "deepseek-v4-pro";

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

pub fn require_deepseek_api_key(test_name: &str) -> String {
    setup_logging();
    load_deepseek_test_env();
    std::env::var(DEEPSEEK_TEST_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{test_name} 必须设置 {DEEPSEEK_TEST_API_KEY_ENV}（环境变量或 tomcat/.env）"
        )
    })
}

pub fn apply_deepseek_llm_config(cfg: &mut tomcat::LlmConfig) {
    cfg.provider = "openai".to_string();
    cfg.api_base = Some(DEEPSEEK_TEST_API_BASE.to_string());
    cfg.api_key_env = Some(DEEPSEEK_TEST_API_KEY_ENV.to_string());
    cfg.default_model = deepseek_test_model();
    cfg.thinking.enabled = true;
    cfg.thinking.level = "high".to_string();
}

pub fn apply_deepseek_app_config(cfg: &mut tomcat::AppConfig) {
    apply_deepseek_llm_config(&mut cfg.llm);
    cfg.context.compaction_model = deepseek_test_model();
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
