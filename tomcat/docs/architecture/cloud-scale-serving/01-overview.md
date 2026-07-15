# Tomcat 云化改造总览：同一套 Core，兼容本地侧车，演进到云端多租户

> 适用范围：`tomcat` 从“一个 IDE 窗口里的本地 sidecar”演进到“同一套 core 既能继续跑本地 `stdio`，又能支撑云端多 worker / 多租户 / 海量冷会话”的总体方案。
>
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。本文 `## 1`–`## 10` 与规范 §1–§10 一一对应；文首“方案导图集”放在 `## 1` 之前，不占正文编号。
>
> 关联但不重复：
>
> - [`../agent-server-and-ui-gateway.md`](../agent-server-and-ui-gateway.md)：回答“Tomcat 怎样把本地能力暴露给进程外 UI”；本文只复用它的 `stdio/gateway` 边界，不重讲现有 `serve` 线协议细节。
> - [`../session-storage.md`](../session-storage.md)：回答“当前 transcript / `sessions.json` / checkpoint 如何落盘”；本文只说明为什么 Phase B/C 必须把这些能力抽到 `Storage trait` 后面，不重写现状文件格式。
> - [`../multi-agent.md`](../multi-agent.md)：回答“父子 Agent 如何派生、限流和级联取消”；本文只复用 `AgentRegistry`、`spawn_depth`、`MAX_CONCURRENT_AGENTS` 这些既有控制面概念，不重写 reviewer / verifier 细节。
>
> 子文档导航：
>
> - [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md)：单机先还债，先把扇出、内存常驻、背压与本地兼容性收住。
> - [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md)：把 `serve` 从“本地多会话调度器”演进到“带订阅、可分片、可换传输、可换存储边界”的网关雏形。
> - [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md)：把单机多 worker 继续外推到集群、多租户、共享存储、灾备恢复与可观测。

**说人话**：这份总览不负责把每个细节讲透，它负责先钉三件事：第一，Tomcat 现在究竟卡在哪；第二，目标架构应该长什么样；第三，为什么要分 A/B/C 三期而不是“一步上云”。读完这里，再进 02/03/04，就不会在“本地兼容性、云端扩展性、现有代码边界”之间来回迷路。

---

## 先看总图：方案导图集

### 阅读顺序建议

1. **A.1 抽象 ASCII 总图**：先看职责、事实源、热/冷分层、事件路由和调度边界，理解“为什么不是继续调 `EventBus` 参数”。
2. **A.2 具体 ASCII 总图**：再看 Tomcat 现有 `serve` / `ChatContext` / `SessionManager` / `AgentRegistry` / `EventBus` 到未来 `mailbox` / `Storage trait` / `Gateway` / `SubscriptionRegistry` 的真实落点。
3. **B 状态机**：最后看“一个会话在冷 / 温 / 热 / 运行中之间怎么切”，这直接决定几十万会话为什么不会把内存打满。

### A.1 抽象 ASCII 总图

```text
输入：用户请求 / IDE / Web / 远程客户端
   │
   │  ① 同一套业务 core，不拆两套产品
   ▼
Edge（本地 stdio / 云端 gateway）
   │
   ├─ 本地边：1 个 IDE 窗口 ↔ 1 个 `tomcat serve`，行为不回退
   └─ 云端边：1 条连接可 subscribe 多个 session，gateway 不跑 LLM
   │
   │  ② 路由不再靠“同一事件名下挂满所有会话 listener”
   ▼
Session Routing + Subscription Registry
   │
   │  key = session_id
   ▼
Worker（水平扩展）
   ├─ hot pool：少量热会话占 CPU / 内存
   ├─ mailbox：事件按 session 直投
   ├─ turn scheduler：同 session 串行、跨 session 并行
   └─ local hooks：会话本地 EventBus 只服务本 worker / 本会话
   │
   │  ③ 状态不再“开了会话就整份常驻”
   ▼
Residency Manager
   ├─ hot  : AgentLoop + mailbox + runtime
   ├─ warm : metadata + subscription + last checkpoint
   └─ cold : transcript / checkpoint / blobs only
   │
   │  ④ 所有可恢复状态都藏在可替换存储边界后面
   ▼
Storage trait
   ├─ local filesystem backend（兼容现状）
   ├─ shared DB / object store backend（云端）
   └─ persist-then-deliver / pending_writes / resume
```

这张抽象图先钉死四个结论。第一，Tomcat 不是要分裂成“本地版 core”和“云端版 core”两套产品，而是同一套 `AgentLoop`、同一套工具执行、同一套事件定义跑在两个不同 edge 上。第二，真正要替换掉的不是 `AgentEvent` 本身，而是“每个会话在共享 bus 上挂几十个 listener，再靠 `sessionId` 过滤”的扇出模型。第三，真正支撑几十万会话的不是“把 `max_sessions` 调大”，而是“绝大多数会话处于 warm/cold，只有少量 hot 会话真的占内存和运行时”。第四，只有把 transcript / checkpoint / tool blobs 藏到 `Storage trait` 后面，Tomcat 才有机会从本地文件演进到共享存储而不把业务逻辑改烂。

**说人话**：要撑几十万会话，靠的不是“一个更大的 `serve` 进程”，而是“连接、路由、热会话、冷存储、恢复”各干各的。会话多不代表同时有很多 Agent 真在跑，绝大多数只该占磁盘，不该占内存。

### A.2 具体 ASCII 总图

```text
┌─ 本地边（保留）──────────────────────────────────────────────────────────────────────┐
│ VSCode / GUI ── stdio NDJSON ──► src/api/serve/{stdin,commands,writer}.rs          │
│ • 继续复用 `ServeCommand` / `OutFrame` / `ControlFrame`                             │
│ • 本地仍允许“默认全热”，但不再把架构假设写死成“永远只有十几个会话”                  │
└───────────────────────────────┬─────────────────────────────────────────────────────┘
                                │
                                ▼
┌─ 现有共享 core（保留）───────────────────────────────────────────────────────────────┐
│ src/api/chat/context.rs                                                            │
│ • `scope_runtime_for()`：按 cwd 复用 `DefaultEventBus`                              │
│ • `ChatContext::from_config_*()`：装配 `SessionManager` / `AgentRegistry` / runtime │
│                                                                                     │
│ src/api/chat/run_loop/mod.rs + src/core/agent_loop/*                                │
│ • `run_chat_turn_with_message()` / `AgentLoop::run()`                               │
│ • 继续是唯一业务主链，不因为云化再造第二套 loop                                     │
└───────────────────────────────┬───────────────────────┬─────────────────────────────┘
                                │                       │
                                ▼                       ▼
┌─ 现状瓶颈（A 期先改）──────────────────────┐   ┌─ 现状持久化（B/C 期抽象）────────────────┐
│ src/infra/event_bus/mod.rs                │   │ src/core/session/manager/session_impl.rs │
│ • `emit_sync()` 持 `listeners.write()`     │   │ • `sessions.json` + transcript JSONL    │
│   排序并同步执行回调                       │   │ • `with_store_mut()` / append per-file   │
│ • 共享 bus + 每会话多 listener             │   │ • 当前没有共享存储 / pending_writes     │
└──────────────────────┬────────────────────┘   └──────────────────────┬─────────────┘
                       │                                               │
                       ▼                                               ▼
┌─ Phase A（新边界）────────────────────────────────────────────────────────────────────┐
│ [new] src/api/serve/session_mailbox.rs                                               │
│ • `session_id -> bounded mpsc`                                                        │
│ • 事件直投 mailbox，不再对共享 bus 上所有会话空转                                     │
│                                                                                      │
│ src/api/serve/event_pump.rs                                                          │
│ • 从“每会话 48 个 listener”收敛为“每会话一个 sink / 一条出站汇流”                     │
│                                                                                      │
│ src/api/serve/writer.rs                                                              │
│ • 继续单写者，但升级为 mailbox-aware / backpressure-aware                            │
│                                                                                      │
│ src/infra/config/types/runtime.rs                                                    │
│ • 激活 `session_idle_unload_ms`，拆出 `max_hot_sessions_per_worker`                   │
└───────────────────────────────┬──────────────────────────────────────────────────────┘
                                │
                                ▼
┌─ Phase B（单机多 worker / 网关雏形）──────────────────────────────────────────────────┐
│ [new] src/api/serve/subscription_registry.rs                                         │
│ [new] src/api/serve/gateway/*                                                        │
│ [new] src/core/session/storage/{mod,fs,trait}.rs                                     │
│ • `SessionRegistry + SubscriptionRegistry`                                            │
│ • `ServeTransport::Ws` 真启用                                                         │
│ • `Storage trait`：`(session_id, ns, checkpoint_id)` + pending_writes                 │
└───────────────────────────────┬──────────────────────────────────────────────────────┘
                                │
                                ▼
┌─ Phase C（集群 / 多租户）─────────────────────────────────────────────────────────────┐
│ external gateway + worker shard + control-plane DB + transcript/blob object store    │
│ • consistent hash / rebalance                                                        │
│ • `Command(resume)` 式 rehydrate                                                     │
│ • tenant quota / rate limit / observability                                          │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张具体图把方案直接压到真实文件级别。现在的 Tomcat 已经有不错的底座，例如 `ServeCommand`/`OutFrame` 线协议、`AgentRegistry` 的级联取消和并发上限、`ScopedEventEmitter` 自动补 `sessionId`。真正缺的是三层新边界：Phase A 的 `SessionMailbox + residency`，Phase B 的 `SubscriptionRegistry + Storage trait + WS transport`，以及 Phase C 的“外部 gateway / worker / 共享存储 / 多租户控制面”。

**说人话**：不是把现有代码推倒重写，而是先在 `serve` 和 `session` 边上加几道新边界。最核心的 `AgentLoop` 继续保留，它只是不该再背“共享广播总线”“全量常驻状态”“本地文件即唯一存储”这三件超纲工作。

### B. 状态机：会话热度与运行态

```text
                subscribe / prompt / resume
       ┌─────────────── hydrate ───────────────┐
       ▼                                        │
┌──────────┐   keep metadata / queue   ┌────────────┐   acquire hot slot   ┌───────────┐
│   cold   │ ─────────────────────────▶│    warm    │─────────────────────▶│ hot_idle  │
└────┬─────┘                           └─────┬──────┘                      └────┬──────┘
     │                                       │                                  │ prompt / follow_up
     │ idle TTL / no subscriber              │ no subscriber / TTL              ▼
     │                                       │                            ┌────────────┐
     │                            checkpoint / transcript only            │  running   │
     │                                                                   └────┬───────┘
     │                                                                        │ ask_question / approval
     │                                                                        ▼
     │                                                                  ┌──────────────┐
     └──────────────────────────────────────────────────────────────────│ awaiting_user │
                                                                        └────┬─────────┘
                                                                             │ response / cancel
                                                                             ▼
                                                                       ┌───────────┐
                                                                       │ hot_idle  │
                                                                       └────┬──────┘
                                                                            │ idle timeout / pressure
                                                                            ▼
                                                                          warm
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `cold` | `subscribe` / `prompt` / `resume` | `warm` | 只加载元数据、checkpoint 索引、订阅关系，不立刻启动完整 `AgentLoop` | 先把“这个会话是谁、东西在哪”弄清楚，再决定要不要真热起来。 |
| `warm` | 抢到 hot 配额 | `hot_idle` | hydrate `ChatContext` / mailbox / tool runtime | 真正要跑时才把重对象拉进内存。 |
| `hot_idle` | `prompt` / `follow_up` | `running` | 获取 turn token、占用 run 配额、开始流式发事件 | 热会话空闲时几乎不占 CPU，一开跑才真用资源。 |
| `running` | `ask_question` / 审批门禁 | `awaiting_user` | 暂停工具推进，保留 run 上下文与 request_id | 等用户拍板不等于整个会话下线。 |
| `awaiting_user` | `control_response` / `cancel` | `hot_idle` | 恢复或收口当前 run | 用户答完继续；用户取消则结束本轮。 |
| `hot_idle` | `idle timeout` / 压力回收 | `warm` | 卸载 `AgentLoop`、保留订阅与恢复锚点 | 不常用的会话别一直霸着热内存。 |
| `warm` | 长时无订阅 / 长时无请求 | `cold` | 仅保留 transcript / checkpoint / blobs | 几十万会话大多应该待在这。 |

**说人话**：Tomcat 真正要管理的不是“会话个数”，而是“热会话个数”和“正在跑的 run 个数”。只要冷热分层做对，几万、几十万甚至更多会话也只是更多存储条目，不是更多常驻 `ChatContext`。

---

## 1. 术语统一

本节只钉全文后面一定会反复出现、而且最容易被混着用的词。一个词若不先钉死，后面的“调度、恢复、存储、网关”都会各说各话。

| 术语 | 语义 | 数据载体 / 单一事实源 | 行为约束 | 说人话 |
|------|------|----------------------|----------|--------|
| **Session** | 用户连续对话的长期身份；它可以跨多个 run、跨多个连接、跨多个 worker 被重新激活 | 现状：`SessionManager` + transcript + `sessions.json`；未来：`Storage trait` 的 `session_id` 主键 | `session_id` 必须稳定；重新连接、恢复、迁移 worker 都不能换 `session_id` | 会话是“这个聊天房间”，不是“一次具体执行”。 |
| **Run** | 一次具体的执行生命周期：从接收 `prompt` / `follow_up` 开始，到 `agent_end` / `agent_idle` 收口 | 现状：`run_chat_turn_with_message()` / `AgentLoop::run()`；未来补 `run_id` | 一个 session 可以有很多 run；同一时刻同一 session 只允许一个 active run | run 是“这次真的开跑的回合”。 |
| **Mailbox** | 按 `session_id` 定向投递的会话出站队列；承担事件聚合、背压和有界缓存 | Phase A 新增 `session_id -> bounded mpsc` | 只允许定向投递，不允许退回“按事件名扫全体 listener” | 每个会话有自己的收件箱，别再把全体邮件先扔大厅里再认领。 |
| **Subscription Registry** | `conn -> {session_ids}` 与 `session_id -> {conn_ids}` 的双向订阅表 | Phase B 新增 `SubscriptionRegistry` | 连接级订阅不等于会话运行态；一个连接可订多个 session | 哪个客户端订了哪个会话，要有专门总账。 |
| **Hot / Warm / Cold** | 会话热度分层：重运行态全在 / 部分在 / 仅磁盘在 | `session_idle_unload_ms`、热度管理器、`Storage trait` | 数量上永远是 `cold >> warm >= hot`；系统容量按 hot 配额算，不按 session 总数算 | 多数会话应该是冷的，少数才是热的。 |
| **Worker** | 一个可独立执行 AgentLoop、持有 hot pool 与 mailbox 的运行单元 | 本地 Phase A 可等同当前 `serve` 进程；Phase B/C 可是进程或 pod | 同 session sticky 到单 worker；跨 worker 恢复只能经 storage / checkpoint | 真跑活的是 worker，不是 gateway。 |
| **Gateway** | 连接、鉴权、握手、订阅、哈希路由的入口，不运行 LLM | 现状仅 `stdio serve`；Phase B 启用 WS；Phase C 外部化 | 不持有业务真状态，不承担 transcript 真相源 | gateway 是门卫和转运站，不是工厂。 |
| **Tenant** | 共享一套基础设施但需要独立隔离、配额和审计边界的主体 | Phase C 控制面 schema / token / quota | session、connection、run、blob 都必须可追溯到 tenant | 多租户不是“多几个用户”，而是每条流都要能追责和限流。 |
| **Storage trait** | 把 transcript、checkpoint、pending_writes、resume 索引藏到后端无关接口后的边界 | Phase B 新增 trait；现状本地文件实现做默认 backend | local fs 与 shared DB/object store 必须共用同一套语义，不允许两套恢复逻辑 | 先把存储边界抽出来，后面换后端才不会大出血。 |
| **Persist-then-deliver** | 先把需要恢复的事实写成 durable state，再把事件发给 UI / 连接 | 现状部分 transcript append；未来统一到 `Storage trait` + writer | 任何需要重放 / 恢复的事件，都不能只 live 发不落锚点 | 真相先入账，再广播，不然挂了就没法复活。 |
| **`run_id` / `parent_ids`** | 事件级解复用字段：标识本事件属于哪一次 run，以及它在父子 run 树上的位置 | Phase A 事件信封升级；参考 LangChain | 同 `session_id` 不互斥；一个 session 内可有多条历史 run | `session_id` 只说“哪个房间”，`run_id` 才说“这次是哪一轮”。 |

**说人话**：这套方案最容易混的有三对词。第一对是 `session` 和 `run`，前者是长期身份，后者是一次执行。第二对是 `gateway` 和 `worker`，前者接流量，后者跑 Agent。第三对是 `hot` 和 `active run`，热会话可以空闲，活跃 run 才真正消耗执行配额。

---

## 2. 竞品 / 选型对比（调研）

本节只放“为什么我们会收敛到这套方案”的调研证据，不在这里做最终裁决；最终裁决放到 `## 3`。

### 2.1 竞品横向表

| 竞品 / 仓库 | 形态 | 关键设计 | 我们借鉴的点 | 暂不照抄的点 | 说人话 |
|-------------|------|----------|--------------|--------------|--------|
| **codex**（`/Users/yankeben/workspace/codex/codex-rs`） | Rust、本地与服务端两栖、同域 | `ThreadManager`、每会话 SQ/EQ、两级注册表、`ThreadStore`、先持久化再发事件、热/冷卸载 | Phase A 的 `SessionMailbox`，Phase B 的 `SessionRegistry + SubscriptionRegistry`，以及 `persist-then-deliver` | codex 的协议层已是成熟 app-server；Tomcat 不能直接把它的全部协议和线程模型硬拷进现有 wire | 它像“Tomcat 的成熟同类版本”，能直接借最多。 |
| **LangGraph**（`/Users/yankeben/workspace/langgraph/libs`） | 状态图运行时、强恢复导向 | `BaseCheckpointSaver`、hydrate-on-demand、`Command(resume)`、`MultitaskStrategy`、`stream_resumable` | Phase B 的 `Storage trait` 语义、Phase C 的跨 worker 恢复、run 与 session 分离 | LangGraph 不等于 IDE agent；它没给我们现成的 `serve` / event bus / UI 协议 | 它最值得抄的是“状态别常驻、凭 checkpoint 随时复活”。 |
| **OpenClaw**（`/Users/yankeben/workspace/openclaw`） | 网关优先、订阅式流式输出 | connect/subscribe、`broadcastToConnIds`、`dropIfSlow`、session lane、控制面/数据面分层 | Phase B 的订阅路由、Phase A 的慢消费者治理、Phase C 的控制面/数据面拆分 | openclaw 明确不做多租户；它的单 gateway 形态和扩展性上限不能直接照抄 | 它教我们怎么拆 gateway，但不替我们解决多租户。 |
| **LangChain / LangGraph SDK 事件层**（`/Users/yankeben/workspace/langchain/libs/core`） | 事件与配置原语 | `run_id`、`parent_ids`、`RunnableConfig`、`ContextVar`、`gather_with_concurrency` | Phase A 事件信封升级、全局并发信号量、上下文跨 async 传播 | 它偏库，不给现成会话存储和连接层 | 它补的是“这条事件到底属于哪次 run”这类基础原语。 |
| **Tomcat 现状**（本仓） | 本地 sidecar、单进程多会话初版 | `ServeCommand` / `OutFrame`、`WriterHandle` 单写者、`ChatContextRegistry`、`AgentRegistry`、`ScopedEventEmitter` 自动补 `sessionId` | 继续保留线协议、单写者、会话路由和取消传播这些好底座 | 当前 `emit_sync` 持锁回调、`sessions.json`、全量 `ChatContext` 常驻、无热/冷分层，不够支撑云端 | 地基不是零，但楼板和配电不够高层建筑用。 |

### 2.2 四条最关键的借鉴结论

1. **先抄 codex 的“每会话队列 + 两级注册表”，再谈扩容。**  
   因为 Tomcat 与 codex 同样是 Rust、同样是编码 agent、同样有本地与服务端两端诉求，`core/src/thread_manager.rs`、`app-server/src/thread_state.rs`、`thread-store/src/store.rs` 这些设计不只是“理念像”，而是代码边界也高度对齐。

2. **状态恢复要按 LangGraph 的思路做，而不是把 `ChatContext` 永远挂内存里。**  
   `BaseCheckpointSaver`、`get_tuple/put/put_writes`、`Command(resume)` 给出的不是具体数据库选型，而是一套“恢复语义优先于内存便利”的约束。这正好补 Tomcat 当前 `session_idle_unload_ms` 还是 TODO 的空档。

3. **连接层与慢消费者治理优先借 OpenClaw，但多租户能力不能照搬。**  
   `subscribe`、`broadcastToConnIds`、`dropIfSlow`、lane queue 很适合 Tomcat Phase B；但 `fly.toml` 那类单 gateway 假设不适合我们的云端目标，所以只能借模式，不能照抄部署模型。

4. **事件信封必须补 `run_id` / `parent_ids`，否则海量并发时很难做准确重放、排障和 UI 解复用。**  
   现在 Tomcat 的 `ScopedEventEmitter` 已经把 `sessionId` 写进 payload，这很好；但只靠 `sessionId` 不够支撑“一个会话有很多历史 run、还有 reviewer/verifier 子 run”的现实。

### 2.3 为什么不是“继续调大现有 serve”

1. **继续扩大 `serve.max_sessions` 只能把同样的架构问题放大。** `src/api/serve/registry.rs` 当前只是把 `sessionId -> SessionSlot` 管起来，没解决热/冷分层，更没解决共享 bus 扫描和持锁回调。
2. **继续把事件全部挂在共享 `DefaultEventBus` 上，不符合当前实现细节。** `src/infra/event_bus/mod.rs` 的 `emit_sync()` 仍然在拿 `listeners.write()` 后排序并执行回调；监听器再多，尾延迟只会更难看。
3. **继续把 transcript / session metadata 只看作本地文件，不利于 worker 恢复。** `src/core/session/manager/session_impl.rs` 的 `sessions.json` 与 transcript JSONL 很适合本地 sidecar，但它们目前不是 cluster-safe 的控制面。
4. **继续让 `ChatContext` 长驻内存，不符合“几十万会话里只有少量热点”的真实负载。** `src/api/chat/context.rs` 当前会装配完整 runtime，这在本地很合理，但在云端必须由 `hot/warm/cold` 改变默认假设。

**说人话**：竞品真正给我们的不是“某个炫酷名词”，而是边界感。codex 告诉我们会话应该有私有队列；LangGraph 告诉我们状态应该能离开内存独立存活；OpenClaw 告诉我们连接层和订阅层应该分出来；LangChain 告诉我们海量并发时事件必须能准确归因。

---

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章先用 `§3.1` 钉死维度上的最终取舍，再用 `§3.2` 把取舍落到 A/B/C 三期实施点与验收锚点。`§3.1` 负责回答“为什么这么选”；`§3.2` 负责回答“到底改哪、怎么分期合”。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **R1 产品形态** | 本地 sidecar 与云端 agent 是否拆两套 core | **采用** 同一套 `AgentLoop` / 工具执行 / 事件定义，同时支持本地 `stdio` edge 与云端 gateway edge；**拒绝** 分裂成本地版 core / 云端版 core 两套实现。 | 本仓：`tomcat/src/api/serve/mod.rs`、`tomcat/src/api/chat/run_loop/mod.rs`；外部：`codex-rs/app-server/src/in_process.rs`、`openclaw/src/gateway/server.impl.ts` | 设计：边界改在 edge、routing、storage、scheduler，不改业务主循环；理由：Tomcat 已有可复用的 `serve` / `AgentLoop` / `AgentRegistry` 底座，拆两套 core 只会让协议、测试与恢复逻辑分叉。 | 未入选：做“本地维持现状、云端另写一套服务端 agent”；拒因：协议与恢复双份实现，长期会把 `AgentEvent`、checkpoint、工具栈全部做成两套真相源。 | 本地和云端要换的是“外壳和基础设施”，不是“脑子和手脚”。 |
| **R2 事件扇出** | 事件应该继续靠共享 EventBus listener 过滤，还是改成按 session 定向投递 | **采用** `SessionMailbox` 按 `session_id` 定向投递，`EventBus` 降级为会话本地 hook；**拒绝** 继续依赖“共享 bus + 多 listener + `sessionId` 过滤”作为主扇出模型。 | 本仓：`tomcat/src/api/serve/event_pump.rs`、`tomcat/src/infra/event_bus/mod.rs`；外部：`codex-rs/core/src/session/mod.rs`、`openclaw/src/gateway/server-broadcast.ts` | 设计：每会话有 bounded mailbox，writer / transport 从 mailbox 读；理由：当前 `emit_sync()` 仍会在共享表上排序并执行回调，直投 mailbox 才能把复杂度收敛到 `O(1)` per hot session event。 | 未入选：只继续扩 event allowlist、只给 writer 加 coalesce；拒因：能缓一点流量，但治不好共享 bus 与常驻 listener 的结构性问题。 | 事件就该直投到目标会话的收件箱，而不是先广播再认领。 |
| **R3 会话驻留** | 会话是否继续默认常驻完整 `ChatContext` | **采用** `hot / warm / cold` 分层 + hydrate-on-demand；**拒绝** “会话一旦创建就整份 runtime 常驻直到手动关闭”。 | 本仓：`tomcat/src/api/chat/context.rs`、`tomcat/src/infra/config/types/runtime.rs`；外部：`langgraph/libs/checkpoint/langgraph/checkpoint/base/__init__.py`、`codex-rs/app-server/src/request_processors/thread_lifecycle.rs` | 设计：把 `session_idle_unload_ms` 真正落地，热会话有硬配额；理由：云端容量应按 hot 数量算，而不是按 session 总量算，且本仓已明确 `session_idle_unload_ms` 目前只是 TODO。 | 未入选：只做更大的机器、更多内存、继续全量常驻；拒因：热会话比例再小也会被冷会话拖进 OOM，且不利于 worker 漂移与恢复。 | 大多数会话应该只躺在磁盘上，真正热起来时再把它叫醒。 |
| **R4 存储边界** | 是否继续把 filesystem 当作唯一真实存储 | **采用** `Storage trait` 把 transcript / checkpoint / pending_writes 语义抽出来，默认 backend 仍是本地文件；**拒绝** 在业务逻辑里继续直接依赖 `sessions.json + *.jsonl`。 | 本仓：`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/api/serve/types.rs`；外部：`codex-rs/thread-store/src/store.rs`、`langgraph/libs/checkpoint-postgres/.../base.py` | 设计：Phase B 抽象 trait，先给出 fs backend；理由：只有先定义稳定语义，后续换 SQLite / Postgres / object store 才不会反复改 AgentLoop 与 hydrate 逻辑。 | 未入选：等 Phase C 再一次性切 DB；拒因：那样会把恢复语义、gateway 路由与持久化耦成一坨，迁移风险最大。 | 先把“存什么、怎么恢复”抽成接口，再谈后端是文件还是数据库。 |
| **R5 调度模型** | 是把 session 与 run 混成一个对象，还是拆出独立 run 生命周期 | **采用** Session 是长期身份、Run 是短周期执行，且同 session 串行、跨 session 并行；**拒绝** 只靠 `busy` 布尔值长期兜底所有调度语义。 | 本仓：`tomcat/src/api/serve/registry.rs`、`tomcat/src/core/agent_registry/mod.rs`；外部：`langgraph/libs/sdk-py/langgraph_sdk/schema.py`、`openclaw/docs/concepts/queue.md` | 设计：保留 session 级长期状态，同时为每次执行发放 `run_id`、队列与策略；理由：一旦引入恢复、排队、审批、子 run，session 与 run 不拆开就很难准确做配额与回放。 | 未入选：永远只有“会话忙/不忙”两态；拒因：不足以表达队列、恢复、审批阻塞、子 run 归属与尾延迟统计。 | 一个房间里可以发生很多次执行，别把“房间”和“这次任务”混成一件事。 |
| **R6 连接与路由** | 一条连接是否仍只服务一个会话 | **采用** `SessionRegistry + SubscriptionRegistry`，允许一条连接订阅多个 session，gateway 按 `session_id` 哈希路由；**拒绝** “连接 = 会话 = runtime” 的一对一模型。 | 本仓：`tomcat/src/api/serve/types.rs`、`tomcat/src/api/serve/writer.rs`；外部：`codex-rs/app-server/src/thread_state.rs`、`openclaw/docs/gateway/protocol.md` | 设计：连接层只负责 subscribe / unsubscribe / control，运行态仍归 worker；理由：Web / IDE tab / 移动端都会要求“一条连接看多个会话”，而 runtime 是否 hot 应由 worker 决定。 | 未入选：继续把 `serve` 看成“一个连接只看一个会话”；拒因：难以支撑 tab、多面板和未来远程客户端，也让 gateway 扩展性极差。 | 连接是浏览窗口，不该等于会话本身。 |
| **R7 一致性与恢复** | 事件应先发再落盘，还是先持久化再对外发布 | **采用** 对可恢复事实执行 `persist-then-deliver`，并引入 `pending_writes` / resumable stream；**拒绝** 只做 live UI 推送而不给恢复锚点。 | 本仓：`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/api/serve/writer.rs`；外部：`codex-rs/core/src/session/mod.rs`、`langgraph/libs/checkpoint/langgraph/checkpoint/base/__init__.py` | 设计：checkpoint / transcript / pending writes 先入 durable state，再发 event；理由：worker 崩溃后若只能依赖 UI 曾经看过的 live 事件，恢复必然有洞。 | 未入选：live first / best effort；拒因：短期看着快，长期无法做跨 worker 恢复、断线续流和一致审计。 | 真相要先落账，UI 只是订阅者，不是唯一记忆。 |
| **R8 分期策略** | 是不是应该直接一步做完云端集群 | **采用** Phase A 先在单机把扇出、热度、背压、兼容性收住，再进 Phase B 的 gateway / storage trait，最后 Phase C 集群化；**拒绝** 不区分阶段的一步到位云化。 | 本仓：`tomcat/src/api/serve/mod.rs`、`tomcat/src/infra/config/types/runtime.rs`；外部：`codex-rs/app-server/*`、`langgraph/libs/checkpoint/*`、`openclaw/docs/refactor/database-first.md` | 设计：每期都能单独验收，并给下一期提供稳定边界；理由：Tomcat 当前最急的是 Phase A 的本地架构债，不先还债就上网关和集群，只会把复杂度放大。 | 未入选：直接先上 gateway / DB / K8s；拒因：本地 sidecar 兼容性、协议稳定性、事件一致性都还没钉死，过早外推风险过大。 | 先把单机改对，再把单机改大，最后再把集群改稳。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PH-A 单机收口** | `SessionMailbox`、热/温/冷降级、`run_id/parent_ids` 事件信封、全局并发信号量、零回退本地 `stdio` 行为 | `src/infra/event_bus/mod.rs`、`src/api/serve/{event_pump,writer,registry,mod}.rs`、`src/infra/config/types/runtime.rs`、`src/api/chat/context.rs` | 现有：`api::serve::tests::writer_test::serve_writer_round_robins_across_sessions`、`api::serve::tests::event_pump_test::serve_lifecycle_events_not_dropped_for_other_sessions`；新增：见 [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md) `§8` | 先把本地单机的结构性问题改对，后面扩容才有地基。 |
| **PH-B 网关与存储边界** | `SessionRegistry + SubscriptionRegistry`、`subscribe`/`unsubscribe`、`ServeTransport::Ws`、`Storage trait`、Run 与 Session 分离、MultitaskStrategy | `src/api/serve/{types,control,mod}.rs`、`[new] src/api/serve/gateway/*`、`[new] src/core/session/storage/*`、`src/core/agent_registry/mod.rs` | 现有：`api::serve::tests::control_test::serve_initialize_control_request_sets_ready_state`、`api::serve::tests::schema_test::serve_emitted_event_validates_against_generated_schema`；新增：见 [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md) `§8` | 把“本地多会话 serve”升级成真正可路由、可订阅、可换后端的 gateway 雏形。 |
| **PH-C 集群与多租户** | 外部 gateway、一致性哈希、控制面 DB、数据面 transcript/blob store、断线重连、跨 worker `resume`、tenant quota / rate limit / observability | `src/api/serve/*`、`[new] src/cloud/*`、`[new] src/core/session/storage/*`、部署与控制面 schema | 新增：见 [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md) `§8` | 最后才进入“多机器、多租户、共享存储、灾备”这层复杂度。 |
| **DOC-MAP 文档入口** | `docs/architecture/README.md` 补充“扩展性与云化”阅读顺序，父文档向下导航 | `tomcat/docs/architecture/README.md` | 本次文档交付即验收 | 文档先把导航理顺，后面评审和落地才不会跳来跳去。 |

#### 3.2.1 Phase A：先还本地架构债

Phase A 的目标不是“让 Tomcat 立刻上云”，而是先把最不适合扩展的三件事切开：共享事件扇出、全量会话常驻、缺乏明确 run/backpressure 边界。完成 Phase A 后，即便还没上 WS、没上 DB，本地 `serve` 也已经从“只能勉强承受少量热会话”的结构，变成“可以明确区分热会话数、总会话数、活跃 run 数”的结构。

#### 3.2.2 Phase B：把边界抽完整

Phase B 不是单纯“加一个 WS 开关”，而是让 `serve` 从“本地 stdio 多会话调度器”演进到“带会话路由、连接订阅、稳定存储边界、可换传输适配器”的 gateway 雏形。到这一步，Tomcat 才真正有资格说“同一套协议既能本地跑，也能远程跑”。

#### 3.2.3 Phase C：把容量、隔离和恢复做成一等公民

Phase C 关注的是云端基础设施该有的那一层：tenant、quota、rehydrate、rebalance、durable checkpoints、断线续流、跨 worker 审计。这期最重要的不是多写几个 endpoint，而是让“任何一个 worker 都能在另一个 worker 掉线后接手 session”变成可靠事实。

---

## 4. 协议（总览层）

本文只钉跨 A/B/C 三期都不应再反复改名的核心字段与 envelope；具体 `subscribe`、`Storage trait`、租户契约的细表分别见 03 / 04。

### 4.1 核心字段表

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionId` | `string` | 是（事件）；命令侧除 `new_session` 等全局命令外应提供 | 无 | 本地 `serve`、WS gateway、storage key 前缀 | 会话稳定主键；`ScopedEventEmitter` 已在现状里写入 payload | 每条消息都要先说“我属于哪个会话”。 |
| `runId` | `string` | Phase A 起：所有 run 级事件必填 | 无 | `agent_start`、`message_*`、`tool_execution_*`、`agent_end` | 区分同一 session 内的不同执行回合 | `sessionId` 说房间，`runId` 说这次任务。 |
| `parentIds` | `string[]` | 子 run / 子 agent 事件必填；根 run 为空数组 | `[]` | reviewer / verifier / follow-up / future child run | 表示事件在父子执行树中的路径 | 让 UI 和审计知道“这次 run 是谁派生出来的”。 |
| `requestId` | `string` | 控制帧必填 | 无 | `control_request` / `control_response` / `control_cancel` | 继续复用现有 ask-question / initialize / approval 语义 | 所有需要回包的控制动作都要能精确对上号。 |
| `checkpointNs` | `string` | Storage trait 的 checkpoint 操作必填 | `"default"` | resume / restore / pending_writes | 参考 LangGraph 的 `(thread_id, checkpoint_ns, checkpoint_id)` 键结构 | 同一会话里允许多类 checkpoint 并存。 |
| `checkpointId` | `string` | restore / resume / pending_writes 关联时必填 | 无 | checkpoint / resume / rollback | 唯一标识一个恢复锚点 | 想复活哪次状态，得有明确锚点。 |
| `tenantId` | `string` | Phase C 网关与控制面必填 | 无 | 鉴权、配额、路由、审计 | 本地单机可为空；云端必须可追溯 | 多租户世界里，任何资源都得知道“是谁家的”。 |

### 4.2 核心 envelope

```text
Command / Control / Event / StorageKey

command:
  { "type": "...", "sessionId": "...", ... }

control:
  { "type": "control_request|control_response|control_cancel",
    "requestId": "...", "sessionId": "...", ... }

event:
  { "type": "...", "sessionId": "...", "runId": "...", "parentIds": [...], ... }

storage key:
  (session_id, checkpoint_ns, checkpoint_id)
```

### 4.3 调用样例

```jsonc
// Phase A：本地 sidecar 仍沿用现有命令形状，只补 run 级事件字段
{ "type": "prompt", "id": "cmd-7", "sessionId": "s-1", "text": "继续上一轮" }

// Phase B：云端 gateway 的连接级订阅（示意）
{
  "type": "subscribe",
  "requestId": "req-9",
  "sessionIds": ["s-1", "s-2"],
  "cursor": { "s-1": "seq:120", "s-2": "seq:9" }
}

// Phase A/B：出站事件（示意）
{
  "type": "message_update",
  "sessionId": "s-1",
  "runId": "run-20260715-001",
  "parentIds": [],
  "assistantMessageEvent": {
    "kind": "content_delta",
    "delta": "hello"
  }
}
```

单一事实源约束：

- **现状协议真相源**：`tomcat/src/api/serve/types.rs` 与 `tomcat/src/infra/events/mod.rs`
- **Phase B 新增真相源**：`[new] src/api/serve/protocol.rs`（连接级订阅契约）与 `[new] src/core/session/storage/mod.rs`
- **Phase C 新增真相源**：控制面 schema / tenant contract 文档与对应实现模块

**说人话**：总览层真正要钉死的是那些以后不能再改名改语义的关键键。只要 `sessionId / runId / parentIds / requestId` 这些骨架稳定，后面无论走本地 `stdio` 还是云端 WS，UI 和恢复逻辑都不会反复返工。

---

## 5. 文件职责总览（One-Glance Map）

```text
┌─ src/api/chat/context.rs ───────────────────────────────────────────────────────────┐
│ • `scope_runtime_for()`：按 cwd 复用 `DefaultEventBus`                              │
│ • `ChatContext::from_config_*()`：装配 Session / EventBus / AgentRegistry / runtime │
│ • Phase A：引入热/温/冷与按需 hydrate 的装配切口                                    │
└───────────────────────────────┬─────────────────────────────────────────────────────┘
                                │
                                ▼
┌─ src/api/chat/run_loop/mod.rs ──────────────────────────────────────────────────────┐
│ • `run_chat_turn_with_message()`：顶层 turn 入口                                    │
│ • 继续是业务真主链；Phase A 起补 `runId/parentIds` 事件上下文                        │
└───────────────────────────────┬───────────────────────────────┬────────────────────┘
                                │                               │
                                ▼                               ▼
┌─ src/infra/event_bus/mod.rs ──────────────────┐   ┌─ src/core/agent_registry/mod.rs ─────────┐
│ • `ScopedEventEmitter`：写入 `sessionId`       │   │ • `register_root()` / `rearm_root()`      │
│ • `emit_sync()`：现状持锁回调                  │   │ • `MAX_CONCURRENT_AGENTS` / cascade_abort │
│ • Phase A：改快照/锁外回调 + run envelope      │   │ • Phase A/B：接入 run 与全局调度语义      │
└──────────────────────┬────────────────────────┘   └─────────────────────┬──────────────┘
                       │                                                  │
                       ▼                                                  ▼
┌─ src/api/serve/{event_pump,writer,registry,types,control}.rs ───────────────────────┐
│ • 现状：多会话 `serve`、单写者、控制回环、schema                                    │
│ • Phase A：`SessionMailbox`、hot slot、backpressure、`agent_idle` 稳定收口           │
│ • Phase B：`SubscriptionRegistry`、WS transport、subscribe/unsubscribe               │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─ [new] src/api/serve/session_mailbox.rs ─────────────────────────────────────────────┐
│ • `session_id -> bounded mpsc`                                                       │
│ • 定向投递、delta merge、slow consumer policy                                        │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─ src/core/session/manager/session_impl.rs ───────────────────────────────────────────┐
│ • 现状：`sessions.json`、transcript JSONL、append / hydrate                           │
│ • Phase B：抽出 fs backend 到 `Storage trait`                                         │
│ • Phase C：为 shared backend、pending_writes、resume 锚点提供语义基线                │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─ [new] src/core/session/storage/{mod,fs,trait}.rs ───────────────────────────────────┐
│ • 统一 `session metadata / transcript / checkpoint / pending_writes` 接口             │
│ • fs backend 兼容现有落盘；shared backend 面向云端                                    │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─ [new] src/api/serve/gateway/* + [new] src/cloud/* ──────────────────────────────────┐
│ • Phase B：连接握手 / 订阅 / 哈希路由 / transport adapter                             │
│ • Phase C：外部 gateway、租户鉴权、控制面 DB、一致性哈希、可观测                      │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─ tests / src/api/serve/tests / src/infra/event_bus/tests ────────────────────────────┐
│ • 本地零回退回归                                                                    │
│ • mailbox / hydration / routing / resume / quota / load tests                         │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张图的阅读顺序是：先看 `ChatContext` 和 `run_loop` 这两个不希望被推倒重写的核心入口，再看 `event_bus` 与 `serve` 这些扩容时最该重新切边的地方，最后落到 `session storage` 与未来 `gateway/cloud`。它故意把“要尽量保留的 core”和“必须新增的新边界”分开画，避免 reviewer 误以为这个方案要把 Tomcat 彻底重做。

**说人话**：真正会大改的是 `serve + event_bus + session storage` 这三块；真正尽量别动的是 `AgentLoop` 那条业务主链。云化成功的关键不是“多写几层 adapter”，而是把这三块边界切干净。

---

## 6. 配置与环境变量

总则：**env > config > 默认值**。本节只列本方案需要稳定化或新增的关键项；更细字段见各分册。

| 变量 / 配置 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `[serve].transport` | `stdio` / `ws` | 现有 edge 传输形态；Phase B 后 `ws` 从“预留”转“可用” | config | 先保留本地 `stdio`，再逐步放开 `ws`。 |
| `[serve].max_sessions` | `usize` | 现状 live session 上限；Phase A 起语义过渡为“兼容别名” | config | 旧名字先兼容，但不再代表真正的容量模型。 |
| `[serve].max_hot_sessions_per_worker` | `usize` | Phase A 新增；单 worker 热会话硬配额 | env / config | 真正限制内存和 runtime 的是它。 |
| `[serve].session_idle_unload_ms` | `u32` | 从 TODO 变成真实行为：会话空闲多久降级 warm/cold | env / config | 热会话多久不活跃就卸载。 |
| `[serve].delta_coalesce_ms` | `u32` | delta 合并窗口 | config | 慢消费者时把碎片流合一合。 |
| `[serve].max_buffered_frames` | `usize` | writer / mailbox 可缓存的最大帧数 | config | 过了就必须背压，而不是一直堆内存。 |
| `[serve].mailbox_capacity` | `usize` | Phase A 新增；单 session mailbox 容量 | env / config | 每个会话收件箱有多大。 |
| `[serve].turn_queue_capacity` | `usize` | Phase A 新增；每 session / 每 worker 的待执行 turn 队列上限 | env / config | 同一个会话排队也得有上限。 |
| `[scheduler].max_concurrent_turns` | `usize` | Phase A/B 新增；worker 级 active run 上限 | env / config | 同时真在跑的回合数。 |
| `[storage].backend` | `fs` / `sqlite` / `postgres` / `hybrid` | Phase B/C 存储后端 | env / config | 先文件，后共享存储。 |
| `[gateway].bind` / `[gateway].token` | 地址 / token | Phase B/C 网关监听与鉴权 | env / config | 联网后才需要它们。 |
| `[tenant].default_quota_profile` | profile id | Phase C 多租户默认配额 | env / config | 每个租户的默认饭量。 |

**说人话**：现在最关键的新配置不是“开不开 WS”，而是“热会话多少、空闲多久降温、队列多长、背压怎么触发”。这些量一旦不显式配置，云化就会退化成“看运气的单机扩容”。

---

## 7. 错误模型 / 截断 / 警告

```text
正常路径
  prompt → accepted/queued → run events → agent_end → agent_idle

可恢复告警
  mailbox 满 / 慢消费者
    → 合并 delta
    → 仍满：drop delta + `llm_notice(backpressure)`
    → 生命周期帧 / control 帧必须保序必达

协议级错误
  unknown_session / unknown_subscription / not_initialized / busy
    → response{success:false,error:"..."}

恢复级错误
  hydrate 失败 / checkpoint 缺失 / storage backend 超时
    → emit terminal error for that session
    → session 留在 warm/cold，可再次重试

一致性级错误
  persist 失败
    → 禁止继续 deliver 需恢复事实的事件
    → 记录 warning / metrics / audit
```

| 结局 | 触发条件 | 对外形态 | 是否可重试 | 说人话 |
|------|----------|----------|------------|--------|
| `busy` | 同一 session 已有 active run，且策略非 `enqueue` | `response.error("busy")` 或显式 `queued` ack | 是 | 同一房间里一次只跑一件事。 |
| `backpressure` | mailbox / writer 缓冲达到上限 | `llm_notice` + metrics；必要时 drop delta，但不丢 lifecycle | 是 | 碎字可以少发，但开始/结束这些大事不能丢。 |
| `unknown_session` | 连接订阅或命令命中不存在的 session | `response.error("unknown_session")` | 视场景而定 | 会话不存在就别假装成功。 |
| `hydrate_failed` | 从 warm/cold 恢复时缺 transcript/checkpoint 或 backend 出错 | terminal event + retryable status | 是 | 唤醒失败要明确告诉外面，而不是静默假死。 |
| `persist_failed` | 应落 durable state 的事实未成功写入 | run 失败或进入 degraded mode | 视策略而定 | 账没记上，就不该假装事件已经安全发出。 |
| `resume_gap` | 断线重连时请求的 cursor 已超出保留窗口 | snapshot + warning + best-effort replay | 是 | 找不到精确断点时，要明确告诉客户端“我给你快照补齐”。 |

**说人话**：云化后最危险的不是报错本身，而是“看起来没报错，其实消息丢了、恢复不了了、订阅断了没人知道”。所以这套方案明确要求：任何需要恢复的事实，要么先落库再发，要么明确告诉外界它只是 live、不能重放。

---

## 8. 测试矩阵（验收）

本节按“已有回归锚点 + 必补新增锚点”的方式列验收口径。状态列只用 `✅ 2026-07-15` 或 `PENDING`。

| 层级 | 目标 | 锚点（测试函数名 / 文件） | 状态 | 说人话 |
|------|------|---------------------------|------|--------|
| 单元 | 现有 `EventBus` 不因为单 listener 失败中断全局 | `infra::event_bus::tests::single_listener_error_does_not_abort_others` | ✅ 2026-07-15 | 先守住已有容错基线。 |
| 单元 | 现有 `ScopedEventEmitter` 稳定写入 `sessionId` | `infra::event_bus::tests::scoped_event_emitter_writes_session_id_to_payload_and_context` | ✅ 2026-07-15 | 现有会话路由标签别回退。 |
| 单元 | 现有 writer 跨 session 公平轮转 | `api::serve::tests::writer_test::serve_writer_round_robins_across_sessions` | ✅ 2026-07-15 | 现有单写者公平性是后面 mailbox 的基线。 |
| 集成 | 现有 `serve` 仍按 `sessionId` 路由命令 | `api::serve::tests::commands_test::serve_command_routes_by_session_id` | ✅ 2026-07-15 | 本地多会话路由能力已经有底子。 |
| 集成 | 现有生命周期事件不会因为别的 session 活跃而丢失 | `api::serve::tests::event_pump_test::serve_lifecycle_events_not_dropped_for_other_sessions` | ✅ 2026-07-15 | 多会话时至少要先守住收口事件。 |
| 集成 | 初始化握手与控制通道稳定 | `api::serve::tests::control_test::serve_initialize_control_request_sets_ready_state` | ✅ 2026-07-15 | 本地协议门禁不能被云化破坏。 |
| 新增单元 | mailbox 定向投递不再扫描共享 bus listener | `api::serve::tests::mailbox_test::session_mailbox_routes_without_global_listener_scan` | PENDING | Phase A 第一块核心回归。 |
| 新增集成 | `session_idle_unload_ms` 触发 warm/cold 降级并可 rehydrate | `api::serve::tests::residency_test::idle_session_unloads_and_rehydrates` | PENDING | 热/冷分层必须可测，不靠人工想象。 |
| 新增集成 | `run_id/parent_ids` 在 root / reviewer / verifier 事件树中完整传递 | `api::serve::tests::event_identity_test::run_tree_fields_roundtrip_across_subagents` | PENDING | 大并发下排障和 UI 归因全靠这组字段。 |
| 新增集成 | subscribe 只向订阅连接投递，且断线可从 cursor 恢复 | `api::serve::tests::subscription_registry_test::subscribe_routes_only_to_bound_connections` | PENDING | Phase B 的连接层不能靠肉眼验。 |
| 新增契约 | `Storage trait` 保证 `persist-then-deliver` 与 `pending_writes` 语义 | `core::session::storage::tests::persist_then_deliver_records_pending_writes` | PENDING | 恢复语义是云化最核心的硬约束。 |
| 新增 E2E | worker 崩溃后可跨 worker `resume` 恢复 session | `tests/cloud_scale_serving_recovery_e2e.rs::resume_after_worker_restart` | PENDING | 没有这条，Phase C 只是“看起来像云”。 |
| 新增压测 | `10^5` 冷会话 + 数百热会话下内存与尾延迟稳定 | `tests/cloud_scale_serving_load.rs::cold_100k_hot_500_tail_latency_budget` | PENDING | 这是整个方案是否成立的最终答案。 |

**说人话**：验收重点不是“文档写得像不像”，而是四类真问题都要被钉住：本地有没有回退、热点扇出有没有变成定向、恢复语义有没有 durable、极端容量下热/冷模型是否真能扛住。

---

## 9. 风险与应对

| 风险 | 影响 | 应对策略 | 触发信号 | 说人话 |
|------|------|----------|----------|--------|
| Phase A 只改 writer，不改事件产生侧 | 共享 bus 扫描与持锁回调仍在，收益有限 | mailbox 与 `emit_sync` 快照/锁外回调必须一起落 | fanout 次数不降、p99 仍高 | 只修出口，不修入口，还是会堵。 |
| 热/冷分层做了，但 checkpoint 语义不稳定 | 冷会话唤不醒、恢复不一致 | Phase A 先激活 unload，但恢复锚点语义要在 Phase B 统一到 `Storage trait` | rehydrate 失败率高、同会话状态漂移 | 只会“卸”，不会“醒”，系统会越跑越碎。 |
| Phase B 过早把协议改太多 | 本地 `stdio` 和现有扩展端回退风险大 | 保持 `ServeCommand` / `OutFrame` 主形状稳定，新增字段向后兼容 | schema fixture 漂移、现有 GUI 事件流变化 | 云端能力不能拿本地可用性做祭品。 |
| Phase C 先上 DB，再补配额与租户隔离 | 会出现 noisy neighbor 和越权风险 | tenant、quota、audit 必须和共享存储一起上线，不允许“先共库后隔离” | 单租户流量拖垮全局、审计无法归因 | 多租户不是数据库表多一列而已。 |
| 断线重连只依赖 live event，不做 snapshot/cursor | UI 容易出现缺帧或重复帧 | `stream_resumable` 必须绑定 cursor + snapshot + `upToSeq` | 重连后消息缺口、重复渲染 | 直播断了以后，要能从录像接上。 |
| 文档边界不清，团队重复实现 | `agent-server-and-ui-gateway`、`session-storage`、本目录互相打架 | 父文档向下导航、子文档只回链父文档；本目录只写跨三者的云化总方案 | 同一概念在多文档不同说法 | 先把地图画清楚，别多人从不同入口各造一半。 |

**说人话**：这套改造最大的风险不是“技术选错一个点”，而是“边界没切清楚，结果每期都半改不改”。所以文档上先把父子关系、阶段边界、稳定字段、恢复语义钉死，是必要动作，不是形式主义。

---

## 10. 历史决策 / 跨文档修订

1. **修正旧诊断**：Tomcat 现状并不是“进程全局唯一 EventBus 承载所有会话”，而是 `ChatContext` 通过 `scope_runtime_for()` 按 cwd 复用 `DefaultEventBus`；但“共享 bus + 多 listener + 持锁回调 + 全量常驻 `ChatContext` 不适合海量并发”这个结论仍成立。  
2. **本目录的角色**：`cloud-scale-serving/` 是“云化改造父导航目录”，负责总览与 A/B/C 分册；后续若补更多专题，只能由本总览继续向下导航，分册不横向串跳。  
3. **与既有文档的分工**：
   - `agent-server-and-ui-gateway.md` 继续是“现有进程边界 / 本地 stdio / gateway 入口”的单一事实源；
   - `session-storage.md` 继续是“现有 transcript / `sessions.json` / checkpoint”语义事实源；
   - `multi-agent.md` 继续是“子 Agent 派生 / reviewer / verifier”事实源；
   - 本目录只回答“海量会话、热/冷分层、共享存储、订阅网关、多租户”这条横向改造线。
4. **阶段顺序固定**：若未来资源有限、只能落一期，默认优先落 Phase A；B/C 不能绕过 A 直接做，因为 B/C 要建立在 A 提供的 mailbox、热度管理、run 身份和本地零回退基线之上。

**说人话**：这次改造真正重要的历史决策只有一句话: 先把本地结构改对，再把它外推到云端。谁要是反过来做，最后大概率会回头重写 Phase A。
