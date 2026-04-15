# pi-tui 与 pi-web-ui

## 先用大白话

- **pi-tui**：在终端里画图形的库。它尽量**少重画**（差分渲染），并用 **CSI 2026** 让一帧输出**原子更新**，减少闪屏。
- **pi-web-ui**：在**浏览器**里做聊天窗口那一套（消息列表、附件、Artifacts 等），给网页或内嵌 WebView 用。

一个像**黑板**（终端里一行行画），一个像**网页聊天室**。

---

## pi-tui（`@mariozechner/pi-tui`）

### 特性（与 README 一致）

- 差分渲染、同步输出、组件化、`ProcessTerminal` 与子进程终端对接。
- 内置组件：Text、TruncatedText、Input、Editor、Markdown、Loader、SelectList、SettingsList、Spacer、Image、Box、Container 等。
- **Focusable + CURSOR_MARKER**：中文输入法等需要正确定位光标。

### 在 coding-agent 里

**InteractiveMode** 用 pi-tui 拼消息区、编辑器、footer、overlay；`session.subscribe` 收到事件后 `requestRender()`。

---

## pi-web-ui（`@mariozechner/pi-web-ui`）

### 技术栈

- **mini-lit** Web Components + **Tailwind CSS v4**（开发依赖里有 `@tailwindcss/cli` 4.x beta，以仓库为准）。

### 依赖说明（读 `packages/web-ui/package.json` + 源码）

- **显式依赖**里能看到 **`@mariozechner/pi-ai`**、**`@mariozechner/pi-tui`**（以及 LM Studio、Ollama、文档预览等库）。
- **源码**（如 `src/index.ts`）会 **从 `@mariozechner/pi-agent-core` 再导出类型**（`Agent`、`AgentMessage` 等），所以阅读 Web UI 时仍会看到 agent-core；若你写「依赖关系图」，建议写：**运行与类型上离不开 agent-core 的概念模型，具体 npm 依赖以 package.json 为准**。

### ASCII：页面与 Agent 的关系

```
  ChatPanel
      |
      +---- AgentInterface  (消息 + 输入)
      |
      +---- ArtifactsPanel  (HTML/SVG/MD 等)
      |
  setAgent(agent)  --->  pi-agent-core 的 Agent 实例（事件驱动 UI）
```

### 存储

**AppStorage** 聚合 **SettingsStore**、**ProviderKeysStore**、**SessionsStore** 等，底层可实现为 **IndexedDB**（详见各 `*Store` 与 README）。

### Web 侧额外工具

如 `createJavaScriptReplTool`、`createExtractDocumentTool`、Artifacts 相关 tool（以 `packages/web-ui/src` 为准）。

---

## 关键路径

| 包 | 位置 |
|----|------|
| pi-tui | `packages/tui/` |
| pi-web-ui | `packages/web-ui/`（含 `example/` 示例应用） |
