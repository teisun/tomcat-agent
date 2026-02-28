# Gateway WebSocket 与连接

**设计思想**：Gateway 在 HTTP server 上挂载 WebSocket 端点，客户端连接后通过 JSON 消息进行请求/响应与事件订阅。连接生命周期、认证、presence 由 `ws-connection`、`message-handler`、`server-ws-runtime` 管理。

---

## 一、连接入口

- **HTTP/WSS**：`openclaw/src/gateway/server-http.ts` 创建 HTTP server，挂载 `/` 或指定路径的 WebSocket。
- **连接处理**：`openclaw/src/gateway/server/ws-connection.ts` 的 `attachGatewayWsConnectionHandler` 处理新连接。
- **消息分发**：`openclaw/src/gateway/server/ws-connection/message-handler.ts` 解析入站消息，路由到对应 handler。
- **运行时挂载**：`openclaw/src/gateway/server-ws-runtime.ts` 的 `attachGatewayWsHandlers` 将 `coreGatewayHandlers` 与 WS 连接绑定。

---

## 二、连接生命周期

1. **握手**：客户端发起 WS 连接，可携带 token 或 password（见 `gateway/auth.ts`）。
2. **认证**：`resolveGatewayAuth` 校验 token/password，失败则拒绝连接。
3. **Hello**：连接建立后可能交换 hello/helloOk，确认协议版本与能力。
4. **请求/响应**：客户端发送 `{ method, params, id }`，服务端返回 `{ id, result }` 或 `{ id, error }`。
5. **事件**：服务端通过 `GATEWAY_EVENTS` 推送 config.updated、sessions.updated 等。
6. **关闭**：客户端断开或服务端调用 `close`。

---

## 三、Presence

- **system-presence**：`openclaw/src/infra/system-presence.ts` 管理客户端在线状态。
- **Presence 版本**：`incrementPresenceVersion`、`getPresenceVersion` 用于通知客户端状态变化。
- **心跳**：`last-heartbeat`、`set-heartbeats` 等 method 支持心跳维持。

---

## 四、关键文件

| 文件 | 职责 |
|------|------|
| openclaw/src/gateway/server-http.ts | HTTP server、WSS 挂载 |
| openclaw/src/gateway/server/ws-connection.ts | 连接接受、消息入口 |
| openclaw/src/gateway/server/ws-connection/message-handler.ts | 消息解析、路由 |
| openclaw/src/gateway/server-ws-runtime.ts | attachGatewayWsHandlers |
| openclaw/src/gateway/auth.ts | 认证、token 校验 |
