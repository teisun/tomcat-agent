# 社区 / 样例扩展兼容性评估矩阵（TASK-05a）

**目的**：对代表性扩展做静态采样，标注主要使用的 ExtensionAPI 与目标 Tier，供 TASK-05b/c 排期。  
**说明**：以下为 Phase 0 人工阅读源码结论，非运行时认证；`pi-rust-wasm` 行为以 pi-mono 语义为准。

| # | 扩展 | 路径（相对 pi-mono 仓库） | 功能简介 | 主要 API / 依赖 | 目标 Tier | Phase 0 后评估 |
|---|------|---------------------------|----------|-----------------|-----------|----------------|
| 1 | tps | `.pi/extensions/tps.ts` | agent 结束时统计并通知 token 吞吐量（TPS）、输入/输出/缓存 token 数与耗时 | `pi.on`，`ctx.hasUI`，`ctx.ui.notify` | 1 | SWC+QuickJS POC 已通过；待真实 ctx 与事件投递 |
| 2 | redraws | `.pi/extensions/redraws.ts` | `/tui` 命令：读取 TUI 完整重绘次数并通知用户 | `registerCommand`，`ctx.ui.custom`，`@mariozechner/pi-tui` | 3 | 依赖 TUI 组件与 custom 渲染 |
| 3 | prompt-url-widget | `.pi/extensions/prompt-url-widget.ts` | 检测用户提示中的 GitHub PR/Issue URL，在终端 widget 中展示标题与作者，并自动设置会话名 | `pi.exec(gh)`，`registerCommand`，TUI 组件 | 2–3 | 依赖 `exec` 与高级 UI |
| 4 | files | `.pi/extensions/files.ts` | `/files` 命令：列出当前会话中模型读/写/编辑过的文件，选中后用 VS Code 打开 | `registerCommand`，`pi.exec`，`ctx.cwd` | 2 | 需 Tier 2 exec + ctx |
| 5 | diff | `.pi/extensions/diff.ts` | `/diff` 命令：展示 git 工作区变更文件列表，选中后以 VS Code diff 视图打开 | `registerCommand`，`pi.exec`，diff 流程 | 2–3 | 与 files 类似，命令+exec |
| 6 | sandbox 示例 | `packages/coding-agent/examples/extensions/sandbox/index.ts` | OS 级沙箱：用 sandbox-exec / bubblewrap 限制 bash 命令的文件系统与网络访问 | `registerTool`，`registerCommand`，`pi.on` | 2 | 工具注册 + 事件 |
| 7 | tool-override | `packages/coding-agent/examples/extensions/tool-override.ts` | 覆盖内置 `read` 工具：添加访问日志记录并拦截敏感文件路径（.env、.ssh 等） | `registerTool`，`registerCommand` | 2 | TypeBox schema 对齐 |
| 8 | preset | `packages/coding-agent/examples/extensions/preset.ts` | 命名预设配置管理：通过 CLI flag、`/preset` 命令或快捷键切换模型、thinking level、工具集和 system prompt | `registerCommand`，`pi.on`（多事件） | 2 | 事件面较大 |
| 9 | dynamic-tools | `packages/coding-agent/examples/extensions/dynamic-tools.ts` | 运行时动态注册工具：启动注册 echo 工具，运行中可通过 `/add-echo-tool` 继续添加 | `registerTool`（动态），`registerCommand`，`pi.on` | 2 | 动态工具注册 |
| 10 | truncated-tool | `packages/coding-agent/examples/extensions/truncated-tool.ts` | 工具输出截断示例：封装 ripgrep 搜索，超限时保存完整输出到临时文件并通知 LLM | `registerTool` | 2 | 工具 schema |
| 11 | antigravity-image-gen | `packages/coding-agent/examples/extensions/antigravity-image-gen.ts` | 通过 Google Antigravity API（Gemini/Imagen）生成图片，支持多种保存模式与宽高比 | `registerTool`（复杂） | 2 | 工具+外部能力 |
| 12 | overlay-qa-tests | `packages/coding-agent/examples/extensions/overlay-qa-tests.ts` | Overlay 浮层全面 QA 测试套件：动画（~30 FPS）、锚点循环、边距/偏移、堆叠、流式输出、焦点切换等 | 大量 `registerCommand`，Overlay/TUI | 3 | 偏 UI 回归，Tier 3 |
| 13 | subagent | `packages/coding-agent/examples/extensions/subagent/index.ts` | 子代理委托：将任务派发给独立 pi 子进程执行，支持单任务、并行和链式（{previous} 占位符）三种模式 | `registerTool`（子代理） | 2 | 工具与编排 |
| 14 | with-deps | `packages/coding-agent/examples/extensions/with-deps/index.ts` | 带 npm 依赖的扩展示例：使用第三方 `ms` 库解析人类可读时间字符串为毫秒 | `registerTool`，npm 依赖 | 2 | 依赖 Node 模块解析 |
| 15 | provider-payload | `packages/coding-agent/examples/extensions/provider-payload.ts` | 拦截并记录发送给 LLM provider 的原始请求 payload 到日志文件（调试/审计用） | Provider 相关 | 4 | 深度 API |

**汇总**：Tier 1 可优先以 **tps** 为验收；**files/diff/preset/sandbox** 等覆盖 Tier 2 主体；**redraws / overlay-qa-tests / prompt-url-widget** 驱动 Tier 3。

---

**更新**：新增标杆扩展或 pi-mono API 变更时同步本矩阵。
