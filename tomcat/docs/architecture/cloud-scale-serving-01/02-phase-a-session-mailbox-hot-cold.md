# Phase A：SessionMailbox、热/温/冷分层与单机模型还债

> 本文是 [`01-overview.md`](./01-overview.md) 的 Phase A 分册，专门回答一个问题：**在不先做分布式的前提下，Tomcat 单机 `serve` 应该先把哪些架构债还掉，才能为云端演进打地基。**
> 关联当前实现：`src/api/serve/*`、`src/infra/event_bus/mod.rs`、`src/core/agent_registry/mod.rs`、`src/core/session/*`。
>
> 本文只处理 **单机内模型**，不引入外部 gateway、共享数据库或跨 worker 路由；但所有接口都要按未来多 worker 可扩展来设计。

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象图**：理解 Phase A 想把当前“广播式推流”改成什么。
2. 再看 **A.2 具体图**：理解当前 `serve` 代码里哪些文件会被重构、哪些会被弱化。
3. 再看 **B 状态机**：理解一个单机会话如何在 `cold / warm / hot` 之间切换，以及 turn 怎样排队和执行。
4. 最后看 **§3 已定稿选型**：这是 Phase A 真正要交付的裁决表与实施点。

### A.1 抽象 ASCII 总图

```text
当前单机 serve
──────────────────────────────────────────────────────────────
AgentLoop.emit
  -> EventBus(HashMap<event, Vec<listener>>)
  -> 每 session 的 event_pump listener 都醒来
  -> 回调里检查 sessionId
  -> writer.send(stdout)

Phase A 单机 serve
──────────────────────────────────────────────────────────────
AgentLoop.emit_user_visible
  -> SessionMailbox(sessionKey)
  -> TurnScheduler / Backpressure / Seq
  -> writer.send(stdout)

AgentLoop.emit_internal
  -> EventBus(仅 worker 内钩子 / 插件 / metrics)
```

读图导读（说人话）：Phase A 并不要求先上云，但它要求**先把“会话事件通路”和“内部事件总线”分家**。当前路径里，`EventBus` 既要给插件、stderr、ask-question 当钩子，又要负责给 UI 推流。这两个需求的复杂度完全不同。前者是进程内少量钩子；后者是按会话高频定向推流。继续混在一条总线上，越往后越难长大。

### A.2 具体 ASCII 总图

```text
当前代码（需要被改造）
──────────────────────────────────────────────────────────────────────────
src/api/serve/commands.rs
  └─ 收命令后直接找 SessionSlot
       └─ 若 busy 则报错/入 follow_up_queue

src/api/serve/event_pump.rs
  └─ 为每个 session 在 EventBus 上挂 48 个白名单 listener

src/api/serve/registry.rs
  └─ ChatContextRegistry<sessionId, SessionSlot>
       SessionSlot = Arc<ChatContext> + run_task + busy + turn_state

src/infra/event_bus/mod.rs
  └─ emit_sync = write lock + sort + invoke all listeners

src/core/agent_registry/mod.rs
  └─ MAX_CONCURRENT_AGENTS = 16
     根 session handle 与 child handle 共用一套上限

─────────────────────── Phase A 目标代码边界 ───────────────────────────────

src/api/serve/session_registry.rs         新/改
  └─ SessionHandle
     - sessionKey
     - heat_state
     - session_summary
     - hot_runtime: Option<HotSessionRuntime>

src/api/serve/session_mailbox.rs          新增
  └─ sessionKey -> bounded queue
     delivery_class = lossless | best_effort

src/api/serve/turn_scheduler.rs           新增
  └─ per-session queue + global running budget
     multitask_policy = reject | enqueue | interrupt

src/api/serve/heat_manager.rs             新增
  └─ cold <-> warm <-> hot
     idle unload / rehydrate

src/api/serve/writer.rs                   保留但增强
  └─ drain SessionMailbox 输出
     seq / coalesce / drop notice

src/infra/event_bus/mod.rs                降级
  └─ 仅插件 / metrics / 内部钩子
     不再承担用户可见主事件通道

src/core/agent_registry/mod.rs            改语义
  └─ session count 与 running roots / child agents 解耦
```

读图导读（说人话）：这张图最重要的是把 `SessionSlot` 这个“大壳子”拆开。当前 `SessionSlot` 把“会话存在”“会话热态”“会话执行中”全糊在一块，所以才会出现 `max_sessions` 跟 `MAX_CONCURRENT_AGENTS` 默认绑死、根 handle 长期占着名额、会话空闲也不卸载的问题。Phase A 的实质，就是把这几个概念拆开并恢复正确边界。

### B. 状态机：单机会话热度与 turn 调度

```text
         open/list
  ┌───────┐──────────────▶┌───────┐ hydrate ok ┌──────────┐ queue turn ┌──────────┐
  │ cold  │               │ warm  │───────────▶│ hot_idle │───────────▶│ queued   │
  └───┬───┘               └───┬───┘            └────┬─────┘            └────┬─────┘
      │ re-open                │ hydrate fail         │ admit                   │ admit
      │                        ▼                      │                         ▼
      │                  ┌──────────┐                │                   ┌──────────────┐
      └──────────────────│ degraded │◀───────────────┘                   │ running      │
                         └────┬─────┘                                    └────┬─────────┘
                              │ retry                                           │ approval
                              │                                                 ▼
                              │                                          ┌──────────────┐
                              │                                          │ awaiting_user│
                              │                                          └────┬─────────┘
                              │                                               │ response
                              └───────────────────────────────────────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `cold` | 用户打开 / 重新选择会话 | `warm` | 只装载轻量 metadata、resume anchor、queue stub | 先把会话“叫醒”，但不急着把整段上下文都抱进内存。 |
| `warm` | 收到 turn / 订阅 | `hot_idle` | hydrate `ContextState`、恢复 `ChatContext`、建立 mailbox | 真要干活了，再把重东西拉起来。 |
| `hot_idle` | 收到新 turn 且可直接执行 | `running` | 创建 `runId`、占用 scheduler 预算、启动 AgentLoop | 会话进入真运行态。 |
| `hot_idle` | 收到新 turn 但当前配额不足 | `queued` | 写入 per-session queue | 先进队列，不要瞎抢资源。 |
| `queued` | 调度器 admit | `running` | 取出队首 turn 开始执行 | 轮到你了再跑。 |
| `running` | 需要审批 / 提问 | `awaiting_user` | 冻结该 run 后续步骤，发 `control_request` | 这个 run 等人拍板，但别卡住别的会话。 |
| `awaiting_user` | 收到控制回包 | `running` | 恢复该 run | 答完接着跑。 |
| `running` | 正常结束 | `hot_idle` | flush transcript、更新 checkpoint、释放运行预算 | 跑完了但先别急着冷却。 |
| `hot_idle` | 达到 idle TTL | `warm` | 释放 `ContextState`、保留轻量 anchor 和 queue | 先降温，保留快速回热能力。 |
| `warm` | 达到更长 TTL | `cold` | 完全释放 hot runtime | 真冷了就别占 runtime 资源。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `SessionHandle` | 单机会话的轻量常驻壳 | `session_registry` map value | 一定存在于 registry；不一定持有 hot runtime | 会话要先有一个轻壳，不能每次都把重 runtime 常驻。 |
| `HotSessionRuntime` | 只有热会话才持有的重量对象 | `Arc<ChatContext>`、`ContextState`、cancel token、queue、mailbox | `heat_state=hot` 时才允许存在 | 重东西只给真正热的会话。 |
| `SessionMailbox` | 用户可见事件的专属输出队列 | bounded mpsc / deque | 只服务单会话；必须可统计 depth / bytes | 用户可见流别再经过公共广播。 |
| `DeliveryClass` | 事件是否可丢 | `lossless` / `best_effort` | `lossless` 绝不能在背压下丢；`best_effort` 可合并或丢 | 不是所有流式字都跟“结束事件”一个优先级。 |
| `HeatState` | 会话热度 | `cold / warm / hot / degraded` | 影响 runtime 是否在内存中 | 会话热不热，决定它到底贵不贵。 |
| `TurnScheduler` | 单机内 turn 排队和 admission 的统一入口 | per-session queue + global semaphore | 不允许绕开它直接起新 turn | 要跑先排队，别绕后门。 |
| `RunGeneration` | 同会话内每次 turn 的世代号 | `u64` / monotonic counter | 新 turn 必须比旧 turn 大，用于去除陈旧回包 | 防止老消息和新消息串线。 |
| `CompatibilityEnvelope` | Phase A 对现有 stdio 客户端的兼容壳 | additive fields in existing frames | 旧客户端看不懂的新字段必须可忽略 | 改内部模型可以，别先把现有 IDE 插件打崩。 |

## 2. 竞品 / 选型对比（调研）

### 2.1 为什么 Phase A 不是“先上 WebSocket”

| 备选 | 现状收益 | 长期收益 | 为什么本期不优先 | 说人话 |
|------|----------|----------|------------------|--------|
| 先上 WebSocket | 远程接入更方便 | 为 Phase B 铺路 | 解决不了 EventBus O(N) 扇出、热态常驻和并发语义混乱 | 换个管道，不会自动把屋里收拾干净。 |
| 先做 SessionMailbox + HeatState + Scheduler | 直接打掉当前最大的局部瓶颈 | 为 Gateway/Worker 保留正确接口 | 本期最值得做 | 先把单机脑子理顺，后面多机才有意义。 |

### 2.2 参考实现给 Phase A 的启发

| 参考 | 文件 | 对 Phase A 的启发 | 说人话 |
|------|------|-------------------|--------|
| `codex-rs` | `app-server/src/request_processors/thread_lifecycle.rs` | 线程在无人订阅且空闲时延迟卸载，而不是永远热着 | 热态要会下线。 |
| `codex-rs` | `app-server/src/request_serialization.rs` | 同一 thread 的请求要有串行域 | 同会话必须一个一个来。 |
| `LangGraph` | `sdk-py/langgraph_sdk/schema.py` | 明确定义 `reject / enqueue / interrupt` 并发策略 | 同会话撞车时要有明文策略。 |
| `OpenClaw` | `src/gateway/worker-environments/live-events.ts` | 事件窗口、pending bytes、ack/seq 概念可前置到本地模型 | 背压和补流不要等到集群才想起。 |

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章先给出 Phase A 的定稿裁决，再给出落地点和验收锚点。Phase A 不求一次做完所有云端能力，但求把**错误的单机模型**先改正确。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| A1 会话主事件通路 | 用户可见事件继续走 EventBus 还是改 mailbox | **采用** `SessionMailbox` 作为主事件通路；**拒绝**继续让 `EventBus` 承担 UI fan-out。 | 本仓：`src/api/serve/event_pump.rs`、`src/infra/event_bus/mod.rs`；外部：`codex-rs/app-server/src/thread_state.rs`、`langchain/libs/core/langchain_core/runnables/base.py` | 设计：用户可见事件直接写入会话 mailbox；理由：复杂度和锁竞争都可控。 | 未入选：保留 event_pump 白名单监听；拒因：listener 数和 emit 成本仍然按会话数线性增长。 | 一条会话就该走一条专线，不该挤总线。 |
| A2 EventBus 职责 | EventBus 要不要被删除 | **保留** EventBus，但降级为内部钩子、插件与 metrics 总线；**拒绝**删掉它或继续让它做主数据通路。 | 本仓：`src/ext/dispatcher/ops.rs`、`src/api/chat/events/stderr.rs`；外部：`langchain` callback manager、`LangGraph` hooks | 设计：不推翻已有插件/工具钩子生态；理由：内部可观测与用户可见推流是两类问题。 | 未入选：彻底删除 EventBus；拒因：插件、stderr、ask_question 等现有扩展点会被迫全部重写。 | 总线还有用，但只该做“内部通知”，不该做“前台大喇叭”。 |
| A3 热度模型 | 是否引入 warm 层 | **采用** `cold / warm / hot / degraded` 四态；**拒绝**只保留 `loaded/unloaded` 二态。 | 本仓：`src/core/session/manager/context.rs`、`src/api/serve/registry.rs`；外部：`codex-rs` delayed unload、`LangGraph` lazy checkpoint hydrate | 设计：warm 保留轻量 anchor 与 queue stub；理由：回热成本和彻底冷启动成本不同，值得拆开。 | 未入选：只有 hot/cold；拒因：要么频繁全量 hydrate，要么长期占着重 runtime。 | “半热”这层很值钱，它让会话醒得快但又不太贵。 |
| A4 会话上限语义 | `max_sessions` 是否继续等于并发 agent 数 | **采用** 拆成 `max_live_sessions`、`max_hot_sessions`、`max_running_turns`、`max_child_agents_per_root`；**拒绝**沿用 `max_sessions == MAX_CONCURRENT_AGENTS`。 | 本仓：`src/api/serve/registry.rs`、`src/core/agent_registry/mod.rs`；外部：`codex-rs/core/src/thread_manager.rs`、`openclaw` 的 admission / placement 分层 | 设计：会话存在数、热态数、正在运行数、子 agent 数是四个不同预算；理由：它们消耗的是不同资源。 | 未入选：继续共用 16；拒因：空闲根 session 会把 child budget 吃光。 | “能开多少会话”和“能同时跑多少 agent”根本不是一个问题。 |
| A5 调度策略 | 同会话忙时，第二个 turn 怎么办 | **采用** `TurnScheduler + MultitaskPolicy`；默认兼容现状：`prompt=reject`、`follow_up/steer=enqueue`，并为未来开放 `interrupt`。 | 本仓：`src/api/serve/commands.rs`、`src/api/chat/run_loop/mod.rs`；外部：`LangGraph` multitask strategy、`codex-rs` per-thread serialization | 设计：先兼容当前交互，再把策略做成显式配置；理由：本地 IDE 体验不能一下大变，但内部要先有统一调度器。 | 未入选：继续散落在 `commands.rs` 里手写分支；拒因：语义零散，未来没法统一迁到 gateway。 | 先保持用户手感，再把底层调度变成正规军。 |
| A6 run identity | 是否在本地就引入 `runId / seq / runGeneration` | **采用** 本地即引入；**拒绝**等 Phase B/Phase C 再补。 | 本仓：`src/api/serve/types.rs`、`src/infra/events/mod.rs`；外部：`openclaw` live events seq window、`LangGraph` thread/run resume | 设计：本地与云端共用事件身份语义；理由：后加这些字段最容易引起兼容事故。 | 未入选：本地先不加；拒因：将来 replay、dedupe、interrupt 都得再破一次协议。 | 事件编号要早点长出来，别等以后再给每条消息补身份证。 |
| A7 背压 | 慢客户端时怎么处理 | **采用** bounded mailbox + delta coalescing + drop notice + lossless/lossy 分级；**拒绝**无限缓冲。 | 本仓：`src/api/serve/writer.rs`、`src/api/chat/cli_turn_renderer.rs`；外部：`openclaw/src/gateway/server-chat.ts`、`codex-rs` lossless/best-effort 分级 | 设计：生命周期事件必达，增量文字可合并可丢；理由：慢消费者不能把内存吃爆。 | 未入选：所有帧无脑堆队列；拒因：本地一样会 OOM，只是规模没那么快显现。 | 该省的流量省，该保的结局保住。 |
| A8 兼容策略 | Phase A 能不能直接改坏现有 stdio 客户端 | **采用** additive compatibility：旧字段保留，新字段只增不删；**拒绝**一次性重做 serve wire。 | 本仓：`src/api/serve/types.rs`、`src/api/serve/control.rs`；外部：`codex-rs/app-server-protocol/src/export.rs` | 设计：新字段如 `runId/seq/queuePosition` 都可被老客户端忽略；理由：先把内核改对，再推动消费端升级。 | 未入选：先改协议再说；拒因：会让 Phase A 变成“架构整改 + 全量客户端升级”的双重风险。 | 先在旧路上铺新轨道，不要让现有插件先掉坑里。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| A1 SessionMailbox | `sessionKey -> mailbox`、`DeliveryClass`、writer 接入 seq 和 drop notice | `src/api/serve/session_mailbox.rs`、`src/api/serve/writer.rs`、`src/core/agent_loop/*` | 见本文 §8.1 / §8.2 | 先把广播换成专线。 |
| A2 HeatState 与 idle unload | `SessionHandle` / `HotSessionRuntime` 拆分，`cold/warm/hot` 迁移 | `src/api/serve/session_registry.rs`、`src/api/serve/heat_manager.rs`、`src/core/session/*` | 见本文 §8.2 | 让空闲会话会降温。 |
| A3 TurnScheduler 与预算拆分 | `max_live_sessions`、`max_hot_sessions`、`max_running_turns`、per-session queue、policy | `src/api/serve/turn_scheduler.rs`、`src/api/serve/commands.rs`、`src/core/agent_registry/mod.rs` | 见本文 §8.1 / §8.2 | 会话数、热态数、运行数分账本。 |
| A4 Protocol compatibility | `runId`、`seq`、`queuePosition`、`multitaskPolicy` 新字段；旧 stdio 行为兼容 | `src/api/serve/types.rs`、`src/api/serve/control.rs`、`src/api/serve/schema.rs` | 见本文 §8.3 | 对外尽量不炸，内部先升级。 |
| A5 Telemetry & migration | listener / hydrate / queue / run metrics；灰度开关 | `src/api/serve/*`、`src/infra/events/*`、配置文件 | 见本文 §8.4 | 没指标就别谈 Phase A 是否成功。 |

#### 3.2.1 A1：SessionMailbox 替代 event_pump

关键设计：

- `SessionMailbox` 按会话独占，输出元素至少包含：
  - `sessionKey`
  - `runId`
  - `seq`
  - `deliveryClass`
  - `payload`
- `message_update` 等高频流直接进 mailbox，不再先注册到 `EventBus`。
- `EventBus` 只保留给：
  - 插件 `events.on`
  - stderr / metrics / tracing
  - ask-question 等进程内协作钩子

这意味着 Phase A 会出现两个“发事件”入口：

1. `emit_internal()`：仍走 `EventBus`
2. `emit_user_visible()`：走 `SessionMailbox`

这样做的目的是把责任拆开，而不是引入两套对外协议。

#### 3.2.2 A2：HeatState 把会话拆成轻壳与重 runtime

推荐结构：

```text
SessionHandle
  ├─ sessionKey / mode / cwd / summary
  ├─ heat_state
  ├─ queue_stub
  ├─ anchors (resume / plan / title / updated_at)
  └─ hot_runtime: Option<HotSessionRuntime>

HotSessionRuntime
  ├─ Arc<ChatContext>
  ├─ ContextState
  ├─ cancel_token
  ├─ SessionMailbox
  └─ in-memory run metadata
```

这样有两个好处：

- `ChatContextRegistry` 不再等于“所有会话的完整运行态集合”。
- idle unload 可以只释放 `hot_runtime`，而不必删除整个会话条目。

#### 3.2.3 A3：把 budget 拆开

当前问题是：

- `max_sessions` 限制会话壳数；
- `MAX_CONCURRENT_AGENTS` 限制根 + 子 agent handle 总数；
- 两者默认都等于 16；
- 根会话 handle 常驻，导致子 agent budget 被平白吞掉。

Phase A 的定稿是：

- `max_live_sessions`：单机 registry 能持有的轻壳数；
- `max_hot_sessions`：同时允许多少 `HotSessionRuntime`；
- `max_running_turns`：同时允许多少 turn 真跑；
- `max_child_agents_per_root` / `max_total_child_agents`：多 agent 预算。

根 handle 不应再因为“会话存在”而长期占用“正在运行 agent”预算。

#### 3.2.4 A4：协议加字段但不破旧客户端

Phase A 对外新增字段建议：

- 命令：
  - `multitaskPolicy`
- 事件：
  - `runId`
  - `seq`
  - `queuePosition`（只在 queued 通知里出现）
  - `deliveryClass`
- 控制：
  - `runId`（审批与中断更精确）

兼容规则：

- 老客户端继续按已有 `type`、`sessionId`、`payload` 工作；
- 新字段默认可忽略；
- schema 与 d.ts 同步更新，但不要求旧消费端立刻使用全部新字段。

#### 3.2.5 A5：指标与灰度

Phase A 不是“觉得更优雅就算成了”，而是要量化这些指标：

- `session_mailbox_depth`
- `session_mailbox_bytes`
- `event_emit_internal_ms`
- `event_emit_user_visible_ms`
- `hot_session_count`
- `warm_session_count`
- `cold_session_rehydrate_ms`
- `queued_turn_count`
- `queue_wait_ms`
- `best_effort_drop_count`

并通过 feature flag 灰度：

- `serve.enable_session_mailbox`
- `serve.enable_heat_state`
- `serve.enable_turn_scheduler`

## 4. 协议

### 4.1 `SessionMailboxEvent`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | 所有 mailbox 事件 | 会话路由键 | 先知道发给谁。 |
| `runId` | `string` | 否 | - | turn 内事件 | 本次运行 ID | 知道属于哪一轮。 |
| `seq` | `u64` | 是 | - | 所有 mailbox 事件 | 会话内单调事件序号 | 后面补流和去重都靠它。 |
| `deliveryClass` | `string` | 是 | - | 所有 mailbox 事件 | `lossless` / `best_effort` | 这条能不能在压力下丢。 |
| `payload` | `object` | 是 | - | 所有 mailbox 事件 | 原事件体 | 内容还是那条内容。 |

### 4.2 `QueuedTurnNotice`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | turn 入队 | 关联会话 | 哪个会话被排队了。 |
| `runId` | `string` | 是 | - | turn 入队 | 排队 turn 的 ID | 这次排队任务自己的编号。 |
| `queuePosition` | `u32` | 是 | - | turn 入队 | 当前队内位置 | 你前面还有几个人。 |
| `policy` | `string` | 是 | - | turn 入队 | `enqueue` / `reject` / `interrupt` | 是怎么决定的。 |

### 4.3 `HeatStateSnapshot`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | 诊断/列表 | 会话键 | 哪个会话。 |
| `heatState` | `string` | 是 | - | 诊断/列表 | `cold/warm/hot/degraded` | 当前温度。 |
| `hasHotRuntime` | `bool` | 是 | - | 诊断/列表 | 是否持有重 runtime | 真正在不在内存里。 |
| `queuedTurns` | `u32` | 是 | `0` | 诊断/列表 | 待跑 turn 数 | 队列里堆了多少。 |
| `lastActiveAt` | `string` | 是 | - | 诊断/列表 | 最近活动时间 | 什么时候最后活过。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | Phase A 变化 | 目的 | 说人话 |
|-------------|-------------|------|--------|
| `src/api/serve/registry.rs` | 拆成 `session_registry.rs` 或重构为 `SessionHandle` + `HotSessionRuntime` | 去掉“壳和重 runtime 糊在一起”的问题 | 别再一个结构装所有概念。 |
| `src/api/serve/event_pump.rs` | 大幅瘦身，或演进成只服务内部 hook 桥接 | 让用户可见事件脱离 EventBus | 这层以后不是 UI 主泵。 |
| `src/api/serve/writer.rs` | 加 `seq`、queue stats、drop notice、coalesce policy | 让单写者变成 mailbox drain | 原来会写，现在还要会控流。 |
| `src/api/serve/commands.rs` | 所有 turn 入口统一走 `TurnScheduler` | 去掉 scattered busy/queue 逻辑 | 入口统一，好管。 |
| `src/core/agent_registry/mod.rs` | 预算字段拆分；根 session 不再永久占 running budget | 解耦会话数和 agent 并发数 | 不同账本别再混。 |
| `src/infra/event_bus/mod.rs` | 增加“内部事件”定位说明，移除 UI fan-out 主通路职责 | 明确职责边界 | 总线以后管内部通知。 |
| `src/core/session/manager/context.rs` | 支撑 warm/cold hydrate 的轻量 anchor | 缩短回热路径 | 冷热切换不能总全量读。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `serve.max_live_sessions` | `1024` 起步 | 单机轻壳会话上限 | 轻壳可以很多。 |
| `serve.max_hot_sessions` | 按内存预算 | 单机重 runtime 上限 | 真正热的要克制。 |
| `serve.max_running_turns` | 按 CPU/LLM 预算 | 同时运行 turn 上限 | 跑起来的别太多。 |
| `serve.max_child_agents_per_root` | `8` | 单会话子 agent 预算 | 某个会话别独吞分身。 |
| `serve.idle_warm_ms` | `30_000` | hot -> warm | 先降温。 |
| `serve.idle_cold_ms` | `300_000` | warm -> cold | 再彻底冷却。 |
| `serve.max_pending_bytes_per_session` | `512KiB` | mailbox 字节上限 | 慢消费者别吃爆内存。 |
| `serve.enable_session_mailbox` | `false`→灰度开启 | Phase A 开关 | 先灰度，不要裸切。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 条件 | 对外表现 | 恢复策略 | 说人话 |
|------|------|----------|----------|--------|
| `turn_rejected_busy` | 策略为 `reject` 且会话正忙 | 命令响应 `busy` | 客户端退避或改为 enqueue | 这个会话现在没空。 |
| `turn_enqueued` | 配额不足或策略为 `enqueue` | 响应 `queued=true` + `queuePosition` | 等 admit 事件 | 先在队里等。 |
| `mailbox_drop_notice` | `best_effort` 事件被合并或丢弃 | 显式事件通知 | UI 按需补拉快照 | 有些字没发全，但结局还在。 |
| `heat_transition_failed` | hydrate / evict 出错 | 会话进 `degraded` | 重试或只读回放 | 会话冷热切换坏了。 |
| `stale_control_response` | 老 `runId` 回包晚到 | 忽略并记录 warning | UI 重新同步当前状态 | 老回答别打到新 run 上。 |

## 8. 测试矩阵（验收）

### 8.1 单元测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| `SessionMailbox` 在满队列时只丢 `best_effort` | `lossless` 永不丢，`drop_notice` 被发出 | 能丢的只有字，不是结局。 |
| `seq` 单调递增 | 同一 `sessionKey` 下无回退、无重复 | 补流靠它，不能乱。 |
| `TurnScheduler` 的 `reject/enqueue/interrupt` 分支 | 不同策略返回符合预期 | 同会话撞车要有稳定规则。 |
| `HeatState` 迁移 | hot->warm->cold / warm->hot 正确释放和恢复对象 | 降温和回热要可预测。 |

### 8.2 集成测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 1000 个 `warm` 会话 + 100 个 `hot` 会话 | 内存占用明显低于“1000 全 hot”基线 | warm 层必须真省钱。 |
| 一个会话高频流式输出，另一个会话正常审批 | 两者互不阻塞 | 一个会话吵，不该把另一个会话憋死。 |
| root session 空闲但 child agent 运行 | child budget 不被空闲根 handle 吞掉 | 预算拆分得起效。 |
| idle unload 后再追问 | 会话能回热、历史不断裂 | 冷热切换不能让人失忆。 |

### 8.3 E2E 测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 现有 stdio 客户端无感升级 | 旧字段兼容，新字段被忽略也不报错 | Phase A 不能先把现有 IDE 插件打坏。 |
| `prompt` 忙时仍返回旧式 `busy`，`follow_up` 仍可排队 | 用户体验与现状保持一致 | 内核升级，不先改用户习惯。 |
| 断开后重连当前本地 `serve` 连接 | 依靠 `seq/runId` 可以重建当前可见状态 | 就算本地，也要先验证补流语义。 |

### 8.4 负载与回归

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 10k 冷会话、1k warm、100 hot | 进程稳定，无线性 listener 膨胀 | 这是 Phase A 最关键的收益点。 |
| 慢 writer / 慢 stdout | 不阻塞 lossless 事件，drop notice 增长可观测 | 慢消费者别把系统拖死。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| Phase A 做成“半套 mailbox，仍然大量依赖 EventBus” | 复杂度翻倍，收益却不够大 | 明确切断用户可见主事件通路 | 要么真切过去，要么别做假切。 |
| warm 定义过重 | 节省效果不明显 | warm 只保留轻量 anchor，不保留完整 `ContextState` | 半热也得真轻。 |
| 兼容层过宽 | 实现很丑，长期背包袱 | 兼容只保留一个过渡期，并在 Phase B 移除旧路径 | 先兼容，但别兼容一辈子。 |
| queue 策略改动影响 UX | 用户觉得“怎么行为变了” | 默认值兼容现状，改变策略需显式配置 | 先稳住手感，再逐步开放能力。 |

## 10. 历史决策 / 跨文档修订

1. Phase A 不引入新 transport，不是因为 WebSocket 不重要，而是因为它解决不了当前最大的内核问题。
2. Phase A 刻意保留 `EventBus`，不是对旧实现妥协，而是承认“内部钩子”和“用户推流”本来就是两件事。
3. Phase A 的 `runId / seq / deliveryClass` 会直接成为 Phase B / C 的协议基石，所以宁可现在多加字段，也不要以后再破一次 wire。
