# ACP

**设计思想**：ACP（Agent Control Protocol）提供独立的协议服务器与客户端，与 GatewayClient 连接并转发事件，支持多节点或远程控制场景。

---

## 一、职责

- **server**：`openclaw/src/acp/server.js`（或 .ts），`serveAcpGateway`。
- **client**：ACP 客户端实现。
- **translator**：协议转换，与 Gateway 协议互操作。
- **session**：`createInMemorySessionStore`、`AcpSessionStore`。

---

## 二、与 Gateway 的衔接

- 通过 GatewayClient 连接 Gateway。
- 事件转发：将 Gateway 事件转发到 ACP 客户端。
- 用于远程控制、多节点部署等场景。
