# pi-tui 与 pi-web-ui

pi-tui 提供终端 UI 框架（差分渲染、同步输出、组件化）；pi-web-ui 提供基于 pi-agent-core 与 pi-ai 的 Web 聊天组件与存储，供浏览器或 Electron 等宿主使用。

---

## 1. pi-tui（@mariozechner/pi-tui）

### 1.1 定位与特性

- **差分渲染**：三种策略，只更新变化部分，减少闪烁。
- **同步输出**：使用 CSI 2026（Synchronized Output）实现原子屏幕更新。
- **组件化**：Component 接口（render、handleInput、invalidate），Container 管理子组件，Focusable 支持 IME 光标定位。
- **内置组件**：Text、TruncatedText、Input、Editor、Markdown、Loader、SelectList、SettingsList、Spacer、Image、Box、Container。
- **Overlay**：在现有内容之上渲染对话框/菜单，支持锚点、百分比、绝对坐标、margin、visible 回调。
- **主题**：组件接受 theme 接口，可统一配色与样式。
- **粘贴与图片**：Bracketed Paste Mode 正确处理大段粘贴；支持 Kitty/iTerm2 内联图片协议。

### 1.2 核心 API

- **TUI(terminal)**：主容器，管理组件与渲染；**addChild** / **removeChild**、**start()** / **stop()**、**requestRender()**。
- **showOverlay(component, options?)**：显示 overlay，可选 width/height、anchor、offset、row/col、margin、visible；返回 OverlayHandle（hide、setHidden、isHidden）。
- **hideOverlay()**：隐藏最顶层 overlay；**hasOverlay()**：是否有可见 overlay。
- **ProcessTerminal**：与子进程终端对接，用于 pi 交互模式。

### 1.3 Component 接口

- **render(width: number): string[]**：返回每行字符串，每行长度不超过 width（否则 TUI 报错）；可用 truncateToWidth、wrapTextWithAnsi 等辅助。
- **handleInput?(data: string)**：获得焦点时接收键盘输入（含 ANSI 转义）。
- **invalidate?()**：清除缓存，下次 render 从头计算。

每行末尾 TUI 会追加 SGR reset 与 OSC 8 reset，样式不跨行，多行需每行单独应用样式或使用 wrapTextWithAnsi。

### 1.4 Focusable 与 IME

- 需要文本光标与 IME（如中日韩）的组件实现 **Focusable**：`focused: boolean`，render 时在光标处输出 **CURSOR_MARKER**（零宽 APC 转义），TUI 将硬件光标定位到该处并显示。
- Editor、Input 已实现；含 Input/Editor 的容器需实现 Focusable 并将 focused 下传到子组件，否则 IME 候选框位置错误。

### 1.5 在 coding-agent 中的使用

- InteractiveMode 使用 pi-tui 构建界面：消息区、Editor、footer、slash 命令 UI、/settings、/model 等；session 事件驱动 requestRender，用户输入经 Editor 提交后调用 session.prompt()。

---

## 2. pi-web-ui（@mariozechner/pi-web-ui）

### 2.1 定位与特性

- **技术栈**：mini-lit Web Components + Tailwind CSS v4。
- **依赖**：pi-agent-core、pi-ai。
- **功能**：完整聊天 UI（消息历史、流式、工具执行）、附件（PDF/DOCX/XLSX/PPTX、图片）与预览/提取、Artifacts（HTML/SVG/Markdown 等沙箱执行）、内置工具（JS REPL、文档提取、Artifacts）、IndexedDB 存储（会话、API Key、设置）、CORS 代理与自定义 Provider（Ollama、LM Studio、vLLM 等）。

### 2.2 架构概览

- **ChatPanel**：顶层组件，内含 **AgentInterface**（消息与输入）与 **ArtifactsPanel**（HTML/SVG/MD 等）。
- **Agent**：来自 pi-agent-core，负责 state、事件、tool 执行；ChatPanel/AgentInterface 通过 setAgent(agent) 绑定并订阅事件。
- **AppStorage**：聚合 **SettingsStore**、**ProviderKeysStore**、**SessionsStore**（及 Metadata）；后端为 **IndexedDBStorageBackend**，需先配置各 Store 的 backend 再 **setAppStorage(storage)**，供 UI 读写配置与会话。

### 2.3 主要组件

- **ChatPanel**：高级聊天界面 + Artifacts 面板；**setAgent(agent, options)**，options 含 onApiKeyRequired、onBeforeSend、onCostClick、sandboxUrlProvider、toolsFactory 等。
- **AgentInterface**：底层聊天组件，可单独用于自定义布局；属性包括 session（Agent）、enableAttachments、enableModelSelector、enableThinkingSelector、showThemeToggle 等。
- **ArtifactsPanel**：管理 Artifact 列表与展示，agent 绑定后提供 **artifactsPanel.tool** 给 Agent，支持创建/更新/删除 HTML、SVG、Markdown 等。

### 2.4 存储

- **IndexedDBStorageBackend**：dbName、version、stores 配置（来自各 Store 的 getConfig()）。
- **SettingsStore**：应用设置。
- **ProviderKeysStore**：各 Provider 的 API Key（加密或明文依实现）。
- **SessionsStore**：会话列表与内容；SessionsStore.getMetadataConfig() 用于元数据表。
- 使用前：`settings.setBackend(backend)` 等，然后 `setAppStorage(new AppStorage(settings, providerKeys, sessions, ..., backend))`。

### 2.5 消息类型与 convertToLlm

- **UserMessageWithAttachments**：role `user-with-attachments`，content + attachments（文件），defaultConvertToLlm 会转为 pi-ai 的 user message（含 image/text 块）。
- **ArtifactMessage**：role `artifact`，action、filename、content，仅 UI/持久化用，defaultConvertToLlm 会过滤掉。
- 自定义消息类型可通过 declaration merging 扩展 CustomAgentMessages，并配合 **registerMessageRenderer** 与自定义 **convertToLlm** 使用。

### 2.6 内置工具（Web 侧）

- **createJavaScriptReplTool()**：浏览器内沙箱执行 JavaScript；可配置 runtimeProvidersFactory（如 AttachmentsRuntimeProvider、ArtifactsRuntimeProvider）以访问附件与 Artifacts。
- **createExtractDocumentTool()**：从 URL 提取文档内容，可设 corsProxyUrl。
- **ArtifactsPanel.tool**：创建/更新/删除 Artifact（HTML、SVG、Markdown 等），由 ChatPanel 集成。

### 2.7 与 Agent 的集成方式

- 创建 **Agent**（initialState、convertToLlm: defaultConvertToLlm 等），订阅事件更新 UI。
- **ChatPanel.setAgent(agent, { onApiKeyRequired, ... })** 后，用户输入经 AgentInterface 调用 **agent.prompt(...)**，流式与 tool 执行通过 agent 事件反映到界面；持久化通过 AppStorage/SessionsStore 与 session 保存逻辑配合（若实现）。

---

## 3. 关键文件路径（参考）

| 包 | 路径/说明 |
|----|-----------|
| pi-tui | packages/tui/：TUI 类、Component/Focusable、Container、Editor/Input/Markdown 等、Overlay、ProcessTerminal、差分与同步输出逻辑 |
| pi-web-ui | packages/web-ui/：ChatPanel、AgentInterface、ArtifactsPanel、AppStorage、*Store、IndexedDBStorageBackend、defaultConvertToLlm、createJavaScriptReplTool、createExtractDocumentTool、app.css；example/ 为完整示例应用 |
