//! # dispatcher 构造时 Tokio handle 捕获回归
//!
//! 背景：`HostApiDispatcher::new` 在构造时通过 `Handle::try_current()` 捕获 Tokio
//! 运行时句柄；异步 hostcall（`pi.fetch` / `createChatCompletion`）依赖它把后台
//! 任务 spawn 到运行时。生产 CLI（`chat_cmd::run_chat_mode`）曾在
//! `tokio::runtime::Runtime::new()` **之前**构造 `ChatContext`，导致 dispatcher
//! 捕获到的句柄为 `None`，web_search 等插件后端在发起任何请求前即抛
//! "async hostcall requires a Tokio runtime handle"。集成测试因 `#[tokio::test]`
//! 始终在运行时内构造而无法暴露该问题——这是一类 test≠prod 缺口。
//!
//! 本用例用普通 `#[test]`（线程上无 Tokio 运行时）固定不变量：
//! - 运行时上下文之外构造 → 无句柄（复现 bug 条件）。
//! - 先建运行时并 `enter()` 后构造 → 有句柄（即修复采用的顺序）。

use std::sync::Arc;

use super::super::HostApiDispatcher;
use crate::infra::DefaultEventBus;

#[test]
fn new_without_runtime_context_has_no_tokio_handle() {
    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    assert!(
        !dispatcher.has_tokio_handle(),
        "运行时上下文之外构造时不应捕获到 Tokio handle"
    );
}

#[test]
fn new_inside_entered_runtime_captures_tokio_handle() {
    let rt = tokio::runtime::Runtime::new().expect("create runtime");
    let _enter = rt.enter();
    let bus = Arc::new(DefaultEventBus::new());
    let dispatcher = HostApiDispatcher::new(bus);
    assert!(
        dispatcher.has_tokio_handle(),
        "在 rt.enter() 上下文内构造应捕获到 Tokio handle（异步 hostcall 依赖它）"
    );
}
