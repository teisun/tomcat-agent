# Wasm 运行时层与插件生命周期 (ext)

## 1. 概述

- **职责**：WasmEdge 运行时骨架、宿主导入绑定、Hostcall 分发、插件生命周期管理；与 design/Architecture 第 3、4 节及 CODE_BLOCK_P1_007/008/009 对齐。
- **所在层级**：宿主 API 层 + WasmEdge 运行时层（依赖 infra、core traits）。
- **核心文件**：
  - `src/ext/mod.rs` — 聚合 engine/instance、host_binding、dispatcher、plugin
  - `src/ext/engine_stub.rs` — WasmEngine 单例与 WasmInstance 创建（桩实现）
  - `src/ext/instance_stub.rs` — 单插件 Wasm 实例（桩）
  - `src/ext/host_binding.rs` — HostRequest/HostResponse、invoke_host_func 入口
  - `src/ext/dispatcher.rs` — HostApiDispatcher，按 module/method 路由
  - `src/ext/plugin.rs` — PluginManifest、PluginInstance、PluginStatus、PluginManager
  - `src/core/*.rs` — PrimitiveExecutor、ToolRegistry、LlmProvider 等 Trait 定义

## 2. 设计要点

- **007**：WasmEngine 全局单例（桩）、单插件独立 WasmInstance（桩）、宿主导入绑定骨架；资源上限预留 Standard 默认。
- **008**：HostApiDispatcher 单入口多路复用；EventBus 必选，PrimitiveExecutor/ToolRegistry/LlmProvider 可选注入。
- **009**：PluginManifest 解析与校验；PluginManager 注册/启用/禁用/卸载；卸载时调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools。

## 3. 依赖与后续

- **005/006/004**：Dispatcher 通过 with_primitive/with_tools/with_llm 注入；未注入时返回明确错误，待合并后接实线。
- **真实 WasmEdge**：当前为桩实现；完整 WasmEdge+QuickJS 可后续通过 feature 或独立构建接入。
