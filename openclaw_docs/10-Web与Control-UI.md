# Web 与 Control UI

## 零、先用大白话

**Control UI** 像 **给塔台用的网页遥控器**：点点配置、看看谁在线。  
**auto-reply** 像 **分拣车间**：各渠道来的包裹先过安检、贴标签，再决定要不要交给 Agent。

**这一节你会学到**：前端代码在 `ui/`；谁托管静态文件；和 WS 怎么说话。

---

**设计思想**：自动回复与 Web 相关能力主要落在 **`src/auto-reply/`**、**`src/media/`**、**`src/gateway/control-ui.ts`** 等；Control UI 前端源码在 **`ui/`**，构建产物由 Gateway 静态托管并与 WS 对接。

---

## ASCII 核心四图

### 1) 结构图

```text
浏览器 -> Control UI（ui/ 构建产物）
        |
        v
Gateway HTTP 静态 + WebSocket
        |
        v
auto-reply / media / sessions（同源进程）
```

### 2) 调用流图

```text
加载 index.html
  -> 前端 WS connect
      -> Methods（chat/config/...）
          -> 入站 web channel -> auto-reply 管线
              -> 回显 UI
```

### 3) 时序图

```text
Browser UI    Gateway WS        auto-reply        Agent
     |             |                 |              |
     | chat.send   |                 |              |
     |------------>| route           |              |
     |             |---------------->| getReply     |
     |             |                 |------------->|
     |<------------| events          |<-------------|
```

### 4) 数据闭环图

```text
UI 操作修改会话/配置
        |
        v
经 Gateway Methods 落盘 ~/.openclaw/
        |
        v
刷新页面仍读同一状态目录
        |
        v
与 CLI doctor 交叉验证
```

---

## 一、自动回复与媒体

- **auto-reply**：**`src/auto-reply/`** — 入站回复编排、`getReplyFromConfig`、命令注册、memory-flush 等（不要把旧路径 `src/web/auto-reply/` 当作当前主位置）。
- **inbound / monitor**：随渠道而异；Web 类渠道的 monitor 入口常经 **`src/plugins/runtime/runtime-web-channel-plugin.ts`** 懒加载重模块再进入 auto-reply。
- **media**：**`src/media/web-media.ts`** 的 **`loadWebMedia`**（file:// / URL、HEIC 等处理）；与 **`src/media/`** 管道协同。
- **login**：各 provider 登录流（实现分散在 `src/` 对应 provider 与 CLI 中）。

---

## 二、Control UI

- **control-ui.ts**：**`src/gateway/control-ui.ts`** — 解析 Control UI 静态根目录（packaged / dist / cwd），返回 `index.html`、JS、CSS 与 MIME。
- **control-ui-assets.ts**：**`src/infra/control-ui-assets.ts`** — 资产路径与打包约定。
- **`ui/`**：Vite 应用源码，构建输出进入发布目录供 Gateway 托管。

---

## 三、与 Gateway 的对接

- WebChat 通过 Gateway WebSocket 连接，收发消息。
- 出站投递复用 **`src/infra/outbound/`** 与各 channel 的 send 路径。

---

## 延伸阅读

- [02-Gateway.md](02-Gateway.md)  
- [01-技术设计总览.md](01-技术设计总览.md)

---

## 常见误会

- **误会**：Control UI 等于 WebChat 的全部。**正解**：WebChat 是 **一种** 用 WS 的界面；Control UI 偏 **运维/配置**。  
- **误会**：`src/web/auto-reply` 仍是主入口。**正解**：主线在 **`src/auto-reply/`**（旧路径别背）。  
- **误会**：浏览器能直连 WhatsApp。**正解**：浏览器连 **Gateway**；IM 仍由 **渠道插件** 在服务端连。
