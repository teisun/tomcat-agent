//! # `agent_loop::tests` 测试目录
//!
//! Phase 4 把原 `tests.rs`（1277 行，超超超红区）按主题拆为同名子 mod；
//! 每个文件 ≤ 350 行，覆盖一组语义相关的测试用例。`mocks.rs` 集中所有
//! `LlmProvider` / `PrimitiveExecutor` 测试替身，子 mod 通过 `use super::mocks::*;`
//! 复用，避免散落在多文件中。
//!
//! ## 测试组织
//!
//! - `mocks`：MockLlmProvider / MockLlmProviderFatal / MockPrimitiveExecutor /
//!   SleepyMockPrimitive / SteerableMockPrimitive。
//! - `classify_test`：`error_classifier::classify_error` 4 个等价类断言。
//! - `run_basic_test`：text-only / 重试 / 工具循环 / 空消息 4 个基础正向测试。
//! - `events_order_test`：wire 事件全序列 + Fatal 终结 + chat_stream Err 分类。
//! - `steering_followup_test`：Steering 跳过剩余工具 + FollowUp 第二轮上下文。
//! - `metrics_test`：5 个 ContextMetricsUpdate 用例（顺序 / payload / 多轮累计 /
//!   无 ctx 跳过 / 纯文本路径）。
//! - `interrupt_test`：Abort / 工具间中断 / 流式中断 partial / token 重建 4 个
//!   T-003/T-004/T-017 硬验收。
//! - `submodules_test`：直接调用 `pub(super)` 子模块函数的焦小测
//!   （handle_overflow_retry / execute_tool）。

mod classify_test;
mod current_tail_guard_behavior_test;
mod current_tail_guard_runtime_test;
mod current_tail_guard_test;
mod defaults_test;
mod events_order_test;
mod interrupt_test;
mod metrics_test;
mod mocks;
mod run_basic_test;
mod steering_followup_test;
mod stream_handler_test;
mod submodules_test;
mod tool_exec_dedup_test;
