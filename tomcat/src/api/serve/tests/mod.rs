//! `serve` 模块单元测试入口。
//!
//! 说明：
//! - 物理位置统一收敛到 `src/api/serve/tests/`
//! - 共享测试夹具继续复用父模块的 `test_support`
//! - 业务源文件不再内联 `#[cfg(test)] mod tests { ... }`

pub(crate) use super::ask_question::*;
pub(crate) use super::commands::*;
pub(crate) use super::control::*;
pub(crate) use super::ndjson::*;
pub(crate) use super::schema::*;
pub(crate) use super::stdin::*;
pub(crate) use super::test_support::*;
pub(crate) use super::types::*;
pub(crate) use super::writer::*;
pub(crate) use super::*;

mod ask_question_test;
mod commands_test;
mod control_test;
mod event_pump_test;
mod ndjson_test;
mod registry_test;
mod schema_test;
mod stdin_test;
mod writer_test;
