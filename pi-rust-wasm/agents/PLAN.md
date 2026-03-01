---
本文档供 agent 读取；
使用方式：在Cursor @xxx_agent.md @PLAN.md 出发以下指令：
 > agent根据自身角色定义读取主项目 [specs规格文档](../openspec/specs/)下文档 + [需求设计文档](../openspec/changes/001-mvp/) 下文档，实现PLAN.md 规划好的功能，完成对应功能后提交代码到各自分支，按要求同步进度到[项目看板](../INTEGRATION.md)
 > 请严格加载 .cursor/rules/commit-guard.mdc 提交规则，所有 Git 提交必须按此规则自动执行、自动校验、自动生成正确 commit message，不允许绕过。
---

# 001-mvp 多 Agent 分工总计划

## 并行度建议

- **首轮**：仅 1 个开发 Agent 完成 T1-P0-001（项目骨架与基础设施），其余等待。
- **001 完成后**：最多 **4 个开发 Agent 同时开工**，对应 T1-P0-002、003、004、007 四条线。
- **后续波次**：005/006（2 条）→ 008（1 条）→ 009（1 条）→ 010/011（2 条），按依赖顺序推进。
- **测试集成**：1 名 **integration_test** 不占开发任务，负责合并分支到 dev、全量测试与验收。

**推荐配置**：**4 开发 + 1 集成** 同时在线（在 002/003/004/007 阶段）；其余阶段按依赖表依次启动或收尾。

---

## 依赖波次表

| 波次 | 任务 ID | 依赖 | 可并行角色 |
|------|---------|------|------------|
| 1 | T1-P0-001 | 无 | infra（仅此 1 个） |
| 2 | T1-P0-002, 003, 004, 007 | 001 | infra, session_cli, llm, wasm_plugin（4 个） |
| 3 | T1-P0-005, 006 | 001, 002 | primitives_tools（002 由 infra 交付） |
| 4 | T1-P0-008 | 005, 006, 007 | wasm_plugin（接续 007） |
| 5 | T1-P0-009 | 002, 008 | wasm_plugin |
| 6 | T1-P0-010 | 001, 003, 009 | session_cli（接续 003） |
| 7 | T1-P0-011 | 002, 003, 004, 005, 006, 009 | chat（最后集成） |

---

## 角色与任务 ID 对照表

| 角色 | 负责任务 ID | 说明 |
|------|-------------|------|
| **infra** | T1-P0-001, T1-P0-002 | 项目骨架、错误/配置/日志/跨平台、事件总线 |
| **session_cli** | T1-P0-003, T1-P0-010 | 会话存储与会话管理；CLI init/doctor/config/session/plugin/audit（010 需 009 完成后推进） |
| **llm** | T1-P0-004 | LLM 统一接入、OpenAI 适配器、流式/非流式、Token 统计 |
| **wasm_plugin** | T1-P0-007, T1-P0-008, T1-P0-009 | WasmEdge+QuickJS、宿主 API 与 JS 绑定、插件生命周期（008 依赖 primitives_tools 的 005/006） |
| **primitives_tools** | T1-P0-005, T1-P0-006 | 4 原语执行引擎、工具注册中心 |
| **chat** | T1-P0-011 | CLI 对话模式、流式渲染、4 原语/工具调用展示与确认（依赖 002/003/004/005/006/009） |
| **integration_test** | — | 合并到 dev、全量测试与验收、问题反馈（不负责具体任务 ID 开发） |

---

## 分支与集成策略

- **分支约定**
  - 主开发分支：`dev`
  - 功能分支：`feature/infra`、`feature/session-cli`、`feature/llm`、`feature/wasm-plugin`、`feature/primitives-tools`、`feature/chat`（与角色一一对应）
- **合并顺序**
  1. 首轮仅合并 `feature/infra`（001+002）到 dev，CI 通过后其余角色基于 dev 拉取并开发。
  2. 002/003/004/007 完成后，由 **integration_test** 按依赖顺序合并：先 primitives_tools（005+006），再 wasm_plugin（007→008→009），再 session_cli 的 010，最后 chat 的 011。
  3. 每次合并前：提交方自测通过（build、clippy、单测）；integration_test 合并后跑全量测试与验收清单，失败则反馈给对应角色，修复后重新合并。
- **验收**
  - 由 integration_test 维护验收清单（与 task.md/tasks_details.md 一致），合并到 dev 后执行；问题记录到 issue 或任务看板，指派回开发角色。

---

## P1/P2/P3 任务认领（建议）

- **T1-P1-001 审计日志**：可由 **infra** 在 P0 收尾后认领，或单列给一名 Agent。
- **T1-P1-002 兼容性测试、T1-P1-003 单测全覆盖、T1-P1-004 全平台测试**：由 **integration_test** 主导执行与回归，各开发角色配合修 bug。
- **T1-P2-001 CLI 体验优化**：**chat** 或 **session_cli** 认领；**T1-P2-002 插件安全扫描**：**wasm_plugin** 认领。
- **T1-P3-001 文档、T1-P3-002 示例插件**：可均摊到各角色（各自模块文档与示例），或由专人统一编写。

具体认领可在迭代站会中再细化，本文档仅作建议。
