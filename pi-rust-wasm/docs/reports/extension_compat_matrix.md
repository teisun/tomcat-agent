# 社区 / 样例扩展兼容性评估矩阵（TASK-05a）

**目的**：对代表性扩展做静态采样，标注主要使用的 ExtensionAPI 与目标 Tier，供 TASK-05b/c 排期。  
**说明**：以下为 Phase 0 人工阅读源码结论，非运行时认证；`pi-rust-wasm` 行为以 pi-mono 语义为准。

| # | 扩展 | 路径（相对 pi-mono 仓库） | 主要 API / 依赖 | 目标 Tier | Phase 0 后评估 |
|---|------|---------------------------|-----------------|-----------|----------------|
| 1 | tps | `.pi/extensions/tps.ts` | `pi.on`，`ctx.hasUI`，`ctx.ui.notify` | 1 | SWC+QuickJS POC 已通过；待真实 ctx 与事件投递 |
| 2 | redraws | `.pi/extensions/redraws.ts` | `registerCommand`，`ctx.ui.custom`，`@mariozechner/pi-tui` | 3 | 依赖 TUI 组件与 custom 渲染 |
| 3 | prompt-url-widget | `.pi/extensions/prompt-url-widget.ts` | `pi.exec(gh)`，`registerCommand`，TUI 组件 | 2–3 | 依赖 `exec` 与高级 UI |
| 4 | files | `.pi/extensions/files.ts` | `registerCommand`，`pi.exec`，`ctx.cwd` | 2 | 需 Tier 2 exec + ctx |
| 5 | diff | `.pi/extensions/diff.ts` | `registerCommand`，`pi.exec`，diff 流程 | 2–3 | 与 files 类似，命令+exec |
| 6 | sandbox 示例 | `packages/coding-agent/examples/extensions/sandbox/index.ts` | `registerTool`，`registerCommand`，`pi.on` | 2 | 工具注册 + 事件 |
| 7 | tool-override | `packages/coding-agent/examples/extensions/tool-override.ts` | `registerTool`，`registerCommand` | 2 | TypeBox schema 对齐 |
| 8 | preset | `packages/coding-agent/examples/extensions/preset.ts` | `registerCommand`，`pi.on`（多事件） | 2 | 事件面较大 |
| 9 | dynamic-tools | `packages/coding-agent/examples/extensions/dynamic-tools.ts` | `registerTool`（动态），`registerCommand`，`pi.on` | 2 | 动态工具注册 |
| 10 | truncated-tool | `packages/coding-agent/examples/extensions/truncated-tool.ts` | `registerTool` | 2 | 工具 schema |
| 11 | antigravity-image-gen | `packages/coding-agent/examples/extensions/antigravity-image-gen.ts` | `registerTool`（复杂） | 2 | 工具+外部能力 |
| 12 | overlay-qa-tests | `packages/coding-agent/examples/extensions/overlay-qa-tests.ts` | 大量 `registerCommand`，Overlay/TUI | 3 | 偏 UI 回归，Tier 3 |
| 13 | subagent | `packages/coding-agent/examples/extensions/subagent/index.ts` | `registerTool`（子代理） | 2 | 工具与编排 |
| 14 | with-deps | `packages/coding-agent/examples/extensions/with-deps/index.ts` | `registerTool`，npm 依赖 | 2 | 依赖 Node 模块解析 |
| 15 | provider-payload | `packages/coding-agent/examples/extensions/provider-payload.ts` | Provider 相关 | 4 | 深度 API |

**汇总**：Tier 1 可优先以 **tps** 为验收；**files/diff/preset/sandbox** 等覆盖 Tier 2 主体；**redraws / overlay-qa-tests / prompt-url-widget** 驱动 Tier 3。

---

**更新**：新增标杆扩展或 pi-mono API 变更时同步本矩阵。
