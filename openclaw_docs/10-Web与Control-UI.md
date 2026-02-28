# Web 与 Control UI

**设计思想**：Web 侧负责 auto-reply、inbound、monitor、login、media；Control UI 为 Vite 构建的 SPA，由 Gateway 静态服务，与 Gateway WS 对接实现 WebChat 与 deliver-reply。

---

## 一、Web 侧

- **auto-reply**：`openclaw/src/web/auto-reply/`（或 auto-reply 根）入站消息处理、monitor、group-activation。
- **inbound**：入站消息解析与上下文构建。
- **media**：`openclaw/src/web/media.ts` 的 `loadWebMedia`，处理 file:// 与远程 URL、HEIC 转 JPEG。
- **login**：登录与认证。

---

## 二、Control UI

- **control-ui.ts**：`openclaw/src/gateway/control-ui.ts`，解析 control-ui 根目录（packaged/dist/cwd），按扩展名返回 MIME，服务 index.html、JS、CSS。
- **control-ui-assets.ts**：`openclaw/src/infra/control-ui-assets.ts`，资产路径与打包。
- **ui/**：Vite 应用源码，构建后输出到 dist/control-ui。

---

## 三、与 Gateway 的对接

- WebChat 通过 Gateway WebSocket 连接，收发消息。
- deliver-reply 将 Agent 回复投递到目标通道。
