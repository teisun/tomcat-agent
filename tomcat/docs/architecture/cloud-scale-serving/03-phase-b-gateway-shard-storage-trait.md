# Phase B：Gateway、分片路由与 Storage Trait

> 父文档：[`01-overview.md`](./01-overview.md)
>
> 前置：[`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md) 已落地，即单机已有 `SessionMailbox`、热/温/冷分层、`runId/parentIds`、run permit 与本地零回退门禁。
>
> 本册范围：
>
> - 把 `serve` 从“本地多会话调度器”升级为“可换传输的 gateway dispatcher”
> - 引入 `SessionRegistry + SubscriptionRegistry`
> - 启用 `ServeTransport::Ws`
> - 抽出 `Storage trait`，让本地文件存储成为默认 backend，而不是唯一 backend
> - 把 Session 与 Run 显式分离，并定义 `MultitaskStrategy`
>
> 不在本册范围：
>
> - 不做外部多租户控制面、控制面 DB、对象存储和跨地域灾备（见 [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md)）

**说人话**：Phase B 的目标不是“云原生集群版 Tomcat 已完工”，而是先把边界抽对。只要网关、订阅、分片、存储语义这几道边界稳定了，本地 `stdio`、单机 WS、多 worker 乃至后面的集群，才能共用同一套 dispatcher 和 core。

---

## 先看总图：方案导图集

### 阅读顺序建议

1. **A.1 抽象 ASCII 总图**：先看一条连接如何完成 `initialize/auth -> subscribe -> hash route -> worker -> persist -> deliver`。
2. **A.2 具体 ASCII 总图**：再看这些边界分别落到 `serve` 哪些模块和哪些新文件。
3. **B 状态机**：最后看连接/订阅的生命周期，理解为什么“连接层”和“运行态”必须拆开。

### A.1 抽象 ASCII 总图

```text
Client（IDE / Web / GUI）
   │
   │  ① initialize / auth
   ▼
Gateway dispatcher
   │
   ├─ Connection state
   ├─ SubscriptionRegistry（conn <-> session）
   └─ Hash router（session_id -> worker）
   │
   │  ② worker 侧只认 session / run，不认具体连接
   ▼
Worker
   ├─ SessionRegistry（session_id -> hot/warm/cold slot）
   ├─ Run scheduler（same session serial, cross session parallel）
   └─ Public event contract（哪些事件允许出网）
   │
   │  ③ 对需要恢复的事实先持久化
   ▼
Storage trait
   ├─ session metadata
   ├─ transcript
   ├─ checkpoint(session_id, ns, checkpoint_id)
   └─ pending_writes / resume anchor
   │
   │  ④ 再把 live 事件发回订阅连接
   ▼
Subscribed connections
```

这张抽象图要表达两件事。第一，连接层和 worker 运行态明确分离: gateway 持有连接与订阅表，但不持有会话真状态；worker 持有 hot/warm/cold、run 和 mailbox，但不关心某个事件最终发给哪条具体连接。第二，存储层在 Phase B 已经不是“本地文件实现细节”，而是 worker 对 durable state 的统一接口。

**说人话**：从这一期开始，Tomcat 的连接和执行不再绑死在一起。你可以把 gateway 理解成“快递分拣中心”，把 worker 理解成“真正干活的车间”，两者之间靠 `session_id` 路由，不再靠“这条 socket 正好连着这个运行时”这种偶然关系。

### A.2 具体 ASCII 总图

```text
┌─ 入口 / 传输层 ─────────────────────────────────────────────────────────────────────┐
│ src/api/serve/{stdin,control,types}.rs                                              │
│ • 现有 stdio 保留                                                                   │
│ • Phase B：`ServeTransport::Ws` 真启用，统一进同一 dispatcher                        │
│ • `initialize` 继续作为连接握手与能力协商入口                                        │
└──────────────────────────────┬──────────────────────────────────────────────────────┘
                               ▼
┌─ [new] src/api/serve/gateway/{mod,server,router}.rs ───────────────────────────────┐
│ • WS accept / auth / initialize                                                     │
│ • `session_id -> worker` sticky hash                                                │
│ • transport adapter：stdio / ws 共用 commands + control + writer                    │
└───────────────┬────────────────────────────┬────────────────────────────────────────┘
                │                            │
                ▼                            ▼
┌─ src/api/serve/registry.rs ───────────────┐  ┌─ [new] src/api/serve/subscription_registry.rs ─┐
│ • SessionRegistry（原 `ChatContextRegistry` 演进）│  │ • `conn -> {session_ids}`                 │
│ • hot/warm/cold slot                       │  │ • `session_id -> {conn_ids}`                 │
│ • 不感知具体连接                           │  │ • cursor / upToSeq / visible state           │
└───────────────┬───────────────────────────┘  └───────────────┬────────────────────────────────┘
                │                                              │
                ▼                                              ▼
┌─ src/api/serve/{commands,event_pump,writer}.rs ─────────────────────────────────────┐
│ • 复用 Phase A mailbox / run scheduler                                              │
│ • 事件先经过 public event contract，再按 subscription 定向到连接                    │
└───────────────┬──────────────────────────────────────────────────────────────────────┘
                ▼
┌─ [new] src/core/session/storage/{mod,fs,trait}.rs ──────────────────────────────────┐
│ • `Storage trait` 统一 session metadata / transcript / checkpoint / pending_writes   │
│ • fs backend 兼容现有 `SessionManager`                                               │
│ • 后续 shared backend 在 Phase C 继续实现                                            │
└───────────────┬──────────────────────────────────────────────────────────────────────┘
                ▼
┌─ src/core/agent_registry/mod.rs + src/api/chat/run_loop/mod.rs ─────────────────────┐
│ • Session 与 Run 分离                                                                │
│ • `prompt` / `follow_up` / `steer` 映射到 `MultitaskStrategy`                        │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张具体图突出三个 Phase B 新模块：`gateway/*`、`subscription_registry.rs`、`storage/*`。这三个新增点分别对应“连接入口”“连接与会话的关系”“会话与存储的关系”。它们一旦稳定，`serve` 就从“本地边界上的多会话调度器”升级成“真正的 session gateway dispatcher”。

**说人话**：本期最重要的新增不是某个协议字段，而是三张表：`session -> worker`、`conn <-> session`、`session -> durable state`。云端化能不能稳，基本就看这三张表有没有拆清楚。

### B. 状态机：连接与订阅生命周期

```text
       ws open / stdio spawn
┌─────────────┐   initialize/auth ok   ┌─────────────┐   subscribe    ┌──────────────┐
│ connected   │───────────────────────▶│ initialized │───────────────▶│ subscribed   │
└─────┬───────┘                        └──────┬──────┘                └──────┬───────┘
      │ auth fail                              │ unsubscribe all                │ live event / snapshot
      ▼                                        ▼                                ▼
┌─────────────┐                          ┌─────────────┐                  ┌──────────────┐
│ auth_failed │                          │ idle_ready  │◀────────────────▶│ streaming    │
└─────────────┘                          └──────┬──────┘   no subscription └──────┬───────┘
                                                │                                   │ disconnect
                                                ▼                                   ▼
                                          ┌─────────────┐                     ┌─────────────┐
                                          │ detached    │                     │ closed      │
                                          └─────────────┘                     └─────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `connected` | `initialize/auth ok` | `initialized` | 协议版本、能力位、鉴权上下文就绪 | 连上不代表能订阅，先证明你是谁。 |
| `connected` | auth fail | `auth_failed` | 关闭连接、记审计 | 不能让匿名连接直通会话。 |
| `initialized` | `subscribe` | `subscribed` | 写入 `SubscriptionRegistry`、执行 hash route | 真正开始“看哪些会话”。 |
| `subscribed` | 首次 snapshot / live events 开始 | `streaming` | 发送快照并接入 live 流 | 进入稳定收流态。 |
| `streaming` | `unsubscribe all` | `idle_ready` | 清空订阅，但连接仍可继续发命令或再订阅 | 连接还活着，只是暂时没人可看。 |
| 任意非终态 | disconnect | `closed` / `detached` | 清理订阅表、释放连接句柄 | 连接死了，订阅表也要跟着清。 |

**说人话**：状态机最大的价值，是把“连接活着”和“已经在看某些会话”区分开。这个区分如果不做清楚，后面就很难准确处理断线重连、订阅重建和多标签页。

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 / 单一事实源 | 行为约束 | 说人话 |
|------|------|----------------------|----------|--------|
| **SessionRegistry** | Worker 侧的会话运行态注册表；维护 `session_id -> slot` | `registry.rs` 演进版 | 只关心会话运行态，不关心具体连接 | 哪个会话现在在哪个 worker 上、冷热如何。 |
| **SubscriptionRegistry** | 连接与会话的双向订阅表 | Phase B 新增 `subscription_registry.rs` | 连接断开时必须可逆清理；不持有业务真状态 | 哪条连接正在看哪些会话。 |
| **Gateway dispatcher** | 统一承接 stdio / ws 的命令、控制、事件适配层 | `gateway/*` + 现有 `serve` 模块 | 只路由不跑 LLM；transport 可换，dispatcher 不换 | 大门口和分拣台。 |
| **Sticky hash route** | 基于 `session_id` 把会话稳定路由到同一 worker 的策略 | `gateway/router.rs` | 同 session 默认 stick；迁移需显式 rebalance | 同一个房间尽量别每次都去不同车间。 |
| **Storage trait** | Worker 与 durable state 交互的稳定语义接口 | `storage/{trait,fs}.rs` | 后端可换，语义不换 | 先定义“存什么、怎么恢复”，后端以后再换。 |
| **Pending writes** | 尚未完全对外可见、但已进入 durable path 的待完成写集 | `Storage trait` | 必须和 checkpoint / transcript 一起参与恢复 | 正在写入中的尾巴，恢复时不能丢。 |
| **Public event contract** | 允许 gateway 对外透出的事件集合与 schema | `types.rs` / `protocol.rs` / `events/mod.rs` | 允许对外的事件要稳定版本化；内部事件不可裸透出 | 出口白名单从“代码细节”升级成“正式契约”。 |
| **MultitaskStrategy** | 当 session 已有 active run 时，新请求如何处理的策略 | `reject` / `interrupt` / `rollback` / `enqueue` | 必须按命令类型稳定映射 | 新请求来了，是拒、抢、回滚还是排队。 |

**说人话**：Phase B 的关键词不再是 mailbox，而是“注册表、路由、契约、语义接口”。因为从这一期开始，Tomcat 不只是在本地单机里自己玩，它要开始对连接、后端和未来 worker 拿出稳定边界。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 聚焦本期的借鉴表

| 竞品 / 仓库 | 本期关注点 | 关键设计 | 我们借鉴的点 | 说人话 |
|-------------|------------|----------|--------------|--------|
| **codex** | app-server 与两级注册表 | `core/src/thread_manager.rs`、`app-server/src/thread_state.rs`、`thread-store/src/store.rs` | `SessionRegistry + SubscriptionRegistry`、`ThreadStore` 风格存储边界 | 最像我们的“本地/服务端双态 dispatcher”样板。 |
| **OpenClaw** | gateway 与 subscribe 模式 | `docs/gateway/protocol.md`、`src/gateway/server-broadcast.ts` | subscribe / unsubscribe、按订阅定向广播、连接级治理 | 怎么把“连接在看谁”做成一等公民。 |
| **LangGraph** | Saver 与 multitask 语义 | `checkpoint/base/__init__.py`、`schema.py` | `(session_id, ns, checkpoint_id)` 键语义、`MultitaskStrategy` | 不是所有新请求都得用同一种处理策略。 |
| **Tomcat 现状** | `serve` 已有的可复用协议底座 | `src/api/serve/types.rs`、`control.rs`、`schema.rs` | 继续复用 `initialize`、`ControlFrame`、schema 导出与 writer | 现有协议不是推倒重来，而是继续向上长。 |

### 2.2 本期最重要的调研结论

1. **两级注册表是必须的，不是实现偏好。**  
   只靠 `SessionRegistry` 你不知道“谁在看这个 session”；只靠 `SubscriptionRegistry` 你又不知道“这个 session 现在在哪个 worker、是不是 hot”。codex 的 `ThreadManager + ThreadStateManager` 已证明这两张表必须并存。

2. **`Storage trait` 要先于 shared backend。**  
   LangGraph 的 checkpoint saver 价值不在 Postgres，而在“先把语义抽象出来”。Tomcat 也应先有 `trait + fs backend`，再谈 SQLite / Postgres / object store。

3. **gateway 必须是 transport adapter，不是第二套业务 runtime。**  
   OpenClaw 最值得借的是连接层思路，而不是把 gateway 变成另一套运行时。Tomcat Phase B 的 gateway 只负责 auth、subscribe、route、adapt。

4. **`MultitaskStrategy` 必须显式化。**  
   本地单机时代很多行为还能靠 `busy` 和隐式队列解释过去；一旦进入 WS、多连接、多 worker，同一个 session 面对多个请求时怎么处置，必须有稳定策略名和稳定映射。

**说人话**：Phase B 的核心不是“加个 websocket”，而是“把‘连接、会话、运行态、存储’四者的边界说清楚”。边界清楚，协议才会稳；边界不清楚，系统越扩越乱。

---

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

`§3.1` 负责维度裁决，`§3.2` 负责落点与验收。本册的裁决重点是“哪个表负责什么”“哪个接口是真相源”“哪个请求在 session 忙时应该如何处理”。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **B1 Gateway 形态** | gateway 是不是要独立出一套新 runtime | **采用** 同一 dispatcher 多 transport（`stdio + ws`），gateway 只做 adapter；**拒绝** 再造第二套业务 runtime。 | 本仓：`tomcat/src/api/serve/mod.rs`、`tomcat/src/api/serve/control.rs`；外部：`codex-rs/app-server/src/in_process.rs`、`hermes-agent/tui_gateway/server.py` | 设计：传输适配层复用现有 commands/control/writer；理由：能让本地 `stdio`、单机 WS、多 worker 共享同一业务路径，不会让协议与行为分叉。 | 未入选：为 WS 单独写一套 server runtime；拒因：会复制命令分发、控制回环、背压和 schema 输出，长期必漂移。 | gateway 是壳，不是第二个脑子。 |
| **B2 注册表拆分** | 连接层与会话层是否能共用一张注册表 | **采用** `SessionRegistry + SubscriptionRegistry` 两张表；**拒绝** 把连接、订阅、worker 归属全塞进一个对象。 | 本仓：`tomcat/src/api/serve/registry.rs`、`tomcat/src/api/serve/types.rs`；外部：`codex-rs/core/src/thread_manager.rs`、`codex-rs/app-server/src/thread_state.rs` | 设计：worker 侧只认 session 运行态，gateway 侧只认订阅关系；理由：这样连接重连、会话迁移、worker 回收都能各自演进。 | 未入选：一条连接 owning 一个 runtime；拒因：不适合多 tab、多观察者和恢复场景。 | 会话和观看者不是一回事，必须分账。 |
| **B3 路由策略** | session 如何在多 worker 间落点 | **采用** `session_id` sticky hash route；**拒绝** 轮询或连接绑定 worker。 | 本仓：`tomcat/src/api/serve/registry.rs`、`tomcat/src/api/serve/fanout_event_bus.rs`；外部：`openclaw/docs/concepts/queue.md`、`codex-rs/app-server/src/request_serialization.rs` | 设计：session 默认 stick 到单 worker，迁移由显式 rebalance 控制；理由：会话热状态、mailbox 与 pending writes 在单 worker 内部更简单，sticky 是最小复杂度路线。 | 未入选：按连接轮询 worker；拒因：同一 session 多连接时会把状态拆散，恢复和顺序保证都更难。 | 同一个房间尽量固定在一个车间，别谁来问都换地方。 |
| **B4 存储边界** | 何时从本地文件过渡到共享存储 | **采用** Phase B 先定义 `Storage trait` 与 fs backend；**拒绝** 继续让 `SessionManager` 直接承担未来 shared storage 语义。 | 本仓：`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/api/serve/types.rs`；外部：`codex-rs/thread-store/src/store.rs`、`langgraph/libs/checkpoint/langgraph/checkpoint/base/__init__.py` | 设计：trait 统一承载 session metadata、transcript、checkpoint、pending_writes；理由：只有先固化语义，后续 DB/object store 才不会反复改 gateway 与 run 恢复逻辑。 | 未入选：等 Phase C 直接用数据库重写 `SessionManager`；拒因：会让 Phase B 的 gateway 与恢复逻辑缺少稳定基线。 | 先把接口说清楚，再换后端。 |
| **B5 多任务策略** | session 已有 active run 时，新请求怎么处理 | **采用** 显式 `MultitaskStrategy`：`prompt -> reject`，`follow_up -> enqueue`，`steer -> interrupt(+replace)`；**拒绝** 继续靠隐式 `busy` 与 ad hoc 队列。 | 本仓：`tomcat/src/api/serve/commands.rs`、`tomcat/src/core/agent_registry/mod.rs`；外部：`langgraph/libs/sdk-py/langgraph_sdk/schema.py`、`openclaw/docs/concepts/queue.md` | 设计：按命令类型固定映射策略；理由：多连接、多 worker 场景下，只有显式策略才能稳定地做协议、UI 和审计。 | 未入选：所有请求都 reject 或都 enqueue；拒因：会牺牲交互性，且不符合 `steer` / `follow_up` 的不同语义。 | 不同请求进来，处理办法得稳定可预期。 |
| **B6 出口契约** | `event_pump` allowlist 是否仍只是代码里的常量数组 | **采用** allowlist 升级为版本化 `PublicEventContract` / schema 导出；**拒绝** 继续只有内部常量而无显式对外契约。 | 本仓：`tomcat/src/api/serve/event_pump.rs`、`tomcat/src/api/serve/schema.rs`；外部：`codex-rs/app-server-protocol/src/export.rs`、`openclaw/docs/gateway/protocol.md` | 设计：允许出网的事件集合与字段作为正式协议导出；理由：WS / 多客户端 / 外部 SDK 接入时，出网事件若不稳定版本化，会造成高频漂移。 | 未入选：只保留内部 allowlist 常量；拒因：本地单进程时代够用，网关时代不够。 | 白名单从代码细节升级成公开合同。 |
| **B7 兼容策略** | B 期是否允许本地 `stdio` 变成次等公民 | **采用** `stdio` 与 `ws` 共用 dispatcher、共用 schema、共用 tests；**拒绝** 出现“WS 路径是新真相源，本地只是兼容模式”。 | 本仓：`tomcat/src/api/serve/types.rs`、`tomcat/src/api/serve/tests/schema_test.rs`；外部：`codex-rs/app-server-transport/src/transport/stdio.rs`、`pi_agent_rust/src/sdk.rs` | 设计：transport 仅是 adapter，协议与业务逻辑继续同源；理由：这能把本地与远程的回归面收敛成一套，而不是两套。 | 未入选：B 期后只优先服务 WS；拒因：会造成未来本地与远程行为漂移、测试翻倍。 | 远程能力长出来以后，本地不能变成被遗忘的分支。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PB-1 两级注册表** | `SessionRegistry + SubscriptionRegistry`、连接断开清理、cursor/visibility 元数据 | `registry.rs`、`[new] subscription_registry.rs` | 新增 `subscription_registry_tracks_conn_and_session_both_ways` | 会话和观看者各记各的账。 |
| **PB-2 Gateway / WS transport** | `ServeTransport::Ws`、`initialize/auth`、subscribe/unsubscribe、sticky route | `[new] gateway/{server,router}.rs`、`types.rs`、`control.rs` | 新增 `ws_initialize_auth_and_subscribe_roundtrip` | 真正把 `serve` 长成可远程接入的 gateway。 |
| **PB-3 Storage trait** | `Storage trait`、fs backend、`pending_writes`、persist-then-deliver | `[new] storage/{trait,fs}.rs`、`session_impl.rs` | 新增 `storage_trait_fs_backend_matches_session_manager_semantics` | 先把语义统一，再谈后端切换。 |
| **PB-4 Run / Session 分离** | `run_id` 生命周期、`MultitaskStrategy`、命令到策略映射 | `commands.rs`、`run_loop/mod.rs`、`agent_registry/mod.rs` | 新增 `follow_up_enqueues_while_steer_interrupts_active_run` | 把“房间”和“这次任务”彻底拆开。 |
| **PB-5 Public event contract** | allowlist 版本化导出、schema/d.ts 更新、订阅面可审计 | `event_pump.rs`、`schema.rs`、`types.rs` | 新增 `public_event_contract_schema_matches_runtime_events` | 哪些事件能出网，得写成正式协议。 |
| **PB-6 本地/远程同源回归** | stdio 与 ws 共用 dispatcher，双路径回归不漂移 | `serve/tests/*`、future gateway tests | 现有 `serve_emitted_event_validates_against_generated_schema` + 新增 `stdio_and_ws_share_same_command_semantics` | 再多 transport，也只能有一套真行为。 |

#### 3.2.1 PB-1：两级注册表

`SessionRegistry` 负责 session 在 worker 侧的运行态，包括 hot/warm/cold、mailbox、run、recovery anchor。`SubscriptionRegistry` 负责连接正在看哪些 session，以及这些 session 当前对应哪个 cursor / visibility / live stream。这两张表分开以后，连接可以断，session 可以继续活；session 可以迁 worker，连接订阅也能重建。

#### 3.2.2 PB-2：Gateway / WS transport

Phase B 的 WS 不是“再造一个新协议”，而是让 `ServeTransport::Ws` 复用 Phase A 已稳住的 dispatcher。WS 连接建立后，第一件事仍然是 `initialize`，只是 payload 会多 auth / client 信息。接着才是 `subscribe` / `unsubscribe` 这些连接级动作。

#### 3.2.3 PB-3：Storage trait

本期的关键不是先定数据库，而是先把接口语义钉死：session metadata 怎么读写、transcript 怎么 append/load、checkpoint 如何按 `(session_id, ns, checkpoint_id)` 键存取、pending writes 如何和恢复结合。fs backend 只是一种实现，不再是唯一事实源。

#### 3.2.4 PB-4：Run / Session 分离与 MultitaskStrategy

从这一期开始，session 是长期身份，run 是短生命周期执行实例。新的请求进来后，Tomcat 不能再只会说“busy”或者“继续队列里看看”，而要稳定地说：这个请求是 `reject`、`enqueue`、`interrupt` 还是 `rollback`。这样 UI、审计和 SDK 才能推断一致行为。

---

## 4. 协议

本期协议重点有两块：连接级 `subscribe/unsubscribe`，以及 `Storage trait` 的稳定语义键。

### 4.1 `subscribe` / `unsubscribe` 字段表

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `string` | 是 | 无 | `subscribe` / `unsubscribe` | 命令类型 | 这是订阅命令。 |
| `requestId` | `string` | 是 | 无 | 全部订阅命令 | 客户端请求关联键 | 哪次订阅操作。 |
| `sessionIds` | `string[]` | `subscribe` 时必填 | 无 | `subscribe` | 批量订阅的会话集合 | 一次可以看多个会话。 |
| `cursor` | `object` | 否 | `null` | `subscribe` | `{sessionId -> lastSeenCursor}`，用于 snapshot / resume | 从哪里续流。 |
| `mode` | `string` | 否 | `"snapshot_then_live"` | `subscribe` | `snapshot_then_live` / `live_only` | 先补历史再接直播，还是只接直播。 |
| `sessionId` | `string` | `unsubscribe` 时必填 | 无 | `unsubscribe` | 单个取消订阅目标 | 取消看某个会话。 |

### 4.2 `Storage trait` 语义表

| 字段 / 键 | JSON / 结构类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|-----------|------------------|------|--------|----------|------|--------|
| `session_id` | `string` | 是 | 无 | metadata / transcript / checkpoint | 会话主键 | 这个 durable state 属于哪个房间。 |
| `checkpoint_ns` | `string` | 是 | `"default"` | checkpoint / pending writes | checkpoint 命名空间 | 同一个会话里不同恢复流的命名空间。 |
| `checkpoint_id` | `string` | 是 | 无 | checkpoint / restore | 单个 checkpoint 主键 | 复活哪次状态。 |
| `pending_writes` | `[]WriteOp` | 否 | `[]` | 恢复 / replay | checkpoint 之后、完全对外可见之前的尾部写集 | 还没完全广播出去但已经要能恢复的尾巴。 |
| `resume_cursor` | `string` | 否 | `null` | subscribe / resume | 最近一个对外可恢复的流位置 | 断线后从哪接上。 |

### 4.3 样例

```jsonc
// initialize 仍是连接级握手；WS 只是在 payload 上带 auth/client 信息
{
  "type": "control_request",
  "requestId": "req-init-1",
  "subtype": "initialize",
  "payload": {
    "transport": "ws",
    "protocolVersion": "2",
    "authToken": "opaque-token",
    "clientInfo": { "name": "vscode", "version": "0.1.0" }
  }
}

// 订阅两个 session，并带各自 cursor
{
  "type": "subscribe",
  "requestId": "req-sub-1",
  "sessionIds": ["s-1", "s-2"],
  "cursor": {
    "s-1": "seq:120",
    "s-2": "seq:9"
  },
  "mode": "snapshot_then_live"
}
```

单一事实源：

- 连接级命令与控制帧：`tomcat/src/api/serve/types.rs`
- Phase B 新增订阅契约：`[new] src/api/serve/protocol.rs`
- 存储语义接口：`[new] src/core/session/storage/trait.rs`

**说人话**：这期协议的本质是两句话：连接先证明自己，再声明“我要看哪些会话”；worker 要先把 durable state 说清楚，再宣布“我可以换后端了”。

---

## 5. 文件职责总览（One-Glance Map）

```text
┌─ src/api/serve/types.rs + control.rs ───────────────────────────────────────────────┐
│ • `ServeCommand` / `ControlFrame` / schema 基础                                     │
│ • Phase B：加入 subscribe/unsubscribe / auth payload 约束                           │
└──────────────────────────────┬──────────────────────────────────────────────────────┘
                               ▼
┌─ [new] src/api/serve/gateway/{server,router}.rs ────────────────────────────────────┐
│ • ws accept / auth / initialize                                                     │
│ • sticky hash route: `session_id -> worker`                                         │
│ • transport adapter：stdio / ws 统一进 dispatcher                                   │
└───────────────┬────────────────────────────┬────────────────────────────────────────┘
                │                            │
                ▼                            ▼
┌─ src/api/serve/registry.rs ───────────────┐  ┌─ [new] src/api/serve/subscription_registry.rs ─┐
│ • SessionRegistry（worker 侧）             │  │ • conn <-> session 双向订阅表                  │
│ • hot/warm/cold / mailbox / run            │  │ • cursor / visibility / cleanup               │
└───────────────┬───────────────────────────┘  └───────────────┬────────────────────────────────┘
                │                                              │
                └──────────────────────┬───────────────────────┘
                                       ▼
┌─ src/api/serve/{commands,event_pump,writer}.rs ─────────────────────────────────────┐
│ • command dispatch                                                                   │
│ • public event contract                                                              │
│ • transport-agnostic outgoing frames                                                 │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ [new] src/core/session/storage/{trait,fs}.rs ──────────────────────────────────────┐
│ • durable state API                                                                  │
│ • fs backend 兼容现有 `SessionManager`                                                │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/core/session/manager/session_impl.rs ───────────────────────────────────────────┐
│ • 逐步退居 fs backend 具体实现                                                       │
│ • 不再直接承担未来 shared storage 的系统边界                                         │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/core/agent_registry/mod.rs + src/api/chat/run_loop/mod.rs ─────────────────────┐
│ • run identity                                                                       │
│ • `MultitaskStrategy` 映射与执行                                                     │
└──────────────────────┬───────────────────────────────────────────────────────────────┘
                       ▼
┌─ tests: serve tests + gateway tests + storage trait tests ───────────────────────────┐
│ • subscribe / auth / route / storage semantic / local-remote parity                  │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张图的阅读顺序是“先看连接入口，再看两张注册表，再看 dispatcher 与 storage，最后看 run identity 与 tests”。它强调 Phase B 的工作重点不在 `AgentLoop`，而在 `serve` 外围长出更完整的连接层、订阅层和 durable-state 层。

**说人话**：Phase B 的所有变化其实都在给 Tomcat 做“接口层手术”。核心大脑继续不动，但进出通路、订阅关系、存储语义都变得更正式。

---

## 6. 配置与环境变量

总则：**env > config > 默认**。

| 变量 / 配置 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `[serve].transport` | `stdio` / `ws` | transport 选择 | env / config | 本地和 WS 共存。 |
| `[gateway].bind` | host:port | WS gateway 监听地址 | env / config | 网关在哪儿开门。 |
| `[gateway].token` | string | 网关鉴权 token | env / config | 连上门也得先刷卡。 |
| `[gateway].max_subscriptions_per_conn` | `usize` | 单连接最多可订阅的 session 数 | env / config | 一个客户端最多同时看多少会话。 |
| `[gateway].subscribe_batch_limit` | `usize` | 单次订阅批量上限 | env / config | 别一口气订爆。 |
| `[storage].backend` | `fs` / `sqlite` / `postgres` / `hybrid` | durable backend | env / config | 先文件，后共享存储。 |
| `[storage].default_checkpoint_ns` | string | checkpoint 默认命名空间 | env / config | checkpoint 默认放哪个抽屉。 |
| `[scheduler].multitask_strategy_default` | `reject` / `enqueue` / `interrupt` / `rollback` | 缺省策略 | env / config | 新请求撞上 active run 时怎么处理。 |
| `[serve].public_event_contract_version` | string | 出口事件契约版本 | config | 对外合同版本号。 |

**说人话**：这一期开始，配置真正多起来的不是 LLM，而是 gateway 和 storage。因为从这一期开始，Tomcat 开始要对外接连接、对后接后端了。

---

## 7. 错误模型 / 截断 / 警告

```text
连接级
  initialize/auth
    ├─ fail -> control_response error + close
    └─ ok   -> ready

订阅级
  subscribe
    ├─ unknown_session -> response.error
    ├─ auth_scope_mismatch -> response.error
    └─ ok -> snapshot_then_live

存储级
  checkpoint / pending_writes fail
    ├─ persist-before-deliver 事件 -> fail current run / do not publish
    └─ best-effort metadata       -> warn + metrics
```

| 结局 | 触发条件 | 对外形态 | 说人话 |
|------|----------|----------|--------|
| `auth_failed` | token/identity 无效 | 初始化错误 + 关闭连接 | 没权限就别进来。 |
| `unknown_subscription` | `unsubscribe` 指向未订阅 session | `response.error("unknown_subscription")` | 取消一个根本没订的会话要报错。 |
| `route_miss` | hash route 找不到目标 worker | retry / degraded response / metrics | 分片表坏了要可见，不能静默。 |
| `persist_blocked_delivery` | 需要恢复的事件尚未 durable | 不发布 live 事件，先返回错误或 degraded 状态 | 账没写上，直播先别发。 |
| `strategy_conflict` | 请求与当前 `MultitaskStrategy` 冲突 | `response.error("busy" / "strategy_conflict")` | 不同命令遇到 active run，处理规则要明确。 |

**说人话**：Phase B 最大的新增错误不在 LLM，而在“连接、订阅、存储”三个边界。因为从这一期开始，Tomcat 不只是自己和自己说话，它开始要对外给出稳定承诺了。

---

## 8. 测试矩阵（验收）

| 层级 | 目标 | 锚点（测试函数名 / 文件） | 状态 | 说人话 |
|------|------|---------------------------|------|--------|
| 单元 | `SubscriptionRegistry` 双向一致性 | `api::serve::tests::subscription_registry_test::tracks_conn_and_session_both_ways` | PENDING | 订阅表别记成单向。 |
| 单元 | sticky hash route 同 `session_id` 稳定命中同 worker | `api::serve::tests::gateway_router_test::session_hash_route_is_sticky` | PENDING | 同房间别一会儿一个工位。 |
| 单元 | `Storage trait` fs backend 语义对齐现有 `SessionManager` | `core::session::storage::tests::fs_backend_matches_session_manager_semantics` | PENDING | 抽接口不能把现有文件语义改坏。 |
| 单元 | `MultitaskStrategy` 映射稳定 | `api::serve::tests::scheduler_test::command_kind_maps_to_multitask_strategy` | PENDING | 不同命令撞上 active run，得有固定处理法。 |
| 集成 | 现有初始化握手基线不回退 | `api::serve::tests::control_test::serve_initialize_control_request_sets_ready_state` | ✅ 2026-07-15 | 旧握手仍然是基线。 |
| 集成 | schema 与 runtime 事件持续对齐 | `api::serve::tests::schema_test::serve_emitted_event_validates_against_generated_schema` | ✅ 2026-07-15 | 对外合同别漂。 |
| 集成 | WS initialize/auth/subscribe 全链路 roundtrip | `api::serve::tests::gateway_test::ws_initialize_auth_and_subscribe_roundtrip` | PENDING | 这一期最核心的远程 happy path。 |
| 集成 | unsubscribe 后不再继续收流 | `api::serve::tests::gateway_test::unsubscribe_stops_live_delivery_for_session` | PENDING | 取消订阅要立刻生效。 |
| 集成 | `follow_up` enqueue、`steer` interrupt 的策略差异可见 | `api::serve::tests::commands_test::follow_up_enqueues_while_steer_interrupts_active_run` | PENDING | 让交互语义稳定下来。 |
| 集成 | 本地 `stdio` 与 WS 共享同一命令语义 | `api::serve::tests::parity_test::stdio_and_ws_share_same_command_semantics` | PENDING | 不能一套本地一套远程。 |
| 契约 | public event contract 导出与 runtime allowlist 对齐 | `api::serve::tests::schema_test::public_event_contract_schema_matches_runtime_events` | PENDING | 出口合同要有机器校验。 |
| 压测 | 单机多 worker + 数千 live subscription 下路由与出流稳定 | `tests/gateway_subscription_load.rs::subscriptions_5k_sessions_steady_tail_latency` | PENDING | 真正验证网关雏形抗压。 |

**说人话**：Phase B 的测试重点从“本地会不会卡”转向“边界会不会漂”。因为一旦网关、订阅、存储接口不稳，后面的集群化就没有可靠地基。

---

## 9. 风险与应对

| 风险 | 影响 | 应对策略 | 说人话 |
|------|------|----------|--------|
| `Storage trait` 语义定义太弱 | Phase C 换后端时反复加临时字段 | Phase B 就把 checkpoint / pending_writes / resume cursor 一次说清 | 接口抽得模糊，后面每换后端都得返工。 |
| 连接层与运行态耦在一起 | 断线重连 / 多观察者 / worker 迁移困难 | 两级注册表强制拆分 | 谁在看和谁在跑，必须不是一回事。 |
| `MultitaskStrategy` 映射不稳定 | UI / SDK / 审计行为不可预测 | 固定 `prompt/follow_up/steer` 映射，并做回归 | 用户最怕同样操作有时排队有时抢占。 |
| 只做 WS，不保本地同源 | 本地和远程行为漂移、测试翻倍 | `stdio` 与 `ws` 共用 dispatcher 和 schema | transport 可以变，行为不能分叉。 |
| Public event contract 未版本化 | 外部客户端接入后容易被字段漂移打断 | schema / d.ts / fixtures 一起锁版本 | 对外事件不该只是“当前代码碰巧长这样”。 |

**说人话**：Phase B 最怕做成“长了 WebSocket，但还是本地临时架构思维”。真正的完成标准，是 gateway、订阅、存储都开始像公共基础设施，而不是像本地 sidecar 的附属脚手架。

---

## 10. 历史决策 / 跨文档修订

1. `agent-server-and-ui-gateway.md` 继续负责解释“Tomcat 如何把能力暴露给进程外 UI”；本册只是把那份文档里的本地 `serve` 路径继续外推到 `SessionRegistry + SubscriptionRegistry + Storage trait`。
2. `session-storage.md` 继续保留当前文件语义的单一事实源地位；本册只定义“未来 shared storage 必须满足哪些语义”，不重写现有落盘格式。
3. 本册为 Phase C 提供的不是现成集群实现，而是稳定边界：sticky route、storage trait、public event contract、multitask semantics。

**说人话**：Phase B 像是在本地单机和真正云端之间搭一座桥。桥先搭稳，车流以后才好放大。
