# Sessions 与 Routing

**设计思想**：Session 是 OpenClaw 中一次对话会话的抽象，由 session key 唯一标识。Routing 根据入站消息的 channel、account、peer 解析出 agentId 与 sessionKey，决定消息归属哪个 Agent 与哪个 Session。Main 会话与群组会话采用不同 key 形式，群组激活策略（mention/always）控制何时响应。

---

## 一、Session 模型

### 1.1 SessionEntry

**定义**：`openclaw/src/config/sessions/types.ts`

**关键字段**：

| 字段 | 类型 | 说明 |
|------|------|------|
| sessionId | string | 唯一会话 ID |
| sessionFile | string | 转录文件路径 |
| thinkingLevel | string | 思考级别 |
| verboseLevel | string | 详细输出 |
| groupActivation | "mention" \| "always" | 群组激活模式 |
| sendPolicy | "allow" \| "deny" | 发送策略 |
| modelOverride | string | 模型覆盖 |
| compactionCount | number | 压缩次数 |
| memoryFlushAt | number | 记忆 flush 时间戳 |

### 1.2 Session Key 形式

- **Main 会话**：`agent:<agentId>:main` 或 `agent:<agentId>:<mainKey>`
- **群组会话**：`agent:<agentId>:<channel>:group:<groupId>`
- **Peer 会话**：`agent:<agentId>:<channel>:dm:<peerId>`（per-sender 等 scope）

**构建**：`openclaw/src/routing/session-key.ts` 的 `buildAgentMainSessionKey`、`buildAgentPeerSessionKey`。

---

## 二、Routing 解析

### 2.1 入口

- **resolveAgentRoute**：`openclaw/src/routing/resolve-route.ts`，根据 `ResolveAgentRouteInput` 返回 `ResolvedAgentRoute`。

### 2.2 输入

```ts
ResolveAgentRouteInput = {
  cfg, channel, accountId?, peer?, guildId?, teamId?
}
```

### 2.3 输出

```ts
ResolvedAgentRoute = {
  agentId, channel, accountId,
  sessionKey,    // 内部持久化用
  mainSessionKey, // 直接聊天折叠用
  matchedBy      // binding.peer | binding.guild | ... | default
}
```

### 2.4 匹配顺序

1. binding.peer：按 peer 匹配
2. binding.guild：按 Discord guild
3. binding.team：按 Slack team
4. binding.account：按 accountId
5. binding.channel：按 channel
6. default：默认 agent

**Bindings**：`openclaw/src/config/types.agents.ts` 的 `AgentBinding`，定义 channel、account、peer 等匹配规则。

---

## 三、群组策略

### 3.1 Group Activation

- **mention**：仅在 @ 提及时激活。
- **always**：始终响应群组消息。

**解析**：`openclaw/src/web/auto-reply/monitor/group-activation.ts` 的 `resolveGroupActivationFor`，从 session store 或 channel 的 group policy 读取。

### 3.2 Group Gating

- **group-gating.ts**：检查群组是否在 allowlist、是否需 mention。
- **mentions**：解析 @ 提及，判断是否激活。

---

## 四、Session Store

- **存储**：`openclaw/src/config/sessions/store.ts`、`sessions.js` 的 `loadSessionStore`、`saveSessionStore`。
- **路径**：`resolveStorePath`、`resolveSessionFilePath`（`openclaw/src/config/sessions/paths.ts`）。
- **main-session**：`resolveMainSessionKey` 解析默认 main 会话 key。

---

## 五、Gateway 协议

- **sessions.patch**：更新 session 的 thinkingLevel、verboseLevel、model、sendPolicy、groupActivation 等。
- **sessions.list**、**sessions.preview**、**sessions.reset**、**sessions.delete**、**sessions.compact**：会话管理。

---

## 六、关键文件

| 文件 | 职责 |
|------|------|
| openclaw/src/config/sessions/types.ts | SessionEntry 类型 |
| openclaw/src/config/sessions/main-session.ts | resolveMainSessionKey |
| openclaw/src/config/sessions/session-key.ts | session key 工具 |
| openclaw/src/config/sessions/store.ts | 存储 |
| openclaw/src/routing/resolve-route.ts | resolveAgentRoute |
| openclaw/src/routing/session-key.ts | buildAgentSessionKey |
| openclaw/src/web/auto-reply/monitor/group-activation.ts | 群组激活 |
| openclaw/src/web/auto-reply/monitor/group-gating.ts | 群组门控 |
