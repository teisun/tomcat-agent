# Gateway 控制面

**版本**：见 [README.md](README.md) 上游同步表（路径均相对 **`openclaw` 仓库根**）。

## 零、先用大白话

Gateway 像 **机场塔台**。  
塔台 **不替飞机装货**：它不替你做「模型推理」或「写业务代码」。  
它管三件事：**谁连着**、**指令往哪派**、**天气（配置）变了怎么广播**。  
CLI、网页控制台、菜单栏 App、Node 设备，都用 **同一条 WebSocket** 跟塔台说话。  
真正「开飞机」的是 **Agent** 和 **各渠道模块**。

**这一节你会学到**：塔台启动大概走哪几步；该去 `src/gateway/` 里翻哪些文件。

---

## ASCII 核心四图

### 1) 结构图

```text
HTTP + WebSocket（默认常与本机 18789 端口一起出现，见官方文档）
        |
        v
+-------------------------------+
|           Gateway             |
| Methods / Sessions / Cron     |
| config-reload + sidecars      |
+---------------+---------------+
                v
    Channels / Agents / Infra（各干各的活）
```

### 2) 调用流图

```text
客户端发 JSON：{ method, params, id }
  -> WebSocket 进 message-handler
      -> coreGatewayHandlers 查表分发
          -> sessions.* / chat.* / config.* / cron.* …
              -> 回 { id, result } 或 { id, error }
```

### 3) 时序图

```text
CLI/UI       Gateway WS        Handler         Session store
  |              |                |                |
  | connect      |                |                |
  |------------->|                |                |
  | chat.send    |                |                |
  |------------->|--------------->| 读写会话      |
  |              |<---------------|<---------------|
```

### 4) 数据闭环图

```text
磁盘上的 openclaw.json 变了
        |
        v
startGatewayConfigReloader 算出 ReloadPlan
        |
        v
新 token / 插件开关 等被后续请求读到
        |
        v
健康快照、presence 版本更新 -> 客户端感知并重连或增量刷新
```

---

## 一、塔台到底管什么（职责）

- **WebSocket**：长连接；JSON 请求 + JSON 响应，外加服务端推送事件。  
- **Gateway Methods**：像塔台指令表；实现里常见 `listGatewayMethods`、`coreGatewayHandlers`。  
- **配置热更**：监听配置文件，尽量 **少重启** 整机（见 [配置热更与侧车](02-Gateway/配置热更与侧车.md)）。  
- **侧车**：浏览器控制、Tailscale 暴露、插件服务等，跟着 Gateway 生命周期走（实现入口见下文）。  
- **Cron / 健康 / Presence**：定时任务、自检缓存、谁在线。

---

## 二、入口与启动（工程师锚点）

**主入口**：[`src/gateway/server.impl.ts`](../openclaw/src/gateway/server.impl.ts) 里的 `startGatewayServer`（默认端口参数里常见 **18789**，最终以你配置与环境变量为准）。

**启动故事线（简化）**：

```text
startGatewayServer
  -> 读配置快照 / 鉴权引导
  -> loadGatewayPlugins
  -> createGatewayRuntimeState（HTTP + WSS）
  -> createChannelManager
  -> 挂上 WS handler、Cron、审批等
  -> startGatewayConfigReloader
  -> startGatewayPostAttachRuntime（Tailscale、侧车、channels…）
```

更细的「HTTP 与 WS 怎么接」见 [`src/gateway/server-runtime-state.ts`](../openclaw/src/gateway/server-runtime-state.ts)。  
WS 单条连接怎么处理，见 [`src/gateway/server/ws-connection.ts`](../openclaw/src/gateway/server/ws-connection.ts)。

**同一台机器上的 HTTP 小网页（Canvas / A2UI）**：官方架构文里写在与 Gateway **同一端口** 下的路径前缀（如 `/__openclaw__/canvas/`、`/__openclaw__/a2ui/`），便于本地调试；细节以 [`openclaw/docs/concepts/architecture.md`](../openclaw/docs/concepts/architecture.md) 为准。

---

## 三、事件与方法（去哪查表）

- **事件名列表**：`src/gateway/server-methods-list.ts`（如 `GATEWAY_EVENTS` 一类常量，文件名以仓库为准）。  
- **方法清单**：`listGatewayMethods` 返回的名字，和 `coreGatewayHandlers` 能对上号。  
- **优雅退出**：`GatewayServer` 上有 `close()` 一类收口。

---

## 四、和邻居怎么配合

| 邻居 | 人话 | 代码上常见接触点 |
|------|------|------------------|
| Config | 读、热更 `openclaw.json` | `src/config/io.ts`、`src/gateway/config-reload.ts` |
| Channels | 各 IM 真正连 provider | `createChannelManager` 一带 |
| Agents | 回复用户 | chat 相关 method → Pi 嵌入式路径 |
| Nodes | 手机/桌面「节点」能力 | `node.*` methods |
| Plugins | 扩展挂进 Gateway | `loadGatewayPlugins`、插件自带 `gatewayMethods` |

---

## 五、子文档（拆开读更轻松）

- [协议与Schema](02-Gateway/协议与Schema.md)  
- [WebSocket与连接](02-Gateway/WebSocket与连接.md)  
- [Methods与RPC](02-Gateway/Methods与RPC.md)  
- [配置热更与侧车](02-Gateway/配置热更与侧车.md)  

---

## 常见误会

- **误会**：Gateway 里跑大模型。**正解**：大模型在 **Agent / 提供商** 一侧；Gateway 是 **控制面**。  
- **误会**：改完 `openclaw.json` 不用重连 WS。**正解**：鉴权、token 等可能 **立刻按新配置解析**；旧连接有时会掉，客户端应能自动重连。  
- **误会**：所有渠道都各开一个 WhatsApp。**正解**：架构上 **一台主机一个 Gateway** 持有 provider 会话；多客户端连的是塔台，不是各开各的会话（见上游 `docs/concepts/architecture.md`）。
