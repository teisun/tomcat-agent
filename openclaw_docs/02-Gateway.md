# Gateway 控制面

**设计思想**：Gateway 是 OpenClaw 的本地优先控制平面，仅负责协调与编排，不承载业务逻辑。通过 HTTP + WebSocket 提供 JSON-RPC 风格 Methods，管理会话、通道、配置、Cron、事件、Nodes、健康等；实际 Agent 运行、Channel 连接、工具执行由各自模块完成。

---

## 一、职责概览

- **WebSocket 服务**：客户端（CLI、UI、macOS 应用、Nodes）通过 WS 连接，发送 JSON 请求，接收响应与事件。
- **Gateway Methods**：`listGatewayMethods` 注册的方法（如 chat.history、chat.send、sessions.patch、config.get、node.invoke、cron.* 等），由 `coreGatewayHandlers` 分发。
- **配置热更**：`startGatewayConfigReloader` 监听配置文件变化，按 `GatewayReloadPlan` 执行 hot reload 或 restart。
- **侧车**：Browser、Canvas-host、Tailscale 等由 `startGatewaySidecars` 启动，随 Gateway 生命周期管理。
- **Cron**：`buildGatewayCronService` 提供定时任务调度。
- **健康与 Presence**：`refreshGatewayHealthSnapshot`、`incrementPresenceVersion`、`getHealthCache`。

---

## 二、入口与启动流程

**入口**：`openclaw/src/gateway/server.impl.ts` 的 `startGatewayServer(port?, opts?)`

**启动流程**（简化）：

```
startGatewayServer
  → resolveGatewayRuntimeConfig
  → migrateLegacyConfig（若需）
  → loadGatewayPlugins
  → createGatewayRuntimeState
  → createChannelManager
  → createAgentEventHandler (server-chat)
  → createNodeSubscriptionManager
  → buildGatewayCronService
  → ExecApprovalManager
  → loadGatewayTlsRuntime（若 TLS）
  → create HTTP server + WSS (server-http)
  → attachGatewayWsHandlers (server-ws-runtime)
  → startGatewaySidecars (browser, canvas-host, tailscale)
  → startGatewayConfigReloader
  → createChannelManager.start (各 channel gateway)
  → startGatewayMaintenanceTimers
```

---

## 三、关键类型与事件

- **GATEWAY_EVENTS**：`openclaw/src/gateway/server-methods-list.ts`，事件名列表（如 config.updated、sessions.updated）。
- **listGatewayMethods**：返回所有注册的 Method 名。
- **GatewayServer**：`{ close }`，用于优雅关闭。

---

## 四、与上下游协作

| 模块 | 协作方式 |
|------|----------|
| Config | loadConfig、config-reload 触发重载 |
| Channels | createChannelManager 启动各 channel 的 gateway 适配器 |
| Agents | createAgentEventHandler 处理 chat 请求，调用 runEmbeddedPiAgent |
| Nodes | NodeRegistry、createNodeSubscriptionManager、node.* methods |
| Plugins | loadGatewayPlugins、各 channel 从 plugins 加载 |
| Cron | buildGatewayCronService |
| Exec Approvals | ExecApprovalManager、server-methods/exec-approval |

---

## 五、子文档索引

- [协议与Schema](02-Gateway/协议与Schema.md)：协议帧、Schema、protocol-gen
- [WebSocket与连接](02-Gateway/WebSocket与连接.md)：WS 连接、认证、presence
- [Methods与RPC](02-Gateway/Methods与RPC.md)：Gateway Methods、调用路径
- [配置热更与侧车](02-Gateway/配置热更与侧车.md)：config-reload、sidecars、browser、canvas、tailscale
