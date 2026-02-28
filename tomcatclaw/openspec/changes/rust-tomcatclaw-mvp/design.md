# Design: tomcatclaw 架构与阶段划分

## 1. 总体架构（目标状态）

```
┌─────────────────────────────────────────────────────────────────────────┐
│  事件来源                                                                 │
│  用户消息(Channel) / Cron / Webhook / Heartbeat                          │
└────────────────────────────────┬────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  系统事件队列（按 session/agent 串行，避免并发冲突）                        │
└────────────────────────────────┬────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  路由：channel + peer → session key → agentId                            │
│  （bindings 配置；MVP 单 agent 单会话）                                    │
└────────────────────────────────┬────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  Agent 执行（远期：Pi 兼容运行时）                                         │
│  加载 Workspace/配置 → LLM 调用（+ 树形会话/工具/记忆/sessions_send）        │
└────────────────────────────────┬────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  Outbound 统一投递 → 各 Channel 实现 → 用户所见渠道                        │
└─────────────────────────────────────────────────────────────────────────┘
```

- **MVP**：仅实现「用户消息 → 路由 → 单 Agent → LLM → Outbound」路径；队列与路由做最小实现，便于后续接入 Cron/心跳/Webhook。

## 2. 模块边界（与 OpenClaw 对照）

| 模块 | 职责 | MVP | 后续 |
|------|------|-----|------|
| **Config** | 加载/校验配置（单文件，兼容 openclaw 子集） | 单 agent、单 channel、LLM 端点 | bindings、多 agent、cron、webhook、channel 策略 |
| **Gateway** | 控制面（WS + Methods）、健康、侧车 | 可不实现或仅 HTTP 回调入口 | 完整 WS + JSON-RPC、config-reload |
| **Channels** | 入站/出站、配对、群级策略 | 钉钉或飞书其一：入站 webhook + 出站 API | 多渠道、requireMention、DM 策略 |
| **Routing** | channel + peer → sessionKey, agentId | 写死或单条 bindings | 完整 bindings 匹配 |
| **Agents** | 执行（LLM、工具、sessions_send） | 单次 LLM 调用、无工具 | Pi 兼容运行时（树形会话、工具、扩展）、Workspace、SOUL、sessions_send、agentToAgent |
| **Outbound** | 统一发送接口 | 单 channel 直接发 | 抽象层 + 多 channel 注册 |
| **Cron** | 定时任务、jobs 存储、触发 | 不实现 | CronService、jobs.json、enqueueSystemEvent |
| **Webhook** | 任务完成回调、SSRF 防护 | 不实现 | 配置 + POST + 安全校验 |
| **Heartbeat** | 定期/立即触发 | 不实现 | 间隔 + requestHeartbeatNow |
| **Memory** | 向量/检索/持久化 | 不实现 | 可选 sqlite-vec、embedding、MEMORY.md |
| **Sandbox/Exec** | 安全执行、命令执行 | 不实现 | 白名单、审批、沙箱 |

## 3. 技术选型（Rust）

| 关注点 | 建议 | 说明 |
|--------|------|------|
| 异步运行时 | tokio | 通用选择，生态好 |
| HTTP 服务/客户端 | axum + reqwest 或 tower | 轻量、可控依赖 |
| TLS | rustls | 避免 OpenSSL，利于体积 |
| 配置 | 单 JSON/TOML，serde | 与 openclaw 配置子集兼容 |
| 条件编译 | `#[cfg(target_os = "android")]` 等 | 裁剪 CLI、daemon、部分 channel |
| 日志 | tracing | 可按 level 裁剪 |

## 4. 目录与包结构（建议）

```
tomcatclaw/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── main.rs              # 桌面/CLI 入口；Android 可另设
│   ├── config/              # 配置加载、校验、兼容 openclaw 子集
│   ├── gateway/             # （可选 MVP）HTTP 或 WS 入口
│   ├── channels/            # 渠道抽象 + 钉钉/飞书实现
│   ├── routing/             # session key、bindings 解析
│   ├── agents/              # Agent 执行、LLM 适配；Phase 2.5 增加 Pi 兼容运行时（树形会话、工具、扩展）
│   ├── outbound/            # 统一投递接口与实现
│   ├── events/              # 事件队列（MVP 最小，后续接 Cron/心跳）
│   ├── cron/                # （后续）定时任务
│   ├── webhook/             # （后续）任务完成回调
│   └── heartbeat/           # （后续）心跳触发
└── ...
```

- MVP 可在 `channels/` 下只实现一个模块（如 `feishu` 或 `dingtalk`），`events/` 仅「入站消息 → 执行」单路径。

## 5. 协议与兼容策略

- **配置**：支持读取 openclaw 风格配置子集（如一个 `agents` 条目、一个 `channels` 条目、`models`/auth），路径可约定为 `~/.openclaw/` 或 `~/.tomcatclaw/`。
- **协议**：MVP 可不暴露完整 Gateway WS；若需与现有 Control UI 互通，预留「Gateway Methods 子集」的请求/响应形状（如 chat、talk、sessions 相关），后续补 WS 层。
- **会话**：session key 格式与 openclaw 对齐（如 `agentId:channel:peerKind:peerId`），便于后续多 Agent、多群。

## 6. Android 与条件编译

- `#[cfg(not(target_os = "android"))]`：CLI、daemon、桌面专用 channel、复杂文件系统等。
- `#[cfg(target_os = "android")]`：仅保留「消息网关 + 会话 + LLM 适配 + 最小协议 + 可选记忆」；沙箱/命令执行可为「不做」或「白名单」。
- 共用 core：协议、会话模型、LLM 适配、内部消息类型统一在 core，Android 与桌面共享。

## 7. 阶段小结

| 阶段 | 内容 |
|------|------|
| **MVP（本变更）** | 钉钉或飞书一条消息入站 → 路由 → 单 Agent（仅 LLM HTTP 调用）→ 出站；配置与协议可扩展；**不含 Pi**；体积与 Android 为后续优化 |
| **Phase 2** | 多 Agent、bindings、Workspace/SOUL、Outbound 抽象、多渠道、身份展示 |
| **Phase 2.5 — Agent 运行时（Pi 兼容，方案 B）** | Rust 原生实现 Pi 兼容的 Agent 运行时：树形会话（分支/回溯/JSONL 持久化）、四大工具 Read/Write/Edit/Bash 或子集、多 LLM 封装、可选扩展（Wasm/Deno/dylib）、与 OpenClaw 所用 Pi 协议/行为对齐；不嵌入 Node 版 Pi |
| **Phase 3** | Cron、Webhook、Heartbeat、事件队列完整、sessions_send、agentToAgent |
| **Phase 4** | 记忆、沙箱/命令执行、Gateway 完整、体积与 Android 裁剪达标 |
