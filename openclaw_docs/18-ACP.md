# ACP

## 零、先用大白话

ACP 像 **标准耳机孔**。  
外部 App 不说官方 CLI 的黑话，只要会说 **ACP 这套协议**，也能接到 **同一个 Gateway**。  
适合「我要自己写控制台 / 自动化」的人。

**这一节你会学到**：ACP 进程和 Gateway 进程谁连谁；入口文件在哪。

---

**设计思想**：ACP（Agent Control Protocol）提供独立的协议服务器与客户端，与 GatewayClient 连接并转发事件，支持多节点或远程控制场景。

---

## ASCII 核心四图

### 1) 结构图

```text
ACP server（独立协议面）
        |
        v
GatewayClient -> 本机 Gateway WS
        |
        v
外部客户端（非官方 CLI）
```

### 2) 调用流图

```text
外部客户端连 ACP
  -> translator 映射为 Gateway 语义
      -> 复用 sessions/chat 能力
          -> 事件回流 ACP 客户端
```

### 3) 时序图

```text
Remote app    ACP server       GatewayClient      Gateway
     |             |                 |              |
     | connect     |                 |              |
     |------------>| attach          |              |
     |             |---------------->| WS call     |
     |             |                 |------------->|
```

### 4) 数据闭环图

```text
多节点控制同一 ~/.openclaw 状态
        |
        v
ACP 会话内存 + Gateway 磁盘
        |
        v
断开重连同步 cursor/订阅
        |
        v
协议版本对齐（随 Gateway 升级）
```

---

## 一、职责（工程师锚点）

- **server**：`src/acp/server.ts` 的 **`serveAcpGateway`**（先起 Gateway WS，再对外接 ACP）。  
- **client**：`src/acp/client.ts` 一带，`GatewayClient` 连本机塔台。  
- **translator / event-mapper**：把两边消息形状对齐。  
- **session**：内存会话表，和磁盘上的 OpenClaw 会话不是同一个概念，别混。

---

## 二、与 Gateway 的衔接

- ACP 进程内部先 **`GatewayClient.start()`**，等 **hello OK** 再接单。  
- Gateway 来的事件经 mapper **回流** 到 ACP 客户端。  
- 适合 **远程控制、自动化、非官方 UI**；协议版本要跟着 Gateway 升。

---

## 常见误会

- **误会**：ACP 可以绕过 Gateway 鉴权。**正解**：仍然走 **GatewayClient + token**；别当成后门。  
- **误会**：ACP 和 WebSocket Methods 完全同一套 JSON。**正解**：外层协议不同；中间有 **翻译**。  
- **误会**：断线后状态全自动对齐。**正解**：要看 **重连、cursor、订阅** 实现；读 `server.ts` 里 onClose 逻辑。
