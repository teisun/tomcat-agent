//! # 共享测试 fixture
//!
//! 当前仅含 `load_dotenv`：从 crate 根目录加载 `.env`，便于本地有 `OPENAI_API_KEY`
//! 时跑真实 API 用例。所有依赖 `.env` 的测试都先调用此函数，避免重复样板。

use std::path::Path;

/// 从 crate 根目录加载 .env，便于本地有 key 时跑测试。
pub(super) fn load_dotenv() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let _ = dotenvy::from_path(path);
}
