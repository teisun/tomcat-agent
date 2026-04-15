# Gateway WebSocket 与连接

## 零、先用大白话

WebSocket 像 **一直通着的电话**。  
拨一次，后面随时能你一句我一句。  
Gateway 用这条线推 **回答**，也推 **事件**（比如配置变了）。

**这一节你会学到**：连接从哪进来；谁验密码/token；断了会怎样。

---

**设计思想**：Gateway 在 HTTP server 上挂载 WebSocket 端点，客户端连接后通过 JSON 消息进行请求/响应与事件订阅。连接生命周期、认证、presence 由 `ws-connection`、`message-handler`、`server-ws-runtime` 管理。

---

## ASCII 核心四图

### 1) 结构图

```text
HTTP server（server-http.ts）
        |
        v
WebSocket 端点
        |
        +--> ws-connection（认证/心跳）
        +--> message-handler（JSON 帧）
        +--> server-ws-runtime（订阅表）
```

### 2) 调用流图

```text
Upgrade / connect
  -> 校验 bearer / origin
      -> 注册连接
          -> 双向：请求/响应 + server push 事件
              -> 断开 -> 清理订阅与 presence
```

### 3) 时序图

```text
Client       HTTP stack        WS runtime       Presence
  |              |                  |               |
  | Upgrade      |                  |               |
  |------------->|----------------->|               |
  | hello/sub    |                  |               |
  |-------------------------------->| 注册          |
  |              |                  |-------------->| bump
```

### 4) 数据闭环图

```text
新连接 / 重连
        |
        v
presence 版本与健康缓存刷新
        |
        v
Control UI / CLI 展示在线状态
        |
        v
配置或密钥轮换 -> 旧连接失效 -> 客户端自动重连
```

---

## 一、连接入口（路径相对 `openclaw/` 根）

- **HTTP/WSS**：`src/gateway/server-http.ts` —— 起 HTTP，再把 WebSocket 挂上去。  
- **连接处理**：`src/gateway/server/ws-connection.ts` 的 `attachGatewayWsConnectionHandler`。  
- **消息分发**：`src/gateway/server/ws-connection/message-handler.ts` —— 把 JSON 帧交给对应 method。  
- **运行时挂载**：`src/gateway/server-ws-runtime.ts` 的 `attachGatewayWsHandlers` —— 把 `coreGatewayHandlers` 绑到连接上。

---

## 二、连接生命周期

1. **握手**：客户端发起 WS 连接，可携带 token 或 password（见 `gateway/auth.ts`）。
2. **认证**：`src/gateway/auth.ts` 一带校验 token/password；失败则拒绝连接。
3. **Hello**：连接建立后可能交换 hello/helloOk，确认协议版本与能力。
4. **请求/响应**：客户端发送 `{ method, params, id }`，服务端返回 `{ id, result }` 或 `{ id, error }`。
5. **事件**：服务端通过 `GATEWAY_EVENTS` 推送 config.updated、sessions.updated 等。
6. **关闭**：客户端断开或服务端调用 `close`。

---

## 三、Presence

- **system-presence**：`src/infra/system-presence.ts` 管理客户端在线状态。
- **Presence 版本**：`incrementPresenceVersion`、`getPresenceVersion` 用于通知客户端状态变化。
- **心跳**：`last-heartbeat`、`set-heartbeats` 等 method 支持心跳维持。

---

## 四、关键文件

| 文件 | 职责 |
|------|------|
| `src/gateway/server-http.ts` | HTTP server、WSS 挂载 |
| `src/gateway/server/ws-connection.ts` | 连接接受、消息入口 |
| `src/gateway/server/ws-connection/message-handler.ts` | 消息解析、路由 |
| `src/gateway/server-ws-runtime.ts` | `attachGatewayWsHandlers` |
| `src/gateway/auth.ts` | 认证、token 校验 |

---

## 常见误会

- **误会**：WS 连上就等于已经登录进某个聊天会话。**正解**：会话是 **后序 method**（如 `chat.*` / `sessions.*`）的事；连接只是「电话通了」。  
- **误会**：断线后服务器一定帮你重放没收到的推送。**正解**：事件可能丢；客户端要有 **重连 + 再拉状态** 的准备。  
- **误会**：浏览器随便哪个网站都能连我家 Gateway。**正解**：有 **origin 等检查**；暴露到公网要配合官方安全文档，别裸奔。
