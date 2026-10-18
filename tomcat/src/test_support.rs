//! 测试共享辅助。
//!
//! 当前提供一把全局锁，两个入口名：
//! - `home_env_lock()`：给会临时覆写 `HOME` 或依赖 `dirs::home_dir()` 稳定值的单测复用。
//! - `cwd_lock()`：给会临时覆写进程级 `current_dir` 的单测复用。
//!   两者都用于避免 `cargo test` 并行时互相污染。
//!   两个入口必须是同一把锁：测试会以任意先后顺序同时改 HOME 和 cwd，
//!   拆成两把锁就会 A 等 B、B 等 A 地死锁。
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
    home_env_lock()
}

/// 设置文件或目录的修改时间。
///
/// Windows 改时间需要 `FILE_WRITE_ATTRIBUTES`，只读打开会被拒绝，打开目录还要
/// `FILE_FLAG_BACKUP_SEMANTICS`；Unix 上目录不能以写方式打开，而只读句柄足以改自己文件的时间。
pub(crate) fn set_mtime(path: &Path, time: std::time::SystemTime) {
    #[cfg(unix)]
    let file = std::fs::File::open(path);
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
        };
        std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    };
    file.and_then(|file| file.set_modified(time))
        .unwrap_or_else(|error| panic!("set mtime of {}: {error}", path.display()));
}

/// 创建目录 junction（`mklink /J` 无需管理员权限）。
#[cfg(windows)]
pub(crate) fn create_junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run mklink");
    assert!(status.success(), "mklink /J failed: {}", link.display());
}

/// 断言文件 ACL 不继承且只有一条"当前用户完全控制"。
///
/// 用 `icacls /save` 的 SDDL（UTF-16）与 `whoami /user` 的 SID 比对，
/// 避开 icacls 普通输出按 OEM 代码页编码导致中文用户名乱码。
#[cfg(windows)]
pub(crate) fn assert_private_to_current_user(path: &Path) {
    let whoami = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .expect("run whoami");
    let whoami = String::from_utf8_lossy(&whoami.stdout);
    let sid = whoami.trim().rsplit(',').next().unwrap().trim_matches('"');
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("acl");
    let status = std::process::Command::new("icacls")
        .arg(path)
        .arg("/save")
        .arg(&saved)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run icacls /save");
    assert!(status.success(), "icacls /save failed: {}", path.display());
    let bytes = std::fs::read(&saved).unwrap();
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let text = String::from_utf16(&units).unwrap();
    let sddl = text.lines().nth(1).unwrap_or_default().trim();
    assert_eq!(sddl, format!("D:PAI(A;;FA;;;{sid})"), "{}", path.display());
}

/// 持有 HOME 锁并把 `HOME` 临时指向 `home`；drop 时（仍持锁）恢复原值。
pub(crate) struct HomeOverride {
    previous: Option<std::ffi::OsString>,
    _lock: TestLockGuard<'static>,
}

impl HomeOverride {
    pub(crate) fn new(home: &Path) -> Self {
        let lock = home_env_lock().lock().unwrap();
        let previous = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home) };
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for HomeOverride {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
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

/// 让 bash 工具打印当前目录的命令：Git Bash 的 `pwd` 输出 `/c/...`（临时目录还会显示成 `/tmp/...`），
/// `pwd -W` 才输出与宿主一致的 `C:/...`。
pub(crate) const PWD_COMMAND: &str = if cfg!(windows) { "pwd -W" } else { "pwd" };

/// 把宿主路径写成 [`PWD_COMMAND`] 的输出形式。
pub(crate) fn shell_pwd(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
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

#[path = "tests/test_support_test.rs"]
mod tests;
