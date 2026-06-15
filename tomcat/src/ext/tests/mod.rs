//! # `ext` 单元测试目录
//!
//! 集中存放 `ext/` 下单文件叶子模块（`host_binding_test.rs` /
//! `runtime_manager_test.rs` / `vm_actor_test.rs` /
//! `ts_compiler.rs`）的单元测试。`dispatcher/` 与 `plugin/` 是真目录模块，
//! 自带 `tests/`，不在此处。
//!
//! `ts_compiler` 历史按主题已拆为 `transpile.rs` + `import_rewrite.rs`，
//! 上抬后保留拆分但加 `ts_compiler_` 前缀。

mod host_binding_test;
mod instance_shim_test;
mod plugin_bundle_test;
mod plugin_function_invoker_test;
mod plugin_tool_executor_test;
mod runtime_manager_test;
mod ts_compiler_import_rewrite_test;
mod ts_compiler_transpile_test;
mod vm_actor_test;
