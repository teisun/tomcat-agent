# Sessions 与 Routing

**版本**：见 [README.md](README.md) 同步表；路径相对 **`openclaw/` 仓库根**。

## 零、先用大白话

**Session** 像 **一段聊天的档案袋**。袋子上有个编号（`sessionKey` / `sessionId`），里面一页页是聊天记录（`jsonl`）。  
**Routing** 像 **分拣机读邮编**：这条消息从哪个 App、哪个群、谁发的？读完决定：**塞进哪个 Agent 的哪只档案袋**。

**这一节你会学到**：`sessionKey` 长啥样；`resolveAgentRoute` 从哪进；会话文件在磁盘哪。

---

## ASCII 核心四图

### 1) 结构图

```text
入站 envelope（channel + account + peer/group …）
        |
        v
   resolveAgentRoute（分拣）
        |
        v
sessionKey -----> sessions.json 里的 SessionEntry
        |
        v
<sessionId>.jsonl（一页页对话）
```

### 2) 调用流图（和 Channels、Agent 串起来）

```text
Channel 把消息交进来
  -> 归一成「路由键」
      -> resolveAgentRoute 得到 agentId + sessionKey
          -> Session store 读/写元数据
              -> transcript append
                  -> enqueueCommandInLane（同一 session 排队）
```

### 3) 时序图

```text
Channel      Routing              Session store       Agent lane
  |              |                      |                 |
  | inbound      |                      |                 |
  |------------->| resolve              |                 |
  |              |--------------------->| 取 sessionId   |
  |              |                      |---------------->| 排队跑 Pi
```

### 4) 数据闭环图

```text
sessions.json 记「现在用哪只档案袋」
        |
        v
jsonl 只追加，像账本
        |
        v
/new、reset、idle 策略 -> 换新 sessionId
        |
        v
元数据指到新 jsonl -> 下一条消息进新袋
```

---

## 一、Session 里存什么

### 1.1 SessionEntry（类型从哪看）

**定义**：`src/config/sessions/types.ts`

**常见字段（人话）**：

| 字段 | 干嘛用 |
|------|--------|
| `sessionId` | 这只档案袋的内部 id |
| `sessionFile` | 聊天记录文件路径 |
| `groupActivation` | 群里是 **只有 @ 才理** 还是 **一直理** |
| `sendPolicy` | 允不允许往外发 |
| `compactionCount` / `memoryFlushAt` | 和压缩、记忆落盘相关的计数 |

### 1.2 sessionKey 长啥样（不用背，认个脸熟）

- **Main**：`agent:<agentId>:main` 一类。  
- **群**：`agent:<agentId>:<channel>:group:<groupId>` 一类。  
- **私聊 per-peer**：`agent:<agentId>:<channel>:dm:<peerId>` 一类。

**拼装工具**：`src/routing/session-key.ts`（如 `buildAgentMainSessionKey` 等；函数名以仓库为准）。

---

## 二、Routing 怎么分拣

### 2.1 入口

- **`resolveAgentRoute`**：`src/routing/resolve-route.ts`  
- **输入**：`ResolveAgentRouteInput`（channel、account、peer、guild、team …）  
- **输出**：`ResolvedAgentRoute`（`agentId`、`sessionKey`、`mainSessionKey`、`matchedBy`）

### 2.2 匹配顺序（简化版）

像 **从上到下试钥匙**：

1. 按 **peer**  
2. 按 **guild**（Discord）  
3. 按 **team**（Slack）  
4. 按 **account**  
5. 按 **channel**  
6. 最后 **默认 agent**

**钥匙从哪来**：`openclaw.json` 里的 **`bindings`**（类型在 `src/config/types.agents.ts` 一带）。

---

## 三、群组：什么时候理你？

- **`mention`**：一般要 **@ 到助理** 才回。  
- **`always`**：群里有动静就可能回（仍可能被别的安全策略挡住）。

**命令**：渠道里 `/activation` 一类；实现从 **`src/auto-reply/group-activation.ts`** 搜 `parseActivation`。

**门控**：没有单一文件名就叫 `group-gating.ts`；逻辑散在 **`src/auto-reply/`** 与渠道相关文件。用 IDE 搜 **`mention`**、**`allowlist`** 最快。

---

## 四、Session Store（磁盘）

- **读写**：`src/config/sessions/store.ts`（`loadSessionStore`、`saveSessionStore` 等）。  
- **路径**：`src/config/sessions/paths.ts` 的 `resolveStorePath`、`resolveSessionFilePath`。  
- **Main session key**：`src/config/sessions/main-session.ts` 的 `resolveMainSessionKey`。

---

## 五、Gateway 上能动的手脚

客户端经 WS 调 **`sessions.*`**：列表、预览、打补丁、重置、删除、压缩等（方法名见 Gateway 分册）。

---

## 六、关键文件速查

| 文件 | 职责 |
|------|------|
| `src/config/sessions/types.ts` | SessionEntry 类型 |
| `src/config/sessions/main-session.ts` | main session key |
| `src/config/sessions/session-key.ts` | 配置侧 session key 工具（与 routing 互补） |
| `src/config/sessions/store.ts` | 存取会话索引 |
| `src/routing/resolve-route.ts` | **分拣总入口** |
| `src/routing/session-key.ts` | 构建 routing 用的 key |
| `src/auto-reply/group-activation.ts` | `/activation` 等 |

---

## 常见误会

- **误会**：`sessionKey` 和 `sessionId` 永远一样。**正解**：一个是 **路由/持久化用的键**，一个是 **档案袋 id**；关系看 `ResolvedAgentRoute` 与 store。  
- **误会**：改 `bindings` 只影响新消息。**正解**：已打开的旧会话可能还指着旧袋；要理解 **reset / 切换** 行为。  
- **误会**：群组 `always` 等于骚扰全群。**正解**：还有 **allowlist、频道安全、模型策略** 多层刹车。
