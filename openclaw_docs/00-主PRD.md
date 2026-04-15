# OpenClaw 产品需求与技术架构主文档

**版本**：1.1  
**基于**：上游 OpenClaw 源码（本机 `openclaw/`）与 [openclaw_docs/README.md](README.md) 中的同步记录  
**说明**：高层需求与架构总览；细节见分文档与官方 [docs.openclaw.ai](https://docs.openclaw.ai)。

---

## 零、先用大白话

OpenClaw 是跑在你电脑或服务器上的 **私人 AI 助理**。它像 **总机 + 翻译**：微信、Telegram、Slack 等渠道来的消息，先汇总到同一个「网关」，再交给配置好的 Agent 思考、查记忆、用工具，最后把回答送回原来的聊天窗口。默认数据在本地 **`~/.openclaw/`**，方便自托管和隐私控制。

---

## ASCII 核心四图

### 1) 结构图

```text
Channels（各 IM / Web / Voice）
        |
        v
┌───────────────────┐
│      Gateway      │  WS / Methods / Cron / Config
└─────────┬─────────┘
          v
   Routing + Sessions（sessionKey / agents）
          v
┌───────────────────┐
│  Agents（Pi 等）   │  Tools / Skills / Memory / Media
└─────────┬─────────┘
          v
    Outbound → 原渠道回复
```

### 2) 调用流图

```text
入站消息
  -> Channel monitor / inbox
      -> 路由 resolve（agentId、sessionKey）
          -> handleCommands 或 getReplyFromConfig
              -> runEmbeddedPiAgent
                  -> deliver-reply / Outbound
```

### 3) 时序图

```text
用户App     Channel        Gateway        Agent        Outbound
  |           |               |             |              |
  | 发消息     | 入站事件       |             |              |
  |---------->|-------------->| 路由/排队   |              |
  |           |               |------------>| 推理+工具    |
  |           |               |             |------------->|
  |           |<-------------|-------------|<-------------|
```

### 4) 数据闭环图

```text
openclaw.json + ~/.openclaw 状态目录
        |
        v
会话 transcript + sessions.json 路由
        |
        v
Memory / compaction / flush 写回工作区 Markdown
        |
        v
下一轮入站仍经 Gateway 聚合
```

---

## 上游近期变更摘要（与文档同步的代码点）

以下与 **本机已拉取的 `openclaw`（`HEAD=3c03d41f13`，npm 版本 `2026.4.14`）** 及上游 `CHANGELOG.md` 对齐，用短句概括「对用户或安全有影响」的方向；**不**逐条罗列数千个提交。

- **Agent / 本地小模型**：可配置 `agents.defaults.localModelMode: "lean"`，少带默认重工具，减轻弱模型的 prompt 压力。Compaction、failover、Ollama 流式超时等与上下文相关的修复持续合并。
- **Gateway / 鉴权**：HTTP、`/v1/*`、升级 WebSocket 等路径会 **按请求解析最新 bearer**，配置或 `secrets.reload` 后旧 token 不会继续生效。`/mcp` 使用常量时间密钥比较并收紧浏览器来源检查。
- **Memory / 安全**：收紧 `memory_get` 可读路径；QMD 与 workspace 文件读取边界加固；dreaming 相关分类与自摄入修复。
- **渠道与附件**：Telegram 文档/电子书等 **二进制不再灌进 prompt**；BlueBubbles 入站 webhook 去重等。
- **SecretRef**：inspect 与 strict 行为在预加载、只读状态与运行时路径上对齐，减少「只读命令却崩」的情况。
- **工程与测试**：大量 CI、QA Matrix、性能与网关测试改进（对架构理解影响小，知道「质量闸门在变紧」即可）。

更细的条目见上游仓库根目录 **`CHANGELOG.md`**；本目录导航见 [README.md](README.md)。

---

## 一、需求

### 1.1 产品定位与目标用户

- **产品定位**：OpenClaw 是一款**本地优先的个人 AI 助手**，用户在自己设备上运行，在已有通讯渠道上统一对话。
- **核心价值**：在 WhatsApp、Telegram、Slack、Discord、Signal、iMessage、Teams、WebChat 等渠道收发消息，由单一控制面（Gateway）统一管理会话、通道与技能；支持多 Agent 路由、语音唤醒、画布与伴侣应用。
- **目标用户**：希望拥有私密、可自托管、多通道、单用户个人助手的用户。

### 1.2 核心功能域（大纲）

- **Gateway 控制面**：会话、通道、工具、事件、配置、Cron、Webhook、Control UI、Canvas 宿主。
- **多通道收件箱**：内置/扩展渠道（WhatsApp、Telegram、Slack、Discord、Signal、iMessage、BlueBubbles、Teams、Matrix、Zalo、WebChat、Voice Call 等）；群组路由、@ 提及门控、DM 配对策略。
- **多 Agent 路由**：按通道/账号/对等体路由到不同 Agent；Workspace + 每 Agent 独立会话。
- **会话模型**：Main 会话、群组会话、激活模式（mention/always）、队列与回复策略。
- **工具与技能**：浏览器、Canvas、Nodes、Cron、Sessions、Discord/Slack 等动作；Agent 间协调（sessions_list、sessions_history、sessions_send、sessions_spawn）；Skills 目录；ClawdHub 技能注册表。
- **记忆（Memory）**：基于 MEMORY.md 与会话转录的向量/混合检索；Embedding（多提供商）；Agent 侧 memory 工具与 session-memory 钩子；CLI `memory status/index/search`；回合后的 memory flush/compaction。
- **媒体与多模态理解**：媒体管道；Media-understanding（图/音/视频描述与转录、多提供商）。
- **人机界面**：CLI（`openclaw` 子命令：onboard、gateway、agent、message、doctor、channels、models、plugins、memory、hooks、cron 等）、TUI、Web Control UI、伴侣应用（macOS 菜单栏、iOS/Android）；渠道内 Chat commands（/status、/new、/compact 等）。
- **安全与合规**：DM 配对策略（pairing/open）、allowlist、执行审批（approvals）、`openclaw doctor` 风险检查。
- **扩展与自动化**：Plugins（`openclaw.plugin.json` 等）；Hooks；Cron；Webhooks/Gmail Pub/Sub；Daemon；Sandbox。
- **外部协议**：ACP（Agent Control Protocol）供外部客户端连接 Gateway。

### 1.3 非功能需求（概要）

- **本地优先**：配置与会话数据本地存储；Gateway 可本地或远程运行。
- **运行环境**：Node ≥22；推荐 pnpm；支持 macOS、Linux、Windows（WSL2 推荐）。
- **模型与鉴权**：多模型配置、OAuth/API Key、按 Agent 的 failover。
- **远程访问**：Tailscale Serve/Funnel 或 SSH 隧道；`openclaw update --channel stable|beta|dev`。

---

## 二、技术架构

### 2.1 技术栈与仓库结构

- **主技术栈**：TypeScript/Node.js（ESM），pnpm monorepo（根包 + `ui` + `extensions/*` + `packages/*` 等）。
- **入口**：npm 全局命令 **`openclaw`** → `openclaw.mjs` → `src/entry.ts` → `src/index.ts`；加载配置、Session Store、Gateway、channel-web、auto-reply、CLI 等。
- **仓库结构概要**：
  - **`src/`**：Gateway、Agents（Pi 嵌入式）、Channels、Sessions、Config、Routing、**`auto-reply/`**、CLI、Control UI、Canvas Host、Media、Plugins、Hooks、ACP、**`memory-host-sdk/`** 等。
  - **`ui/`**：Vite 前端（Control UI）。
  - **`extensions/`**：各渠道扩展 workspace 包，通过 **`openclaw.plugin.json`** 声明。
  - **`skills/`**：技能定义（SKILL.md + 脚本/服务）。
  - **`apps/`**：macOS、iOS、Android 等原生应用。
  - **`scripts/`**：含 `protocol-gen.ts`、`protocol-gen-swift.ts` 等协议生成脚本。

### 2.2 架构分层与数据流（ASCII 主图）

```text
        Channels（各 IM / Web / 扩展）
                    |
                    v
              +-----------+
              |  Gateway  |  HTTP + WS，Methods，配置热更
              +-----+-----+
                    |
                    v
        Routing（sessionKey，选哪个 Agent）
                    |
                    v
              +-----------+
              |  Agents   |  Pi 嵌入式，Tools / Skills
              +-----+-----+
                    |
                    v
        回到 Gateway -> Outbound -> 各渠道把话送回去
```

**这一节你会学到**：消息从哪进、在哪排队、谁说话、话怎么回去。

- **Gateway**：HTTP + WSS；Gateway Methods（chat、sessions、config、agents 等）；配置热更见 `src/gateway/config-reload.ts`。
- **Channels**：ChannelPlugin 契约；出站经 `src/infra/outbound` 的 deliveryMode。
- **Agents**：Pi 嵌入式 `src/agents/pi-embedded.ts`，session lane 串行。
- **Sessions**：`src/config/sessions`、`src/routing`、auto-reply 群组策略等。
- **Memory**：索引与检索主要在 **`extensions/memory-core/`**；宿主/SDK 在 **`src/memory-host-sdk/`**、**`src/plugin-sdk/memory-core.ts`**；Agent 侧 **`src/agents/memory-search.ts`** 与 session-memory hooks。
- **Media**：`src/media/`、`src/media-understanding/`。
- **Plugins / Hooks**：`src/plugins/`、`src/hooks/`；扩展清单 **`openclaw.plugin.json`**。
- **ACP**：`src/acp/`。
- **配置**：类型为 **`OpenClawConfig`**，入口类型分散在 `src/config/types.*.ts` 并由 `src/config/types.ts` 再导出；加载 **`src/config/io.ts`**，默认配置文件名 **`openclaw.json`**，状态目录默认 **`~/.openclaw/`**（详见 `src/config/paths.ts`）。

### 2.3 模块与文档映射（供后续分文档使用）

| 主 PRD 中的模块 | 源码主要位置 | 后续分文档建议 |
| ------------------ | ------------------------------------------------------- | ----------------------------- |
| Gateway | `src/gateway/` | Gateway 控制面、协议、WS、Methods |
| Channels | `src/channels/`, `extensions/*` | 通道插件模型、outbound、配对与安全 |
| Agents | `src/agents/`, `src/commands/` | Pi 嵌入式、工具流、CLI agent |
| Sessions & Routing | `src/config/sessions/`, `src/routing/`, `src/web/auto-reply/` | 会话模型、路由、群组策略 |
| Memory | `extensions/memory-core/`, `src/memory-host-sdk/`, `src/plugin-sdk/memory-core.ts`, `src/agents/memory-search.ts`, `src/hooks/bundled/session-memory/` 等 | 向量/混合检索、索引、embedding |
| Media | `src/media/`, `src/media-understanding/` | 媒体管道、多模态理解 |
| Config | `src/config/` | 配置结构、加载、校验、热更 |
| CLI | `src/cli/` | 子命令与入口 |
| Web / Control UI | `src/control-ui/`, `src/web/`, `ui/` | Control UI、WebChat、auto-reply |
| Skills & Tools | `skills/`, `src/agents/tools/` | 技能契约、工具 |
| Plugins | `src/plugins/`, `src/plugin-sdk/` | 插件加载、slots |
| Hooks | `src/hooks/` | 事件钩子 |
| Cron / Webhooks | `src/cron/` 等 | 定时任务、Webhook |
| Daemon | `src/daemon/` | 服务安装与生命周期 |
| Sandbox & Approvals | `src/agents/sandbox/`, gateway exec 审批相关 | 沙箱、审批 |
| Pairing | `src/pairing/` | DM 配对、allowlist |
| ACP | `src/acp/` | ACP 桥接 |

---

## 三、常见误会

- **误会**：OpenClaw 等于「装了一个聊天机器人网页」。**正解**：它是 **本机优先** 的一整套：网关 + 渠道 + Agent + 状态目录；网页控制台只是其中一块。  
- **误会**：所有「记忆」都在模型里。**正解**：长期笔记多在 workspace 的 Markdown；向量索引是 **派生数据**，可重建。  
- **误会**：改 `openclaw.json` 一定会立刻全局生效。**正解**：多数会热更；若你改了密钥类字段，留意 Gateway 对 token 的重新解析（详见 Gateway 分册）。

---

## 四、附录

### 4.0 历史名称与迁移（读旧资料时对照）

项目对外品牌为 **OpenClaw**；CLI 与 npm 包名为 **`openclaw`**。旧状态目录 **`~/.clawdbot`**、旧主配置 **`clawdbot.json`** 仍可能被 `paths.ts` 中的兼容逻辑识别。遇到告警或迁移问题，优先运行 **`openclaw doctor`**（参见上游 `docs/gateway/doctor.md`）。

### 4.1 术语表

| 术语 | 简要说明 |
| ----- | ----- |
| Gateway | 控制平面：WebSocket + JSON-RPC 风格方法，管理会话、通道、配置、Cron、事件等。 |
| Session | 对话会话，由 session key 标识。 |
| Agent | 配置的 AI 助手实例，绑定 workspace。 |
| Channel | 通讯渠道，以 ChannelPlugin 注册。 |
| Skill | 由 SKILL.md 描述的能力单元。 |
| Memory | 基于 MEMORY.md 与转录的检索与索引。 |
| Hook | 按事件触发的扩展点。 |
| ACP | Agent Control Protocol。 |
| Presence | 客户端在线状态。 |
| Chat command | 渠道内命令，如 /new、/compact。 |
| ClawdHub | 技能注册表。 |

### 4.2 参考

- 项目 README：`openclaw/README.md`
- 官方文档：<https://docs.openclaw.ai>
- 本目录索引：[README.md](README.md)

### 4.3 相关文档

- [01-技术设计总览](01-技术设计总览.md)
- [02-Gateway](02-Gateway.md)（含 [协议与Schema](02-Gateway/协议与Schema.md)、[WebSocket与连接](02-Gateway/WebSocket与连接.md)、[Methods与RPC](02-Gateway/Methods与RPC.md)、[配置热更与侧车](02-Gateway/配置热更与侧车.md)）
- [03-Channels](03-Channels.md)（含 [插件模型与适配器](03-Channels/插件模型与适配器.md)、[Outbound与配对](03-Channels/Outbound与配对.md)）
- [04-Agents](04-Agents.md)（含 [Pi嵌入式运行时](04-Agents/Pi嵌入式运行时.md)、[工具流与订阅](04-Agents/工具流与订阅.md)、[技能与Pi适配](04-Agents/技能与Pi适配.md)）
- [05-Sessions与Routing](05-Sessions与Routing.md)
- [06-Memory](06-Memory.md)（含 [索引与检索](06-Memory/索引与检索.md)、[Embedding与同步](06-Memory/Embedding与同步.md)）
- [07-Media与Media-understanding](07-Media与Media-understanding.md)
- [08-Config](08-Config.md)
- [09-CLI](09-CLI.md)
- [10-Web与Control-UI](10-Web与Control-UI.md)
- [11-Skills与Tools](11-Skills与Tools.md)
- [12-Plugins](12-Plugins.md)
- [13-Hooks](13-Hooks.md)
- [14-Cron与Webhooks](14-Cron与Webhooks.md)
- [15-Daemon](15-Daemon.md)
- [16-Sandbox与Approvals](16-Sandbox与Approvals.md)
- [17-Pairing](17-Pairing.md)
- [18-ACP](18-ACP.md)
- [19-目录结构详解](19-目录结构详解.md)
