//! 集成测试公共模块：日志初始化、`.env` 加载与共享 fixture。
//! 使用 Once 保证并行测试下只初始化一次，避免重复 init 导致 panic。

use std::path::Path;
use std::sync::Once;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

/// 为依赖 `OPENAI_API_KEY` 的集成测试加载环境变量（与 `UNIT_TEST_SPEC` / `INTEGRATION_TEST_SPEC` 对齐）。
///
/// 顺序（`dotenvy` 默认不覆盖已存在的环境变量）：
/// 1. `tomcat/.env`（`CARGO_MANIFEST_DIR`，与 `src/core/llm/tests/mocks.rs::load_dotenv` 一致）
/// 2. `dotenvy::dotenv()`：从当前工作目录向上查找 `.env`（`cargo test` 在 crate 根执行时通常同上）
pub fn load_openai_test_env() {
    let manifest_env = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(&manifest_env);
    let _ = dotenvy::dotenv();
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
    let base =
        tomcat::resolve_dot_tomcat_temp_dir().expect("resolve ~/.tomcat/temp");
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
