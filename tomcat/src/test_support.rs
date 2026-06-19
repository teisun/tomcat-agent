//! 测试共享辅助。
//!
//! 当前提供两把全局锁：
//! - `home_env_lock()`：给会临时覆写 `HOME` 或依赖 `dirs::home_dir()` 稳定值的单测复用。
//! - `cwd_lock()`：给会临时覆写进程级 `current_dir` 的单测复用。
//!   两者都用于避免 `cargo test` 并行时互相污染。
//!
//! 这里显式使用**可重入**锁，原因是：
//! - 某些测试先拿锁，再调用 `ChatContext::from_config()`；
//! - `from_config()` 在 `#[cfg(test)]` 下也会拿同一把锁，保证读取 HOME/current_dir
//!   时不会被并行测试污染；
//! - 若继续使用 `std::sync::Mutex`，同线程二次进入会自锁，且 panic 会 poison 后续测试。

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use std::convert::Infallible;
use std::path::Path;
use std::sync::OnceLock;

pub(crate) type TestLockGuard<'a> = ReentrantMutexGuard<'a, ()>;

pub(crate) struct TestLock(ReentrantMutex<()>);

impl TestLock {
    pub(crate) fn lock(&self) -> Result<TestLockGuard<'_>, Infallible> {
        Ok(self.0.lock())
    }
}

pub(crate) fn home_env_lock() -> &'static TestLock {
    static LOCK: OnceLock<TestLock> = OnceLock::new();
    LOCK.get_or_init(|| TestLock(ReentrantMutex::new(())))
}

pub(crate) fn cwd_lock() -> &'static TestLock {
    static LOCK: OnceLock<TestLock> = OnceLock::new();
    LOCK.get_or_init(|| TestLock(ReentrantMutex::new(())))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TestModelOverride<'a> {
    pub id: &'a str,
    pub model_name: Option<&'a str>,
    pub api: &'a str,
    pub provider: &'a str,
    pub api_key_env: &'a str,
    pub base_url: &'a str,
    pub thinking_format: Option<&'a str>,
    pub vision: bool,
    pub files: bool,
    pub tools: bool,
    pub reasoning: bool,
    pub web_search: bool,
}

impl<'a> TestModelOverride<'a> {
    pub(crate) fn gpt54_openai_responses(api_key_env: &'a str) -> Self {
        Self {
            id: "gpt-5.4",
            model_name: None,
            api: "openai-responses",
            provider: "openai",
            api_key_env,
            base_url: "https://api.openai.com",
            thinking_format: Some("openai"),
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        }
    }

    pub(crate) fn with_base_url(mut self, base_url: &'a str) -> Self {
        self.base_url = base_url;
        self
    }
}

pub(crate) fn write_models_override(work_dir: &Path, entries: &[TestModelOverride<'_>]) {
    let path = work_dir.join("models.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create models.toml parent");
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str("[[models]]\n");
        text.push_str(&format!("id = {:?}\n", entry.id));
        if let Some(model_name) = entry.model_name {
            text.push_str(&format!("model_name = {:?}\n", model_name));
        }
        text.push_str(&format!("api = {:?}\n", entry.api));
        text.push_str(&format!("provider = {:?}\n", entry.provider));
        text.push_str(&format!("api_key_env = {:?}\n", entry.api_key_env));
        text.push_str(&format!("base_url = {:?}\n", entry.base_url));
        if let Some(thinking_format) = entry.thinking_format {
            text.push_str(&format!("thinking_format = {:?}\n", thinking_format));
        }
        text.push_str(&format!(
            "capabilities = {{ vision = {}, files = {}, tools = {}, reasoning = {}, web_search = {} }}\n\n",
            entry.vision, entry.files, entry.tools, entry.reasoning, entry.web_search
        ));
    }
    std::fs::write(path, text).expect("write test models.toml override");
}
