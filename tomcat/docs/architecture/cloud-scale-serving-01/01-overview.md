# Tomcat 云端规模化 Serving 总览：从桌面侧车到多租户云 Agent

> 适用范围：`tomcat` 从当前本地 `serve` 单进程模式，演进到可承载几十万到几百万并发会话的云端多租户 Agent Serving 体系。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。
> 关联文档：[`../agent-server-and-ui-gateway.md`](../agent-server-and-ui-gateway.md)、[`../multi-agent.md`](../multi-agent.md)、[`../session-storage.md`](../session-storage.md)、[`../chat-resume-hydration.md`](../chat-resume-hydration.md)、[`../work-dir-and-data-layout.md`](../work-dir-and-data-layout.md)。
>
> 本文回答五件事：
>
> 1. **为什么现在的 `tomcat serve` 扛不住几万到几十万会话？** 因为它是桌面侧车，不是云端控制面。
> 2. **真正的云端 Agent Serving 该长什么样？** 要把会话、执行、存储、事件和沙箱分层拆开。
> 3. **为什么不能只优化 `EventBus`？** 因为问题不只是广播复杂度，而是产品模型错位。
> 4. **怎么从当前代码平滑演进过去？** 按 Phase A / B / C 三期拆，先还单机架构债，再做多 worker，再做集群。
> 5. **跟 codex / LangGraph / OpenClaw / LangChain 比，Tomcat 应该借什么、不借什么？** 本文会给出明确裁决。

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象 ASCII 总图**：抓住“会话、网关、worker、存储、沙箱”五层职责。
2. 再看 **A.2 具体 ASCII 总图**：把目标架构落到 Tomcat 的当前模块与未来代码落点。
3. 再看 **B 状态机**：理解一个会话如何在 `cold / warm / hot` 之间迁移，以及 turn 如何排队、运行、等待审批、回放恢复。
4. 接着看 **§2 调研**：确认这不是拍脑袋，而是吸收了 codex、LangGraph、OpenClaw、LangChain 的成熟经验。
5. 最后看 **§3 已定稿选型**：这里是本文真正的裁决结果，也是后续 02–07 分册的边界来源。

### A.1 抽象 ASCII 总图

```text
┌──────────────────────────── 客户端与租户边界 ────────────────────────────┐
│ Web / IDE / Remote SDK / CLI                                            │
│ ① 发命令：start / follow_up / interrupt / subscribe(sessionKey)         │
│ ② 收事件：message_delta / lifecycle / control / metrics                 │
└───────────────────────────────┬──────────────────────────────────────────┘
                                │
                                ▼
┌──────────────────────── Gateway 控制面 ──────────────────────────────────┐
│ 职责：鉴权、租户配额、连接复用、订阅表、sessionKey→worker 路由、回放入口 │
│ 不跑 AgentLoop；只管“谁该去哪里、谁该收到什么、什么时候该拒绝/排队”      │
└───────────────┬───────────────────────────────┬─────────────────────────┘
                │                               │
                │ 命令面                        │ 事件面
                ▼                               ▲
┌──────────────────────── Worker 数据面 ───────────────────────────────────┐
│ 少量热会话驻留：                                                         │
│   sessionKey → SessionMailbox → AgentLoop / Tool Runtime / SandboxLease │
│                                                                         │
│ 原则：                                                                   │
│ - 跨 session 真并发                                                     │
│ - 同 session 串行                                                       │
│ - 事件只按 sessionKey 定向投递                                           │
│ - 背压先在 mailbox，再在 gateway                                         │
└───────────────┬───────────────────────────────┬─────────────────────────┘
                │                               │
                │ 读写状态                       │ 沙箱快照 / 大对象
                ▼                               ▼
┌────────────────────── 持久化层 ──────────────────────┐  ┌───────────────┐
│ SessionCatalog / Transcript / Plan / Todo / Checkpoint │  │ Blob/Object   │
│ 冷会话只占存储，不占 AgentLoop、连接、listener         │  │ tool-results   │
│ Worker 随时可 hydrate / evict / replay                 │  │ workspace diff │
└───────────────────────────────────────────────────────┘  └───────────────┘
                │
                ▼
┌──────────────────────── 每会话沙箱层 ────────────────────────────────────┐
│ 容器 / microVM / gVisor + overlayfs workspace + egress policy           │
│ bash/read/write/edit 不再直接打到宿主共享文件系统                         │
└──────────────────────────────────────────────────────────────────────────┘
```

读图导读（说人话）：这张图故意把系统切成五层，因为云端 Serving 的问题本质上不是“多开几个 `AgentLoop`”，而是“怎样让**只有少数热会话**占 CPU/内存，怎样让**海量冷会话**只占便宜的存储，怎样让**事件只发给真正订阅者**，以及怎样让**每个会话的执行环境互相隔离**”。如果继续沿用现在的“一个 `serve` 进程、一张 EventBus、一堆 per-session listener”的心智模型，优化到再细也只是在桌面侧车上贴创可贴。

### A.2 具体 ASCII 总图

```text
当前 Tomcat（已落地）                                   目标 Tomcat Cloud（本方案）
────────────────────────────────────────────────────────────────────────────────────
UI/IDE
  │  spawn `tomcat serve --stdio`
  ▼
src/api/serve/mod.rs
  ├─ ChatContextRegistry(sessionId -> SessionSlot)
  ├─ writer.rs (单 stdout writer)
  ├─ event_pump.rs (每 session 注册 48 白名单 listener)
  ├─ commands.rs (按 sessionId 路由命令)
  └─ control.rs / ask_question.rs
       │
       ▼
src/infra/event_bus/mod.rs
  └─ DefaultEventBus
     HashMap<event_name, Vec<listener>>
     emit_sync = write lock + sort + iterate all listeners
       │
       ▼
src/core/agent_loop/*
  └─ 每 turn 新建 AgentLoop
       │
       ▼
src/core/session/*
  ├─ sessions.json
  ├─ *.jsonl transcript
  ├─ *.resume-index.json
  ├─ tool-results/
  └─ plans / todos / checkpoints

──────────────────────── 目标演进边界 ────────────────────────

Client / SDK / IDE / Web
  │  HTTP command / stdio compatibility / WS subscribe
  ▼
src/cloud/gateway/*
  ├─ auth.rs               鉴权、tenant、sessionKey 归属
  ├─ subscriptions.rs      conn -> {sessionKey}
  ├─ placement.rs          sessionKey -> worker
  ├─ replay.rs             reconnect / seq catch-up
  └─ admission.rs          配额 / queue / drain
       │
       ▼
src/cloud/worker/*
  ├─ session_runtime.rs    hot/warm/cold 生命周期
  ├─ session_mailbox.rs    sessionKey -> bounded channel
  ├─ turn_scheduler.rs     queued / running / interrupt policy
  ├─ event_sink.rs         AgentLoop -> mailbox -> gateway
  └─ sandbox_lease.rs      每会话沙箱租约
       │
       ▼
src/cloud/storage/*
  ├─ session_catalog.rs    Postgres / SQLite
  ├─ transcript_store.rs   append / replay / compaction
  ├─ blob_store.rs         tool-results / workspace diff
  ├─ checkpoint_store.rs   durable checkpoint / pending writes
  └─ plan_store.rs         plan / todo / ask_question 持久化
       │
       ▼
src/cloud/sandbox/*
  ├─ provider.rs           gVisor / Firecracker provider trait
  ├─ workspace.rs          overlayfs / snapshot / restore
  └─ egress_policy.rs      net_guard policy + sandbox net enforcement
```

读图导读（说人话）：左边是当前代码的真实落点，右边是建议新增的模块边界。最重要的变化只有三条。第一，`EventBus` 不再承担“跨会话广播给 UI”的职责，它只保留为**worker 内钩子和观测总线**。第二，新增 `session_mailbox` 和 `turn_scheduler`，把“会话路由”和“执行调度”从 `commands.rs` 的即时分发里抽出来。第三，持久化与沙箱被抬成一级模块，不再让 `ChatContext` 和本机目录同时扮演“会话内存”“会话磁盘”“会话执行环境”三种身份。

### B. 状态机：会话热度与 turn 生命周期

```text
             open/subscribe
   ┌───────┐───────────────▶┌────────┐ hydrate ok ┌──────────┐ start turn ┌──────────┐
   │ cold  │                │ warming│───────────▶│ hot_idle │───────────▶│ running  │
   └───┬───┘                └────┬───┘            └────┬─────┘            └────┬─────┘
       │  evict snapshot          │ hydrate fail         │ idle ttl               │ control_request
       │                          ▼                      ▼                        ▼
       │                    ┌──────────┐          ┌──────────┐              ┌───────────────┐
       └────────────────────│ degraded │◀─────────│ cooling  │◀─────────────│ awaiting_user │
                            └────┬─────┘          └────┬─────┘              └───────┬───────┘
                                 │ retry                │ snapshot ok                │ response/cancel
                                 └──────────────────────┴────────────────────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `cold` | 用户打开会话 / 连接重新订阅 | `warming` | 读取 session metadata、transcript tail、plan/todo、sandbox snapshot | 冷会话只在真正有人看或真正要跑时才拉热。 |
| `warming` | hydrate 成功 | `hot_idle` | 建立 `SessionMailbox`、恢复 `ContextState`、绑定沙箱租约 | 会话进入可立即开跑的热态。 |
| `hot_idle` | 收到 `start_turn` | `running` | 向 turn scheduler 申请配额、生成 `runId`、开始流式事件 | 真正消耗 CPU/LLM 并发的是这里。 |
| `running` | 触发审批 / 提问 | `awaiting_user` | 发送 `control_request`、冻结该 run 的后续工具执行 | 人工决策会卡住一个 run，但不该卡住整个 worker。 |
| `awaiting_user` | 收到审批回包 | `running` | 继续该 run；必要时补发恢复事件 | 批准后接着干，拒绝则中止或走替代路径。 |
| `running` | 正常结束 | `hot_idle` | flush transcript、更新 checkpoint、释放运行配额 | 结束后会话仍热，方便立刻追问。 |
| `hot_idle` | 达到 idle TTL | `cooling` | 落盘、写快照、释放 LLM/工具上下文 | 不活跃的热会话要及时降温。 |
| `cooling` | snapshot 成功 | `cold` | 释放 mailbox、listener、沙箱租约 | 冷掉后只剩外置存储成本。 |
| 任意态 | hydrate / snapshot 失败 | `degraded` | 记录告警、降级只读回放或重试 | 不能因为单个会话坏掉拖垮整台 worker。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `sessionKey` | 云端会话的全局路由键，推荐形态为 `tenantId:agentId:sessionId` | 命令帧、事件帧、placement 表、存储主键 | 集群内唯一；任何网关和 worker 都不得只靠裸 `sessionId` 路由 | 这是“这是谁家的哪一个会话”的身份证。 |
| `runId` | 一次 turn / 一次 agent 运行的唯一标识 | turn scheduler、event envelope、审计日志 | 同一 `sessionKey` 下单调生成； reconnect 后仍可按 `runId` 回放 | 这是“这一次回答任务”的编号。 |
| `seq` | 每会话事件流的单调递增序号 | event envelope、gateway replay cache | 同一 `sessionKey` 内严格单调；即使某些 delta 被丢弃也要消耗序号 | 这是“这条流里的第几条消息”，断线重连靠它续上。 |
| `SessionMailbox` | worker 内每会话唯一的有界消息队列 | `sessionKey -> mpsc` 或等价结构 | 只负责本会话；满了先触发背压，不得退化成全局广播 | 每个会话一条专属快递轨道。 |
| `HeatState` | 会话热度：`cold / warm / hot` | session runtime 状态机 | `hot` 才允许长期驻留执行上下文；`cold` 只能读存储 | 绝大多数会话应该是冷的，便宜地躺着。 |
| `Placement` | 会话当前落在哪个 worker 上 | placement store / gateway cache | 允许迁移，但任一时刻只能有一个 authoritative worker | 这是“这会话现在由哪台机器管”。 |
| `StorageTrait` | 持久化边界，屏蔽 FS / SQLite / Postgres / S3 差异 | Rust trait + backend impl | 先本地后云端，接口不变、实现可替换 | 先把“存哪儿”与“怎么跑”分开。 |
| `PendingWrites` | run 中已完成但未整体提交的局部结果 | checkpoint store / transcript WAL | 断点恢复时必须幂等重放，不能重复执行已成功工具 | 防止一半成功、一半失败时从头瞎重跑。 |
| `SandboxLease` | 每会话执行环境的租约 | worker runtime + sandbox provider | 绑定 `sessionKey`；超时、配额不足或 worker drain 时可回收 | 每个会话都要有自己的安全“工位”。 |
| `MultitaskPolicy` | 同一会话重复发起 turn 时的策略 | command field / scheduler config | 至少支持 `reject / enqueue / interrupt` | 同一个会话里下一条消息来早了，到底拒绝、排队还是打断，得说清楚。 |

## 2. 竞品 / 选型对比（调研）

### 2.1 当前 Tomcat 的真实问题是什么

| 现状 | 代码证据 | 影响 | 说人话 |
|------|----------|------|--------|
| 会话上限默认 16 | `src/api/serve/registry.rs`、`src/core/agent_registry/mod.rs`、`src/infra/config/types/runtime.rs` | 从入口就不允许海量会话常驻 | 现在的目标从来不是“云端百万会话”。 |
| EventBus 按事件名挂 `Vec<listener>`，`emit_sync` 写锁 + sort + 全量遍历 | `src/infra/event_bus/mod.rs` | 高频流式事件下 O(L log L) 且全局锁竞争明显 | 一条 token 更新也要先把同名 listener 全扫一遍。 |
| serve 每会话注册 48 白名单 listener + 1 ask listener | `src/api/serve/event_pump.rs`、`src/api/serve/mod.rs` | 会话数一大，listener 数和回调数线性膨胀 | 不是你的会话的事件，也先回调你再说“不是我”。 |
| 热态成本在 `ContextState.messages` 与 `messages.clone()` | `src/core/session/manager/types.rs`、`src/core/agent_loop/reasoning_loop.rs` | 一旦热会话太多，内存和 clone CPU 都很贵 | 历史对话越长，热会话越重。 |
| 全状态落本机文件系统 | `src/core/session/*`、`src/core/plan_runtime/*`、`src/core/checkpoint/*` | 无法跨 worker 恢复，也无法做多租户统一治理 | 会话跟机器绑死了。 |

### 2.2 外部参考对比

| 竞品 | 形态 | 关键设计 | 我们借鉴的点 | 说人话 |
|------|------|----------|---------------|--------|
| `codex-rs` | Rust app-server + Thread manager | `ThreadStore` trait、`ThreadStateManager`、per-thread request serialization、30 分钟 delayed unload | 热/冷分层、线程级订阅、多连接 fan-out、存储边界 trait | 这最像 Tomcat 应该长成的 Rust 版本。 |
| `LangGraph` | durable graph runtime + cloud SDK | `BaseCheckpointSaver`（memory/sqlite/postgres）、pending writes、`interrupt()` + `Command(resume)`、HTTP command + WS events | durable checkpoint、断点续跑、显式人工回包、命令/事件分离 | 这是“如何让中断恢复真的可靠”的最好参考。 |
| `OpenClaw` | gateway + worker environments | placement FSM、SQLite CAS、seq-ack event windows、session routing、drain 模式 | 网关/worker 拆分、事件窗口、优雅缩容、placement 防重入 | 这是“云端怎么把会话发给哪台 worker”的最实战参考。 |
| `LangChain` | runnable / callback ecosystem | `astream_events`、hierarchical callback manager | 统一事件信封、可继承 tracer | 这是“观测总线如何统一”的好模板。 |

### 2.3 第一性原理裁决

1. **会话数 ≠ 同时运行数。** 几十万会话里，同时占 CPU/LLM 的往往只有很小一部分。架构必须默认“冷会话极多、热会话极少”。
2. **跨会话 fan-out 必须从 O(N listeners) 变成 O(1) 定向投递。** 这不是优化细节，而是能不能上数量级的分水岭。
3. **执行环境必须按会话隔离。** 云端不允许 `bash/read/write/edit` 落到共享宿主文件系统。
4. **本地文件系统只能当 Phase A 的 backend，不能当最终架构。** 一旦要跨 worker 恢复，就必须有外置 catalog、checkpoint 和 blob store。
5. **命令面与事件面必须分开。** `stdio` 适合本地子进程；云端更适合 `HTTP command + WS event`，但要保留同一语义和同一 envelope。

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章分两部分：

- **§3.1 决策矩阵**：回答“最终采用什么、为什么不采用别的”。
- **§3.2 实施点总表**：回答“分几期落地、交什么、改哪里、怎么验收”。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| R1 产品模型 | 会话是不是一开就常驻完整 runtime | **采用** `cold / warm / hot` 三层模型，默认绝大多数会话为 `cold`；**拒绝**“开了会话就常驻 `ChatContext + listener + sandbox`”。 | 本仓：`src/api/serve/registry.rs`、`src/core/session/manager/context.rs`；外部：`codex-rs/app-server/src/request_processors/thread_lifecycle.rs`、`langgraph/libs/checkpoint/README.md` | 设计：把“会话存在”与“会话热跑”解耦；理由：海量会话的主要成本必须落到便宜存储层，而不是内存和连接。 | 未入选：当前 `SessionSlot` 常驻模型；拒因：会话数与内存/监听器数线性绑定，无法上量。 | 绝大多数会话只是“存在”，不是“正在跑”；别把所有会话都当活人养。 |
| R2 事件扇出 | 继续优化 `DefaultEventBus` 还是换模型 | **采用** `SessionMailbox` 定向投递，`EventBus` 降级为 worker 内钩子；**拒绝**“全局总线 + per-session listener 过滤”。 | 本仓：`src/infra/event_bus/mod.rs`、`src/api/serve/event_pump.rs`；外部：`codex-rs/app-server/src/thread_state.rs`、`langchain/libs/core/langchain_core/runnables/base.py` | 设计：AgentLoop 的用户可见事件直接写入该会话 mailbox，再由 gateway/worker fan-out；理由：复杂度从 O(N) 降到 O(1)。 | 未入选：继续在 EventBus 上做 sessionId 过滤；拒因：锁竞争和 listener 数仍然线性膨胀。 | 用户能看到的事件，不该先广播给所有会话再让它们各自说“这不是我”。 |
| R3 进程拓扑 | 是把 `serve` 无限做大，还是拆 Gateway / Worker | **采用** Gateway 控制面 + Worker 数据面；**保留**本地 stdio `serve` 作为兼容入口；**拒绝**“一个超级 `serve` 进程同时管一切”。 | 本仓：`docs/architecture/agent-server-and-ui-gateway.md`、`src/api/serve/*`；外部：`openclaw/src/gateway/worker-environments/placement-dispatch.ts`、`codex-rs/core/src/thread_manager.rs` | 设计：本地模式保留，云端模式拆层；理由：网关更适合鉴权/订阅/回放，worker 更适合执行和持有热态。 | 未入选：继续单进程承载路由、执行、存储、回放、隔离；拒因：职责过重，无法水平扩展。 | 一台总管机器什么都做，迟早又慢又脆；应该把“前台接待”和“后厨做饭”分开。 |
| R4 持久化边界 | 继续直接读写本机文件，还是抽 Storage trait | **采用** `Storage trait` 抽象，推荐 `Postgres + object store` 终态，Phase A 先用本地 FS/SQLite backend；**拒绝**把云端恢复逻辑直接耦合到现有目录结构。 | 本仓：`src/core/session/*`、`src/core/checkpoint/*`、`src/core/plan_runtime/*`；外部：`codex-rs/thread-store/src/store.rs`、`langgraph/libs/checkpoint/langgraph/checkpoint/base/__init__.py` | 设计：统一 `SessionCatalog / Transcript / Blob / Checkpoint / Plan` trait；理由：先分边界，后换 backend，演进最稳。 | 未入选：直接把现有 `~/.tomcat` 目录挂远端共享盘；拒因：并发写、锁、索引、回放都不可控。 | 先把“存什么”定义清楚，再决定“存哪儿”，别让目录结构绑死架构。 |
| R5 路由键与回放 | 路由靠 `sessionId` 还是更强主键 | **采用** `sessionKey + runId + seq` 三元组；**拒绝**只靠裸 `sessionId` 做全系统寻址。 | 本仓：`src/infra/events/mod.rs`、`src/api/serve/types.rs`；外部：`openclaw/src/gateway/server-session-key.ts`、`langgraph/libs/sdk-py/langgraph_sdk/schema.py` | 设计：`sessionKey` 管归属，`runId` 管一次执行，`seq` 管重放续流；理由：这三层语义不能混在一个 ID 里。 | 未入选：只沿用 `sessionId`；拒因：多租户下容易冲突，也不利于跨连接回放和审计。 | “是谁家的哪个会话、这次是哪一轮、流发到第几条了”是三件不同的事。 |
| R6 调度与配额 | 并发限制放在哪里 | **采用** gateway admission + worker turn scheduler + tenant/global quota 三层联合；**拒绝**只靠 `busy` 标志和 LLM semaphore。 | 本仓：`src/api/serve/commands.rs`、`src/core/agent_registry/mod.rs`、`src/core/llm/openai.rs`；外部：`openclaw/src/process/gateway-work-admission.ts`、`langgraph/libs/sdk-py/langgraph_sdk/schema.py` | 设计：前门拦过载、worker 排队、公平调度、对租户设预算；理由：云端要管“谁先跑、谁不能抢太多”。 | 未入选：继续沿用 `MAX_CONCURRENT_AGENTS=16` + per-session busy；拒因：既不公平也不可多租户治理。 | 不是“能跑就跑”，而是要有人排队、有人记账、有人限流。 |
| R7 传输协议 | 云端首发要不要继续只做 stdio | **采用** 语义统一、传输多态：本地继续 `stdio NDJSON`，云端主推 `HTTP command + WS event`；**拒绝** 为云端另造一套事件语义。 | 本仓：`src/api/serve/*`、`docs/architecture/agent-server-and-ui-gateway.md`；外部：`langgraph/libs/sdk-py/langgraph_sdk/stream/transport/ws.py`、`codex-rs/app-server/src/transport.rs` | 设计：一套 envelope，多种 transport adapter；理由：本地和云端是部署形态不同，不是产品语义不同。 | 未入选：本地一套、云端一套完全不同协议；拒因：客户端和服务端都要双倍维护。 | 管道和 WebSocket 只是“走哪条路”，不该变成“说两种语言”。 |
| R8 沙箱隔离 | 用共享宿主工作区还是每会话独立执行环境 | **采用** `SandboxProvider` 抽象；近端推荐 gVisor/容器池，硬隔离租户可升级 Firecracker；**拒绝**继续让 primitive 直接命中宿主共享文件系统。 | 本仓：`src/infra/net_guard.rs`、`src/core/tools/primitive/*`、`docs/architecture/work-dir-and-data-layout.md`；外部：`openclaw/deploy/*`、`openclaw/docker-compose.yml`、`codex` 的 thread/process 隔离经验 | 设计：每会话自己的 workspace overlay、凭证、网络策略和资源配额；理由：云端安全边界必须以会话为最小单位。 | 未入选：共享宿主目录 + 逻辑权限判断；拒因：一旦工具或插件出错，跨会话污染和越权风险太高。 | 云端里每个会话都该有独立工位，不能把所有人都扔进同一间机房随便碰。 |
| R9 故障恢复 | 失败后从哪儿恢复、如何续流 | **采用** `checkpoint + pending writes + seq replay`；**拒绝**只依赖 transcript 尾部和最佳努力补写。 | 本仓：`src/core/checkpoint/*`、`src/core/session/resume_index.rs`；外部：`langgraph/libs/checkpoint/README.md`、`openclaw/src/gateway/worker-environments/live-events.ts` | 设计：turn 内局部成果先写 pending，再在完成时提升为 durable state；理由：要避免重复执行已成功工具。 | 未入选：worker 挂了就让用户重试；拒因：多工具链路下重复执行代价和风险都太高。 | 坏了能接着跑，不是从头再来，更不是让用户赌运气。 |

### 3.2 实施点（已闭环设计）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| P0 基线与观测 | 给现有 `serve` 补可量化指标：listener 数、emit 耗时、hydrate bytes、hot session 数、queued turn 数 | `src/api/serve/*`、`src/infra/event_bus/*`、`src/core/session/*` | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) 的基线测试 | 先量出来现在到底卡在哪，不然后面都是感觉。 |
| Phase A 单机还债 | `SessionMailbox`、`HeatState`、idle unload、run identity、turn queue、本地 stdio 兼容 | `src/api/serve/*`、`src/infra/event_bus/*`、`src/core/agent_registry/*` | 见 [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md) | 先把单机模型从“桌面侧车”修到“可继续长大”。 |
| Phase B 多 worker | gateway dispatcher、placement、WS transport、Storage trait、按需 hydrate | `src/cloud/gateway/*`、`src/cloud/worker/*`、`src/cloud/storage/*` | 见 [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md) | 再把一台机器，变成很多台机器协同。 |
| Phase C 集群多租户 | 多租户配额、断线续流、跨 worker 恢复、autoscaling、drain、全局 LLM 治理 | `src/cloud/control_plane/*`、`deploy/*`、`ops/*` | 见 [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md) | 这一步才是真正意义上的云服务。 |
| 横切沙箱 | 每会话 sandbox provider、workspace snapshot、egress policy、凭证注入、资源治理 | `src/cloud/sandbox/*`、`src/infra/net_guard.rs` | 见 [`05-sandbox-workspace-isolation.md`](./05-sandbox-workspace-isolation.md) | 不把安全和隔离补上，云端就是拿着火把进油库。 |
| 上线与迁移 | 测试矩阵、灰度、回滚、特性开关、负载模型、执行 WBS | 文档与 CI / deploy 配置 | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md)、[`07-development-plan-todos.md`](./07-development-plan-todos.md) | 方案不是写完就算，得能一步步安全进生产。 |

#### 3.2.1 P0：先补基线，不急着改架构

必须先让当前系统能回答这几个问题：

- 一次 `message_update` 在当前 listener 数下平均耗时多少？
- 每个 session 实际热态内存大概多少？
- 当前 transcript hydrate 在 1k / 10k / 100k entries 下读了多少字节？
- `follow_up_queue`、writer queue 和 `MAX_CONCURRENT_AGENTS` 到底谁先顶满？

没有这些基线，后面每一阶段都无法判断收益是否真实。

#### 3.2.2 Phase A：把“单机会话模型”改正确

本期只做**单机还债**，不做分布式，但所有接口都要为分布式留门：

- 从 `EventBus -> event_pump -> writer` 改为 `AgentLoop -> SessionMailbox -> writer`。
- 引入 `cold / warm / hot` 和 idle unload。
- 把 `serve.max_sessions` 与 `MAX_CONCURRENT_AGENTS` 解耦。
- 给每个 turn 引入 `runId`，给每条会话事件引入 `seq`。
- turn 调度从“来就跑”升级为“能 queue、能 reject、能 interrupt”。

#### 3.2.3 Phase B：把本地 `serve` 长成 gateway/worker

这一期的关键不是上 K8s，而是把**本地单机模块边界**抽出来：

- 网关负责连接、订阅、鉴权、placement。
- worker 负责执行、热态管理、沙箱。
- 存储接口从本地目录抽成 trait。
- `stdio` 和 `WebSocket` 只是 transport adapter，不改变业务语义。

#### 3.2.4 Phase C：把“可扩展”变成“可运营”

当 Gateway / Worker / Storage 三层边界稳定后，才值得进入集群治理：

- tenant quota
- region / shard / drain
- 全局 LLM token 预算
- reconnect replay
- worker crash recover
- autoscaling / admission control

#### 3.2.5 横切沙箱：不要等到集群期才想起安全

沙箱必须在架构设计期就成为一等概念，因为它会倒逼：

- workspace 如何快照和恢复；
- `read/write/edit/bash` 如何落点；
- `net_guard` 如何从逻辑校验升级为运行时隔离；
- 凭证与环境变量如何按 `sessionKey` 发放和销毁。

#### 3.2.6 迁移与上线：先兼容，再替换

迁移顺序必须是：

1. 保持当前本地 `tomcat serve --stdio` 可用；
2. 在同一语义下加新 envelope、新指标、新 queue；
3. 再引入 gateway / worker；
4. 最后才让 cloud mode 成为新默认。

这保证 IDE、本地插件和后续云端 SDK 不会各自长出一套协议。

## 4. 协议

### 4.1 命令帧（CommandFrame）

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `string` | 是 | - | 所有命令 | `start_turn` / `follow_up` / `interrupt` / `subscribe` / `set_visibility` 等 | 这条命令到底要干什么。 |
| `id` | `string` | 是 | - | 所有命令 | 命令请求 ID，用于应答配对 | 这是这次请求的流水号。 |
| `tenantId` | `string` | 云端是 | 本地可省 | 网关鉴权 | 本地 stdio 可由进程上下文补齐 | 先知道你是谁家的。 |
| `sessionKey` | `string` | 是 | - | 会话级命令 | 全局会话键 | 这条命令要打到哪个会话。 |
| `runId` | `string` | 否 | 新建 turn 时生成 | follow-up / interrupt / replay | 用于定向操作某次运行 | 有些动作是针对“这次执行”，不是整个会话。 |
| `multitaskPolicy` | `string` | 否 | `reject` 或配置默认 | start_turn | `reject` / `enqueue` / `interrupt` | 同会话撞车时怎么处理。 |
| `payload` | `object` | 否 | `{}` | 命令细节 | 文本、附件、审批回包、订阅参数等 | 真正的业务内容放这里。 |

### 4.2 事件帧（EventEnvelope）

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `string` | 是 | - | 所有事件 | `agent_start` / `message_update` / `tool_execution_end` / `control_request` 等 | 这是什么事件。 |
| `sessionKey` | `string` | 是 | - | 所有事件 | 路由键 | 这事件属于哪个会话。 |
| `runId` | `string` | 否 | - | turn 内事件 | 同一 turn 的全链路归属 | 这事件属于哪一轮回答。 |
| `seq` | `u64` | 是 | - | 所有事件 | 会话内单调序号 | 重连补流和去重都靠它。 |
| `durability` | `string` | 是 | - | 所有事件 | `lossless` / `best_effort` | 哪些事件绝不能丢。 |
| `source` | `string` | 是 | - | 所有事件 | `worker` / `gateway` / `sandbox` | 方便审计和排障。 |
| `payload` | `object` | 是 | - | 所有事件 | 复用现有 `AgentEvent` / `ExtensionEvent` 负载 | 具体内容放这里。 |

事件 durability 约束：

- **lossless**：`agent_*`、`turn_*`、`control_*`、`error`、`checkpoint_*`
- **best_effort**：`message_update`、`thinking_delta`、长工具流式增量

### 4.3 控制帧（ControlFrame）

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `string` | 是 | - | 控制面 | `control_request` / `control_response` / `control_cancel` | 需要双向配对的控制消息。 |
| `requestId` | `string` | 是 | - | 审批 / 初始化 / reconnect | 双向请求配对 ID | 这次来回对话的编号。 |
| `sessionKey` | `string` | 否 | 连接级请求可空 | 审批 / reconnect | 某些连接级请求不绑定单会话 | 这次控制往返是哪个会话的。 |
| `kind` | `string` | 是 | - | 控制面 | `initialize` / `ask_question` / `approval` / `replay_ready` | 具体控制动作。 |
| `payload` | `object` | 是 | `{}` | 控制面 | 问题、选项、恢复游标、审批结果等 | 控制消息的正文。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | 当前职责 | 目标职责 | 说人话 |
|-------------|----------|----------|--------|
| `src/api/serve/mod.rs` | 单进程 stdio serve 装配 | 保留本地兼容入口；云端模式只做本地 adapter | 本地入口继续留着，但别再背所有未来职责。 |
| `src/api/serve/registry.rs` | `sessionId -> SessionSlot` 本地注册表 | Phase A 演进为 `HeatStateRegistry`；Phase B 迁到 worker runtime | 这张表以后不只管“有没有会话”，还要管“它热不热”。 |
| `src/api/serve/event_pump.rs` | EventBus listener 转发 | 被 `SessionMailbox` / `event_sink` 取代 | 不再靠挂 listener 挡洪水。 |
| `src/api/serve/writer.rs` | stdout 单写者 | 保留本地 writer；云端新增 WS/SSE adapter 与 replay cache | 单写者思路是对的，只是出口不再只有 stdout。 |
| `src/infra/event_bus/mod.rs` | 全局事件钩子与转发 | 降级为 worker 内观察 / 插件回调 / metrics tracer | EventBus 以后是“内部总线”，不是“会话推流主通道”。 |
| `src/core/agent_loop/*` | 单 turn 执行、工具调度、事件发射 | 保持主循环不重写，只替换其 event sink、checkpoint、sandbox 接口 | AgentLoop 不该因为上云被推翻重写。 |
| `src/core/session/*` | 本地 FS transcript / hydration | 被 `Storage trait` 包装；FS backend 成为 Phase A/本地实现 | 这些逻辑仍有价值，但要从“本机实现”升成“后端实现”。 |
| `src/core/checkpoint/*` | shadow git checkpoint | 成为 `CheckpointStore` 的一个 backend；增加 pending writes 语义 | checkpoint 要从“本机快照”升级为“分布式恢复锚点”。 |
| `src/cloud/gateway/*` | 新增 | 连接、订阅、placement、配额、回放 | 这是未来云端的前门。 |
| `src/cloud/worker/*` | 新增 | 热态、mailbox、scheduler、sandbox lease | 这是未来云端的执行平面。 |
| `src/cloud/storage/*` | 新增 | catalog / transcript / blob / checkpoint / plan 抽象 | 这是未来云端的状态平面。 |
| `src/cloud/sandbox/*` | 新增 | 隔离执行环境、workspace snapshot、egress policy | 这是未来云端的安全边界。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 所属阶段 | 说明 | 说人话 |
|--------|----------|----------|------|--------|
| `serve.max_hot_sessions` | 按内存预算计算 | Phase A | 每 worker 同时保温的热会话上限 | 热会话不是越多越好，要按钱和内存算。 |
| `serve.max_queued_turns_per_session` | `8` | Phase A | 同会话排队 turn 上限 | 一个人不能把队列占满。 |
| `serve.idle_warm_ms` | `30_000` | Phase A | 热态降温为 warm 的空闲时间 | 短时间没动先别全冻死。 |
| `serve.idle_cold_ms` | `300_000` | Phase A | warm 降为 cold 的空闲时间 | 更久没动就彻底放回存储。 |
| `cloud.gateway.max_subscriptions_per_conn` | `256` | Phase B | 单连接最多订阅多少会话 | 一个前端连接别无限挂会话。 |
| `cloud.gateway.replay_cache_events` | `2048` | Phase B | gateway 保留的最近事件数 | 断线续流先靠短缓存，超出再回放存储。 |
| `cloud.worker.max_running_turns` | 按 CPU/LLM 预算 | Phase B | 单 worker 同时运行的 turn 数 | 真正跑 LLM 的并发要控住。 |
| `cloud.storage.catalog_backend` | `sqlite`→`postgres` | Phase B/C | session catalog backend | 先能跑，再上集中式。 |
| `cloud.storage.blob_backend` | `fs`→`s3` | Phase B/C | tool-results / workspace diff backend | 大对象别硬塞数据库。 |
| `cloud.sandbox.provider` | `gvisor` | Phase B | 默认沙箱实现 | 先选接入成本低的默认方案。 |
| `cloud.sandbox.hardened_provider` | `firecracker` | Phase C | 高隔离租户或高风险任务使用 | 真高风险场景再上更重隔离。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 触发条件 | 对外表现 | 恢复策略 | 说人话 |
|------|----------|----------|----------|--------|
| `queue_full` | `SessionMailbox` 超过字节或条数上限 | 返回 `busy` / `queued_rejected` / drop notice | 客户端退避或切换 `interrupt` 策略 | 会话自己已经堆爆了，先别继续塞。 |
| `quota_exceeded` | tenant / worker / model 配额触顶 | 命令被拒绝或延后 | 等窗口恢复或人工扩容 | 资源账本说你先等等。 |
| `hydrate_failed` | transcript / plan / sandbox snapshot 恢复失败 | 会话进 `degraded`，允许只读回放或手动修复 | 重试 hydrate / fallback full scan | 会话数据坏了，但不该把整台服务拉死。 |
| `sandbox_unavailable` | 无可用沙箱槽位或 provider 错误 | turn 不启动，保留排队 | 等待预热池补位或迁移 worker | 没空工位就别硬开工。 |
| `replay_gap` | gateway 缓存不够，`seq` 跨越太大 | 客户端收到 `replay_required` 控制帧 | 从 durable transcript + snapshot 重建 | 短缓存接不上了，就走完整补流。 |
| `best_effort_drop` | 慢客户端导致 delta 背压 | 发显式 drop notice，不影响 lossless 事件 | UI 重拉当前快照 | 字太多发不过来，但结束态和审批不能丢。 |

## 8. 测试矩阵（验收）

| 测试层 | 核心场景 | 验收锚点（示例） | 说人话 |
|--------|----------|------------------|--------|
| 单元测试 | `SessionMailbox` 有界背压、`seq` 单调、placement 迁移、pending writes 幂等 | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.1 | 小件先在本地证明对。 |
| 集成测试 | worker crash 恢复、hydrate / evict 循环、approval 回包、WS reconnect replay | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.2 | 把几个模块串起来测，不要只看单件。 |
| E2E | IDE / Web 从打开会话到关闭、断线重连、后台会话审批、跨 worker 迁移 | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.3 | 从用户视角真跑一遍。 |
| 负载测试 | 10 万冷会话、1 万连接、千级热会话、百级并发 turn、慢消费者 | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.4 | 真正上数量级看会不会趴。 |
| 混沌 / 故障注入 | 杀 worker、断对象存储、卡数据库、沙箱池耗尽 | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.5 | 故意把它搞坏，看是不是能自己站起来。 |

目标负载假设（作为设计基线）：

- 100 万注册会话中，`cold` 占 98% 左右；
- 同时在线连接 10 万量级；
- 同时 `hot` 会话 1 万量级；
- 同时真正 `running` 的 turn 为百到千量级；
- 同一租户允许的并发和速率必须可独立配置，不能让大租户挤死小租户。

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 先做分布式、后补本地边界 | 容易直接重写过度，且难回归 IDE 本地模式 | 严格按 A→B→C 走，先在单机把模型改对 | 地基不稳时别先盖高楼。 |
| `Storage trait` 抽象过早过大 | 接口太宽，所有实现都很痛苦 | 先按 `catalog / transcript / blob / checkpoint / plan` 五类最小拆分 | 抽象不是越大越好，够用就行。 |
| 沙箱方案一开始就选太重 | 工程复杂度暴涨，团队迟迟落不了地 | 默认 gVisor/容器池，Firecracker 作为高安全档位 | 先让 80% 场景跑起来，再啃最硬的 20%。 |
| `seq` / replay 设计不严 | 断线重连重复、漏帧、错序 | 把 `seq` 作为一等协议字段，并在测试矩阵里单独验 | 不把编号钉死，补流一定出鬼。 |
| 多租户配额只做 worker 本地 | 无法全局公平，租户可能跨 worker 打满 | 配额至少要有 gateway/控制面 authoritative 层 | 只在每台机器各管各的，整体就没人管。 |

## 10. 历史决策 / 跨文档修订

1. 本方案把 [`../agent-server-and-ui-gateway.md`](../agent-server-and-ui-gateway.md) 的“单进程多会话 stdio gateway”视为**本地进程边界方案**，而不是最终云端形态。两者不是冲突关系，而是 **Phase A 的基线** 与 **Phase B/C 的前置条件**。
2. 本方案不推翻现有 `AgentLoop`、`ChatContext`、transcript、checkpoint 思路；它推翻的是**“这些东西必须绑定在同一台本机、同一张 EventBus、同一个工作区”**的部署假设。
3. 本方案明确把 `cloud-scale-serving-01/01-overview.md` 作为父入口；后续 `02–07` 只回链本文，不做链式跳转。

## 子文档导航

- [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md)：先在单机把 fan-out、热/冷、queue、run identity 改正确。
- [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md)：把 `serve` 抽成 gateway / worker / storage 三层。
- [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md)：把多 worker 演进成真正可运营的集群。
- [`05-sandbox-workspace-isolation.md`](./05-sandbox-workspace-isolation.md)：每会话执行沙箱、workspace snapshot 与网络策略。
- [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md)：测试矩阵、SLO、灰度、回滚和风险登记。
- [`07-development-plan-todos.md`](./07-development-plan-todos.md)：按 Phase A / B / C 的详尽 WBS、排期与 DoD。
