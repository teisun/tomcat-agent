# Agents 模块

**设计思想**：OpenClaw 使用 Pi 作为嵌入式 Agent 运行时，进程内队列化执行。同一 session 的请求通过 session lane 串行（`enqueueCommandInLane`），避免并发冲突。Agent 由 CLI、auto-reply、cron、hooks 等触发，工具通过 Pi 的 tool 机制暴露，受 tool-policy 与 sandbox 白名单约束。

---

## 一、职责概览

- **Pi 嵌入式运行**：`runEmbeddedPiAgent` 入队后执行 `runEmbeddedAttempt`，创建 Pi session 并 `subscribeEmbeddedPiSession` 订阅事件。
- **Session lane**：`resolveSessionLane(sessionKey)` 得到 lane，`enqueueCommandInLane` 串行化。
- **触发方**：CLI `agent` 命令、auto-reply 的 getReply、cron 任务、hooks 的 llm-slug 等。
- **工具**：gateway-tool、sessions-*、memory-tool、browser-tool 等，经 pi-tools 注册，受 tool-policy 与 sandbox 约束。

---

## 二、入口与调用链

**入口**：`openclaw/src/agents/pi-embedded.ts` 导出 `runEmbeddedPiAgent`；实际实现在 `openclaw/src/agents/pi-embedded-runner/run.ts`。

**调用链**：

```
runEmbeddedPiAgent(params)
  → resolveSessionLane(sessionKey)
  → enqueueCommandInLane(sessionLane, () => enqueueGlobal(...))
  → runEmbeddedAttempt (run/attempt.ts)
  → 创建 Pi embedded session
  → subscribeEmbeddedPiSession (pi-embedded-subscribe.ts)
  → 事件流：message_start/update/end、tool_execution_*、agent_*
  → 回调：onToolResult、onPartialReply、onBlockReply、onReasoningStream
```

---

## 三、工具与 Policy

- **pi-tools**：`openclaw/src/agents/pi-tools.ts`，构建传给 Pi 的工具列表。
- **tool-policy**：`openclaw/src/agents/tool-policy.ts`，根据 session、sandbox 模式决定允许/拒绝的工具。
- **sandbox 白名单**：非 main session 在 Docker sandbox 中运行时，仅允许 bash、process、read、write、sessions_* 等，拒绝 browser、canvas、cron 等。

---

## 四、子文档索引

- [Pi嵌入式运行时](04-Agents/Pi嵌入式运行时.md)：run/attempt、subscribe、事件流
- [工具流与订阅](04-Agents/工具流与订阅.md)：handlers、onToolResult、gateway-tool 等
- [技能与Pi适配](04-Agents/技能与Pi适配.md)：SKILL.md、plugin-skills、ClawdHub
