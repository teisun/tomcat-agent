# Gateway 协议与 Schema

**设计思想**：Gateway 与客户端通过 WebSocket 交换 JSON 消息，采用类 JSON-RPC 的请求/响应与事件推送模式。协议结构、帧格式、会话/配置/agents 等 schema 定义在 `openclaw/src/gateway/protocol/`，并通过 `protocol-gen.ts`、`protocol-gen-swift.ts` 生成 TypeScript 与 Swift 类型。

---

## 一、协议目录结构

```
openclaw/src/gateway/protocol/
  index.ts
  schema.ts
  schema/
    types.ts
    frames.ts
    protocol-schemas.ts
    sessions.ts
    snapshot.ts
    wizard.ts
    ...
  client-info.ts
```

---

## 二、协议帧

- **请求**：客户端发送 `{ method, params, id? }` 形式的消息。
- **响应**：服务端返回 `{ id, result? }` 或 `{ id, error }`。
- **事件**：服务端主动推送 `{ event, data }`，无 id。

**帧类型**：见 `openclaw/src/gateway/protocol/schema/frames.ts` 与 `protocol-schemas.ts`。

---

## 三、Schema 定义

- **sessions**：SessionEntry、session key、sessions.patch 参数。
- **config**：config.get、config.set、config.patch 的 payload。
- **agents**：agents.list、agent 相关。
- **wizard**：wizard.start、wizard.next、wizard.status。
- **nodes**：node.list、node.describe、node.invoke 等。

Schema 使用 `@sinclair/typebox` 或类似方式定义，并导出为 `dist/protocol.schema.json`。

---

## 四、protocol-gen

- **protocol-gen.ts**：根据 schema 生成 `dist/protocol.schema.json`。
- **protocol-gen-swift.ts**：生成 `apps/macos/Sources/ClawdbotProtocol/GatewayModels.swift`，供 Swift 客户端使用。
- **校验**：`pnpm protocol:check` 确保生成产物与源码一致。
