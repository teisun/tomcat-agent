# Phase A：会话邮箱、热/冷分层与单机容量重构

> 父文档：[`01-overview.md`](./01-overview.md)
>
> 适用范围：只讨论 **单机 / 单进程 / 本地 `stdio` 行为不回退** 前提下，Tomcat 如何先把“事件扇出、内存常驻、调度与背压”三件结构性问题改对，为后续 gateway / shared storage / cluster 打地基。
>
> 不在本册范围：
>
> - 不引入外部 gateway 与多租户鉴权（见 [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md)、[`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md)）
> - 不改变现有 `ServeCommand` / `ControlFrame` 主形状
> - 不把 transcript / checkpoint 立刻迁走到数据库；本期只先把“可迁移语义”钉死

**说人话**：Phase A 的任务很朴素：先把单机架构债还掉。只要这一期做对，本地 sidecar 体验不变，但 Tomcat 就不再建立在“共享 bus 扫描 + 全量会话常驻 + 无明确热度管理”的脆弱假设上。

---

## 先看总图：方案导图集

### 阅读顺序建议

1. **A.1 抽象 ASCII 总图**：先看一次本地 `prompt` 如何从命令进入 run、再进入 mailbox、最后出到 writer。
2. **A.2 具体 ASCII 总图**：再把这条链落到 `event_bus` / `serve` / `ChatContext` / `SessionManager` 的真实文件上。
3. **B 状态机**：最后看热/温/冷与 `running/awaiting_user` 怎么组合，理解为什么 Phase A 已经能让单机容量模型变正确。

### A.1 抽象 ASCII 总图

```text
UI / IDE / 本地 GUI
   │  `prompt` / `follow_up` / `steer`
   ▼
Serve command router
   │
   ├─ 同 session 已有 active run ─► `busy` / queue（仅 follow_up / steer）
   └─ 可受理
   │
   ▼
Residency manager
   ├─ cold / warm ─► hydrate 到 hot slot
   └─ hot_idle  ─► 直接复用
   │
   ▼
Run admission
   ├─ per-session 串行
   └─ global run semaphore
   │
   ▼
AgentLoop.run()
   │
   ├─ 会话本地 EventBus（仅 hook / tool / panel）
   └─ SessionMailbox（主出站）
   │
   ▼
Writer / stdio
   ├─ delta coalesce
   ├─ drop-if-slow（仅增量）
   └─ lifecycle / control 必达
   │
   ▼
本地 UI 收流；turn 结束后会话继续 hot 或按 TTL 降到 warm/cold
```

这张抽象图强调三层隔离。第一层是 **命令受理** 和 **run 真正开跑** 分离，避免一进入 `serve` 就立刻占满运行时。第二层是 **会话本地 hook** 与 **主出站路径** 分离，避免继续依赖共享 `EventBus` 做大规模扇出。第三层是 **会话热度** 与 **run 活跃度** 分离，避免把“会话存在”误当成“会话必须一直占内存”。

**说人话**：一次 `prompt` 真正该走的是“受理 -> 热起来 -> 排到运行位 -> 跑 -> 出流 -> 降温”这条链，而不是“进来就挂一堆 listener、整个 `ChatContext` 一直不放、所有流都走共享总线”。

### A.2 具体 ASCII 总图

```text
┌─ src/api/serve/commands.rs ─────────────────────────────────────────────────────────┐
│ • 继续解析 `ServeCommand`                                                           │
│ • `prompt`/`follow_up`/`steer` 保持现有外部语义                                     │
│ • Phase A：命令不再直接依赖“slot busy = 唯一调度模型”                               │
└──────────────────────────────┬──────────────────────────────────────────────────────┘
                               ▼
┌─ src/api/serve/registry.rs ─────────────────────────────────────────────────────────┐
│ • `ChatContextRegistry` 继续维护 `sessionId -> SessionSlot`                         │
│ • `busy` 仍保留，但只表示“该 session 当前有 active run”                             │
│ • Phase A：新增 hot-state / idle timestamps / residency metadata                    │
└───────────────┬───────────────────────────────┬────────────────────────────────────┘
                │                               │
                ▼                               ▼
┌─ [new] src/api/serve/residency.rs ───────────┐  ┌─ [new] src/api/serve/session_mailbox.rs ───────┐
│ • hot/warm/cold 状态转移                      │  │ • `session_id -> bounded mpsc`                  │
│ • `session_idle_unload_ms` 落地               │  │ • merge delta / warn slow consumer              │
│ • `max_hot_sessions_per_worker`               │  │ • lifecycle/control 帧强保序                    │
└───────────────┬──────────────────────────────┘  └───────────────┬─────────────────────────────────┘
                │                                                  │
                ▼                                                  ▼
┌─ src/api/chat/run_loop/mod.rs + src/core/agent_loop/* ──────────────────────────────┐
│ • `run_chat_turn_with_message()` / `AgentLoop::run()`                                │
│ • Phase A：所有 run 级事件补 `runId` / `parentIds`                                   │
│ • 子 Agent 继续复用 `AgentRegistry`，但不再只靠 `sessionId` 解复用                   │
└───────────────┬──────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─ src/infra/event_bus/mod.rs ─────────────────────────────────────────────────────────┐
│ • 现状：`emit_sync()` 拿 `listeners.write()` 后排序并逐个同步回调                    │
│ • Phase A：改为“注册期有序插入 + emit 期读快照 + 锁外回调 + once 回写收尾”          │
│ • 角色收缩：只服务本会话 / 本 worker 内 hook，不再承担主 fanout                      │
└───────────────┬──────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─ src/api/serve/{event_pump,writer}.rs ───────────────────────────────────────────────┐
│ • `event_pump.rs` 从“每会话 48 listener”收敛为“会话 sink -> mailbox / writer”        │
│ • `writer.rs` 保留单写者、公平轮转、delta coalesce                                   │
│ • Phase A：writer 从直接消费 event_pump 过渡到消费 mailbox 输出                       │
└───────────────┬──────────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─ src/core/session/manager/session_impl.rs ───────────────────────────────────────────┐
│ • 继续使用 `sessions.json` + transcript JSONL                                        │
│ • 但 session 不再因“存在磁盘记录”就默认常驻 `ChatContext`                            │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

这张具体图最关键的新增文件只有两个：`residency.rs` 和 `session_mailbox.rs`。其余大量工作其实都是“调整现有职责边界”：`registry.rs` 不再只是 live slot 名单，`event_bus/mod.rs` 不再是主扇出通道，`event_pump.rs` 不再是“每会话挂一整份 allowlist listener”的核心结构。

**说人话**：Phase A 不是大拆大建，它更像是给现有 `serve` 补两块真正缺失的器官：一个是会话热度管理器，一个是按会话定向发流的收件箱。

### B. 状态机：Phase A 的 hot / warm / cold + run 生命周期

```text
                  prompt / restore
     ┌──────────── hydrate ────────────┐
     ▼                                  │
┌──────────┐    metadata only    ┌────────────┐    admit run     ┌───────────┐
│   cold   │────────────────────▶│    warm    │─────────────────▶│ hot_idle  │
└────┬─────┘                     └─────┬──────┘                  └────┬──────┘
     │                                 │                               │
     │ no subscriber / TTL             │ hot slot available            │ prompt
     │                                 │                               ▼
     │                                 │                        ┌────────────┐
     │                                 │                        │  queued    │
     │                                 │                        └────┬───────┘
     │                                 │                             │ global permit
     │                                 │                             ▼
     │                                 │                       ┌────────────┐
     │                                 │                       │  running   │
     │                                 │                       └────┬───────┘
     │                                 │                            │ ask_question
     │                                 │                            ▼
     │                                 │                      ┌──────────────┐
     │                                 └─────────────────────▶│ awaiting_user │
     │                                                        └────┬─────────┘
     │                                                             │ response / cancel
     └─────────────────────────────────────────────────────────────▼
                                                                  hot_idle
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `cold` | `prompt` / `resume` | `warm` | 装入元数据、checkpoint 索引、最近恢复锚点 | 先把会话“认出来”，不急着把整套 runtime 都拉起来。 |
| `warm` | 抢到 hot slot | `hot_idle` | hydrate `ChatContext` / mailbox / cancel token | 真要跑时再热起来。 |
| `hot_idle` | `prompt` | `queued` | 记录待执行 run，等待全局 permit | 会话热着，不代表立刻就能跑到 CPU/LLM 上。 |
| `queued` | 拿到 permit | `running` | 安装本轮 `runId`、rearm root token、开始事件流 | 真正开始执行的一刻才算 active run。 |
| `running` | `ask_question` / approval | `awaiting_user` | 暂停工具推进，但不卸载 hot state | 等用户拍板时，会话还得保持热的。 |
| `awaiting_user` | `control_response` / `cancel` | `hot_idle` | 恢复或收口当前 run | 回答来了继续，取消了就收尾。 |
| `hot_idle` | 空闲超时 / 热配额回收 | `warm` | 卸载 runtime，保留恢复锚点 | 不常用的热会话要主动降温。 |
| `warm` | 长时无请求 | `cold` | 仅保留持久化事实 | 最后只剩磁盘记录。 |

**说人话**：Phase A 的容量关键不在 `queued` 能排多长，而在 `hot_idle` 和 `running` 总数始终可控。只要 `cold -> warm -> hot` 这条链稳了，Tomcat 就从“按会话总数扩内存”变成“按热点数扩资源”。

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 / 单一事实源 | 行为约束 | 说人话 |
|------|------|----------------------|----------|--------|
| **会话本地 EventBus** | 仍保留的 hook 总线，只服务本 session / 本 worker 内的工具、panel、插件 | `src/infra/event_bus/mod.rs` + `ChatContext::scope_runtime_for()` | 不能再承担主出站 fanout；emit 必须锁外执行回调 | EventBus 继续用，但只做屋内插线板，不做全楼广播。 |
| **SessionMailbox** | 按 `session_id` 归属的有界出站队列 | Phase A 新增 `session_mailbox.rs` | 同 session 顺序必须稳定；delta 可合并，lifecycle/control 不可丢 | 每个会话自己的快递柜。 |
| **Hot slot** | 持有完整 `ChatContext`、mailbox、cancel token 的热运行位 | `registry.rs` + `residency.rs` | 数量受 `max_hot_sessions_per_worker` 约束 | 真正贵的是 hot slot，不是 session 条目。 |
| **Warm state** | 无完整 `AgentLoop`，但可快速恢复的中间态 | `residency.rs` + session metadata + checkpoint 索引 | 允许保留订阅、最近恢复锚点；不占完整 runtime | 半醒着，随时能叫醒。 |
| **Run permit** | 允许某个 queued run 真正进入 `AgentLoop.run()` 的执行许可 | Phase A scheduler / semaphore | 受全局并发约束；不同 session 竞争同一池子 | 跑 Agent 的上场名额。 |
| **`runId`** | 单次 run 的稳定标识 | Phase A 升级后的 event envelope | 一次 run 全程固定；与 `sessionId` 解耦 | 这次回合的身份证。 |
| **`parentIds`** | 表示本 run 在父子执行树中的路径 | reviewer / verifier / future child runs | 根 run 为空数组；子 run 从父树继承并追加 | 这轮是谁生出来的，必须能追。 |
| **Backpressure policy** | 当 mailbox / writer 过载时如何保序、合并、丢弃、告警 | `writer.rs` 现状 + Phase A mailbox 策略 | 只允许丢 best-effort delta，禁止丢 lifecycle / control | 堵车时先少发碎字，别把“结束了”这种大事丢了。 |
| **Zero-regression local stdio** | 对本地 `ServeCommand` / `OutFrame` 主形状、初始化握手、现有 GUI 事件流的兼容约束 | `src/api/serve/types.rs`、现有 `serve/tests/*` | Phase A 不得要求扩展端重写协议 | 本地用户不该感觉自己被拿去试验云化。 |

**说人话**：Phase A 最关键的词有三个：`mailbox`、`hot slot`、`run permit`。它们分别代表“怎么发流”“什么东西真的占内存”“什么东西真的占执行位”。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 聚焦本期的借鉴表

| 竞品 / 仓库 | 本期关注点 | 关键设计 | 我们借鉴的点 | 说人话 |
|-------------|------------|----------|--------------|--------|
| **codex** | 每会话队列与会话串行域 | `core/src/session/mod.rs` 的 SQ/EQ、`app-server/src/request_serialization.rs` 的 per-thread 串行域 | `SessionMailbox`、同 session 串行、跨 session 并行 | 会话就该有私有队列，而不是抢一个大喇叭。 |
| **LangChain** | 事件级身份与并发上限 | `langchain_core/tracers/event_stream.py`、`runnables/utils.py` | `runId/parentIds`、semaphore 风格的并发门 | 海量并发时，事件必须能分清“这是哪一轮的”。 |
| **LangGraph** | hydrate-on-demand | `pregel/_loop.py` 与 checkpoint saver 语义 | `hot/warm/cold` 的恢复心智模型 | 状态可以不住内存，但必须能随叫随到。 |
| **OpenClaw** | 慢消费者治理 | `src/gateway/server-broadcast.ts` 的 `dropIfSlow`、节流与断慢连 | delta 合并、slow-consumer notice、lossless/best-effort 分层 | 不是所有流都值得拼命保。 |

### 2.2 为什么 Phase A 不先上 gateway / DB

1. **本期最大瓶颈在本机结构，不在网络。** `src/infra/event_bus/mod.rs` 的 `emit_sync()` 与 `src/api/chat/context.rs` 的完整 runtime 装配，决定了 Tomcat 还没到“先上 gateway 才能继续推进”的阶段。
2. **现有本地 `serve` 已经有足够多的可复用基础设施。** `src/api/serve/writer.rs` 的单写者、公平轮转、delta 合并，以及 `src/api/serve/registry.rs` 的多会话路由，都说明 Phase A 可以先在本地把容量模型改对。
3. **不先修本地热/冷与 mailbox，就算网关提前做出来，也只是把单机架构债变成分布式架构债。**

### 2.3 Phase A 的四条定性结论

1. `EventBus` 在本期不删除，但必须从“主扇出通道”降级为“本地 hook 通道”。
2. `writer` 在本期不重写，但必须从“直接消费 event_pump listener”升级为“消费 mailbox 出站”。
3. `session_idle_unload_ms` 在本期必须从 TODO 变成真实行为，否则热/冷分层只是口号。
4. `runId/parentIds` 在本期必须进入事件信封，否则后面 B/C 做恢复和多租户时事件无法精确归因。

**说人话**：本期借竞品，借的是结构，不是全套产品形态。最像的方向是 codex 的私有会话队列，最像的恢复心智是 LangGraph，最像的慢消费者处理是 OpenClaw，最像的事件身份证是 LangChain。

---

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

`§3.1` 只回答“为什么这样定”；`§3.2` 只回答“这一期到底改哪、怎么验”。二者刻意分开，避免“理由”和“落点”混成一张表后可执行性变差。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **A1 EventBus 角色** | `EventBus` 在 Phase A 里是删掉、保留还是降级 | **采用** 保留 `EventBus`，但把它降级为“会话本地 hook 总线”，主出站改走 `SessionMailbox`；**拒绝** 继续让它承担主 fanout。 | 本仓：`tomcat/src/infra/event_bus/mod.rs`、`tomcat/src/api/chat/context.rs`；外部：`codex-rs/core/src/session/mod.rs`、`openclaw/src/gateway/server-broadcast.ts` | 设计：保留 `ScopedEventEmitter` 与现有 panel/plugin 接缝，但主出站从 bus 转为 mailbox；理由：能最小化对 `AgentLoop` 与插件生态的扰动，同时收掉共享广播的复杂度。 | 未入选：彻底删除 EventBus；拒因：会把 ask_question、plugin hook、现有 CLI 订阅全部一起翻新，Phase A 风险过大。 | 先把总线从“主干道”降成“会话内支线”，比全拆更稳。 |
| **A2 `emit_sync` 执行模型** | 现有 `emit_sync()` 是否还能接受 | **采用** “注册期有序插入 + emit 期读快照 + 锁外回调 + once 收尾”；**拒绝** 继续在 `emit_sync()` 里拿写锁排序并同步执行回调。 | 本仓：`tomcat/src/infra/event_bus/mod.rs`、`tomcat/src/infra/event_bus/tests/suite_test.rs`；外部：`codex-rs/app-server/src/outgoing_message.rs`、`langchain/libs/core/langchain_core/runnables/utils.py` | 设计：emit 时最多持读锁拿快照，真正回调在锁外；理由：当前写锁跨回调天然把慢 writer / 慢 plugin 带进尾延迟，且每次 sort 都在热路径上。 | 未入选：只给当前实现加更多 `warn` / metrics；拒因：能看见问题，但不能治问题。 | 热路径上的锁要尽量短，慢订阅者不能拖住所有人。 |
| **A3 事件身份** | 事件是否继续只带 `sessionId` | **采用** 所有 run 级事件补 `runId`，子 run / 子 agent 事件补 `parentIds`；**拒绝** 只靠 `sessionId` 解复用。 | 本仓：`tomcat/src/infra/events/mod.rs`、`tomcat/src/api/chat/run_loop/mod.rs`；外部：`langchain/libs/core/langchain_core/tracers/event_stream.py`、`langchain/libs/core/langchain_core/runnables/schema.py` | 设计：保持现有 `sessionId` 不变，在 envelope 上增补 run 树字段；理由：Phase A 虽仍是本地单机，但后续恢复、压测、审计与 reviewer/verifier 归因都提前依赖这些字段。 | 未入选：等 B/C 再补；拒因：那会让 Phase A 产生一批未来要推翻的事件与测试快照。 | 现在就把“哪一轮、谁的孩子”贴清楚，后面少返工。 |
| **A4 主扇出通路** | `event_pump` 是否继续“每会话一整份 allowlist listener” | **采用** 收敛成“会话 sink -> mailbox -> writer”，保留 allowlist 但语义降级为“哪些事件允许出网”；**拒绝** 继续按 48 个事件名逐个注册 listener。 | 本仓：`tomcat/src/api/serve/event_pump.rs`、`tomcat/src/api/serve/writer.rs`；外部：`codex-rs/core/src/session/mod.rs`、`openclaw/src/gateway/server-chat.ts` | 设计：allowlist 保留，但只在 sink 汇总出口判断；理由：继续按事件名逐个注册 listener 会让 slot 生命周期、bus 注册和出站路径耦得太深。 | 未入选：只缩短 allowlist；拒因：会减少数量，不会改变结构。 | 白名单还要，但它该是出口规则，不该是注册风暴。 |
| **A5 会话驻留** | 是否在本期真正启用 `session_idle_unload_ms` | **采用** 本期落地 `session_idle_unload_ms`、引入 `max_hot_sessions_per_worker` 与 warm/cold 恢复；**拒绝** 把热/冷分层留到 B/C。 | 本仓：`tomcat/src/infra/config/types/runtime.rs`、`tomcat/src/api/serve/registry.rs`；外部：`langgraph/libs/langgraph/langgraph/pregel/_loop.py`、`codex-rs/app-server/src/request_processors/thread_lifecycle.rs` | 设计：本期即允许热会话自动降温，不再默认全部 `ChatContext` 常驻；理由：这是几十万 session 是否可能的前置条件，与 gateway / DB 无关。 | 未入选：先只做 mailbox，后面再做热/冷；拒因：扇出问题解了，内存问题仍会先炸。 | 不先会降温，再好的扇出也救不了 OOM。 |
| **A6 调度与背压** | 本期是否要做全局并发门与慢消费者治理 | **采用** `run permit semaphore + per-session serialization + delta coalesce/drop notice`；**拒绝** 继续把 writer 缓冲当唯一背压点。 | 本仓：`tomcat/src/api/serve/writer.rs`、`tomcat/src/core/agent_registry/mod.rs`；外部：`langchain/libs/core/langchain_core/runnables/utils.py`、`openclaw/src/gateway/server-broadcast.ts` | 设计：run 开跑前先过 permit，writer/mailbox 只负责出站压力；理由：不把 admission 和 delivery 分层，最终要么压死 AgentLoop，要么压爆 writer 内存。 | 未入选：无限缓冲、只在 writer 满时硬丢；拒因：会把上游压测结果变成随机性很强的偶发故障。 | 先控制“同时跑多少”，再控制“输出堆多深”，两道门缺一不可。 |
| **A7 本地兼容性** | 本期是否允许扩展端改协议配合 | **采用** 本地 `stdio` 主形状零回退：`ServeCommand` / `ControlFrame` 不破坏、`busy` 语义保留、`agent_idle` 继续是 ready 权威信号；**拒绝** 为了云化先破现有本地 UI。 | 本仓：`tomcat/src/api/serve/types.rs`、`tomcat/src/api/serve/tests/commands_test.rs`；外部：`codex-rs/app-server/src/in_process.rs`、`pi_agent_rust/src/rpc.rs` | 设计：内部换扇出与热度管理，外部协议尽量只增补字段不重构形状；理由：本期目标是“本地变快、结构变对”，不是“逼扩展端同步大改”。 | 未入选：引入全新 Phase A 协议；拒因：会把应由架构层承担的复杂度转嫁到 UI 适配层。 | 这期要用户几乎无感，复杂度应该留在服务端内部。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PA-1 会话本地 EventBus + 快照发射** | `emit_sync()` 改快照/锁外回调；`EventBus` 角色收缩为会话本地 hook | `src/infra/event_bus/mod.rs`、`src/infra/event_bus/tests/*` | 新增 `infra::event_bus::tests::emit_sync_snapshot_callbacks_do_not_hold_write_lock` | 先把总线热路径变短，别被慢回调卡住。 |
| **PA-2 事件信封升级** | 所有 run 级事件补 `runId` / `parentIds`；schema 与 fixture 更新 | `src/infra/events/mod.rs`、`src/api/serve/types.rs`、`src/api/serve/tests/schema_test.rs` | 新增 `api::serve::tests::schema_test::serve_schema_includes_run_identity_fields` | 把事件身份证补齐。 |
| **PA-3 SessionMailbox + 单 sink** | 新增 mailbox；`event_pump` 从 48 listener 收敛为会话 sink；writer 消费 mailbox 输出 | `[new] src/api/serve/session_mailbox.rs`、`src/api/serve/{event_pump,writer}.rs` | 现有 `serve_writer_round_robins_across_sessions` + 新增 `session_mailbox_routes_without_global_listener_scan` | 每会话一个收件箱，出流从这儿走。 |
| **PA-4 热/温/冷落地** | 激活 `session_idle_unload_ms`；新增 `max_hot_sessions_per_worker`；建立热度迁移与 rehydrate | `[new] src/api/serve/residency.rs`、`src/api/serve/registry.rs`、`src/api/chat/context.rs`、`src/infra/config/types/runtime.rs` | 新增 `idle_session_unloads_and_rehydrates` | 会话终于能降温和叫醒。 |
| **PA-5 全局 run permit + 背压归一化** | worker 级并发门、admission queue、delta 合并/告警、lifecycle/control lossless | `[new] src/api/serve/scheduler.rs`、`src/api/serve/writer.rs`、`src/core/agent_registry/mod.rs` | 现有 `serve_writer_backpressure_notice_emitted_once` + 新增 `queued_run_waits_for_global_permit` | 同时跑多少、同时堆多少，都得有硬门。 |
| **PA-6 本地 stdio 零回退门禁** | 确保 `prompt`/`busy`/`interrupt`/`initialize`/schema/GUI 兼容性不回退 | `src/api/serve/tests/*`、相关 GUI / schema fixture | 现有 `serve_same_session_second_prompt_is_busy`、`serve_initialize_control_request_sets_ready_state`、`serve_emitted_event_validates_against_generated_schema` | 本期再怎么改，现有本地体验不能坏。 |

#### 3.2.1 PA-1：会话本地 EventBus + 快照发射

Phase A 不删除 `EventBus`，而是把它从“主扇出通道”改成“本会话内 hook 通道”。这意味着 `emit_sync()` 必须从当前的“拿写锁、sort、逐个同步回调”改成“读快照、锁外执行、必要时回写 once 清理”。这一步完成后，即便还有插件或 panel 很慢，也不会再把所有 emit 热路径一起卡住。

#### 3.2.2 PA-2：事件信封升级

`sessionId` 继续保留，因为它是会话身份；但从 Phase A 开始，每个 run 级事件都必须稳定带上 `runId`，子 run 相关事件还要带 `parentIds`。这一步的价值不只是未来云端恢复，也直接提升本地排障、压测和 UI 调试的可归因性。

#### 3.2.3 PA-3：SessionMailbox + 单 sink

当前 `event_pump.rs` 把 allowlist 展开成 48 个事件名，每个会话都要注册一整份 listener。Phase A 改成“会话 sink + mailbox”后，allowlist 仍然保留，但只在汇总出口判断哪些事件允许出网，不再变成 listener 注册数量的乘法器。

#### 3.2.4 PA-4：热/温/冷落地

本期必须让 `session_idle_unload_ms` 从注释变成行为，并新增 `max_hot_sessions_per_worker`。热度管理器负责决定何时 hydrate、何时 unload、何时因为 hot 配额不足而让 warm session 等待，不再让 `ChatContext` 是否存在取决于“这个会话是不是曾经创建过”。

#### 3.2.5 PA-5：调度与背压

`busy` 仍然保留，但语义更窄：它只表示“该 session 当前已有 active run”。真正的 worker 级调度由 `run permit semaphore` 决定，真正的出站压力由 mailbox / writer 决定。这样 admission 和 delivery 两段压力不会再互相污染。

---

## 4. 协议

Phase A 不新增新命令类型，但会升级出站事件 envelope，并把慢消费者语义从“纯内部实现”升级为“可观测协议行为”。

### 4.1 事件字段表

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionId` | `string` | 是 | 无 | 全部出站事件 | 延续现状；会话稳定主键 | 这条流属于哪个会话。 |
| `runId` | `string` | Phase A 起对 run 级事件必填 | 无 | `agent_start`、`message_*`、`tool_execution_*`、`agent_end` | 标识本次执行实例 | 这次回合的身份证。 |
| `parentIds` | `string[]` | 根 run 可空数组；子 run 必填 | `[]` | `sub_agent_*`、future child-run events | 父子 run 树路径 | 这次回合是谁生出来的。 |
| `type` | `string` | 是 | 无 | 全部帧 | 继续复用现有 `AgentEvent` / `WireEvent` 类型字面量 | 事件名不重做，只补身份字段。 |
| `finishReason="backpressure"` | `string` | 仅 `llm_notice` 时出现 | 无 | 慢消费者 / delta 丢弃提示 | 显式告诉 UI：这次是背压告警，不是模型自然停下 | 告诉前端“我不是正常结束，我是在减压”。 |

### 4.2 协议约束

1. `ServeCommand`、`ControlFrame`、`ResponseFrame` 主形状 **不变**。
2. `busy` 语义保留：同一 session 第二个 `prompt` 仍返回 `busy`，不改成静默排队。
3. `follow_up` / `steer` 的既有排队/控制语义保留。
4. `agent_idle` 继续是“该 session 可再次发送”的权威信号。
5. `lifecycle` / `control` 帧在任何背压策略下都必须 lossless。

### 4.3 样例

```jsonc
// Phase A 事件：仅示意新增 identity 字段
{
  "type": "tool_execution_start",
  "sessionId": "s-1",
  "runId": "run-42",
  "parentIds": [],
  "toolCallId": "call-3",
  "toolName": "read_file"
}

// 背压告警：只对 best-effort delta 生效
{
  "type": "llm_notice",
  "sessionId": "s-1",
  "runId": "run-42",
  "parentIds": [],
  "finishReason": "backpressure",
  "message": "serve writer dropped message deltas under backpressure"
}
```

单一事实源：

- 现状命令/控制/响应：`tomcat/src/api/serve/types.rs`
- 现状事件类型：`tomcat/src/infra/events/mod.rs`
- Phase A mailbox / residency 内部契约：`[new] src/api/serve/session_mailbox.rs`、`[new] src/api/serve/residency.rs`

**说人话**：本期协议改动的原则非常克制：外部命令几乎不动，只把出站事件补齐“这是谁、属于哪一轮、背压发生了什么”。这样本地 UI 不需要大手术，但后续扩展性会大很多。

---

## 5. 文件职责总览（One-Glance Map）

```text
┌─ src/api/serve/commands.rs ─────────────────────────────────────────────────────────┐
│ • 命令受理：`prompt` / `follow_up` / `steer` / `interrupt`                          │
│ • 本期：接入 residency + run admission，不改外部命令主形状                         │
└──────────────────────────────┬──────────────────────────────────────────────────────┘
                               ▼
┌─ src/api/serve/registry.rs ─────────────────────────────────────────────────────────┐
│ • `ChatContextRegistry` / `SessionSlot`                                             │
│ • 本期：增加 hot metadata、idle timestamps、residency hooks                         │
└──────────────┬─────────────────────────────┬────────────────────────────────────────┘
               │                             │
               ▼                             ▼
┌─ [new] src/api/serve/residency.rs ────────┐   ┌─ [new] src/api/serve/scheduler.rs ───────┐
│ • hot/warm/cold 状态机                     │   │ • global run permit semaphore              │
│ • `session_idle_unload_ms`                 │   │ • queued → running admission               │
│ • `max_hot_sessions_per_worker`            │   │ • 与 `busy` 语义分离                       │
└──────────────┬────────────────────────────┘   └──────────────┬───────────────────────────┘
               │                                               │
               └──────────────────────┬────────────────────────┘
                                      ▼
┌─ src/api/chat/run_loop/mod.rs + src/core/agent_loop/* ──────────────────────────────┐
│ • 继续唯一业务主链                                                                  │
│ • 本期：run identity、permit 进入点、完成后回写 hot state                           │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/infra/event_bus/mod.rs ─────────────────────────────────────────────────────────┐
│ • `ScopedEventEmitter` 保留                                                         │
│ • 本期：emit 快照、锁外回调、局部 hook 总线                                         │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ [new] src/api/serve/session_mailbox.rs ─────────────────────────────────────────────┐
│ • `session_id -> bounded mpsc`                                                       │
│ • delta coalesce / slow-consumer notice / lossless 分类                              │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/api/serve/{event_pump,writer}.rs ───────────────────────────────────────────────┐
│ • `event_pump`：会话 sink / allowlist                                               │
│ • `writer`：单写者、公平轮转、flush 到 stdio                                         │
│ • 本期：writer 消费 mailbox 出站而不是直接靠多 listener fanout                       │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/infra/config/types/runtime.rs ──────────────────────────────────────────────────┐
│ • 激活 `session_idle_unload_ms`                                                     │
│ • 新增 `max_hot_sessions_per_worker` / `mailbox_capacity` / `max_concurrent_runs`   │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ tests: src/api/serve/tests/* + src/infra/event_bus/tests/* ────────────────────────┐
│ • 零回退回归 + mailbox / residency / backpressure / identity 新增用例               │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

阅读顺序建议：先看 `commands/registry/residency/scheduler` 这四个“会话能不能热起来、能不能进入 run”的入口，再看 `run_loop/event_bus/mailbox/writer` 这条“run 期间事件怎么出流”的链，最后看 `runtime config` 和 tests。这样一眼就能分清 Phase A 究竟是在改“会话生命周期”，还是在改“事件热路径”。

**说人话**：Phase A 的代码变化看起来分散，但实际上只有两条主链：一条是“会话怎么热起来和降温”，一条是“事件怎么不再靠共享广播发出去”。

---

## 6. 配置与环境变量

总则：**env > config > 默认**。

| 变量 / 配置 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `[serve].max_hot_sessions_per_worker` | `usize` | 热会话上限 | env / config | 真正限制内存的硬阈值。 |
| `[serve].session_idle_unload_ms` | `u32` | 空闲多久降到 warm/cold | env / config | 热会话多久不动就降温。 |
| `[serve].mailbox_capacity` | `usize` | 单 session mailbox 容量 | env / config | 每个会话快递柜多大。 |
| `[serve].max_concurrent_runs_per_worker` | `usize` | worker 同时 active run 数 | env / config | 同时真开跑多少轮。 |
| `[serve].delta_coalesce_ms` | `u32` | delta 合并窗口 | env / config | 碎字合并的时间窗。 |
| `[serve].max_buffered_frames` | `usize` | writer / mailbox 缓冲上限 | env / config | 输出堆多深。 |
| `[serve].turn_queue_capacity` | `usize` | 待 admit run 队列上限 | env / config | 还没跑起来的待执行槽位。 |
| `[serve].max_sessions` | `usize` | 兼容旧字段；Phase A 起视为 `max_hot_sessions_per_worker` 的别名 / 迁移入口 | config | 旧名字先不删，但意思要变清楚。 |

**说人话**：本期最重要的三个配置不是 transport、不是 schema，而是 `max_hot_sessions_per_worker`、`session_idle_unload_ms`、`max_concurrent_runs_per_worker`。它们分别控制“热内存、降温节奏、执行位”。

---

## 7. 错误模型 / 背压归一化

```text
命令受理
  prompt
    ├─ session busy（同 session 已 running） ─► response.error("busy")
    ├─ hot slot 不足 / permit 不足          ─► accepted + queued（内部等待）
    └─ 正常                                 ─► running

出站背压
  mailbox / writer 满
    ├─ delta within coalesce window ─► merge
    ├─ delta beyond limit            ─► drop + llm_notice(backpressure)
    └─ lifecycle/control             ─► must deliver

恢复异常
  hydrate fail
    └─ terminal event + metrics + stay warm/cold
```

| 结局 | 触发条件 | 对外形态 | 说人话 |
|------|----------|----------|--------|
| `busy` | 同一 session 已有 active run，且命令是第二个 `prompt` | `response.error("busy")` | 保持现有本地语义，不暗中排第二个 prompt。 |
| `queued` | session 可受理，但暂时没拿到 hot slot 或 global permit | `response.ok({accepted:true})`，实际 run 稍后开始 | 对用户看起来仍是“已接单”，只是晚一点真开跑。 |
| `backpressure notice` | delta 被合并后仍超出缓存上限 | `llm_notice{finishReason:"backpressure"}` | 告诉前端：不是没输出，而是系统主动减压了。 |
| `hydrate_failed` | cold/warm → hot 恢复失败 | terminal error + metrics | 会话叫不醒要明确失败，不静默卡死。 |
| `idle_unload` | hot session 达到空闲阈值 | internal transition + metrics | 这是正常行为，不是错误。 |

**说人话**：Phase A 里最容易误解的一点是“排队”和“忙”。`busy` 仍只代表“同一个会话已经在跑”；而 `queued` 代表“系统接单了，但还在等热位或运行位”。这两个语义不能混。

---

## 8. 测试矩阵（验收）

| 层级 | 目标 | 锚点（测试函数名 / 文件） | 状态 | 说人话 |
|------|------|---------------------------|------|--------|
| 单元 | 现有 EventBus 基线：panic / Err 不影响其他 listener | `infra::event_bus::tests::single_listener_error_does_not_abort_others` | ✅ 2026-07-15 | 先守住老行为。 |
| 单元 | 现有 `sessionId` 自动写入不回退 | `infra::event_bus::tests::scoped_event_emitter_writes_session_id_to_payload_and_context` | ✅ 2026-07-15 | 老标签不能丢。 |
| 单元 | emit 改快照后不再持写锁跨回调 | `infra::event_bus::tests::emit_sync_snapshot_callbacks_do_not_hold_write_lock` | PENDING | Phase A 热路径核心用例。 |
| 单元 | mailbox 路由不扫描共享 bus listener | `api::serve::tests::mailbox_test::session_mailbox_routes_without_global_listener_scan` | PENDING | 定向投递是否真成立。 |
| 单元 | mailbox 满时仅丢 delta 且只发一次告警 | `api::serve::tests::mailbox_test::mailbox_drops_best_effort_delta_and_emits_notice_once` | PENDING | 慢消费者治理是否可控。 |
| 集成 | 本地同 session 第二个 prompt 仍是 `busy` | `api::serve::tests::commands_test::serve_same_session_second_prompt_is_busy` | ✅ 2026-07-15 | 零回退门禁。 |
| 集成 | 现有 writer 公平轮转仍成立 | `api::serve::tests::writer_test::serve_writer_round_robins_across_sessions` | ✅ 2026-07-15 | 单写者公平性不能退。 |
| 集成 | 现有 lifecycle 事件不被别的 session 挤掉 | `api::serve::tests::event_pump_test::serve_lifecycle_events_not_dropped_for_other_sessions` | ✅ 2026-07-15 | 关键收口帧必须活着。 |
| 集成 | 热会话空闲后自动 unload，再次 prompt 可 rehydrate | `api::serve::tests::residency_test::idle_session_unloads_and_rehydrates` | PENDING | 热/冷分层主验收。 |
| 集成 | run identity 在 root / reviewer / verifier 事件树中稳定传播 | `api::serve::tests::event_identity_test::run_tree_fields_roundtrip_across_subagents` | PENDING | `runId/parentIds` 的主验收。 |
| 集成 | run permit 生效时 queued run 能在 permit 释放后继续 | `api::serve::tests::scheduler_test::queued_run_waits_for_global_permit` | PENDING | admission queue 是否真工作。 |
| E2E / 回归 | 初始化握手、schema、控制通道不回退 | `api::serve::tests::control_test::serve_initialize_control_request_sets_ready_state`、`api::serve::tests::schema_test::serve_emitted_event_validates_against_generated_schema` | ✅ 2026-07-15 | 本地 UI 入口不能坏。 |
| 压测 | `10^4` 冷会话 + 数百热会话下 p99、内存、水位稳定 | `tests/cloud_scale_serving_phase_a_load.rs::cold_10k_hot_300_memory_and_tail_budget` | PENDING | 本期容量结论最终得看压测。 |

**说人话**：Phase A 的验收不是“写出了 mailbox 这个词”，而是三件事同时成立：本地行为没退、热路径确实缩短、热/冷分层真的能把会话总数和热内存解耦。

---

## 9. 风险与应对

| 风险 | 影响 | 应对策略 | 说人话 |
|------|------|----------|--------|
| 只加 mailbox，不改 `emit_sync()` | 慢回调仍拖尾延迟，收益打折 | PA-1 与 PA-3 必须同批推进 | 只修出口，不修入口，还是堵。 |
| 只做 unload，不把 hydrate 设计清楚 | 会话降温后唤不醒 | warm state 先保留恢复锚点，hydrate 失败要显式可见 | 只会睡，不会醒，系统就废。 |
| `runId` 晚引入 | B/C 要改 event schema、fixture、UI 解复用两遍 | 本期先补身份字段，后面只扩不改 | 现在偷懒，后面加倍返工。 |
| 热配额太低导致频繁抖动 | 反复 hydrate/unload，尾延迟飙高 | 暴露 hot metrics，允许按 worker 调节阈值 | 会话别刚热起来就被冻回去。 |
| 背压策略太激进 | UI 内容断裂、用户误判模型输出 | 仅允许 best-effort delta 丢弃，且必须发 notice | 减压可以，但要让前端知道发生了什么。 |
| 为了云化把本地命令语义改了 | 扩展端回归风险大 | 零回退门禁挂在现有 serve tests 和 schema tests 上 | 本期再重要，也不能拿本地用户开刀。 |

**说人话**：Phase A 最怕做成“局部 patch 大杂烩”。正确姿势是同时把 `EventBus` 热路径、`mailbox` 主扇出、热/冷分层和调度背压这四件事一起钉住，少一件都会让这期收益打折。

---

## 10. 历史决策 / 跨文档修订

1. 本册只负责单机收口，不抢 `03` 的 gateway / storage trait 叙事，也不抢 `04` 的多租户 / cluster 叙事。
2. `session_idle_unload_ms` 在现状配置里已存在但尚未生效；Phase A 明确把它从“占位字段”提升为真实行为。
3. `EventBus` 在本册被重新定位为“本地 hook 总线”，这不是否定既有设计，而是把它放回更适合的职责边界。

**说人话**：Phase A 结束后，Tomcat 仍然是本地 sidecar，但它已经不再是那种“只要会话多一点就结构上不成立”的 sidecar。
