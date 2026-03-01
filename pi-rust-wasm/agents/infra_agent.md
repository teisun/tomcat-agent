# infra_agent：基础设施与事件总线

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责一期 MVP 的**基础设施层**：项目骨架、统一错误与配置、日志、跨平台适配、全局事件总线。交付可被其他所有模块依赖的公共类型、配置与事件能力。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-001 | 项目骨架搭建与基础设施层落地 |
| 2 | T1-P0-002 | 全局事件总线核心实现 |

001 无依赖，必须最先完成；002 依赖 001。完成后 primitives_tools、wasm_plugin、chat 等均依赖 002。

## 依赖与协作

- **依赖**：无（001 为全项目起点）。
- **被依赖**：session_cli、llm、wasm_plugin、primitives_tools、chat 均依赖 001；primitives_tools、wasm_plugin、chat 依赖 002。
- **接口约定**：
  - **AppError**：项目统一错误枚举，所有层通过 `Result<T, AppError>` 或 `anyhow` 包装使用。
  - **AppConfig / PrimitiveConfig / SecurityConfig 等**：配置结构体与加载入口，供各层读取。
  - **EventBus**：Trait `on/once/off/emit_sync/emit_async/remove_plugin_listeners`；扩展侧使用字符串事件名，与 pi-mono 一致。
  - **AgentEvent / ExtensionEvent**：事件枚举与 payload（Architecture.md 约定），type snake_case、payload camelCase。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 1 节「基础设施层」、CODE_BLOCK_P1_001～P1_004
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-001、T1-P0-002 原子子任务
- [Architecture.md](../openspec/specs/Architecture.md)「事件系统设计」

## 验收标准

- **T1-P0-001**：Rust 项目初始化、Cargo.toml 依赖就绪；AppError 定义完整（不含 Db）；配置加载与合并、默认配置、合法性校验；tracing 分级日志与跨平台适配；clippy 通过，基础设施层单测覆盖率≥90%。
- **T1-P0-002**：AgentEvent/ExtensionEvent 与 Architecture 一致；EventBus Trait 与实现；同步/异步、优先级、remove_plugin_listeners；**边界**：单 listener 抛错时其余 listener 仍执行、主流程不崩溃；单测覆盖率≥90%。
