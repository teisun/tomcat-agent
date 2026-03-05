| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| wasm_plugin_agent | 2025-03-05 19:30 | DONE | feature/wasm-plugin |

### ✅ DONE (已完成)
- [✓] **[P0]** T1-P0-007 WasmEdge 运行时与 QuickJS 集成：WasmEngine/WasmInstance 桩、宿主导入绑定骨架（HostRequest/HostResponse、invoke_host_func）、Standard 资源上限预留 @2025-03-05
- [✓] **[P0]** T1-P0-008 宿主 API 层与 JS 绑定：HostApiDispatcher 单入口多路复用、core Trait（PrimitiveExecutor/ToolRegistry/LlmProvider）定义、log/fs/llm/tools/events 路由与占位、invoke_host_func_with 接入 @2025-03-05
- [✓] **[P0]** T1-P0-009 插件生命周期管理：PluginManifest/PluginInstance/PluginStatus、parse_manifest 与校验、PluginManager 注册/启用/禁用/卸载、EventBus.remove_plugin_listeners 与 ToolRegistry.unregister_plugin_tools 清理 @2025-03-05
- [✓] 技术文档：`docs/02-wasm-runtime-and-plugin.md` 已编写

### 🔌 INTERFACE (接口变更)
- **ext 层**：新增 `WasmEngine`、`WasmEngineConfig`、`WasmInstance`、`HostRequest`、`HostResponse`、`invoke_host_func`、`invoke_host_func_with`、`HostApiDispatcher`、`PluginManager`、`PluginManifest`、`PluginInstance`、`PluginStatus`、`PluginInfo`、`parse_manifest`。
- **core 层**：新增 `PrimitiveExecutor`、`ToolRegistry`、`LlmProvider` 及配套类型（EditOperation、Tool、ChatRequest 等），供 008 分发与 009 卸载对接。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
