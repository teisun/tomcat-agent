# Channels 通道模块

**设计思想**：OpenClaw 将各通讯渠道（WhatsApp、Telegram、Slack 等）抽象为 ChannelPlugin，通过统一的适配器契约（config、gateway、outbound、pairing、security 等）注册与运行。内置渠道在 `src/`，扩展在 `extensions/*`，均通过 `clawdbot.plugin.json` 注册。入站消息经 channel monitor 进入 Gateway/auto-reply，出站经 `infra/outbound` 按 deliveryMode 投递。

---

## 一、职责概览

- **入站**：各 channel 的 monitor 接收消息，经 debounce、group-gating、pairing 检查后，调用 auto-reply 的 getReply 或 handleCommands。
- **出站**：`infra/outbound` 的 `deliverOutboundPayloads` 根据 channel 的 `outbound.deliveryMode`（direct/gateway/hybrid）选择直接发送或经 Gateway send method。
- **配对**：DM 配对策略（pairing/open）、allowlist、`clawdbot pairing approve`。
- **安全**：ChannelSecurityAdapter、dmPolicy、allowFrom。

---

## 二、ChannelPlugin 契约

**定义**：`openclaw/src/channels/plugins/types.plugin.ts`

**核心字段**：

| 字段 | 类型 | 说明 |
|------|------|------|
| id | ChannelId | 渠道 ID |
| meta | ChannelMeta | 元信息 |
| capabilities | ChannelCapabilities | 能力标记 |
| config | ChannelConfigAdapter | 账户解析、allowFrom |
| gateway | ChannelGatewayAdapter | startAccount、loginWithQr、logout |
| outbound | ChannelOutboundAdapter | deliveryMode、sendText、sendMedia |
| pairing | ChannelPairingAdapter | 配对码、approve |
| security | ChannelSecurityAdapter | dmPolicy、allowlist |
| groups | ChannelGroupAdapter | requireMention |
| mentions | ChannelMentionAdapter | @ 解析 |

---

## 三、内置与扩展加载

- **内置**：telegram、slack、discord、signal、whatsapp、imessage 等在 `src/` 下，通过 `channels/plugins/index` 或 registry 注册。
- **扩展**：`extensions/telegram`、`extensions/slack` 等，每个含 `clawdbot.plugin.json`，声明 id、入口等。
- **加载路径**：Plugins loader 扫描 extensions，按 plugin 配置加载 channel 实现。

---

## 四、子文档索引

- [插件模型与适配器](03-Channels/插件模型与适配器.md)：各适配器职责、extension 实现模式
- [Outbound与配对](03-Channels/Outbound与配对.md)：deliveryMode、deliver、pairing、allowlist
