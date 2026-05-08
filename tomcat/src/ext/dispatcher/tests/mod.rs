//! # `ext::dispatcher::tests` 测试目录
//!
//! 历史 `tests.rs` 1151 行已超过 RUST_FILE_LINES_SPEC §A 的红线，按主题拆分为：
//!
//! - `mocks`：跨用例共享的 `MockPrimitive` / `MockLlm` / `MockToolRegistry`
//!   以及 `make_dispatcher_with_primitive` 工厂函数。
//! - `dispatch_no_extension_test`：未注入扩展时各模块的兜底分支（005/006/004 错误码、
//!   audit 触发计数、events 不依赖扩展也能工作）。
//! - `dispatch_with_extension_test`：注入 primitive / llm / tools / session 后的成功
//!   路径与负向断言（如 `registerTool` 缺 `name`）。
//! - `async_calls_test`：8.4.8 引入的 hostcall `__async` 提交-轮询协议。
//! - `events_test`：事件通道注册 / 投递 / 等待 / 清理 / 反压。

mod async_calls_test;
mod dispatch_no_extension_test;
mod dispatch_with_extension_test;
mod events_test;
mod mocks;
