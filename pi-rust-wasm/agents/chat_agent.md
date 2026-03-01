# chat_agent：CLI 对话模式

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责 **CLI 对话模式**：对话主循环、流式响应渲染、Markdown/代码高亮、多轮上下文与会话切换、4 原语/工具调用展示与用户确认、快捷键与中断、历史导航。交付 `pi-awsm chat`（及无参数默认进入的对话）的完整交互能力，对齐 pi-mono 行为。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-011 | CLI 对话模式核心实现 |

011 依赖 002、003、004、005、006、009，需在 infra、session_cli、llm、primitives_tools、wasm_plugin 的对应任务全部完成后启动，为最后集成角色。

## 依赖与协作

- **依赖**：T1-P0-002（EventBus）；T1-P0-003（SessionManager、上下文组装）；T1-P0-004（LlmProvider）；T1-P0-005（PrimitiveExecutor、用户确认）；T1-P0-006（ToolRegistry）；T1-P0-009（插件与工具在对话中的参与）。
- **被依赖**：integration_test 验收对话流程与体验。
- **接口约定**：
  - 实现 4 原语/工具调用时的**用户确认交互**（实现 primitives_tools 定义的用户确认接口），以及 CLI 侧的展示与提示。
  - 会话切换、`--resume`、无参数默认 chat 等行为与 pi-mono 对齐（见 design CLI 节）。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 5 节「CLI 交互层」、核心交互设计
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-011

## 验收标准

- **T1-P0-011**：对话主循环与流式渲染；Markdown/代码高亮；多轮上下文与会话切换；4 原语/工具调用展示与用户确认；**边界**：用户拒绝 4 原语确认时的提示与审计；快捷键 Ctrl+C 中断、Ctrl+D 退出、↑↓ 历史导航；会话切换与 `--resume`；**边界/验收**：会话切换后会话级 LLM/插件配置正确隔离；可选：切换时进行中 tool call 的简单策略（等待或取消）。
