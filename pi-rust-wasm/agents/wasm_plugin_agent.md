# wasm_plugin_agent：WasmEdge 运行时与插件生命周期

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责 **WasmEdge 运行时与 QuickJS 集成**、**宿主 API 层与 JS 绑定**、**插件生命周期管理**：全局 Engine、独立 Wasm 实例、QuickJS 与 Node 兼容层、宿主导入绑定；将 4 原语/LLM/工具/事件/会话/配置/日志等 API 暴露给插件（pi-mono 100% 兼容）；插件加载/初始化/启用/禁用/卸载及事件与工具自动清理。交付可加载并运行 pi-mono 插件的沙箱环境。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-007 | WasmEdge 运行时与 QuickJS 集成 |
| 2 | T1-P0-008 | 宿主 API 层与 JS 绑定实现（依赖 005、006、007，需 primitives_tools 与 llm 就绪） |
| 3 | T1-P0-009 | 插件生命周期管理模块落地 |

007 仅依赖 001，可与 002/003/004 并行；008 依赖 005、006、007，需在 primitives_tools 完成 005/006 后推进；009 依赖 002、008。

## 依赖与协作

- **依赖**：T1-P0-001；T1-P0-002（EventBus、remove_plugin_listeners）；T1-P0-005、006（PrimitiveExecutor、ToolRegistry）；T1-P0-004（LlmProvider）；session_cli 的 SessionManager（会话 API 绑定）。
- **被依赖**：session_cli（010 的 plugin 子命令依赖 009）；chat（011 依赖 009 的插件与工具联动）。
- **接口约定**：
  - **宿主 API**：与 design「核心 API 分类与对齐规范」一致（4 原语、LLM、工具、事件、会话、配置、日志）；在 QuickJS 中暴露为全局 `agent` 对象。
  - **PluginManager**（或等价）：加载/卸载/启用/禁用、清单解析与校验；调用 EventBus.remove_plugin_listeners、ToolRegistry.unregister_plugin_tools 做清理。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 3、4 节「宿主 API 层」「WasmEdge 运行时层」、CODE_BLOCK_P1_008/009
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-007、008、009
- [Architecture.md](../openspec/specs/Architecture.md) 第 3、4 节

## 验收标准

- **T1-P0-007**：WasmEdge Engine 单例与生命周期；单插件独立 Wasm 实例创建与销毁；QuickJS 与 Node 兼容层启用；宿主导入绑定骨架与 Rust↔JS 最小通道；跨平台编译与运行验证。
- **T1-P0-008**：宿主 API 列表与 Rust 实现、权限与审计集成；WasmEdge 导入表与 QuickJS 绑定；Rust/JS 类型转换与异步调度；错误捕获与透传；单测覆盖率≥80%。
- **T1-P0-009**：PluginManifest/PluginInstance/PluginStatus；加载→初始化→启用/禁用→卸载；事件与工具自动清理；**边界**：清单非法、权限不满足、Wasm/QuickJS 初始化失败时错误清晰、宿主不崩溃、可恢复；无内存泄漏（有条件时做简单检测）；单测覆盖率≥80%。
