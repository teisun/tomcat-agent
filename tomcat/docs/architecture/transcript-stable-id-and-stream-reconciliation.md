# Transcript 稳定 ID 与流式/历史收敛

**一句话定位**：把 assistant 的稳定 `entry.id` 从“落盘时才临时生成”前移到 `message_start` 前 mint；同时把同一套“出生地铸造 + forced-id 落盘”契约延伸到 user / steer / follow_up，让 `assistantMessageId` / `userMessageId` 与 transcript `entry.id` 全链路收敛，为 `tomcat-vscode-ext` webview 的 `upsert-by-id` 与 busy 会话切回重建提供统一身份。

> 本文回答六件事：
>
> 1. **为什么 reload / 切走切回会话后 transcript 会错乱？** 因为 live assistant/thinking 用临时 id、磁盘历史用 `entry.id`，前端只能靠文本去重；一旦 thinking 文本不一致，就会把残留 live 条目甩到末尾。
> 2. **根治思路是什么？** 不是继续优化“怎么合并两条 timeline”，而是让 live 和 history 从一开始就共享同一个稳定 id，同时让切回 busy 会话时只保留 runtime 真正在追踪的 live 尾巴。
> 3. **稳定 id 应该在哪一层产生？** 规则只有一条：**谁先把实体生出来，谁就铸造它的 id**。assistant 出生在后端 `message_start` 前；user 出生在前端回车那一刻。
> 4. **为什么 user 不能等到 `start_turn` 再起名？为什么不单独上 `user_message` 广播？** 因为前端在显示 user 气泡那一刻就必须已经知道这条消息是谁；若等 `start_turn` 才起名，前端只能先造临时壳再 swap，旧问题原样复活。单独补 `user_message` 广播是多端同步需求，不是这次稳定 id 的必要前提。
> 5. **tool / thinking / text-only turn 各怎么对齐？** tool UI 继续以 `toolCallId` 为稳定键；thinking 由前端按 `${assistantMessageId}-thinking` 派生；text-only turn 也必须在 `turn_end` 回填 `assistantMessageId`。
> 6. **为什么选这条方案，不选前端特判叠加？** 因为竞品基本都采用“单一时间线 + 稳定身份 + upsert”而不是“live 数组 + history 数组拼接再去重”；后者能修当前 bug，但会继续留下协议脆弱点。

---

## 先看总图：文首导读

### 阅读顺序建议

0. **A.0 粒度问答**：先搞清“一个 user prompt 产生几个 `assistantMessageId`、`message_*` 发几遍、它和 `entry.id` 什么关系”。
1. **A.1 抽象 ASCII 总图**：再看“身份何时产生、沿哪条链路传播、哪里落盘、哪里重放”。
2. **A.2 具体 ASCII 总图**：再看真实文件：`stream_handler.rs`、`accessors.rs`、`session_impl.rs`、`events/mod.rs`、`schema.rs`。
3. **B 状态机**：最后看**一条** assistant 消息（即一轮）从“未分配 → 已 mint → streaming → persisted → replayed”的生命周期；一个 user prompt 有几轮就重复几次。
4. **再下钻正文**：想看为什么选这条路读 §2 / §3；想看字段表读 §4；想看落点图读 §5；想看风险和验收读 §8 / §9。

### A.0 粒度问答（先把六个最常被问的问题钉死）

> 这一节直接回答“一个 user prompt 到底产生几个 `assistantMessageId`、`message_*` 发几遍、它和已有 `entry.id` 是什么关系”。结论全部以真实代码（`run_reasoning_loop` 的 `loop`、`stream_handler::run_chat_stream`、`session_impl::generate_entry_id`）与真实 transcript（“打飞机”会话的 `_13/_15/_17`）为准。

| 疑问 | 结论 | 依据 |
|------|------|------|
| **Q1：user prompt 也要稳定身份证吗？谁来 mint？** | **要，而且由前端 mint。** 规则不是“都在后端 mint”，而是“谁先把实体生出来，谁负责第一次起名”。assistant 出生在后端 `message_start` 前，所以后端 mint `assistantMessageId`；user 出生在前端回车那一刻，所以前端 mint `userMessageId`，再通过 `prompt / steer / follow_up` 上送给后端。 | `tomcat-vscode-ext/src/ui/webview/provider.ts`；`tomcat/src/api/serve/types.rs::ServeMessageParams.user_message_id` |
| **Q2：两个 user prompt 之间，是共用一个 `assistantMessageId`，还是每条 assistant 各一个？** | **每条 assistant message 各自一个、互不复用。** 一个 user prompt 会触发 `run_reasoning_loop` 的多轮循环（tomcat 代码里每轮叫一个 *turn*，`turn_index++`），**每轮产出一条 assistant message**，每轮 mint 一个**全新**的 id。 | `reasoning_loop.rs` `loop{…}`；“打飞机”一个 prompt = `_13`/`_15`/`_17` 三条 assistant |
| **Q3：`message_start/update/end` 在一个 user prompt 期间发一次还是每条 assistant 都发一遍？** | **每条 assistant message 各发一遍。** 每轮 `run_chat_stream` 只发一对 `MessageStart → MessageUpdate* → MessageEnd`；有几轮就有几对，各自带本轮的 `assistantMessageId`。 | `stream_handler.rs:123`（每次调用发一次 `MessageStart`） |
| **Q4：为什么 user 不能等到 `start_turn` 再 mint？** | **因为那时前端已经把气泡显示出来了。** 若等 `start_turn` / ack 回来才有正式 id，前端只能先用临时 id 占位，再把临时壳和磁盘真身 swap；这正是这次要消灭的重复/乱序根源。所以 user 必须在前端显示前就拿到稳定 id。 | `tomcat-vscode-ext/src/ui/webview/provider.ts`；`tomcat-vscode-ext/src/ui/webview/state.ts` |
| **Q5：`assistantMessageId` / `userMessageId` 和 transcript 里每条 message 的 `id` 是什么关系？** | **就是同一个值。** `assistantMessageId` 是某条 assistant 的 `entry.id`，只是 mint 时机前移到 `message_start` 前；`userMessageId` 是某条 user / steer / follow_up 的 `entry.id`，只是 mint 地点放在前端实体出生处。后端只负责执行统一 forced-id 契约：**给我 id 就按它落盘，不给才 `generate_entry_id()` 兜底**。 | `session_impl.rs::append_message_with_id`；`commands.rs::persist_turn_input_message` |
| **Q6：为什么不直接补一个 `user_message` 广播事件？** | **那是正交需求，不是这次稳定 id 的前提。** 就算以后补多端同步事件，也应继续复用同一个 `userMessageId`；否则会从“一条稳定 id”退回“两条 id + 映射 + swap”。 | `ServeMessageParams.user_message_id` 现已覆盖 `prompt / steer / follow_up`；webview 仍靠 history 命中同 id 原地收敛 |

一句话：**assistant 和 user 都要稳定 id，但各自按“出生地铸造”**：`assistantMessageId` = “某条 assistant message 的 `entry.id`，提前到直播开始前发给前端”；`userMessageId` = “某条 user / steer / follow_up 的 `entry.id`，在前端气泡出生时先铸好，再强制带到落盘层”。一个 user prompt 有几条 assistant message，就有几个独立的 `assistantMessageId`，`message_*` 就各发几遍。

### A.1 抽象 ASCII 总图

```text
一个 user prompt，可能触发「多轮(round)」assistant 回复
（tomcat 代码里每轮叫一个 turn：run_reasoning_loop 的 loop 每转一圈 = 一轮 = 一条 assistant message）

用户发来一个问题（前端先 mint `userMessageId = U`，显示在途气泡；后端落盘时沿用 U）
   │
   ▼
run_reasoning_loop 开始循环
   │
   ├─ 第 1 轮 run_chat_stream ─► 预 mint assistant 身份证 E1（全新）
   │     message_start / update* / end 都带 assistantMessageId = E1
   │     这轮调了 write 工具(toolCallId = T1)；落盘 assistant entry.id = E1
   │     └─ 对应真实 transcript 的 _13
   │
   ├─ 第 2 轮 run_chat_stream ─► 预 mint assistant 身份证 E2（全新，绝不复用 E1）
   │     message_start / update* / end 都带 assistantMessageId = E2
   │     这轮调了 bash 工具(toolCallId = T2)；落盘 assistant entry.id = E2
   │     └─ 对应 _15
   │
   └─ 第 3 轮 run_chat_stream ─► 预 mint assistant 身份证 E3（全新）
         message_start / update* / end 都带 assistantMessageId = E3
         这轮无工具、纯文本收尾；落盘 assistant entry.id = E3，循环结束
         └─ 对应 _17

最终效果（对其中任意一条，比如 E1）：
   直播里这条 assistant  = E1
   磁盘里这条 assistant  = E1
   reload 读回来还是      = E1
=> 前端按 id 认人：E1/E2/E3 各更新各的一条，不再排成两条，也不会互相串
```

这张图讲两件事：① **每条 assistant message 一轮一个独立身份证**（不是整个 prompt 共用一个）；② **每条的身份证在直播开始前就定下来、且全链路不改名**。方案成立的关键不在“前端 dedup 写得多聪明”，而在“上游别再给前端两套互不相干的 id”。

**说人话**：一个问题里 agent 可能边想边干、回好几条；每条回答都有自己的身份证，从直播到落盘到 reload 都不换。前端不用再猜“这条和那条是不是同一条”。

### A.2 具体 ASCII 总图

```text
webview prompt / steer / follow_up
   │
   ├─ 前端：实体出生 -> mint userMessageId = U
   │          -> 显示 in-flight user 气泡
   │          -> params.userMessageId = U 上送
   │
   ▼
┌─ src/core/agent_loop/stream_handler.rs ─────────────────────────────────────┐
│ • run_chat_stream() 在 MessageStart 前 mint pending_assistant_entry_id = E │
│ • emit MessageStart/Update/End{ assistant_message_id: E }                  │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                │
                                ▼
┌─ src/infra/events/mod.rs ───────────────────────────────────────────────────┐
│ • AgentEvent::MessageStart/Update/End 新增 assistant_message_id            │
│ • turn_end 继续承载 assistant_message_id / tool_call_ids                   │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                │
                                ▼
┌─ src/core/agent_loop/accessors.rs ──────────────────────────────────────────┐
│ • persist_message_if_needed() / push_message() 消费 pending id             │
│ • 不再让 append_message() 自己再 generate 一个新 id                        │
└───────────────┬──────────────────────────────┬──────────────────────────────┘
                │                              │
                ▼                              ▼
┌─ src/core/agent_loop/turn_finalize.rs ─┐  ┌─ src/core/agent_loop/tool_dispatcher.rs ─┐
│ • text-only turn 复用 E 落盘            │  │ • tool-call turn 先落 assistant(E)再跑工具 │
│ • turn_end.assistant_message_id = E     │  │ • tool UI 继续用 toolCallId，不改身份语义  │
└──────────────────────┬──────────────────┘  └──────────────────────┬──────────────────┘
                       │                                             │
                       └──────────────────────┬──────────────────────┘
                                              ▼
┌─ src/core/session/manager/session_impl.rs ──────────────────────────────────┐
│ • append_message_with_id(E) / append_message_internal(forced_id)           │
│ • MessageEntry.id = E，只有 fallback 路径才 generate_entry_id()            │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                │
                                ▼
┌─ src/api/serve/schema.rs ────────────────────────────────────────────────────┐
│ • `tomcat serve --print-schema` 导出新字段 assistantMessageId               │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ tomcat-vscode-ext/src/serveClient/wire.d.ts ───────────────────────────────┐
│ • 扩展侧用 assistantMessageId / toolCallId 做 upsert-by-id                  │
└──────────────────────────────────────────────────────────────────────────────┘
```

这张图把抽象链落到真实文件。最该记住的是：**唯一新的“源头状态”是 `pending_assistant_entry_id`**；其它变更本质上都是“把它一路透传并复用”。

**说人话**：后端这次不是要新造一套复杂状态机，而是多加一根“assistant 身份线”，让 stream、落盘、schema、前端都顺着这根线走。

### B. 状态机：一条 assistant 消息（一轮）的身份生命周期

> 下图描述**单条** assistant message（一轮 `run_chat_stream`）的身份流转。一个 user prompt 有几轮，这张图就按轮次重复几次，各轮 id 互相独立（E1、E2、E3…）。

```text
┌───────────────┐ mint E ┌───────────────────┐ first delta ┌────────────────┐
│ unassigned    │───────▶│ pending_id_minted │────────────▶│ streaming(E)   │
└───────────────┘        └─────────┬─────────┘             └──────┬─────────┘
                                   │ persist with E               │ interrupt / finish
                                   ▼                              ▼
                            ┌───────────────┐ get_messages ┌───────────────┐
                            │ persisted(E)  │──────────────▶│ replayed(E)   │
                            └───────────────┘               └───────────────┘
                                   ▲
                                   │ abort before persist
                                   │（若已形成 partial assistant，仍用 E 落 partial）
                                   └──────────────────────────────────────────
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `unassigned` | 进入 `MessageStart` | `pending_id_minted` | `generate_entry_id()`，写入 `pending_assistant_entry_id` | 一开流就先办好这条 assistant 的身份证。 |
| `pending_id_minted` | `message_update` 首个 delta | `streaming(E)` | `message_*` 事件都带 `assistantMessageId=E` | UI 从第一帧起就知道“你是谁”。 |
| `streaming(E)` | 正常收束 | `persisted(E)` | `append_message_with_id(E)` | 写盘不能再换身份证。 |
| `streaming(E)` | 中断但已有 partial | `persisted(E)` | partial assistant 仍以 E 落盘 | 中断不等于丢身份；已出现的内容仍然归到同一条。 |
| `persisted(E)` | `get_messages` / reload | `replayed(E)` | history entry `id = E` | reload 回来还是同一条，不会多出一份。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `entry.id` | transcript 中每条 `MessageEntry` 的稳定身份 | `session_impl.rs` 生成并写入 JSONL 的 `MessageEntry.id` | assistant / user / tool 各自独立；assistant/user 一旦确定，live 与 history 必须共用 | 落盘后的正式身份证。 |
| `assistantMessageId` | 本轮 assistant 消息对外暴露的稳定身份 | `AgentEvent::MessageStart/Update/End`、`TurnEnd` 新字段 | `message_start` 前 mint，一轮 assistant 只 mint 一次；text-only 与 tool-call turn 都必须可回填 | 给 UI 看的那张身份证，最终要和落盘那张完全同一张。 |
| `userMessageId` | user / steer / follow_up 对外暴露并最终落盘的稳定身份 | webview intent `params.userMessageId`；history user `entry.id` | 前端显示气泡前必须已存在；后端只负责复用，不得另起一张新证 | user 这边也要从出生起就叫同一个名字。 |
| `pending_assistant_entry_id` | 当前正在 streaming 的 assistant 预分配 id | `AgentLoop` 运行时字段 | 只在一条 assistant 生命周期内有效；落盘后清空；下一轮必须重新 mint | 这轮先暂存着的身份证。 |
| `in-flight user` | 已显示、已有稳定 id、但磁盘暂时还没有的 user 消息 | webview runtime 跟踪集合 + timeline item | runtime 仍在跟踪时可跨 rebuild 保留；history 命中同 id 后原地收敛并解除跟踪 | 不是“乐观占位符”，就是这条消息本人，只是暂时还没写进文件。 |
| `toolCallId` | LLM provider 分给某次工具调用的稳定身份 | `tool_execution_*` 事件、assistant `tool_calls[].id`、tool role message 的 `tool_call_id` | UI 工具卡继续按它 upsert；不强行转换成 transcript `entry.id` | 工具那边本来就有稳定 id，不用重造。 |
| `thinking id` | UI 对 thinking 块使用的稳定身份 | 前端派生规则：`${assistantMessageId}-thinking` | 后端不单独 wire 一个 thinking id；前后端都按同一派生规则计算 | 思考块跟着 assistant 走，不另办证。 |
| `epoch`（历史请求代际） | 前端本地为每个会话维护的代际号，**用途收窄**：只给「前端自己发起的异步 `getMessages`」做代际门闩 | webview runtime 里的前端本地整数，从 1 起；**后端完全不知道、不发送它** | 切会话 / 切回 / 重拉历史时 **加 1**；异步历史返回时若其捕获的代际 ≠ 当前代际则丢弃。**不用来拦 live 事件**（live 靠稳定 id 幂等 upsert） | 它只管「你切走时发出的那次拉历史回来晚了，别覆盖新界面」，不管直播包。 |
| `upsert-by-id` | 用稳定身份原地更新同一逻辑条目，而不是追加新条目 | 后端：`append_message_with_id`；前端：`applyEvent/hydrateHistory` 同 key 覆盖 | 同 id 永远只代表一条逻辑消息；较新状态覆盖较旧状态 | 同一个人来两次，不是排两行，是更新同一行。 |

## 2. 竞品 / 选型对比（调研）

> 专业：这类问题的本质不是“前端列表渲染有 bug”，而是“live streaming 与 persisted history 是否拥有同一身份模型”。竞品的分歧主要在：身份何时产生、snapshot 与 delta 如何收敛、切会话时如何丢弃旧流量。
>
> 说人话：先看别人怎么避免“聊天记录越切越乱”，再决定 Tomcat 是补丁式修，还是一步到位修。

> 调研基础：本节结论基于对四个仓库的源码实读（`/Users/yankeben/workspace/{cline,continue,opencode,vscode}`）。下表「身份何时产生 / live·history 怎么合 / 陈旧事件怎么挡」三列是逐项核对过的真实做法，不是凭命名推测。

| 竞品 | 身份何时产生 | live · history 怎么合 | 陈旧事件怎么挡（关键） | 我们取舍 |
|------|--------------|------------------------|------------------------|----------|
| `opencode` | 服务端 mint **单调可排序** `msg_/prt_` id（流开始前） | 单一 store 按 `sessionID` 分桶、按 id **幂等 upsert** | **不给事件盖 epoch**：id 幂等 → 晚到事件无害；只用前端本地 `generations` 计数器拦「异步 REST 拉取」的过期返回；跨会话靠 `sessionID` 分桶 | **主对标**：稳定 id + 幂等 upsert；epoch 收窄到「异步历史请求」 |
| `vscode` 内置 Chat | request 时即生成稳定 `request_/response_` id | `ChatModel` 是 SSOT，`ChatViewModel` 是可销毁的纯投影 | **完全不需要 epoch**：progress 按对象引用 / id 写进 model；`CancellationToken` + `isComplete` 守卫已完成响应 | 借「视图可重建、模型身份不可重建」+ 完成态守卫 |
| `cline` | 扩展宿主 `MessageIdMinter` mint `ts`（流开始时） | webview「收敛副本」reducer，partial + snapshot 双通道 | **宿主在产出每帧时同步盖 `epoch`+`seq`+`stateVersion`**，webview 丢弃 `epoch < 当前`。能成立是因为「盖章的人」==「产出事件的人」 | 借 `seq` 式「完成态/新旧」守卫；**不照搬「逐事件 epoch」**（见下「为什么不照搬 cline」） |
| `continue` | 客户端 `uuidv4()`（占位时） | 单一 `history[]`，流式就地改最后一条 | `AbortController` + `isStreaming` 软门闩；无服务端 sequence/epoch | 借「只有一条 timeline，绝不合并两份」 |

**综合结论（这四家其实只有两种范式）**：

1. **opencode / vscode 范式**：身份在源头稳定 → 客户端按 id 幂等 upsert → 晚到/乱序的 live 事件是「无害」而不是「陈旧」，**根本不需要 epoch**。
2. **cline 范式**：承认通道不可靠、可重复投递，于是宿主在产出端逐帧盖 epoch/seq，webview 当「收敛副本」。

Tomcat 的真实架构更接近 (1)：`tomcat serve` 是独立进程、走可靠的 in-process stdio NDJSON（单次投递），而且**切走会话 A 后 serve 仍在真实产出 A**——这些晚到事件不是「旧代际」，只是「晚到」。所以本方案选 **opencode/vscode 范式**：用稳定 id 让 live 事件天然幂等，把 `epoch` 这类「代际门闩」**收窄到唯一真正会破坏性竞态的地方——前端自己发起的异步历史请求**（详见 R6 与前端伴随方案 §3.2.3）。

为什么不照搬 cline 的「逐事件 epoch」：cline 能给每条 live 事件盖 epoch，是因为**盖章方（扩展宿主翻译 SDK 事件）和产出方是同一段代码**，能在产出时刻盖上「当前代际」。Tomcat 里产出方是独立的 `serve` 进程，它不知道、也不该知道 webview 的「界面第几代」；若让中间层 `provider.ts` 在转发时盖「转发时刻的 epoch」，晚到事件反而会被盖上**新** epoch，门闩失效。结论：**逐事件 epoch 在我们的进程边界上不成立，也不必要。**

为什么不是“只做前端 overlay 特判”：

1. **当前 bug 的根因在上游身份不一致**：前端再聪明也只能猜“这两条是不是同一个东西”，而不是知道。
2. **tool 侧已经证明稳定键可行**：`toolCallId` 本来就让工具卡在 live / history / reload 间天然对齐。
3. **reload 之外还有 loadOlderHistory / switchSession / closeSession fallback**：只补某一条入口，后面还会继续长重复特判。
4. **协议一旦钉死，前端逻辑会立刻变简单**：`timelineMergeKeys` 文本兜底、`[...history, ...liveOnly]` 拼接、空 thinking 锚点清理都可以删掉。
5. **后端最有资格做这件事**：`entry.id` 的真相、`append_message` 的时机、`turn_end` 的收束都在后端，前端不该倒过来发明“推测 id”。

## 3. 落地选型与实施（已定稿）

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| R1 assistant 身份产生时机 | streaming 开始时有没有稳定身份 | **采用“`MessageStart` 前预 mint `entry.id`，整轮 assistant 复用同一个 id”**。 | 本仓：`tomcat/src/core/agent_loop/stream_handler.rs`、`tomcat/src/core/session/manager/session_impl.rs`；外部：`agent/opencode + packages/opencode/src/id/id.ts`、`agent/cline + apps/vscode/src/sdk/message-id-minter.ts` | 设计：在最早可见时刻产生身份；理由：live UI、turn 收束、history replay 全部天然对齐，彻底消除“落盘后身份突变”。 | `continue/gui/src/redux/slices/sessionSlice.ts` 的“流式直接改最后一条”适合单进程单 store，不解决跨进程 wire 与 reload；现状“append 时才 generate”拒因：live 与 history 先天不同键。 | 别等写盘时才补身份证；一开流就办好，后面一路都用它。 |
| R2 流式事件字段形态 | 是把 id 塞进 `message` 里，还是顶层显式给 | **采用顶层 `assistantMessageId` 字段，挂到 `message_start / message_update / message_end / turn_end`**。 | 本仓：`tomcat/src/infra/events/mod.rs`、`tomcat-vscode-ext/src/serveClient/wire.d.ts`；外部：`agent/cline + apps/vscode/webview-ui/src/components/chat/chat-view/messageReducer.ts` | 设计：把“这条 assistant 是谁”做成一等字段；理由：前端无需解析 `message:any` 内部结构，也与 `turn_end.assistantMessageId` 命名对齐。 | “继续只发 `message:{}` 空壳”拒因：前端拿不到身份；“把 id 偷塞进 `message.id`”拒因：`Message` 在 wire 中是 `any`，语义不够显式，消费端容易误判。 | 直接在事件顶上写“这条 assistant 的 id 是 E”，最省脑子。 |
| R3 落盘复用策略 | 预 mint 的 id 如何保证真的落进 transcript | **采用统一 forced-id 契约**：assistant 继续走 `append_message_with_id(E)` / `forced_id`；user / steer / follow_up 也复用同一条路，前端给了 `userMessageId=U` 就按 U 落盘，不给才 `generate_entry_id()`。 | 本仓：`tomcat/src/core/agent_loop/accessors.rs`、`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/api/serve/commands.rs`；外部：`agent/opencode + packages/core/src/session/projector.ts` | 设计：把“谁决定 id”上收，持久化层只负责“有 id 就复用”；理由：assistant 与 user 最终落在同一条落盘契约上，避免一边稳定、一边临时。 | “先把 `msg.msg_id` 填上，再沿用旧 `persist_message_if_needed` 的 `is_some() => skip`”拒因：会直接跳过落盘；“让 `start_turn` 才给 user 起名”拒因：前端已经显示了 user 气泡，只能先造临时壳。 | 不管是谁生的，只要带着身份证来，落盘层就按这张证写。 |
| R4 thinking 身份 | thinking 块要不要单独在 wire 里再发一份 id | **采用前端派生：thinking id = `${assistantMessageId}-thinking`，后端不新增独立 thinking id 字段**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`（历史 hydration 已使用 `${entry.id}-thinking`）；外部：`agent/vscode + src/vs/workbench/contrib/chat/common/model/chatModel.ts` | 设计：thinking 作为 assistant 的从属视图块，不单独占用 wire 身份；理由：减少协议面，同时保证 live 与 history 的派生规则一致。 | “thinking 单独生成第二个后端 id”拒因：协议复杂度升高，且不增加表达力；“继续用前端临时 thinking-N”拒因：又回到当前 bug。 | 思考块跟着 assistant 走，用同一张身份证加个后缀就够了。 |
| R5 tool 身份 | tool UI 应按 transcript `entry.id` 还是 `toolCallId` 对齐 | **采用 `toolCallId` 继续作为 tool UI 稳定键；assistant/tool 的归组关系由 `assistantMessageId` 指向**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat/src/core/agent_loop/tool_dispatcher.rs`；外部：`agent/opencode + packages/tui/src/context/sync.tsx` | 设计：tool UI 键继续沿用 provider 已稳定暴露的调用 id；理由：一条 tool 调用会经历 start/update/end 多次 live 事件，而 transcript tool role message 只是一条最终记录。 | “把 tool UI 改成按 transcript tool `entry.id`”拒因：live start/update 阶段根本还没有那条最终 tool message；“给 tool 再加第二套映射表”拒因：复杂度高且没有收益。 | 工具这边本来就有好用的身份证，别把简单事情再复杂化。 |
| R6 切会话 / reload 陈旧事件 | 切走再切回时，怎么保证界面不被旧流量弄乱 | **三层分工：① 稳定 id + 幂等 upsert 让晚到的 live 事件无害；② 前端 `rebuildHistoryTimeline()` 只回灌 runtime 明确跟踪的在途实体（精准版 scalpel），不再把“窗口外旧条目”误当 live 尾巴；③ 前端本地「历史请求代际门闩」`epoch` 只拦异步 `getMessages` 的过期返回。后端只负责①的 id，不发任何 epoch**。 | 本仓：`tomcat-vscode-ext/src/ui/webview/provider.ts`、`tomcat-vscode-ext/src/ui/webview/state.ts`；外部：`opencode packages/app/src/context/server-session.ts(generations)`、`vscode chatModel.ts(isComplete)`、`cline messageReducer.ts(seq)` | 设计：让 live 事件靠幂等 upsert 天然安全（opencode/vscode 范式），并把切回 busy 会话时的重建判据从“位置”改成“状态”；理由：serve 是独立进程、晚到≠陈旧，真正有害的是窗口外旧条目被错甩到尾部。 | “照搬 cline 逐事件 epoch（让 provider 给每条 live 事件盖代际）”拒因：盖章方不是产出方，晚到事件会被盖上新代际，门闩失效；“切走时一刀全清”拒因：idle 会话没问题，但 busy 会话会误杀当前流式轮。 | live 事件按身份证认人就行；切回 busy 会话时要用手术刀，只留下 runtime 真正在追的活尾巴。 |
| R7 text-only turn 的收束 | 没有 tool 的纯文本回合如何让 summary / reload 也对齐 | **采用 text-only turn 在 `turn_end` 回填 `assistantMessageId`，与 tool-call turn 同口径**。 | 本仓：`tomcat/src/core/agent_loop/turn_finalize.rs`、`tomcat/src/core/agent_loop/tool_dispatcher.rs`；外部：`agent/vscode + src/vs/workbench/contrib/chat/common/model/chatViewModel.ts` | 设计：无论本轮有没有 tool，都给 UI 一个可追溯的 assistant 收束锚点；理由：summaryTitle、thinking 分组、历史重建不必再分 text/tool 两套路径。 | “只对 tool-call turn 带 id，text-only turn 保持 None”拒因：同一类 assistant 消息被拆成两套协议口径，前端又要写分支。 | 文本回合也是 assistant 回合，不能因为没调工具就不给身份证。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| B1 预 mint 与运行时缓存 | 新增 `pending_assistant_entry_id`；`message_start` 前 mint；每轮 assistant 生命周期独占一个 pending id | `tomcat/src/core/agent_loop/types.rs`、`tomcat/src/core/agent_loop/stream_handler.rs` | 见 §8 `BE-T1` / `BE-T2` | 先把身份证生成并放到运行时口袋里，后面谁用都从这儿拿。 |
| B2 流式事件与落盘复用 | `message_start/update/end` 带 `assistantMessageId`；`append_message` 支持 forced id；`prompt / steer / follow_up` 传 `userMessageId`；text-only / tool-call / abort partial / queued follow_up / queued steer 都复用同一条 forced-id 契约 | `tomcat/src/infra/events/mod.rs`、`tomcat/src/core/agent_loop/accessors.rs`、`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/core/agent_loop/turn_finalize.rs`、`tomcat/src/core/agent_loop/tool_dispatcher.rs`、`tomcat/src/core/agent_loop/reasoning_loop.rs`、`tomcat/src/api/serve/commands.rs` | 见 §8 `BE-T3` / `BE-T4` / `BE-T5` | 流里叫 E / 前端 user 里叫 U，写盘也必须还是 E / U，不能半路换名。 |
| B3 schema / wire 生成 | `serve --print-schema` 输出新增字段；扩展侧 `wire.d.ts` 同步生成；前后端编译期共享同一事件形状 | `tomcat/src/api/serve/schema.rs`、`tomcat-vscode-ext/src/serveClient/wire.d.ts` | 见 §8 `BE-T6` / `FE-T1` | 协议改了不靠口头同步，直接让生成产物告诉前端。 |
| B4 webview 单一时间线 | `assistantMessageId` 驱动 assistant/thinking upsert；`toolCallId` 驱动 tool upsert；删除文本兜底与 `[...history,...liveOnly]` 拼接；补「完成态守卫」+「历史请求代际门闩」（详见前端方案 §3.2.3） | `tomcat-vscode-ext/src/ui/webview/state.ts`、`tomcat-vscode-ext/src/ui/webview/provider.ts`、`tomcat-vscode-ext/gui/src/components/sessionList/groupTimelineByAssistantResponse.ts` | 见 §8 `FE-T2` / `FE-T3` / `E2E-T1` | 前端以后只维护一条 timeline，不再猜“哪条像是哪条”。 |
| B5 文档与回链 | 后端主方案 + 前端伴随方案成对落地；旧文在 §10 登记修订意图 | 本文、`tomcat-vscode-ext/docs/architecture/webview-transcript-stable-id-upsert.md` | 见 §8 `DOC-T1` | 不让协议真相散落在聊天记录和计划文件里。 |

#### 3.2.1 预 mint 与复用

> 专业：`generate_entry_id()` 从“append 时隐式副作用”前移为“message_start 前显式动作”，随后通过 `pending_assistant_entry_id` 串到 `emit_event` 与 `append_message_with_id`。这样身份的分配时机和消息的第一可见时机重合。
>
> 说人话：后端以后不能“先直播，后起名”；必须是“先起名，再直播”。

```text
run_chat_stream()
   │
   ├─ pending_assistant_entry_id = generate_entry_id()
   ├─ emit message_start{assistantMessageId:E}
   ├─ emit message_update{assistantMessageId:E, ...}*
   ├─ emit message_end{assistantMessageId:E}
   └─ push_message(...) -> append_message_with_id(E)
```

#### 3.2.2 text-only / tool-call / abort 三路收束统一

> 专业：本方案不允许 text-only turn、tool-call turn、abort partial push 各自走一套不同的 id 逻辑。三路都必须先消费 `pending_assistant_entry_id`，再在各自时机补 `turn_end.assistantMessageId`。
>
> 说人话：不管这轮是纯文本、调工具还是半路中断，UI 看到的都得是“同一个 assistant E”。

```text
                ┌─ text-only ──────► turn_finalize.rs      ──► turn_end(E)
pending id = E ─┼─ tool-call ──────► tool_dispatcher.rs    ──► turn_end(E,[toolCallIds])
                └─ abort partial ──► reasoning_loop.rs     ──► partial assistant 持久化(E)
```

#### 3.2.3 schema 生成与前端消费

> 专业：协议字段一旦升级，`wire.d.ts` 必须由 `serve --print-schema` 生成，不允许扩展侧手写 `assistantMessageId` / `userMessageId` 类型。前端伴随方案只描述消费方式，不复制后端字段定义。
>
> 说人话：字段怎么长以后由后端生成文件说了算，前端别自己抄一份。

## 4. 协议（入参 / 出参 / Schema）

> 专业：本方案分成两半：**出参侧**新增/强化 `assistantMessageId` 事件字段；**入参侧**为 `prompt / steer / follow_up` 的 `ServeMessageParams` 增加可选 `userMessageId`。`get_messages` 的历史条目结构不变，但 assistant / user 现在都会和各自的 live 身份共享同一个 `entry.id`。
>
> 说人话：不只是事件里多了一根“assistant 身份线”，用户发消息这头也多了一根“user 身份线”。

### 4.0 `prompt / steer / follow_up` 的 `userMessageId`

单一事实源：[`../../src/api/serve/types.rs`](../../src/api/serve/types.rs) 的 `ServeMessageParams`，以及由 [`../../src/api/serve/schema.rs`](../../src/api/serve/schema.rs) 生成的 [`../../../tomcat-vscode-ext/src/serveClient/wire.d.ts`](../../../tomcat-vscode-ext/src/serveClient/wire.d.ts)。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `params.userMessageId` | string | 否 | `null` | `prompt` / `steer` / `follow_up` | 若客户端已在实体出生时 mint 稳定 id，则后端必须按该 id 落盘；空串/非法/冲突时可回退 `generate_entry_id()` | 用户这条消息如果已经有身份证，就把同一张证带进后端。 |
| `params.attachments` | array | 否 | `[]` | `prompt` / `follow_up` 等既有路径 | 与本方案无关，但仍与 `userMessageId` 同处 `ServeMessageParams` | 附件逻辑不变，只是旁边多带了一张证。 |

### 4.1 `message_start / message_update / message_end`

单一事实源：[`../../src/infra/events/mod.rs`](../../src/infra/events/mod.rs) 的 `AgentEvent::MessageStart/Update/End`，以及由 [`../../src/api/serve/schema.rs`](../../src/api/serve/schema.rs) 生成的 [`../../../tomcat-vscode-ext/src/serveClient/wire.d.ts`](../../../tomcat-vscode-ext/src/serveClient/wire.d.ts)。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | string | 是 | — | 三个事件通用 | 固定为 `"message_start"` / `"message_update"` / `"message_end"` | 这是哪一帧。 |
| `sessionId` | string | 否 | active session | 三个事件通用 | 既有会话路由字段，不变 | 属于哪个会话。 |
| `assistantMessageId` | string | 是 | — | 三个事件通用 | 本轮 assistant 的稳定身份；由 `message_start` 前预 mint，直至落盘 | 这条 assistant 叫啥。 |
| `message` | object | 是 | `{}` | 三个事件通用 | 保持既有 `Message` 外壳；本方案不依赖解析其内部结构来拿 id | 保留原来的 message 壳子。 |
| `assistantMessageEvent` | object | 仅 `message_update` 必填 | — | `message_update` | 既有 delta 载体；`kind=content_delta|thinking_delta` 等保持不变 | 真正的增量内容还放这儿。 |

### 4.2 `turn_end`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | string | 是 | — | `turn_end` | 固定 `"turn_end"` | 这是回合收尾帧。 |
| `turnIndex` | number | 是 | — | `turn_end` | 既有字段，不变 | 第几回合。 |
| `assistantMessageId` | string | 条件必填 | `null` | 只要本轮形成 assistant transcript entry（text-only / tool-call / partial）就必须带 | 与最终 assistant `entry.id` 完全相等 | 这回合最后那条 assistant 叫啥。 |
| `toolCallIds` | string[] | 否 | `[]` | `turn_end` | 仅承载本轮工具调用身份；**不是** transcript `entry.id` | 这回合调了哪些工具。 |
| `summaryTitle` | string | 否 | `null` | `turn_end` | thinking/turn summary 标题；前端按 `assistantMessageId` 归组 | 给这轮思考块的标题。 |
| `toolResults` | array | 是 | `[]` | `turn_end` | 既有字段，不变 | 这回合的工具结果摘要。 |

### 4.3 UI 侧派生规则

| 派生项 | 输入 | 规则 | 说人话 |
|--------|------|------|--------|
| assistant block id | `assistantMessageId` | 直接等于 `assistantMessageId` | assistant 自己就用这张身份证。 |
| thinking block id | `assistantMessageId` | `${assistantMessageId}-thinking` | thinking 跟着 assistant 走。 |
| tool card id | `toolCallId` | 直接等于 `toolCallId` | 工具卡继续沿用工具调用 id。 |
| assistant-tool 归组 | `assistantMessageId + toolCallId` | tool 卡保留 `assistantMessageId` 作为父组锚点 | 工具知道自己挂在哪条 assistant 下。 |

### 4.4 jsonc 调用样例

```jsonc
// 0) webview 发 prompt：先带上 userMessageId
{
  "type": "prompt",
  "sessionId": "s_demo",
  "text": "继续修这个 bug",
  "params": {
    "userMessageId": "4b2e5525-3d85-4d9f-a614-dfbcc41a97f0"
  }
}
```

```jsonc
// 1) 本轮 assistant 开始 streaming：先拿到稳定 id
{
  "type": "message_start",
  "sessionId": "s_demo",
  "assistantMessageId": "1782635000123456_42",
  "message": {}
}

// 2) streaming 中的 thinking delta / content delta 都复用同一个 id
{
  "type": "message_update",
  "sessionId": "s_demo",
  "assistantMessageId": "1782635000123456_42",
  "message": {},
  "assistantMessageEvent": {
    "kind": "thinking_delta",
    "delta": "我先看一下目录结构..."
  }
}
{
  "type": "message_update",
  "sessionId": "s_demo",
  "assistantMessageId": "1782635000123456_42",
  "message": {},
  "assistantMessageEvent": {
    "kind": "content_delta",
    "delta": "我来先排查一下。"
  }
}

// 3) tool-call turn 收束时，turn_end 仍回同一个 assistantMessageId
{
  "type": "turn_end",
  "sessionId": "s_demo",
  "turnIndex": 7,
  "assistantMessageId": "1782635000123456_42",
  "toolCallIds": ["call_abc123"],
  "summaryTitle": "Used 1 tool",
  "toolResults": []
}

// 4) get_messages 返回的 assistant 历史条目 id 与上面完全一致
{
  "type": "message",
  "id": "1782635000123456_42",
  "message": {
    "role": "assistant",
    "content": "我来先排查一下。",
    "thinking_text": "我先看一下目录结构..."
  }
}
```

## 5. 文件职责总览（One-Glance Map）

```text
┌─ src/core/agent_loop/types.rs ───────────────────────────────────────────────┐
│ • AgentLoop 新增 pending_assistant_entry_id: Option<String>                 │
│ • 约束：单条 assistant 生命周期独占，收束后清空                            │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/core/agent_loop/stream_handler.rs ─────────────────────────────────────┐
│ • MessageStart 前 mint E                                                    │
│ • MessageStart/Update/End 都带 assistant_message_id = E                     │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/infra/events/mod.rs ────────────────────────────────────────────────────┐
│ • AgentEvent::MessageStart/Update/End 新字段 assistant_message_id           │
│ • TurnEnd.assistant_message_id 作为 turn 收束锚点                           │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/core/agent_loop/accessors.rs ──────────────────────────────────────────┐
│ • persist_message_if_needed / push_message 消费 E                           │
│ • 负责把“流式 id”带到“落盘调用”                                             │
└───────────────┬──────────────────────────────┬──────────────────────────────┘
                ▼                              ▼
┌─ src/core/agent_loop/turn_finalize.rs ─┐  ┌─ src/core/agent_loop/tool_dispatcher.rs ─┐
│ • text-only turn 落 assistant(E)        │  │ • tool-call turn 落 assistant(E)          │
│ • turn_end.assistant_message_id = E     │  │ • toolExecution_* 继续用 toolCallId       │
└──────────────────────┬──────────────────┘  └──────────────────────┬──────────────────┘
                       └──────────────────────┬──────────────────────┘
                                              ▼
┌─ src/core/session/manager/session_impl.rs ──────────────────────────────────┐
│ • append_message_with_id(E) / forced_id                                     │
│ • fallback 才走 generate_entry_id()                                         │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ src/api/serve/schema.rs + src/api/serve/tests/commands_test.rs ────────────┐
│ • `serve --print-schema` 导出 assistantMessageId                            │
│ • serve 侧字段与事件样例测试                                                 │
└───────────────────────────────┬─────────────────────────────────────────────┘
                                ▼
┌─ tomcat-vscode-ext/src/serveClient/wire.d.ts + state.ts + tests/… ─────────┐
│ • 扩展消费 assistantMessageId / toolCallId                                  │
│ • upsert-by-id / epoch / switch-back E2E                                    │
└──────────────────────────────────────────────────────────────────────────────┘
```

阅读顺序：先从 `stream_handler.rs` 看 E 在哪儿产生，再看 `accessors.rs + session_impl.rs` 它怎么落盘，最后看 `schema.rs` 和扩展侧它怎么被消费。`tool_dispatcher.rs` 与 `turn_finalize.rs` 是两个收束分叉，必须都看。

**说人话**：这张图的重点不是“改了很多文件”，而是“所有文件都围着同一件事转：把 E 原封不动送完全程”。

## 6. 配置与环境变量

> 专业：本方案**不新增**运行时 env / config 键。身份模型变更由代码契约和 schema 生成驱动，不通过配置开关灰度。
>
> 说人话：这不是一个需要开关的功能，而是协议真相本身要改对。

## 7. 错误模型 / 截断 / 警告

```text
正常路径
  message_start mint E
    → message_update*(E)
    → append_message_with_id(E)
    → turn_end(E)

中断路径（已有 partial assistant）
  message_start mint E
    → message_update*(E)
    → interrupt
    → partial assistant 仍以 E 落盘
    → agent_interrupted

无 sink / 纯测试路径
  message_start mint E
    → 仅 streaming 可见 E
    → 不落 transcript（允许）

协议破坏（实现 bug）
  已发出 message_start(E)
    → append 时丢失 E
    → 视为内部不变量破坏；测试必须拦住
```

| 结局 | 是否抛错 | 处理动作 | 说人话 |
|------|----------|----------|--------|
| 正常完成 | 否 | streaming 与 transcript 共用 E | 一切照常，只是身份稳定了。 |
| 中断且已有 partial | 否 | partial assistant 仍以 E 持久化；`agent_interrupted` 维持既有语义 | 停了也别换身份证，免得 reload 对不上。 |
| `message_append_sink = None` | 否 | 允许只存在 live E、不写 JSONL；主要用于测试/非会话路径 | 有些场景只看流，不看落盘，这不算错误。 |
| 已发出 E 但落盘未复用 E | 是（内部 bug） | 通过单测/集成测阻断；实现层可 `debug_assert!` | 这不是用户输入问题，是我们自己把合同写破了。 |

## 8. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元 | `BE-T1` `agent_loop_tests::message_start_mints_stable_assistant_message_id` | PENDING | 一开流就有 id。 |
| 单元 | `BE-T2` `agent_loop_tests::streaming_and_persisted_assistant_share_same_entry_id` | PENDING | 流里和盘里真的是同一个 E。 |
| 单元 | `BE-T3` `agent_loop_tests::tool_call_turn_reuses_pending_assistant_message_id` | PENDING | 调工具这条分支别偷偷换 id。 |
| 集成 | `BE-T4` `commands_test::serve_turn_end_reports_text_only_assistant_message_id` | PENDING | 纯文本回合也得把 id 报出来。 |
| 集成 | `BE-T5` `commands_test::serve_message_update_schema_contains_assistant_message_id` | PENDING | schema 和实际事件别两张皮。 |
| 前端单元 | `FE-T1` `state.test.ts::hydrates_history_by_stable_assistant_id_without_duplicate_thinking` | PENDING | 有了同 id 以后，reload 不应再冒空 thinking。 |
| 前端集成 | `FE-T2` `webview_provider_flow.test.ts::switch_back_keeps_single_transcript_timeline` | PENDING | 切走再切回也只是一条 timeline。 |
| E2E | `E2E-T1` `installed.test.ts::assertTranscriptSwitchBackOrder` | PENDING | 真实 VS Code 里复刻用户路径也不能乱。 |
| 关键承诺 | `assistantMessageId == transcript entry.id == history replay id` | PENDING | 这就是本方案最核心的承诺。 |
| 文档 | 本文 + [`../../../tomcat-vscode-ext/docs/architecture/webview-transcript-stable-id-upsert.md`](../../../tomcat-vscode-ext/docs/architecture/webview-transcript-stable-id-upsert.md) | ✅ 2026-06-29 | 主方案和消费方案都写清楚了。 |

## 9. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| text-only 与 tool-call 两条收束路径改一半 | 高 | `turn_finalize.rs` 与 `tool_dispatcher.rs` 同时改，并各有独立测试 | 最怕只修一条分支，另一条继续偷偷换 id。 |
| 前端 / 后端二进制版本错配 | 中 | `serve --print-schema` 生成 `wire.d.ts`；握手能力/版本校验继续保留；打包时绑定同仓构建产物 | 让编译和握手尽早发现你两边不是一套协议。 |
| thinking 仍沿用旧临时 id | 高 | 前端 companion 文档明确 thinking id 派生规则；state 单测锁死 `${assistantMessageId}-thinking` | assistant 稳了但 thinking 还乱起名，一样会出问题。 |
| 中断时 pending id 遗留到下一轮 | 中 | `AgentLoop` 在 turn 收束与新一轮 `MessageStart` 入口都显式清空/重置；多轮 tool loop 测试覆盖 | 这轮的身份证别带到下一轮。 |
| 未来 transcript rewrite / hydration 另起新口径 | 中 | 在 [`chat-resume-hydration.md`](./chat-resume-hydration.md) 的 §10 登记“assistant entry.id 现承担 live/history 对齐职责” | 以后谁想改恢复逻辑，先看到这里这根线不能断。 |

## 10. 历史决策 / 跨文档修订

- ~~沿用现状：assistant `entry.id` 只在 `append_message()` 时生成~~ → **否**：这会导致 live 与 history 永远不是同一身份，只能靠文本去重，正是本次 webview 错乱的根因。
- ~~前端修复只做 overlay 特判，不改后端协议~~ → **否**：能治当前症状，但会把“身份不一致”的问题继续埋在协议里。

跨文档修订意图：

1. [`agent-server-and-ui-gateway.md`](./agent-server-and-ui-gateway.md)：补一条“`message_*` 现在显式带 `assistantMessageId`，作为 UI/serve 的 transcript 主锚点”。
2. [`llm-stream-events-cli-pipeline.md`](./llm-stream-events-cli-pipeline.md)：补“MessageStart 前预 mint id，streaming 与落盘共用”。
3. [`chat-resume-hydration.md`](./chat-resume-hydration.md)：补“assistant `entry.id` 现在同时承担 live/history 对齐职责，history replay 不再是纯离线概念”。
4. [`../../../tomcat-vscode-ext/docs/architecture/webview-transcript-stable-id-upsert.md`](../../../tomcat-vscode-ext/docs/architecture/webview-transcript-stable-id-upsert.md)：作为扩展伴随方案，承接本文 §4 协议字段的消费方式，不重复定义字段真相。

---

一句话总结：**这次不是让前端“更会去重”，而是让后端第一次把 assistant 的身份在最早时刻钉死，然后流式、落盘、重放都说同一种话。**
