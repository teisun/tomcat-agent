# Proposal: tomcatclaw — 跨平台轻量 Rust AI Agent

## 变更名称

`rust-tomcatclaw-mvp`

## 一句话目标

用 Rust 实现跨全平台（含商用 Android）、小体积低内存、功能对标 OpenClaw 的轻量 AI Agent（tomcatclaw）；MVP 先跑通「一条消息」闭环（钉钉或飞书），架构预留多 Agent、Cron、心跳、Webhook 与 openclaw 协议兼容。

## 背景与动机

- **产品定位**：本地优先、多通道、单控制面的个人 AI 助手（与 OpenClaw/Clawdbot 理念一致），需支持 Windows / macOS / Linux / 商用 Android。
- **选 Rust 的原因**：学习、安全、可控、性能与资源占用；目标无容器、无 chroot、无 ROOT，编译后尽量 <10MB，可条件编译裁剪到 Android。
- **兼容诉求**：与现有 openclaw 生态兼容（协议与配置子集优先），先做最有必要的功能（MVP），再逐步对齐完整能力。
- **Agent 核心与路线图**：OpenClaw 以 **Pi** 为 Agent 核心（树形会话、四大工具、有状态扩展、多 LLM）。tomcatclaw 长期采用 **方案 B**：用 Rust 实现 **Pi 兼容的 Agent 运行时**（树形会话、工具、可选扩展），不嵌入 Node 版 Pi；**MVP 不包含 Pi**，仅单次 LLM HTTP 调用以先跑通一条消息。

## 目标能力清单

### 平台与约束

| 项 | 说明 |
|----|------|
| 平台 | Windows / macOS / Linux / 商用 Android |
| 体积与运行 | 小体积、低内存；无容器、无 chroot、无 ROOT；编译后目标 <10MB |
| 裁剪 | 可条件编译裁剪到商用 Android（仅保留必要模块） |

### 功能域（对标 OpenClaw）

1. **消息闭环**：入站 → 路由 → 会话隔离 → LLM 适配 → 出站；MVP 支持钉钉或飞书一条消息跑通。
2. **协议与网关**：内部统一会话/文本模型；预留 Gateway 协议形状（WS + JSON-RPC 子集）与配置子集兼容。
3. **多 Agent 隔离**：每 Agent 独立 Sessions、AgentDir、Workspace；通过 bindings（channel + peer）路由到不同 Agent；群级策略（如 requireMention）后续支持。
4. **Agent 间协作**：sessions_send、agentToAgent 白名单（主 Agent 派单、专家执行），后续实现。
5. **Workspace 与灵魂**：每 Agent 独立 workspace（SOUL.md、PROMPT.md、USER.md、memory/ 等）；MVP 可单配置/单 prompt。
6. **身份展示**：Per-Agent 的 name、emoji（及后续头像），在渠道侧展示。
7. **Agent 运行时（Pi 兼容）**：Rust 原生实现 Pi 兼容的 Agent 运行时（方案 B）—— 树形会话管理、四大工具 Read/Write/Edit/Bash（或子集）、可选扩展（Wasm/Deno/dylib）、多 LLM 封装、与 OpenClaw 所用 Pi 协议/行为对齐；路线图独立阶段，MVP 不做。
8. **沙箱安全与命令执行**：沙箱 + 命令执行能力，后续实现。
9. **记忆**：向量/混合检索与持久化，后续实现。
10. **Agent 主动联系用户**：
   - **Cron**：定时任务（jobs 存储、触发执行、系统事件队列）。
   - **Webhook**：任务完成时 POST 通知（URL + token），SSRF 防护。
   - **Heartbeat**：定期触发 + 立即触发（requestHeartbeatNow），用于邮件检查、状态监控等。
   - **统一 Outbound**：多渠道消息投递（DM + 群组、可配置提及策略）。
11. **事件驱动**：定时/Webhook/心跳 → 系统事件队列 → Agent 执行 → Outbound → 用户渠道。

## MVP 范围（本变更聚焦）

- **交付**：一个 Rust 二进制，在 macOS/Linux/Windows 上可从钉钉或飞书**收一条消息并回一条**（经 LLM）。
- **包含**：最小消息网关（单渠道入站/出站）、协议与配置可扩展、会话隔离（单会话或单 Agent）、LLM 适配（HTTP 兼容 OpenAI API）。
- **不包含**：Pi / Pi 兼容运行时、Cron、Webhook、Heartbeat、多 Agent、sessions_send、记忆、沙箱/命令执行；仅预留架构与配置形状。

## 非目标（本变更不做）

- 完整实现所有渠道、所有 OpenClaw Methods、Control UI、Node 扩展/技能运行时、ClawdHub。
- 首版即达成 <10MB（先跑通再优化体积）；Android 首版可仅验证构建与裁剪思路，不要求首版上架。

## 成功标准

- MVP：钉钉或飞书任选其一，端到端一条消息入站 → Agent → 一条回复出站，可配置 LLM 端点与 API Key。
- 代码结构支持后续按阶段加入：Pi 兼容 Agent 运行时、多 Agent、bindings、Cron、Heartbeat、Webhook、Outbound 抽象、记忆、沙箱。

## 参考

- 项目 PRD：`learn_openclaw/PRD/00-主PRD.md`、`01-技术设计总览.md`
- OpenClaw 多 Agent 实操（飞书 bindings、sessions_send、agentToAgent）：微信文章《OpenClaw多Agent实操：一个人指挥一支AI军队》
- OpenClaw 主动联系用户机制：Cron（server-cron.ts）、Webhook、Heartbeat、Outbound（delivery.ts）
- OpenClaw Agent 核心：Pi 嵌入式运行时（pi-embedded.ts、Pi SDK 模式）；Pi 能力见官方文档（树形会话、四大工具、有状态扩展、多运行模式）
