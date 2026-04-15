# Channels Outbound 与配对

## 零、先用大白话

Outbound 像 **统一邮局**。  
助理写好回信了，不管要发到微信还是 Slack，都先走 **同一套打包规则**。  
**配对**像 **小区门禁**：不认识的人，先对暗号再放行。

**这一节你会学到**：`deliveryMode` 三种口味；配对码大致怎么流转。

---

**设计思想**：出站消息经 `infra/outbound` 统一投递，按 channel 的 `deliveryMode` 选择直接调用 channel 的 send 或经 Gateway `send` method。配对（pairing）控制 DM 访问，未知发件人需先 approve 才能处理消息。

---

## ASCII 核心四图

### 1) 结构图

```text
Agent 回复 / 系统通知
        |
        v
infra/outbound（统一打包）
        |
        +--> direct -> channel.send*
        +--> gateway -> WS send method
        +--> hybrid -> 按场景分支
```

### 2) 调用流图

```text
deliver 目标解析
  -> 查 pairing allowlist
      -> 未批准 -> 生成配对提示 / 丢弃
          -> 已批准 -> sendMessage(deliveryMode)
```

### 3) 时序图

```text
Unknown DM    Outbound        Pairing store      Channel send
     |             |                 |                |
     | 入站        |                 |                |
     |------------>| 无 allow        |                |
     |<------------| 配对码挑战     |                |
     | 用户确认    |---------------->| 写入 allow    |
     |------------>|-------------------------------->| 投递
```

### 4) 数据闭环图

```text
配对码 / allowlist 写入 ~/.openclaw/
        |
        v
后续同 peer 消息直投
        |
        v
出站回执写 transcript
        |
        v
撤销 allow 或 rotate token -> 重新配对
```

---

## 一、DeliveryMode

- **direct**：直接调用 channel 的 outbound.sendText/sendMedia，不经过 Gateway。
- **gateway**：通过 Gateway `send` method，由 Gateway 转发给 channel。
- **hybrid**：根据场景选择 direct 或 gateway。

**解析**：`src/infra/outbound/message.ts` 的 `sendMessage` 根据 `plugin.outbound?.deliveryMode` 分支。

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

- **存储**：`src/pairing/`，配对码与 allowlist 存储。
- **流程**：未知发件人发 DM → 返回配对码 → 用户执行 `openclaw pairing approve <channel> <code>` → 发件人加入 allowlist。
- **dmPolicy**：`pairing`（默认）或 `open`，后者需显式 allowlist 配置。

---

## 四、关键文件

| 文件 | 职责 |
|------|------|
| `src/infra/outbound/deliver.ts` | 投递入口 |
| `src/infra/outbound/targets.ts` | resolveTarget |
| `src/infra/outbound/message.ts` | sendMessage |
| `src/pairing/` | 配对码、allowlist |

---

## 常见误会

- **误会**：`gateway` 模式一定比 `direct` 慢所以要全改 direct。**正解**：`gateway` 让 **远程 CLI** 也能发；看部署形态选。  
- **误会**：配对只影响出站。**正解**：**入站**也会被挡；陌生人可能只看到配对提示。  
- **误会**：`dmPolicy: open` 等于全网随便聊。**正解**：仍要配 **allowlist** 等；安全见官方文档。
