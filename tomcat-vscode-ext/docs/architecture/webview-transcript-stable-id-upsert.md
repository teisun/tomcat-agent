# Webview Transcript：稳定 ID 与 Upsert-by-ID 渲染

> 适用范围：`tomcat-vscode-ext` 的 webview / provider / state store 如何消费后端新增的 `assistantMessageId` / `userMessageId`，把当前“history + live 拼接再去重”的 timeline 模型改成“单一时间线 + 稳定键 upsert”的渲染模型。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。本文 `## 1`–`## 10` 与规范 §1–§10 一一对应；文首导读置于 `## 1` 之前、不占用 § 编号。
> 上游主方案：[`../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md`](../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md)。**协议字段真相以主方案 + [`../../src/serveClient/wire.d.ts`](../../src/serveClient/wire.d.ts) 为准**，本文只回答“扩展侧怎么消费它”。
> 关联文档：[`tomcat-vscode-extension-phase2/05-webview-ui-architecture.md`](./tomcat-vscode-extension-phase2/05-webview-ui-architecture.md)、[`tomcat-vscode-extension-phase2/04-protocol-runtime.md`](./tomcat-vscode-extension-phase2/04-protocol-runtime.md)、[`transcript-checkpoint-restore.md`](./transcript-checkpoint-restore.md)、[`tomcat-vscode-extension.md`](./tomcat-vscode-extension.md)。
> 单一事实源：
> 1. host↔serve 协议消费以 [`../../src/serveClient/wire.d.ts`](../../src/serveClient/wire.d.ts) 为准；
> 2. webview timeline 状态收敛以 [`../../src/ui/webview/state.ts`](../../src/ui/webview/state.ts) 为准；
> 3. host 生命周期与历史加载时机以 [`../../src/ui/webview/provider.ts`](../../src/ui/webview/provider.ts) 为准；
> 4. 渲染分组以 [`../../gui/src/components/sessionList/groupTimelineByAssistantResponse.ts`](../../gui/src/components/sessionList/groupTimelineByAssistantResponse.ts) 为准。
>
> 本文回答五件事：
>
> 1. **为什么切回 busy 会话会把旧 user 甩到当前页尾？** 因为旧实现把“这条不在最近 80 条里”误判成“它一定是 live 尾巴”，`rebuildHistoryTimeline()` 会把窗口外旧条目错追加到末尾。
> 2. **有了 `assistantMessageId` / `userMessageId` 之后，前端最重要的变化是什么？** 不是“多了两个字段”，而是 assistant / thinking / tool / approval / 未落盘 user 都能各自找到稳定键，`applyEvent` 和 `hydrateHistory` 可以走同一个 upsert 模型。
> 3. **工具卡和审批卡怎么处理？** 工具卡继续用 `toolCallId` 作为逻辑稳定键；审批卡继续用 `requestId`；assistant/thinking 才切到 `assistantMessageId`。
> 4. **“乐观 user” 这个词还成立吗？** 不成立。现在只有“未落盘的在途 user 消息”：它从回车那一刻就拿稳定 id，将来磁盘命中同一个 `entry.id` 原地收敛，不再有“临时壳”和“真身”两套东西。
> 5. **切走再切回时，旧流量怎么挡？谁需要 `epoch`？** 分三层：① 稳定 id 让晚到的 **live 事件**按身份证幂等 upsert、天然无害（不需要 epoch）；② `rebuildHistoryTimeline()` 只回灌 runtime 明确跟踪的在途实体，不再按“在不在最近 80 条里”猜尾巴；③ `epoch`（历史请求代际）**只**拦「前端自己发起的异步 `getMessages` 回来晚了、覆盖新界面」。后端不发 epoch，晚到的 live 事件也不带 epoch（详见 §3.2.3）。

**一句话定位**：后端主方案负责“身份证从哪来”，本文负责“拿到身份证以后，webview 不再靠长相认人”。

---

## 先看总图：文首导读

### 阅读顺序建议

1. **A.1 抽象 ASCII 总图**：先看“event / history 各自带什么键进来，最后如何收成一条 timeline”。
2. **A.2 具体 ASCII 总图**：再看 `provider.ts`、`wire.d.ts`、`state.ts`、`groupTimelineByAssistantResponse.ts` 怎么串。
3. **B 状态机**：最后看“live 创建 → history 命中同 id → 原地更新 → message_end 后完成态守卫挡散落 delta”的状态迁移。
4. **再下钻正文**：想看为什么这么选读 §2 / §3；想看消费哪些字段读 §4；想看落点读 §5；想看验收和风险读 §8 / §9。

### A.1 抽象 ASCII 总图

> 粒度提醒：一个 user prompt 期间 agent 可能回多条 assistant message（每轮一条，各自独立 `assistantMessageId`，详见后端主方案 A.0）。下图描述其中**任意一条**的收敛逻辑；前端对每条 `E` 各自按 id upsert、互不相干。

```text
同一条 assistant 回复，会从两条路来到前端
   │
   ├─ 第一路：直播时先到
   │    assistantMessageId = E
   │    thinking           = E-thinking
   │    tool               = toolCallId = T
   │
   └─ 第二路：稍后 reload / 拉历史再到
        assistant entry.id = E
        tool tool_call_id  = T

前端拿到后，不再先问“它来自 live 还是 history”
而是先问“它是不是同一个东西”
   │
   ├─ assistant 看 E
   ├─ thinking  看 E-thinking
   ├─ tool      看 T
   └─ approval  看 requestId
   │
   ▼
命中同一个键  -> 原地补全 / 覆盖
没命中        -> 新增一条
   │
   ▼
最终 session.timeline 里
同一条 assistant / thinking / tool 只保留一份
```

这张图只讲前端最核心的心智模型：**不要先分 live/history 两条表，再去猜怎么合并；所有入口都先落成“实体键”，再决定是更新还是新增。**

**说人话**：不看“这条消息从哪条路来”，只看“它是不是那个人”。证件号一样，就更新原来那一行。

### A.2 具体 ASCII 总图

```text
tomcat serve stdout event / get_messages history
   │
   ▼
┌─ src/ui/webview/provider.ts ────────────────────────────────────────────────┐
│ • handleServeEvent(event) -> stateStore.applyEvent(event)                  │
│ • refreshSessionHistory(sessionId) -> stateStore.hydrateHistory(history)   │
│ • switchSessionView / selectSession / loadOlderHistory 负责 bump epoch      │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/serveClient/wire.d.ts ─────────────────────────────────────────────────┐
│ • message_start/update/end: assistantMessageId                              │
│ • turn_end: assistantMessageId + toolCallIds                               │
│ • tool_execution_*: toolCallId                                             │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/ui/webview/state.ts ────────────────────────────────────────────────────┐
│ • assistant item id      = assistantMessageId                              │
│ • thinking item id       = `${assistantMessageId}-thinking`                │
│ • tool entity key        = toolCallId                                      │
│ • approval entity key    = requestId                                       │
│ • applyEvent / hydrateHistory / rebuildHistoryTimeline 全都按 key upsert    │
│ • 删除 timelineMergeKeys 文本兜底与 [...history,...liveOnly] 拼接          │
└───────────────┬──────────────────────────────┬──────────────────────────────┘
                ▼                              ▼
┌─ protocol.ts ──────────────────────────┐  ┌─ groupTimelineByAssistantResponse.ts ─┐
│ • WebviewTimelineItem 仍是 UI 壳类型    │  │ • 按 assistantMessageId 做展示分组       │
│ • 但每类 item 的 id 口径统一稳定化      │  │ • 不再承担“补救乱序/重复”的职责          │
└────────────────────────────┬───────────┘  └───────────────────────┬────────────┘
                             ▼                                      ▼
                    TranscriptView / MessageBubble / ThinkingBlock / ToolRow
```

这张图把抽象心智模型落到真实落点。最该记住的是：**`provider.ts` 决定“什么时候重建”，`state.ts` 决定“同一个东西怎么算同一个”，`groupTimelineByAssistantResponse.ts` 只负责“怎么摆得更好看”**。

**说人话**：后端把身份证给你，`state.ts` 负责按证找人，UI 组件只负责把人画出来。

### B. 状态机：一条 assistant 组在前端的生命周期

```text
┌──────────────┐ message_start(E) ┌────────────────┐ message_update(E) ┌──────────────┐
│ not_present  │─────────────────▶│ live_streaming │──────────────────▶│ live_streaming│
└──────────────┘                  └──────┬─────────┘   (原地 append)    └──────┬───────┘
                                         │ message_end(E) / 磁盘已有 E 终稿            │
                                         ▼                                            │
                                   ┌──────────────┐ history hit(E) ┌──────────────┐  │
                                   │ closed(E)    │───────────────▶│ reconciled   │  │
                                   └──────┬───────┘                └──────────────┘  │
                                          │ 散落的 delta(E) 又晚到                     │
                                          ▼                                          │
                                   ┌──────────────┐                                  │
                                   │ delta 被忽略  │  (完成态守卫：不再 append)         │
                                   └──────────────┘                                  │
   注：epoch 不在这张图里。它只作用于「异步 getMessages 的返回」，不作用于 live 事件——见 §3.2.3。
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `not_present` | `message_start(E)` | `live_streaming` | 创建 assistant `id=E`、thinking `id=E-thinking`（按需） | 先看到直播时，把这条 assistant 记成 E。 |
| `live_streaming` | `message_update(E)` | `live_streaming` | 原地 append 到同一个 assistant/thinking（按 E 定位，不靠 runtime 临时指针） | 直播继续来，就更新同一条。 |
| `live_streaming` | `message_end(E)` 或磁盘已有 E 终稿 | `closed(E)` | 标记 E 流式结束 | 这条 assistant 直播完了。 |
| `closed(E)` | history assistant `entry.id=E` | `reconciled` | 用磁盘版覆盖/补全同一实体，不新增第二条 | reload 回来命中同一个 E，只会变完整，不会多一条。 |
| `closed(E)` | 散落的 `content_delta/thinking_delta(E)` 又晚到 | `closed(E)` | **完成态守卫**：忽略，不再 append（防重复文本） | 收尾后再飘来的半句话，别重复贴上去。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `assistantMessageId` | assistant 逻辑实体的稳定身份 | live `message_*` / `turn_end` 事件；history assistant `entry.id` | 同一条 assistant 在 live 与 history 中必须完全相等 | assistant 的正式身份证。 |
| `userMessageId` | user / steer / follow_up 在前端出生时铸造的稳定身份 | webview `prompt` / `steer` / `retryUserMessage` intent；history user `entry.id` | 前端显示这条 user 气泡时就必须已存在；落盘后 `entry.id` 必须与之相等 | user 这边也有正式身份证，而且是出生即定。 |
| `thinking id` | thinking UI 块的稳定身份 | 前端派生：`${assistantMessageId}-thinking` | 仅由 assistant 身份派生；不单独从 wire 获取 | 思考块跟着 assistant 走。 |
| `toolCallId` | tool UI 实体的稳定身份 | `tool_execution_*` 事件、history tool role message 的 `tool_call_id` | tool card 的 upsert pivot；不切到 transcript tool `entry.id` | 工具卡继续按工具调用 id 认人。 |
| `requestId` | approval UI 实体的稳定身份 | `control_request.ask_question` | approval 卡继续按它去重/更新 | 提问/审批卡自己的身份证。 |
| `WebviewTimelineItem.id` | UI 渲染用的稳定 `id` 字段 | `protocol.ts` 里的各类 timeline item | assistant/thinking/approval 直接等于逻辑实体键；tool 至少要可稳定映射回 `toolCallId` | UI 这一行最终叫啥。 |
| `entity key` | state store 用来做 upsert 的逻辑键 | `state.ts` 内部归一化规则 | live `applyEvent` 与 history `hydrateHistory` 必须共用 | 真正决定“是不是同一个东西”的键。 |
| `in-flight user` | 已显示、已有稳定 id、但磁盘还没有的 user 气泡 | `state.ts` runtime `localUserMessageIds` + timeline item | 只要 runtime 仍在跟踪，就能跨 rebuild 保留；一旦 history 命中同 id，原地收敛并解除跟踪 | 不是“乐观壳”，就是这条消息本人，只是暂时还没写进文件。 |
| `completion guard`（完成态守卫） | 决定一条 delta 该不该被 append | **复用 `state.ts` 既有的 `runtime` 在流 id**，不另建并行 map | `content_delta/thinking_delta(E)` 仅当 `E === runtime 当前在流 id` 时才 append；`message_end(E)` 经 `clearStreaming` 清空在流 id 后，散落 delta(E) 自然落空被忽略 | 收尾后飘来的半句话别重复贴。对标 vscode `isComplete` / cline `seq`，但零新增状态。 |
| `epoch`（历史请求代际） | 前端本地为每个会话维护的代际号，**用途收窄**：只给「前端自己发起的异步 `getMessages`」做代际门闩 | `provider.ts` runtime 里的前端本地整数，从 1 起；**后端完全不知道、不发送它** | 切会话 / 切回 / 重拉历史时加 1；异步历史返回时若其捕获代际 ≠ 当前代际则丢弃。**不用来拦 live 事件** | 只管「你切走时发出的那次拉历史回来晚了别覆盖新界面」，不管直播包。对标 opencode `generations` / continue `AbortController`。 |
| `single timeline` | 不再维护“live 列表 + history 列表”两套数据 | `session.timeline` | 所有入口最终都落在同一数组 / 同一实体图上 | 以后只有一份聊天记录，不玩双轨并行。 |

## 2. 竞品 / 选型对比（调研）

> 专业：前端侧真正要借鉴的，不是某个具体组件长什么样，而是“state 到底是一条线还是两条线”“streaming 是 append 还是 upsert”“重开视图时旧流量怎么隔离”。
>
> 说人话：看别人的重点不是 UI 漂不漂亮，而是他们为什么不会越切越乱。

> 调研基础：下表为四仓源码实读结论（`/Users/yankeben/workspace/{cline,continue,opencode,vscode}`）。重点看「陈旧事件怎么挡」这一列——它直接决定我们 `epoch` 的正确用法。

| 竞品 | live · history 模型 | 陈旧事件怎么挡（关键） | 我们取舍 |
|------|---------------------|------------------------|----------|
| `opencode` | 服务端单调 id；REST / SSE / optimistic 全 upsert 进同一按 id 索引的 store，按 `sessionID` 分桶 | **不给事件盖 epoch**：id 幂等→晚到无害；仅用前端 `generations` 计数器拦「异步 REST 拉取」的过期返回；`message.part.delta` 是非幂等 append，用 `staleDeltas` 处理 | **主对标**：稳定 id 幂等 upsert + 把 `epoch` 收窄到异步历史请求 |
| `vscode` Chat | `ChatModel` 长生命周期 SSOT，`ChatViewModel` 可销毁纯投影 | **完全不需要 epoch**：progress 按对象引用/id 直接写 model；`CancellationToken` + `isComplete` 守卫已完成响应 | 借「视图可重建、模型身份不可重建」+ 完成态守卫（`isComplete`） |
| `cline` | webview「收敛副本」reducer，partial + snapshot 双通道 | **宿主在产出每帧时盖 `epoch`+`seq`+`stateVersion`**，webview 丢 `epoch<当前`、按 `seq` 保最新。成立前提：盖章方==产出方 | 借 `seq` 式「新旧/完成态」守卫；**不照搬逐事件 epoch**（serve 不是产出端，盖不了） |
| `continue` | 单一 Redux `history[]`，流式改最后一条 | `AbortController` + `isStreaming` 软门闩；无服务端 sequence/epoch | 借「只有一条 timeline，绝不合并两份」 |

**两种范式，我们选哪个**：opencode/vscode = 「源头身份稳定 → 客户端幂等 upsert → 晚到事件无害，不需要 epoch」；cline = 「通道不可靠、宿主逐帧盖代际、webview 当收敛副本」。Tomcat 走可靠的 in-process stdio（单次投递），且切走会话 A 后 serve 仍在**真实产出** A——晚到 ≠ 陈旧。所以我们采 **opencode/vscode 范式**，`epoch` 只留给「前端自己发起的异步 `getMessages`」（§3.2.3）。

> 为什么不照搬 cline 逐事件 epoch：cline 的扩展宿主**既翻译事件又盖 epoch**，能在产出时刻盖「当前代际」。Tomcat 的产出方是独立 `serve` 进程，不知道 webview「第几代界面」；若让 `provider.ts` 在转发时盖「转发时刻的 epoch」，晚到事件反而会被盖上**新**代际，门闩失效。故逐事件 epoch 在我们的进程边界上既不成立、也不必要。

## 3. 落地选型与实施（已定稿）

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| F1 时间线模型 | state store 维护一条 timeline 还是两条（live/history） | **采用单一 `session.timeline`，live 与 history 都按稳定键 upsert 到这一份上**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat-vscode-ext/src/ui/webview/provider.ts`；外部：`agent/continue + gui/src/redux/slices/sessionSlice.ts`、`agent/vscode + src/vs/workbench/contrib/chat/common/model/chatModel.ts` | 设计：一份实体图，多个输入源；理由：切会话、reload、loadOlderHistory 都不再是“两个列表怎么拼”，而是“同一实体怎么补全”。 | 现状 `rebuildHistoryTimeline = [...historyItems, ...liveOnly]` 拒因：哪怕只错一个 thinking 文本，就会把 live 残留整片甩回末尾。 | 以后只有一份聊天记录，直播和历史都往这份里写。 |
| F2 assistant / thinking 身份 | assistant 与 thinking 用什么键对齐 live / history | **assistant 直接用 `assistantMessageId` / history `entry.id`；thinking 固定派生 `${assistantMessageId}-thinking`**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat-vscode-ext/src/ui/webview/protocol.ts`；外部：`agent/opencode + packages/app/src/context/server-session.ts` | 设计：assistant 是一等实体，thinking 是从属视图块；理由：assistant 身份从后端来，thinking 规则在前后端文档里固定一次即可。 | 继续沿用 `createTimelineId(session, "assistant-group")` / `thinking-N` 拒因：reload 后永远不可能命中历史同一条。 | assistant 自己一张证，thinking 用“这张证 + 后缀”。 |
| F2b user 身份 | user prompt / steer / follow_up 该由谁 mint 身份 | **采用“出生地铸造”规则：assistant 出生在后端 → 后端 mint `assistantMessageId`；user 出生在前端 → 前端 mint `userMessageId`，后端按 forced-id 原样落盘**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/provider.ts`、`tomcat-vscode-ext/src/ui/webview/state.ts`；外部：`agent/opencode + packages/opencode/src/id/id.ts` | 设计：谁先把实体展示出来，谁就负责第一次起名；理由：这样前端不需要临时 id，也不需要 ack 后 swap。 | “让后端到 `start_turn` 再给 user 起名”拒因：气泡已经显示了，前端只能先造临时 id，等于把旧问题原样留着；“单独加 user_message 广播事件”拒因：那是多端同步需求，不是这次稳定 id 的必要前提。 | user 这边别再搞占位符，从回车那一刻就用将来会落盘的同一个 id。 |
| F3 tool 身份 | tool card 是按 transcript tool `entry.id` 还是按 `toolCallId` | **采用 `toolCallId` 继续作为 tool UI 的稳定键，并把 `assistantMessageId` 作为归组锚点**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat-vscode-ext/gui/src/components/sessionList/groupTimelineByAssistantResponse.ts`；外部：`agent/cline + apps/vscode/src/sdk/task-proxy.ts` | 设计：tool 从 start/update/end 到 history 都围绕同一 `toolCallId`；理由：tool transcript entry 是最终结果，不适合作为 live 阶段主键。 | “tool 改按 transcript entry.id”拒因：start/update 阶段还没有那条最终 tool message；“tool 再建一张映射表”拒因：复杂且无收益。 | 工具卡本来就认 `toolCallId`，别折腾。 |
| F4 history 重建算法 | rebuild 时按什么标准保留 live 尾巴 | **采用“精准版 scalpel”**：磁盘部分全部按 history 重建；内存里只回灌 runtime 正在跟踪的在途实体（streaming assistant / thinking / running tool / pending approval / in-flight user）；其它不在 history 页里的旧条目一律视为“窗口外已落盘项”丢弃。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`；外部：`agent/opencode + packages/tui/src/context/sync.tsx` | 设计：判据从“位置”改成“状态”；理由：busy 会话切回时要保住当前流式轮，但不能把窗口外旧 user 甩到末尾。 | “只要不在最近 80 条里就当 live 尾巴”拒因：这是本次旧 user 漂尾的直接根因；“切走时一刀全清”拒因：idle 会话没问题，但 busy 会话会把当前流式轮误杀掉。 | 不是拿大锤把现场全清，而是用手术刀只留下 runtime 真正在追的那点活尾巴。 |
| F5 切会话与旧事件 | 只有稳定 id 够不够，还要不要挡晚到事件 | **三层：① live 事件靠稳定 id 幂等 upsert（晚到无害）；② `message_end` 后用「完成态守卫」忽略散落 delta；③ `epoch` 只给前端自己发起的异步 `getMessages` 做代际门闩**。后端不发 epoch。 | 本仓：`tomcat-vscode-ext/src/ui/webview/provider.ts`、`tomcat-vscode-ext/src/ui/webview/state.ts`；外部：`opencode server-session.ts(generations)`、`vscode chatModel.ts(isComplete)`、`cline messageReducer.ts(seq)` | 设计：晚到的 live 事件是「无害」不是「陈旧」，用幂等 upsert 处理（opencode/vscode 范式）；epoch 收窄到唯一破坏性竞态——在途历史拉取覆盖新界面。 | “逐事件 epoch（provider 给每条 live 事件盖代际）”拒因：盖章方≠产出方，晚到事件会被盖新代际，门闩失效；“只看 `sessionId`”拒因：切回同会话 sessionId 不变，但在途旧 `getMessages` 仍会覆盖。 | live 事件认身份证就够；要拦的是「切走时那次拉历史回来晚了别覆盖新界面」。 |
| F6 展示分组职责 | `groupTimelineByAssistantResponse.ts` 要不要继续承担补救职责 | **保留它的展示职责，但移除它对重复/错位输入的隐式兜底预期**。 | 本仓：`tomcat-vscode-ext/gui/src/components/sessionList/groupTimelineByAssistantResponse.ts`；外部：`agent/vscode + src/vs/workbench/contrib/chat/common/model/chatViewModel.ts` | 设计：grouping 只做“如何把 assistant + thinking + tools 摆成一组”；理由：数据正确性应由 state store 保证，而不是由渲染层顺手修补。 | “让 collectGroup 继续扫描整条 timeline 帮我们补救乱序”拒因：这会把渲染层和状态层搅在一起，bug 更隐蔽。 | 分组函数只负责摆盘，不负责补锅。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| FE1 协议消费升级 | `message_*` / `turn_end` 消费 `assistantMessageId`；thinking 派生 id；tool 继续用 `toolCallId` | `tomcat-vscode-ext/src/serveClient/wire.d.ts`、`tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat-vscode-ext/src/ui/webview/protocol.ts` | 见 §8 `FE-T1` / `FE-T2` | 先把“拿什么当身份证”定死。 |
| FE2 单一 timeline + upsert | `applyEvent`、`hydrateHistory`、`rebuildHistoryTimeline` 全部按 entity key upsert；`appendLatestHistory` 改 merge；重建时只保留 runtime 在途尾巴 | `tomcat-vscode-ext/src/ui/webview/state.ts` | 见 §8 `FE-T3` / `FE-T4` | 以后不再拼两份列表，只更新一份，而且不会再把窗口外旧 user 甩到末尾。 |
| FE3 完成态守卫 + 历史请求代际门闩 | 完成态守卫：`content_delta/thinking_delta` 仅当其 `assistantMessageId === runtime 在流 id` 时 append（复用既有 `clearStreaming`，零新增状态）；历史代际门闩：`provider.ts` 给每个会话维护 `epoch`，`getMessages`/`loadOlderHistory` 调用时闭包捕获代际、返回时比对，过期则不交给 `hydrateHistory`/`prependHistory` | `tomcat-vscode-ext/src/ui/webview/provider.ts`、`tomcat-vscode-ext/src/ui/webview/state.ts` | 见 §8 `FE-T5` / `FE-T5b` | 收尾后别重复贴；切走时发出的旧拉取回来晚了别覆盖。 |
| FE4 渲染层对齐 | `groupTimelineByAssistantResponse.ts` 继续按 `assistantMessageId` 分组；`TranscriptView` / `ThinkingBlock` / `ToolRow` 不再依赖“重复项后来被隐藏”的隐式行为 | `tomcat-vscode-ext/gui/src/components/sessionList/groupTimelineByAssistantResponse.ts`、`tomcat-vscode-ext/gui/src/components/TranscriptView.tsx` | 见 §8 `FE-T6` | UI 负责展示，不再背着修数据的锅。 |
| FE5 验收链路 | 单测、provider flow、E2E、installed VSIX 切走切回路径都锁住 | `tomcat-vscode-ext/src/ui/webview/tests/state.test.ts`、`tomcat-vscode-ext/tests/webview_provider_flow.test.ts`、`tomcat-vscode-ext/src/test/suite/support/hostE2eScenario.ts`、`tomcat-vscode-ext/e2e-harness/src/test/installed.test.ts` | 见 §8 `FE-T7` / `E2E-T1` | 用户怎么复现，我们就怎么把它测死。 |

#### 3.2.1 同一套 upsert 入口

> 专业：`applyEvent(live)` 与 `hydrateHistory(history)` 不再各自维护一套“创建 item”逻辑，而是共享同一组 entity key 规则：assistant=`E`，thinking=`E-thinking`，tool=`toolCallId`，approval=`requestId`。
>
> 说人话：无论是直播来的，还是文件里读回来的，只要它们本来是同一个东西，就得走同一个入口更新。

```text
live event ─────┐
                ├─ normalize(entity key) ──► upsert(item)
history entry ──┘
```

#### 3.2.2 重建只决定顺序，不决定身份

> 专业：`rebuildHistoryTimeline()` 仍以 history 决定“当前可见顺序”，但不再借由文本去重来决定“谁是谁”。身份已经由 entity key 决定，重建只负责“把同 key 的 live 尾巴收进正确位置”。这里的关键不是“它在不在最近 80 条里”，而是“runtime 此刻有没有明确跟踪它仍在途”。
>
> 说人话：排序归排序，认人归认人，这两件事以后分开。

```text
切回 busy 会话时：
  磁盘部分             = 全部按 history 重建（这就是“以磁盘为准”）
  内存 live 尾巴       = 只保留 runtime 正在追踪的那一小段

旧（错）：
  不在最近 80 条里  -> 当成 live 尾巴 -> 甩到末尾

新（对）：
  runtime 说它仍在流式/未落盘 -> 才保留
  其它不在 history 页里的项   -> 一律当窗口外旧条目丢弃
```

`idle` 会话和 `busy` 会话的差别也在这里：

```text
              idle 会话                      busy 会话
一刀全清       看起来能工作                   会误杀当前流式轮
精准版 scalpel 没尾巴可保留，结果一样          既保住当前轮，也不让旧 user 漂尾
```

所以这里不是“切走时彻底清空”与“不清空”二选一，而是：**磁盘部分全部重拉 + 只保留 runtime 在追的 live 尾巴**。

#### 3.2.3 切走切回的三层防线（含 `epoch` 链路 ASCII 图）

先认清三个角色——`provider.ts`、`state.ts` 是什么，`tomcat serve` 又是什么：

```text
                          VS Code 扩展进程（你的插件代码）
   ┌──────────────────────────────────────────────────────────────────┐
   │  provider.ts  ＝ 调度层 / 中间人                                    │
   │    • 全局订阅一次 tomcat serve 的事件流（切会话时并不重订阅）        │
   │    • 发起 getMessages(sessionId) 异步拉历史                         │
   │    • 决定「何时清空并重建某会话的界面」                              │
   │    • 维护每会话的 epoch（历史请求代际，纯前端本地整数）              │
   │                                                                    │
   │  state.ts     ＝ 内存状态仓库（被动）                               │
   │    • 只有一条 session.timeline                                      │
   │    • applyEvent(live) / hydrateHistory(history) 都按稳定 id upsert   │
   │    • 维护 per-assistantMessageId 的「完成态守卫」标记               │
   └───────────────▲───────────────────────────▲──────────────────────┘
                   │ live 事件(stdout NDJSON)    │ 历史(getMessages 异步返回)
                   │                            │
        ┌──────────┴────────────────────────────┴──────────┐
        │  tomcat serve  ＝ 独立进程（真正的后端）            │
        │    • 只认 sessionId / assistantMessageId(E) / toolCallId │
        │    • 不知道、也不发送 epoch                         │
        └────────────────────────────────────────────────────┘
```

**关键澄清（直接回答“晚到的事件会带旧 epoch 吗”）**：不会。`tomcat serve` 根本不发 epoch；晚到的 live 事件也**不带**任何 epoch。`epoch` 只贴在「`provider.ts` 自己发起的那一次 `getMessages` 调用」上——靠 JS 闭包在调用时把当时的代际号捕获下来。所以**根本不需要后端参与**。

切走切回时，三种旧流量各有各的防线：

| 旧流量来源 | 它真的「陈旧」吗 | 防线 | 靠什么 |
|------------|------------------|------|--------|
| 晚到的 live 事件（A 仍在跑） | 否，只是晚到 | 按 `assistantMessageId/toolCallId` **幂等 upsert** | 稳定 id（§3.2.1） |
| `message_end` 后散落的 delta | 是（已收尾） | **完成态守卫**：closed 后忽略 | per-id closed 标记 |
| 切走时发出、回来晚了的 `getMessages` | 是（旧代际） | **历史请求代际门闩 `epoch`** | provider 闭包捕获代际 |

只有第三种需要 `epoch`。它的链路如下：

```text
你在看会话 A（A 的 epoch = 1）
   │
   ├─(t1) provider 调 getMessages(A)，闭包记下 capturedGen = 1 ──────┐
   │                                                  （IPC 往返，慢） │
   ├─(t2) 你切到 B，再切回 A                                          │
   │        provider 把 A 的 epoch: 1 → 2                            │
   │        并重新 getMessages(A)，新闭包记下 capturedGen = 2 ──┐     │
   │                                                            │     │
   │   ┌──(t3) 旧请求结果回来了（capturedGen=1）◄───────────────┼─────┘
   │   │        provider 比对：1 ≠ 当前 2  → 丢弃，不调 hydrateHistory
   │   │        （这步就是「门闩」：旧拉取不许覆盖新界面）
   │   │
   │   └──(t4) 新请求结果回来（capturedGen=2）◄────────────────┘
   │            provider 比对：2 == 当前 2  → 交给 state.hydrateHistory(A)
   ▼
state.timeline 只被「当代」的历史快照重建；期间 A 的 live 事件照常按 id upsert，无害
```

一句话：**没有谁去给晚到的 live 事件“盖旧 epoch”——那条路在我们的进程边界上根本不成立（见 §2「为什么不照搬 cline」）。`epoch` 的全部职责，是让 `provider.ts` 认出「这是我切走之前发出的那次拉历史」，回来晚了就丢掉。**

**与 cline/opencode 的对应**：本节 = opencode 的 `generations`（拦过期 REST 拉取）+ vscode 的 `isComplete`（完成态守卫），刻意避开 cline 的「逐事件 epoch」。

## 4. 协议消费（入参 / 出参 / Schema）

> 专业：本节不重写后端协议定义，而是列出**前端真正会消费的字段**、它们各自映射成什么 entity key、以及不同来源（live / history）如何对齐。
>
> 说人话：这里不讨论字段怎么生成，只讨论前端拿到字段以后怎么用。

### 4.1 live 事件消费表

单一事实源：[`../../src/serveClient/wire.d.ts`](../../src/serveClient/wire.d.ts)。

| 事件 / 字段 | JSON 类型 | 必填 | 来源 | 前端消费动作 | 说人话 |
|-------------|-----------|------|------|--------------|--------|
| `message_start.assistantMessageId` | string | 是 | serve live 事件 | 创建/定位 assistant `id=E`；为 thinking 预留 `E-thinking` 的归组锚点 | 先把这条 assistant 认出来。 |
| `message_update.assistantMessageId` | string | 是 | serve live 事件 | 定位同一 assistant / thinking 实体 | 后面的 delta 都回到同一个人身上。 |
| `message_update.assistantMessageEvent.kind=content_delta` | enum | 是 | serve live 事件 | append 到 assistant `id=E` 的文本 | assistant 说话就往 E 身上加。 |
| `message_update.assistantMessageEvent.kind=thinking_delta` | enum | 是 | serve live 事件 | append 到 thinking `id=E-thinking` 的文本 | 思考也跟着 E 走。 |
| `message_end.assistantMessageId` | string | 是 | serve live 事件 | 结束当前 assistant streaming 生命周期 | 这条 assistant 直播到此结束。 |
| `tool_execution_start/update/end.toolCallId` | string | 是 | serve live 事件 | upsert tool 实体键 `T`；并记录 `assistantMessageId=E` 归组 | 工具卡照旧按 `toolCallId` 更新。 |
| `turn_end.assistantMessageId` | string | 条件必填 | serve live 事件 | 给 summaryTitle / turn-level 归组与 finalize 提供锚点 | 这回合最后那条 assistant 是谁。 |
| `control_request.requestId` | string | 是 | host→webview 控制帧 | approval card 按 `requestId` upsert | 审批卡继续按 requestId 认人。 |

### 4.2 history 条目消费表

单一事实源：`get_messages` 返回的 transcript entries 与 [`../../src/ui/webview/state.ts`](../../src/ui/webview/state.ts) 的 history 解析逻辑。

| history entry | 读取字段 | entity key | 前端消费动作 | 说人话 |
|---------------|----------|------------|--------------|--------|
| assistant message | `entry.id` | `assistant=entry.id` | assistant 文本进入与 live 同一实体；`thinking_text` 进入 `entry.id-thinking` | 磁盘里的 assistant 和直播里的 assistant 本来就是同一个 E。 |
| tool role message | `message.tool_call_id` | `tool=toolCallId` | 命中 live tool 实体，补全 summary / result | 工具卡也命中自己那张老证。 |
| user / notice / warn / error | `entry.id` 或稳定文本策略 | 各自实体键 | 与既有 message block 模型对齐 | 普通消息照旧渲染。 |
| custom plan / session events | `entry.id` / `plan_id` / `path` | 现有 plan/session 键 | 沿用现有 plan card / session todo 路由 | 这次不改 plan 卡身份模型。 |

### 4.3 jsonc 映射样例

```jsonc
// live: assistant 开始 streaming
{
  "type": "message_start",
  "sessionId": "s_demo",
  "assistantMessageId": "1782635000123456_42",
  "message": {}
}
// state.ts 归一化后：
// assistant item.id = "1782635000123456_42"
// thinking  item.id = "1782635000123456_42-thinking"

// history: reload 后拿到同一条 assistant
{
  "type": "message",
  "id": "1782635000123456_42",
  "message": {
    "role": "assistant",
    "content": "我来先排查一下。",
    "thinking_text": "我先看一下目录结构..."
  }
}
// hydrateHistory 命中同一 key，只补全，不新增第二条。
```

## 5. 文件职责总览（One-Glance Map）

```text
┌─ src/ui/webview/provider.ts ────────────────────────────────────────────────┐
│ • handleServeEvent()：live 事件进 stateStore                               │
│ • refreshSessionHistory()/prependHistory()：history 进 stateStore          │
│ • switchSessionView()/selectSession()：切换期 bump epoch                   │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/serveClient/wire.d.ts ─────────────────────────────────────────────────┐
│ • live assistant 使用 assistantMessageId                                   │
│ • tool 使用 toolCallId                                                     │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/ui/webview/state.ts ────────────────────────────────────────────────────┐
│ • applyEvent()：live -> normalize key -> upsert                            │
│ • hydrateHistory()/prependHistory()：history -> normalize key -> upsert     │
│ • rebuildHistoryTimeline()：history 为序，live 为同 key 补全               │
│ • 删掉 timelineMergeKeys 文本兜底 / [...history,...liveOnly]               │
└───────────────┬──────────────────────────────┬──────────────────────────────┘
                ▼                              ▼
┌─ src/ui/webview/protocol.ts ──────────────┐  ┌─ gui/src/components/sessionList/... ─┐
│ • WebviewTimelineItem 类型壳              │  │ • group by assistantMessageId         │
│ • item.id 口径统一                         │  │ • consumedItemIds 只服务展示分组      │
└──────────────────────┬────────────────────┘  └────────────────────┬───────────────┘
                       ▼                                           ▼
┌─ gui/src/components/TranscriptView.tsx / ThinkingBlock.tsx / ToolRow.tsx ─┐
│ • 只渲染 stable state，不再隐式补救重复 / 错位                              │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ tests: state.test.ts / webview_provider_flow.test.ts / hostE2eScenario.ts ┐
│ • 单测锁 key 收敛；集成测锁切换；E2E 锁真实 DOM 顺序                         │
└──────────────────────────────────────────────────────────────────────────────┘
```

阅读顺序：先看 `provider.ts` 什么时机把 live/history 送进来，再看 `state.ts` 用什么键收进去，最后看 `groupTimelineByAssistantResponse.ts` 只是怎样把已经正确的一条 timeline 重新分组展示。测试文件对应三层：状态、宿主、真实 UI。

**说人话**：关键全在 `state.ts`，其它文件基本都在配合它“按同一套身份证认人”。

## 6. 配置与环境变量

| 变量 / 常量 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `HISTORY_PAGE_ENTRIES` | number（建议 `80`） | webview 首屏 history 页大小；扩大后减少“看起来像开头丢了”的误判 | 代码常量 | 首屏多拉一点，别让用户误以为历史没对齐。 |

> 专业：本方案前端侧不新增 env；唯一行为性常量是 history 首屏窗口，可作为体验优化同步调整。
>
> 说人话：主要修身份，不靠配置开关；只是顺手把首屏页宽一点。

## 7. 错误模型 / 截断 / 警告

```text
正常
  live(E) 到达
    → 按 id upsert assistant/thinking/tool
    → history(E) 命中同 key
    → 原地补全

晚到的 live 事件（A 仍在跑）
  live(E) 乱序/晚到
    → 按 id 幂等 upsert（无害，不需要 epoch）

message_end 后散落的 delta
  delta(E) 在 closed(E) 之后到达
    → 完成态守卫：忽略，不再 append

过期的异步历史返回
  getMessages(A) 回来时 capturedGen ≠ 当前 epoch
    → provider 丢弃，不调 hydrateHistory

协议破坏 / 版本错配
  message_* 缺 assistantMessageId
    → 视为不满足同版本契约
    → 不走 upsert-by-id 主路径
```

| 结局 | 是否抛错 | 处理动作 | 说人话 |
|------|----------|----------|--------|
| 同 key 命中 | 否 | 原地更新同一实体 | 正常补全，不新增第二条。 |
| 晚到/乱序的 live 事件 | 否 | 按稳定 id 幂等 upsert | 晚到≠陈旧，认身份证就对。 |
| `closed(E)` 后又来 delta | 否 | 完成态守卫丢弃，不 append | 收尾后飘来的半句话别重复贴。 |
| 过期的异步 `getMessages` 返回 | 否 | provider 按代际门闩丢弃 | 切走时那次拉历史回来晚了，别覆盖新界面。 |
| `message_*` 缺 `assistantMessageId` | 视为协议破坏 | 由 host/版本校验尽早发现；同版本构建不支持退回文本去重主路径 | 这不是“特殊情况”，而是前后端不是同一套协议。 |

## 8. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元 | `FE-T1` `state.test.ts::message_start_uses_assistant_message_id_as_stable_key` | PENDING | live 一开始就按 E 建实体。 |
| 单元 | `FE-T2` `state.test.ts::hydrate_history_hits_same_assistant_entity_without_duplicate_thinking` | PENDING | history 命中 E 只补全，不多一条空 thinking。 |
| 单元 | `FE-T3` `state.test.ts::rebuild_history_timeline_upserts_by_id_instead_of_concat` | PENDING | 重建后不再拼接 `liveOnly`。 |
| 单元 | `FE-T4` `state.test.ts::tool_cards_keep_tool_call_id_as_entity_key` | PENDING | 工具卡还是按 `toolCallId` 认人。 |
| 单元 | `FE-T4b` `state.test.ts::late_or_out_of_order_live_event_upserts_without_duplicate` | PENDING | 晚到/乱序的 live 事件按 id 幂等收敛、不重复。 |
| 单元 | `FE-T5` `state.test.ts::delta_after_message_end_is_ignored_by_completion_guard` | PENDING | 收尾后散落的 delta 不再重复 append。 |
| 单元 | `FE-T5b` `webview_provider_flow.test.ts::stale_get_messages_result_is_dropped_by_epoch` | PENDING | 过期的异步历史返回被代际门闩挡掉。 |
| 集成 | `FE-T6` `webview_provider_flow.test.ts::switching_away_and_back_keeps_single_timeline` | PENDING | 切到 B 再切回 A 也不会越切越多。 |
| E2E | `E2E-T1` `hostE2eScenario::assertTranscriptSwitchBackOrder` | PENDING | 真实 DOM 顺序与 transcript 源一致。 |
| E2E | `E2E-T2` `installed.test.ts::webview_transcript_remains_aligned_after_switch_back` | PENDING | 打包后装上的 VSIX 也得稳。 |
| 关键承诺 | “assistant/thinking/tool/approval 各按稳定键 upsert，切走切回零增长” | PENDING | 这就是本方案的最终承诺。 |
| 文档 | 本文 + [`../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md`](../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md) | ✅ 2026-06-29 | 后端讲真相，前端讲消费。 |

## 9. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| 只改 `applyEvent`，没改 `hydrateHistory` / `rebuildHistoryTimeline` | 高 | 三处入口统一走 normalize+upsert helper，拒绝“live 一套 / history 一套” | 只修直播，不修 reload，问题一定会换个入口再冒出来。 |
| thinking 还用旧临时 id | 高 | thinking id 规则在 `state.ts` 和本文 §4 固定为 `${assistantMessageId}-thinking`，并用单测锁死 | assistant 稳了但 thinking 还乱，最后看起来还是乱。 |
| 工具卡误切到 transcript `entry.id` | 中 | 在 `state.ts` / 测试里明确“tool entity key = toolCallId” | 工具卡别换身份证，不然又会多出第二套映射。 |
| `groupTimelineByAssistantResponse` 继续依赖“乱输入也能被藏住” | 中 | 把去重职责回收到 `state.ts`；渲染层只保留展示分组 | UI 不能继续偷偷给状态层擦屁股。 |
| 前后端版本错配 | 中 | 依赖 `wire.d.ts` 同仓生成 + 握手能力/版本检查；不支持回退到旧文本去重主路径 | 不是同一版就别指望这套稳定 id 契约还能自动成立。 |

## 10. 历史决策 / 跨文档修订

- ~~`rebuildHistoryTimeline()` 末尾继续 `session.timeline = [...historyItems, ...liveOnly]`，靠 `timelineMergeKeys` 文本兜底 dedup~~ → **否**：这正是本次“切走切回后一堆空 thinking 行”的直接根因。
- ~~`groupTimelineByAssistantResponse.ts` 继续承担“把重复/错位项聚拢后看起来没事”的隐式职责~~ → **否**：展示层不应该承担状态修复职责。

跨文档修订意图：

1. [`tomcat-vscode-extension-phase2/05-webview-ui-architecture.md`](./tomcat-vscode-extension-phase2/05-webview-ui-architecture.md)：应补“`state.ts` 已从 history+实时合并器演进为稳定 id upsert 收敛器”的描述。
2. [`tomcat-vscode-extension-phase2/04-protocol-runtime.md`](./tomcat-vscode-extension-phase2/04-protocol-runtime.md)：应补 `assistantMessageId` 在 webview 侧的消费语义与 `toolCallId` 分工。
3. [`tomcat-vscode-extension.md`](./tomcat-vscode-extension.md)：应登记“这是 transcript / reload / switch-back 子问题的专项 companion”。
4. [`../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md`](../../../tomcat/docs/architecture/transcript-stable-id-and-stream-reconciliation.md)：作为协议主方案，字段真相若有变更，以主方案与生成的 `wire.d.ts` 为准。

---

一句话总结：**后端负责把 assistant 的身份证发准，前端负责从此以后只按身份证认人；只要这两件事都做到，切走切回、reload、loadOlderHistory 都不该再把 transcript 弄乱。**
