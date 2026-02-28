# Hooks

**设计思想**：Hooks 为事件驱动的扩展点，事件类型与注册在 internal-hooks；bundled 提供 session-memory、llm-slug、command-logger、boot-md、soul-evil 等；hooks CLI 管理安装与状态。

---

## 一、事件与注册

- **internal-hooks.ts**：`registerInternalHook`、`triggerInternalHook`、`createInternalHookEvent`。
- **HookEventType**：事件类型枚举。
- **HookHandler**：处理函数签名。

---

## 二、Bundled Hooks

- **session-memory**：/new 时保存会话到 memory。
- **llm-slug**：LLM slug 生成。
- **command-logger**：命令日志。
- **boot-md**：启动 Markdown。
- **soul-evil**：特定业务逻辑。

---

## 三、Hooks CLI

- **hooks-cli**：`openclaw/src/cli/hooks-cli.ts`（或 register.subclis 中 hooks），安装、列表、状态。
- **gmail**：Gmail 相关 hooks，与 webhooks 配合。
