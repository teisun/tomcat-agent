# Pairing

**设计思想**：Pairing 管理设备配对码与 allowlist，控制哪些设备/用户可访问；与 Channels 的 outbound、deliveryMode 配合，决定消息投递目标。

---

## 一、职责

- **配对码**：生成、验证配对码。
- **allowlist**：存储允许的发送者/设备列表。
- **与 Outbound**：`openclaw/src/infra/outbound/` 的 targets、deliver 使用 pairing 信息。
- **与 Channels**：ChannelPlugin 的 pairing 适配器。

---

## 二、关键路径

- **pairing/**：`openclaw/src/pairing/`，配对逻辑与存储。
- **allowlist**：allowlist 存储与查询。
- **deliveryMode**：direct、gateway、hybrid 与 pairing 的关系。
