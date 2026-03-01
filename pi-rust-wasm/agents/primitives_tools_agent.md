# primitives_tools_agent：4 原语与工具注册中心

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责**4 原语执行引擎**与**工具注册中心**：read/write/edit/bash 及路径白名单、命令分级、用户确认接口、备份/diff/原子化与审计；Tool 定义与注册/注销/检索/调用、插件级权限与卸载时自动注销。交付 PrimitiveExecutor、ToolRegistry 等 Trait 及实现，供宿主 API 层（008）与 chat（011）调用。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-005 | 4 原语执行引擎核心实现 |
| 2 | T1-P0-006 | 工具注册中心核心实现 |

005、006 均依赖 001 与 002；可与 wasm_plugin 的 007 并行，但需在 008 之前完成（008 依赖 005、006、007）。

## 依赖与协作

- **依赖**：T1-P0-001、T1-P0-002（EventBus 用于插件卸载时清理等）。
- **被依赖**：wasm_plugin（008 将 4 原语与工具 API 绑定到 JS）；chat（011 对话中 4 原语/工具调用展示与用户确认）。
- **接口约定**：
  - **PrimitiveExecutor**：Trait 含 read_file、list_dir、write_file、edit_file、execute_bash、require_user_confirmation；EditOperation、PrimitiveOperation 等类型（design CODE_BLOCK_P1_006）。
  - **用户确认接口**：Trait 或回调，由 CLI（chat/session_cli）实现具体交互，本模块只调用。
  - **ToolRegistry**：Trait 含 register_tool、unregister_tool、get_tool、list_tools、call_tool、unregister_plugin_tools；Tool 结构对齐 pi-mono（design CODE_BLOCK_P1_007）。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 2.3、2.4 节「4 原语执行引擎」「工具注册中心」、CODE_BLOCK_P1_006/007
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-005、T1-P0-006

## 验收标准

- **T1-P0-005**：read/write/edit/bash 实现；路径白名单与命令分级；用户确认接口定义；备份、diff、原子化与审计；**边界**：用户拒绝确认时的错误返回与审计记录；单测覆盖率≥90%。
- **T1-P0-006**：Tool 结构与 ToolRegistry 实现；注册/注销/检索/调用；插件卸载时 unregister_plugin_tools；插件级工具权限；call_tool 返回值与 AgentToolResult（content、details）一致；单测覆盖率≥80%。
