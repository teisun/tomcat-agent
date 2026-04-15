# Channels 通道模块

**版本**：见 [README.md](README.md) 同步表；路径相对 **`openclaw/` 仓库根**。

## 零、先用大白话

Channels 像 **不同品牌的对讲机**。  
微信、Telegram、Slack……协议都不一样。  
OpenClaw 给每品牌一个 **统一插口**（`ChannelPlugin`）。  
Gateway 只认插口，不认品牌细节。  
消息 **进来** 走监控器 → auto-reply；**出去** 走 `infra/outbound` 统一打包。

**这一节你会学到**：入站出站各谁负责；插件从哪加载；该翻 `src/channels` 还是 `extensions/`。

---

## ASCII 核心四图

### 1) 结构图

```text
extensions/* 里的渠道包  +  仓库内置实现
              |
              v
        ChannelPlugin（一堆小适配器拼起来）
              |
    +-----------+-----------+
    v                       v
inbound monitor        outbound.send*
    |                       ^
    v                       |
 auto-reply 管线      infra/outbound（邮局）
```

### 2) 调用流图（入站 → 回复 → 出站）

```text
IM 平台推消息
  -> 渠道 monitor 变成「统一形状」的事件
      -> debounce / 群门控 / 配对检查
          -> handleCommands（/new 等）或 getReplyFromConfig（要模型回）
              -> deliver-reply（按 deliveryMode 选路）
```

### 3) 时序图

```text
IM 平台     ChannelPlugin      Gateway/auto-reply      Outbound
  |              |                    |                    |
  | 新消息        |                    |                    |
  |------------->| 规范化              |                    |
  |              |------------------->| 路由 + 排队 Agent |
  |              |                    |------------------->| 发回 IM
```

### 4) 数据闭环图

```text
openclaw.plugin.json 写明「我是谁、能干啥」
        |
        v
Gateway 加载插件 -> 渠道账号连上
        |
        v
会话写进 ~/.openclaw/...jsonl
        |
        v
改插件或改配置 -> 热更或重启 -> 下一轮走新逻辑
```

---

## 一、职责（拆开想就不晕）

- **入站**：各渠道的 **monitor** 把原始消息变成内部事件。后面是 **debounce、群是否 @、配对是否放行**（细节散在 `src/auto-reply/` 与渠道目录）。  
- **出站**：别让每个渠道自己乱发。统一走 **`src/infra/outbound/`**，按 **`deliveryMode`**（direct / gateway / hybrid）决定怎么发。  
- **配对与安全**：陌生人能不能聊，见 [Outbound与配对](03-Channels/Outbound与配对.md) 与 [17-Pairing.md](17-Pairing.md)。

---

## 二、ChannelPlugin 契约（工程师锚点）

**类型定义**：[`src/channels/plugins/types.plugin.ts`](../openclaw/src/channels/plugins/types.plugin.ts)

**一块块「七巧板」**（实现时按需拼）：

| 块 | 人话 |
|----|------|
| `config` | 读账号、谁能发给我 |
| `gateway` | 登录、挂线、断线 |
| `outbound` | 怎么把字和图发回去 |
| `pairing` | 陌生人敲门怎么对暗号 |
| `security` | DM 策略、白名单 |
| `groups` / `mentions` | 群里要不要理你、@ 谁算数 |

---

## 三、内置 vs 扩展

- **内置**：`src/channels/`、`src/telegram/`、`src/slack/` 等与核心绑在一起的实现（以仓库为准）。  
- **扩展**：`extensions/<名字>/`，里面有 **`openclaw.plugin.json`**，被插件发现逻辑扫进来。

---

## 四、子文档

- [插件模型与适配器](03-Channels/插件模型与适配器.md)  
- [Outbound与配对](03-Channels/Outbound与配对.md)  

---

## 常见误会

- **误会**：每个渠道各开一个 Gateway。**正解**：**一台机子一个 Gateway**；渠道是连在 Gateway 上的「线」。  
- **误会**：Channel 里直接跑 Pi。**正解**：Channel 多半只做 **收发**；推理在 **Agent 路径**（见 [04-Agents.md](04-Agents.md)）。  
- **误会**：改了 `extensions/` 不用重启。**正解**：看 **config-reload** 规则；有的改动会整网关重启。
