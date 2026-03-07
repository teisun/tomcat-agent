# Wasm 运行时层与插件生命周期 (ext)

## 1. 概述

- **职责**：WasmEdge 运行时骨架、宿主导入绑定、Hostcall 分发、插件生命周期管理；与 design/Architecture 第 3、4 节及 CODE_BLOCK_P1_007/008/009 对齐。
- **所在层级**：宿主 API 层 + WasmEdge 运行时层（依赖 infra、core traits）。
- **核心文件**：
  - `src/ext/mod.rs` — 聚合 engine/instance、host_binding、dispatcher、plugin
  - `src/ext/engine_stub.rs` — WasmEngine 单例与 WasmInstance 创建（桩实现）
  - `src/ext/instance_stub.rs` — 单插件 Wasm 实例（桩）
  - `src/ext/host_binding.rs` — HostRequest/HostResponse、invoke_host_func / invoke_host_func_with 入口
  - `src/ext/dispatcher.rs` — HostApiDispatcher，按 module/method 路由
  - `src/ext/plugin.rs` — PluginManifest、PluginInstance、PluginStatus、PluginManager
  - `src/core/*.rs` — PrimitiveExecutor、ToolRegistry、LlmProvider 等 Trait 定义

## 2. 设计要点

- **007**：WasmEngine 全局单例、单插件独立 WasmInstance、宿主导入绑定骨架；资源上限预留 Standard 默认。**默认构建为桩实现**；启用 feature `wasmedge` 且安装 WasmEdge C 库后为真实实现（见下节）。
- **008**：HostApiDispatcher 单入口多路复用；EventBus 必选，PrimitiveExecutor/ToolRegistry/LlmProvider 可选注入。
- **009**：PluginManifest 解析与校验；PluginManager 注册/启用/禁用/卸载；卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。

## 3. WasmEdge 真实实现（feature wasmedge）

- **启用方式**：`cargo build --features wasmedge`；需先安装 WasmEdge（见 https://wasmedge.org/docs/start/install）。默认构建（无 feature）仍为桩，保证无 WasmEdge 环境可编译。
- **WasmEngine**：全局单例，Config 开启 WASI、统计、内存上限（max_memory_pages）；`set_memory_limit` 已预留，MVP 使用固定 Standard 值。
- **WasmInstance**：每插件独立 Vm；宿主导入 `env.__pi_host_call` 注册，供 QuickJS 映射到全局；`run_script` 通过 wasmedge_quickjs.wasm 执行 JS（需设置环境变量 `WASMEDGE_QUICKJS_PATH`）。
- **Node 兼容层**：由 wasmedge_quickjs.wasm 提供，范围包括 fs、path、process、console、http 等常用模块；具体能力以 WasmEdge QuickJS 扩展为准。
- **线性内存边界**：Hostcall 时宿主通过 WasmEdge 的 `get_data`/`set_data` 访问线性内存；**边界检查由 WasmEdge 运行时保证**，防止越界访问。响应缓冲区不足时仅回写长度，由 guest 重试更大缓冲区。

## 4. 依赖与后续

- **005/006/004**：Dispatcher 通过 with_primitive/with_tools/with_llm 注入；未注入时返回明确错误，待合并后接实线。
- **跨平台**：Windows/macOS/Linux 各需在对应环境安装 WasmEdge 后执行 `cargo build --features wasmedge` 验证。
