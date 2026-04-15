# OpenClaw 中文导读索引（`openclaw_docs/`）

## 零、先用大白话

**这里不是官方说明书。**  
本目录放在 Tomcat 仓库里，给 **新同事、审阅者、未来的你** 用：**几分钟知道 OpenClaw 在干什么、代码大概在哪。**  
**真正教用户怎么装、怎么配**，请看官方站：[https://docs.openclaw.ai](https://docs.openclaw.ai)。上游源码仓库：[openclaw/openclaw](https://github.com/openclaw/openclaw)。

---

## ASCII 核心四图

### 1) 结构图

```text
你（读者）
    |
    v
openclaw_docs/（导读，中文、短句）
    |
    +----------------------> openclaw/（TypeScript 源码，事实来源）
    |
    v
~/.openclaw/（本机状态：配置、会话、缓存，像「助理的抽屉」）
```

### 2) 调用流图

```text
打开 00 或 01
  -> 建立「地图」
      -> 按主题打开 02～19
          -> 对照 openclaw/ 里的路径
              -> 和下面「同步表」里的版本对一下
                  -> 对不上就改导读（上游 PR 另说）
```

### 3) 时序图

```text
读者        本目录              上游 openclaw
  |            |                      |
  | 读某一章    |                      |
  |----------->|                      |
  |            | 给出 src/... 路径     |
  |----------------------------------->|
  |            |                      | 打开 .ts 核对
  |<-----------------------------------|
```

### 4) 数据闭环图

```text
上游有新提交
      |
      v
更新本 README 的「同步记录」表
      |
      v
你本地 pull openclaw、跑 CLI
      |
      v
~/.openclaw/ 里文件变掉
      |
      v
回到对应分册改「路径 / 行为」说明
```

---

## 上游同步记录（对照代码用）

| 项 | 值 |
|----|-----|
| 记录日期 | 2026-04-15 |
| 本机 `openclaw` `HEAD` | `3c03d41f135fcc9910a88794af1b3761ede39461` |
| `openclaw/package.json` 版本 | `2026.4.14` |
| 变更清单从哪读 | 仓库根 [`openclaw/CHANGELOG.md`](../openclaw/CHANGELOG.md)；**产品向短摘要**只在 [00-主PRD.md](00-主PRD.md) 第二节 |

**曾用名**：以前叫过 Clawdbot 等。老路径 `~/.clawdbot`、老文件名 `clawdbot.json` 有时还能被认出来。心里没底就运行 **`openclaw doctor`**。

---

## 推荐阅读顺序

1. [00-主PRD.md](00-主PRD.md) — 产品做什么、大块拼图  
2. [01-技术设计总览.md](01-技术设计总览.md) — 设计想法、主数据流  
3. [19-目录结构详解.md](19-目录结构详解.md) — 仓库里、磁盘上各有什么  
4. 按需：Gateway → Channels → Sessions → Agents → Memory → …

---

## 分册索引（一句话）

| 文档 | 一句话 |
|------|--------|
| [00-主PRD.md](00-主PRD.md) | 需求与架构总览 |
| [01-技术设计总览.md](01-技术设计总览.md) | 原则、数据流、模块边界 |
| [02-Gateway.md](02-Gateway.md) | 塔台：WS、Methods、热更、侧车 |
| [02-Gateway/协议与Schema.md](02-Gateway/协议与Schema.md) | 对讲机口令（协议帧与 schema） |
| [02-Gateway/WebSocket与连接.md](02-Gateway/WebSocket与连接.md) | 谁怎么连上塔台 |
| [02-Gateway/Methods与RPC.md](02-Gateway/Methods与RPC.md) | 塔台能办哪些事 |
| [02-Gateway/配置热更与侧车.md](02-Gateway/配置热更与侧车.md) | 改配置不关机、侧车是啥 |
| [03-Channels.md](03-Channels.md) | 各聊天入口怎么接进来 |
| [03-Channels/插件模型与适配器.md](03-Channels/插件模型与适配器.md) | 渠道插件契约 |
| [03-Channels/Outbound与配对.md](03-Channels/Outbound与配对.md) | 回信怎么走、陌生人怎么拦 |
| [04-Agents.md](04-Agents.md) | 大脑：Pi 嵌入式等 |
| [04-Agents/Pi嵌入式运行时.md](04-Agents/Pi嵌入式运行时.md) | Pi 跑在进程里那段 |
| [04-Agents/工具流与订阅.md](04-Agents/工具流与订阅.md) | 工具、事件订阅 |
| [04-Agents/技能与Pi适配.md](04-Agents/技能与Pi适配.md) | Skills 和 Pi 怎么配合 |
| [05-Sessions与Routing.md](05-Sessions与Routing.md) | 会话键、路由到哪个 Agent |
| [06-Memory.md](06-Memory.md) | 便利贴墙 + 档案柜（记忆） |
| [06-Memory/索引与检索.md](06-Memory/索引与检索.md) | 怎么翻得快 |
| [06-Memory/Embedding与同步.md](06-Memory/Embedding与同步.md) | 向量、同步 |
| [07-Media与Media-understanding.md](07-Media与Media-understanding.md) | 图音视频怎么进模型 |
| [08-Config.md](08-Config.md) | `openclaw.json` 从哪来、类型是啥 |
| [09-CLI.md](09-CLI.md) | 命令行入口 |
| [10-Web与Control-UI.md](10-Web与Control-UI.md) | 网页控制台、WebChat |
| [11-Skills与Tools.md](11-Skills与Tools.md) | 技能包与工具面 |
| [12-Plugins.md](12-Plugins.md) | 插件怎么挂进来 |
| [13-Hooks.md](13-Hooks.md) | 事件钩子 |
| [14-Cron与Webhooks.md](14-Cron与Webhooks.md) | 定时与 Webhook |
| [15-Daemon.md](15-Daemon.md) | 常驻服务、安装 |
| [16-Sandbox与Approvals.md](16-Sandbox与Approvals.md) | 沙箱、危险操作要批准 |
| [17-Pairing.md](17-Pairing.md) | 私聊配对、白名单 |
| [18-ACP.md](18-ACP.md) | ACP 桥到 Gateway |
| [19-目录结构详解.md](19-目录结构详解.md) | 目录与状态路径详解 |

---

## 每篇导读怎么写（本目录约定）

1. **零、先用大白话**：一两句，只回答「这是啥、干啥用」。  
2. **ASCII 核心四图**：结构、调用流、时序、数据闭环（` ```text ` 框）。  
3. **正文**：先故事线（谁把消息交给谁），再给 **`openclaw/` 相对路径** + 每个路径一句话。  
4. **常见误会**：2～4 条，专治想当然。

更通用的写作结构可参考：[pi-rust-wasm/openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md](../pi-rust-wasm/openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md)。源码路径一律相对 **`openclaw` 仓库根**（例：`src/gateway/server.impl.ts`）。

---

## 常见误会

- **误会**：读完导读就不用看官方文档了。**正解**：导读是地图；操作步骤以 [docs.openclaw.ai](https://docs.openclaw.ai) 为准。  
- **误会**：Gateway 就是「大模型」。**正解**：Gateway 像塔台，**不**负责推理；推理在 Agent / 模型那一侧。  
- **误会**：`openclaw_docs` 里的路径永远和上游一致。**正解**：以本 README **同步表**里的 `HEAD` 为准；上游大改后要重对路径。

---

## 维护提示（给改文档的人）

对照 `HEAD` 做 **抽样** 路径检查即可：`src/gateway/`、`src/config/`、`src/cli/` 等。环境变量常用 `OPENCLAW_STATE_DIR`、`OPENCLAW_CONFIG_PATH`、`OPENCLAW_GATEWAY_PORT`（默认值以官方文档为准，默认网关端口常见为 **18789**）。
