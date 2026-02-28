# Product Brief: tomcatclaw

## 产品名

**tomcatclaw** — 跨全平台轻量龙虾 AI Agent

## 问题描述
- 现有的openclaw太重了
- 代码量太大，40万行代码
- 资源占用过大，内存达到1.5G
- 我要学习做一个AI Agent

## 产品定位与目标用户
- 产品定位：Clawdbot 是一款**本地优先的个人 AI 助手**，用户在自己设备上运行，在已有通讯渠道上统一对话。
- 目标用户：希望拥有私密、可自托管、多通道、单用户个人助手的用户。

## 核心价值
- 跨平台，在 Windows / macOS / Linux / Android 上运行的、小体积低内存、功能对标 OpenClaw 的 Rust AI Agent；支持单二进制部署、条件编译裁剪，兼容 openclaw 生态。
- 安全与可控：Rust 实现, 内存安全、设计安全
- 与openclaw一样虽然重复但也是价值，在 钉钉、飞书、WhatsApp、Telegram、Slack、Discord、Signal、iMessage、Teams、WebChat 等渠道收发消息，由单一控制面（Gateway）统一管理会话、通道与技能；支持记忆；支持多 Agent 路由、语音唤醒、画布与伴侣应用。



## 核心价值

- **全平台 + 轻量**：一份代码适配PC 与 Android(可裁剪)；编译后目标 <10MB，轻量资源占用低
- **功能完整**：消息闭环、多 Agent 隔离、Cron/心跳/Webhook 主动触达、统一 Outbound、会话隔离、记忆、沙箱与命令执行（分阶段实现）。
- **兼容 openclaw**：协议与配置子集兼容，便于与现有 Control UI、配置、多 Agent 玩法衔接。
- **安全与可控**：Rust 实现，类型与并发安全；沙箱与 SSRF 防护等安全机制。



## 技术栈

- **语言**：Rust  
- **目标**：学习、安全、可控、性能与资源占用  
- **交付**：单二进制；Android 通过条件编译裁剪

## 与 OpenClaw 的关系

- 理念对齐：本地优先、Gateway 控制面、多通道、多 Agent。
- 实现独立：Rust 重写，不依赖 Node；通过协议与配置兼容与现有生态协作。
- 参考文档：`learn_openclaw/PRD/`、OpenClaw 源码与社区文章、Pi 官方文档与 SDK。

## MVP范围

1. **单渠道消息收发**：支持钉钉或飞书其一，入站 webhook 接收用户消息，出站 API 将回复发回同一会话/群。
2. **最小配置**：单 agent、单 channel、LLM 端点 URL 与 API Key；从文件或环境变量加载，校验必填项。
3. **路由与会话**：内部 session key 格式与 openclaw 兼容；MVP 单 agent 单会话，预留 bindings 扩展。
4. **LLM 适配**：单次 HTTP 调用兼容 OpenAI 的 API；入站文本 → 构造 messages → 取回复文本，无工具、无记忆。
5. **端到端验收**：在钉钉或飞书配置好机器人与 webhook 后，发一条消息能收到一条 LLM 生成的回复（macOS/Linux/Windows 可运行）。

## 关键能力（路线图）
| 能力 | 说明 |
|------|------|
| 消息网关 | 多通道入站/出站（钉钉、飞书、Telegram 等），统一会话与路由 |
| **Agent 运行时（Pi 兼容）** | Rust 原生 Pi 兼容运行时（方案 B）：树形会话、Read/Write/Edit/Bash 工具、可选扩展（Wasm/Deno/dylib），与 OpenClaw 所用 Pi 对齐；MVP 不做 |
| 多 Agent | 每 Agent 独立 Sessions / Workspace / 灵魂文件；bindings 路由 |
| 主动触达 | Cron 定时、Webhook 回调、Heartbeat 定期/立即触发 |
| Agent 协作 | sessions_send、agentToAgent 白名单，主 Agent 派单、专家执行 |
| 记忆与工具 | 记忆检索、沙箱、命令执行（后续） |
| 协议兼容 | Gateway 协议形状、配置子集，与 openclaw 生态互通 |

---

*本 Brief 为 openspec/specs 下的产品级说明；具体变更与 MVP 见 `openspec/changes/rust-tomcatclaw-mvp/`。*
