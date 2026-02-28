# Pi 嵌入式运行时

**设计思想**：Pi Agent 以嵌入式方式运行在同一进程，通过 `@mariozechner/pi-agent-core` 等库创建 session，不跨进程 RPC。事件通过 `subscribeEmbeddedPiSession` 订阅，流式回调给上层。

---

## 一、Run 与 Attempt

- **runEmbeddedPiAgent**：`openclaw/src/agents/pi-embedded-runner/run.ts`，入队后执行。
- **runEmbeddedAttempt**：`openclaw/src/agents/pi-embedded-runner/run/attempt.ts`，创建 Pi session、构建 payload、调用 Pi API、订阅事件。

---

## 二、Subscribe 与事件流

- **subscribeEmbeddedPiSession**：`openclaw/src/agents/pi-embedded-subscribe.ts`，订阅 Pi session 的 message、tool、agent 事件。
- **事件类型**：message_start/update/end、tool_execution_start/update/end、agent_start/end。
- **handlers**：`pi-embedded-subscribe.handlers.ts`、`handlers.tools.ts`、`handlers.messages.ts`、`handlers.lifecycle.ts` 处理各事件。

---

## 三、与 Pi 核心库边界

- Pi 库负责：session 管理、模型调用、streaming、tool 调用。
- OpenClaw 负责：lane 队列、payload 构建、事件订阅与回调、工具实现（gateway-tool 等）、compaction、memory-flush。
