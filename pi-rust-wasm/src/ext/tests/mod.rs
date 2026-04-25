//! # `ext` 单元测试目录
//!
//! 集中存放 `ext/` 下单文件叶子模块（`host_binding.rs` /
//! `instance_stub.rs` / `runtime_manager.rs` / `vm_actor.rs` /
//! `ts_compiler.rs`）的单元测试。`dispatcher/` 与 `plugin/` 是真目录模块，
//! 自带 `tests/`，不在此处。
//!
//! `engine_stub.rs` 需要测私有 `WasmEngine._config` 字段，按
//! [RUST_FILE_LINES_SPEC §A 第 9 条] 走 `#[cfg(test)] #[path] mod tests;`
//! 挂载（测试文件物理位置仍在本目录 `engine_stub.rs`，但模块挂在被测源文件下，
//! 故此处**不**声明 `mod engine_stub;`）。
//!
//! `ts_compiler` 历史按主题已拆为 `transpile.rs` + `import_rewrite.rs`，
//! 上抬后保留拆分但加 `ts_compiler_` 前缀。

mod host_binding;
mod instance_stub;
mod runtime_manager;
mod ts_compiler_import_rewrite;
mod ts_compiler_transpile;
mod vm_actor;
