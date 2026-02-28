# Channels Outbound 与配对

**设计思想**：出站消息经 `infra/outbound` 统一投递，按 channel 的 `deliveryMode` 选择直接调用 channel 的 send 或经 Gateway `send` method。配对（pairing）控制 DM 访问，未知发件人需先 approve 才能处理消息。

---

## 一、DeliveryMode

- **direct**：直接调用 channel 的 outbound.sendText/sendMedia，不经过 Gateway。
- **gateway**：通过 Gateway `send` method，由 Gateway 转发给 channel。
- **hybrid**：根据场景选择 direct 或 gateway。

**解析**：`openclaw/src/infra/outbound/message.ts` 的 `sendMessage` 根据 `plugin.outbound?.deliveryMode` 分支。

---

## 二、Outbound 流程

```
deliverOutboundPayloads (deliver.ts)
  → loadChannelOutboundAdapter
  → resolveTarget（若需）
  → chunker 分块（若 textChunkLimit）
  → sendText / sendMedia 调用 channel 实现
  → 返回 OutboundDeliveryResult
```

**依赖注入**：`OutboundSendDeps` 注入各 channel 的 send 函数（sendWhatsApp、sendTelegram 等），供 deliver 调用。

---

## 三、配对（Pairing）

- **存储**：`openclaw/src/pairing/`，配对码与 allowlist 存储。
- **流程**：未知发件人发 DM → 返回配对码 → 用户执行 `clawdbot pairing approve <channel> <code>` → 发件人加入 allowlist。
- **dmPolicy**：`pairing`（默认）或 `open`，后者需显式 allowlist 配置。

---

## 四、关键文件

| 文件 | 职责 |
|------|------|
| openclaw/src/infra/outbound/deliver.ts | 投递入口 |
| openclaw/src/infra/outbound/targets.ts | resolveTarget |
| openclaw/src/infra/outbound/message.ts | sendMessage |
| openclaw/src/pairing/ | 配对码、allowlist |
