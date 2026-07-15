# Phase B：Gateway、分片路由与 Storage Trait

> 本文是 [`01-overview.md`](./01-overview.md) 的 Phase B 分册，建立从“修好的单机 serve”到“多 worker 可扩展服务”的桥梁。
> 它承接 [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md)，假设单机内已经有 `SessionMailbox`、`HeatState`、`TurnScheduler` 与 `runId/seq`。
>
> Phase B 的目标不是一次做完多租户运营，而是把架构拆成真正可水平扩展的三层：
>
> - **Gateway**：连接、订阅、鉴权、placement、回放入口
> - **Worker**：热会话、AgentLoop、tool runtime、sandbox lease
> - **Storage**：catalog / transcript / blob / checkpoint / plan

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象图**：理解 Gateway / Worker / Storage 三层各自只做什么、不做什么。
2. 再看 **A.2 具体图**：理解当前 Tomcat 哪些本地模块会演进成云端模块。
3. 再看 **B 状态机**：理解一个会话如何从“没有 placement”变成“黏在某个 worker 上并可随时回放”。
4. 最后看 **§3 决策矩阵**：这里给出本期的协议、分片、存储与 hydrate 裁决。

### A.1 抽象 ASCII 总图

```text
客户端 / SDK
  ├─ 发命令: start_turn / follow_up / interrupt / subscribe
  └─ 收事件: lifecycle / delta / control / replay
            │
            ▼
┌──────────────────────── Gateway ────────────────────────┐
│ auth │ connection manager │ subscription index │ replay │
│ placement resolver │ quota pre-check │ stdio/ws/http adapter │
└───────────────┬───────────────────────────────┬─────────┘
                │                               │
                │ 命令下发                       │ 事件回流 / replay
                ▼                               ▲
┌──────────────────────── Worker ─────────────────────────┐
│ shard runtime │ SessionMailbox │ TurnScheduler │ hot sessions │
│ AgentLoop │ tool runtime │ sandbox lease │ checkpoint writer │
└───────────────┬───────────────────────────────┬─────────┘
                │                               │
                ▼                               ▼
┌────────────────── Storage Traits ───────────────────────┐
│ SessionCatalog │ TranscriptStore │ BlobStore │ PlanStore │
│ CheckpointStore │ PlacementStore │ optional local cache │
└─────────────────────────────────────────────────────────┘
```

读图导读（说人话）：Gateway 这层最容易被误解。它不是“更大的 `serve`”，也不是“把所有事件再做一遍”。它只管**谁有资格连、谁订了哪些会话、哪条命令该发给哪台 worker、断线后怎么从哪儿补流**。真正的 AgentLoop、tool runtime、沙箱和热态上下文，仍然都在 Worker 里。Storage 层则让“本机目录”不再是唯一世界观。

### A.2 具体 ASCII 总图

```text
Phase A 之后的本地 serve                    Phase B 之后的云端模块
────────────────────────────────────────────────────────────────────────────
src/api/serve/
  ├─ session_registry.rs
  ├─ session_mailbox.rs
  ├─ turn_scheduler.rs
  ├─ writer.rs
  └─ commands.rs

────────────────────── 演进为 ──────────────────────

src/cloud/gateway/
  ├─ transport_stdio.rs     复用本地子进程模式
  ├─ transport_ws.rs        WebSocket 连接层
  ├─ transport_http.rs      命令入口
  ├─ auth.rs                tenant / token / session ownership
  ├─ subscriptions.rs       conn -> {sessionKey}
  ├─ replay.rs              seq catch-up / snapshot handoff
  ├─ placement.rs           sessionKey -> worker sticky
  └─ admission.rs           queue / quota / drain

src/cloud/worker/
  ├─ shard_runtime.rs       worker 级入口
  ├─ session_runtime.rs     SessionHandle / HotRuntime / HeatState
  ├─ session_mailbox.rs     从 Phase A 提升为 worker 模块
  ├─ turn_scheduler.rs      从 Phase A 提升为 worker 模块
  ├─ event_sink.rs          AgentLoop -> mailbox -> gateway bridge
  ├─ hydrate.rs             cold/warm -> hot
  └─ sandbox_lease.rs

src/cloud/storage/
  ├─ session_catalog.rs     current session pointer / title / metadata
  ├─ transcript_store.rs    append / scan_tail / compact / replay
  ├─ blob_store.rs          tool-results / workspace diff / large payload
  ├─ checkpoint_store.rs    pending writes / durable checkpoint
  ├─ plan_store.rs          plan / todo / ask_question state
  └─ placement_store.rs     current worker + generation
```

读图导读（说人话）：Phase B 不是在当前 `src/api/serve/` 目录上继续堆文件，而是把“连接逻辑”和“会话执行逻辑”彻底拆层。这样未来本地 `stdio` 和云端 `WS/HTTP` 才能共享一套语义、不同一套实现。最重要的是 `storage/`：这层一旦抽出来，现有本地 JSONL / sidecar / plan.md 才能变成 backend，而不再是架构本体。

### B. 状态机：会话 placement 与回放生命周期

```text
  ┌────────────┐ resolve placement ┌────────────┐ subscribe ok ┌────────────┐
  │ unassigned │──────────────────▶│ attaching  │─────────────▶│ attached    │
  └─────┬──────┘                   └────┬───────┘              └────┬───────┘
        │ cold open / migrate             │ hydrate fail               │ worker drain / failover
        │                                 ▼                            ▼
        │                           ┌────────────┐               ┌────────────┐
        └──────────────────────────▶│ replaying  │◀──────────────│ migrating  │
                                    └────┬───────┘               └────┬───────┘
                                         │ replay done                  │ attach new worker
                                         ▼                              ▼
                                   ┌────────────┐                ┌────────────┐
                                   │ live       │────────────────▶│ attached    │
                                   └────────────┘                └────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `unassigned` | 新命令 / 新订阅 | `attaching` | placement resolver 选 worker，必要时创建 placement 记录 | 先决定这会话归谁管。 |
| `attaching` | worker hydrate 成功 | `attached` | 建立 live subscription 和 command path | 会话挂上 worker 了。 |
| `attaching` | 需要先补历史 | `replaying` | 通过 snapshot + seq 补齐可见状态 | 先把旧状态补给客户端。 |
| `replaying` | replay 完成 | `live` | 转入实时订阅 | 补完历史，开始收实时流。 |
| `attached/live` | worker drain / fail | `migrating` | 锁 placement generation、冻结新命令 | 会话准备换机器。 |
| `migrating` | 新 worker attach 成功 | `attached` | seq 续接、恢复 live | 换机成功，继续干。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `Gateway` | 连接与控制平面入口 | WS/HTTP/stdio adapter + auth + placement cache | 不持有重会话上下文，不直接跑 AgentLoop | 前台接待，不进后厨。 |
| `WorkerShard` | 一台 worker 上的一组热会话 | worker 进程内 runtime map | 一个 `sessionKey` 任一时刻只能归一个 authoritative shard | 同一会话不能同时被两台机器各管一半。 |
| `PlacementRecord` | `sessionKey -> workerId` 的 authoritative 绑定 | placement store row | 带 generation / lease；迁移时必须 CAS 更新 | 会话换机器得有正式交接单。 |
| `ReplayCursor` | 客户端已看到的最后事件位置 | `sessionKey + seq + runId?` | reconnect 时只能向前补，不能回退覆盖 | 补流得知道你看到哪儿了。 |
| `StorageTrait` | 一类持久化能力的接口边界 | Rust trait | 允许本地和云端不同 backend，共同语义一致 | 抽的是“能力”，不是“某种数据库”。 |
| `Hydrate` | 从持久化层恢复可运行会话热态 | worker lifecycle | 必须幂等、可取消、可观测 | 把冷会话重新热起来。 |
| `Sticky Routing` | 命令尽量发回已有 placement 的 worker | placement cache + resolver | 只有迁移时才改 worker | 会话别来回跳机器。 |

## 2. 竞品 / 选型对比（调研）

| 参考 | 文件 | 借鉴点 | 不直接照抄的地方 | 说人话 |
|------|------|--------|------------------|--------|
| `codex-rs` | `thread-store/src/store.rs` | `ThreadStore` 作为统一持久化边界 | Codex 以本地 app-server 为主，Tomcat 需要更强的多租户治理 | 存储抽象这套思路非常值得直接搬。 |
| `codex-rs` | `app-server/src/thread_state.rs` | per-thread 多连接订阅、ordered listener | Codex 的协议和我们的 AgentEvent 不同 | 线程/会话扇出模型可借，事件语义不必照抄。 |
| `LangGraph` | `checkpoint/.../base/__init__.py` | Checkpointer 接口、pending writes、interrupt/resume | LangGraph 是 graph runtime，不是 shell/tool-heavy coding agent | 持久化和恢复语义很成熟，但执行环境模型不同。 |
| `OpenClaw` | `placement-dispatch.ts`、`live-events.ts` | placement FSM、generation CAS、bounded event windows | OpenClaw 代码量和部署栈更重 | 会话怎么派给 worker、怎么迁移，这里最像真实云服务。 |

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

Phase B 的裁决围绕四件事：

1. Gateway 到底怎么接入。
2. 会话如何分片和 sticky。
3. 存储接口怎么抽。
4. reconnect / replay / hydrate 如何形成闭环。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| B1 连接模型 | 命令和事件走什么传输 | **采用** `HTTP command + WS events` 作为云端主路径，同时保留 `stdio adapter` 兼容本地；**拒绝**只做 WS 全双工或只做 HTTP 轮询。 | 本仓：`docs/architecture/agent-server-and-ui-gateway.md`、`src/api/serve/*`；外部：`langgraph_sdk/stream/transport/ws.py`、`codex-rs/app-server/src/transport.rs` | 命令天然适合 request/response，事件天然适合长连接订阅；本地和云端共享 envelope 最稳。 | 未入选：纯 WS command+event；拒因：弱化幂等 command 语义，代理/鉴权/审计也不如 HTTP 清晰。 | 发命令像点菜，用 HTTP；听流像看大屏，用 WS。 |
| B2 分片策略 | 会话怎么落到 worker | **采用** `sessionKey` 一致性哈希 + sticky placement；**拒绝**随机打散或每条命令现算 worker。 | 本仓：Phase A `sessionKey` / `HeatState` 设计；外部：`openclaw/src/gateway/worker-environments/placement-dispatch.ts`、`server-session-key.ts` | 保持热会话在同一 worker，减少 rehydrate 和 sandbox 迁移频率。 | 未入选：轮询分发；拒因：每次命令都可能打到不同 worker，热态价值被抹平。 | 会话应该尽量待在熟悉它的那台机器上。 |
| B3 placement 存储 | placement 放内存还是持久化 | **采用** `PlacementStore` 持久化 authoritative 绑定，并带 generation/CAS；**拒绝**只放 gateway 内存。 | 本仓：当前 `ChatContextRegistry` 只有进程内 map；外部：`openclaw/src/gateway/worker-environments/placement-store.ts` | worker 宕机、gateway 重启、drain 迁移都要求 authoritative placement 可恢复。 | 未入选：内存 map；拒因：一重启 placement 全丢，命令和 replay 都会漂。 | 谁管哪个会话，不能只记在脑子里。 |
| B4 存储边界 | 一条大 `Store` 还是多 trait | **采用** 至少拆成 `SessionCatalogStore`、`TranscriptStore`、`BlobStore`、`CheckpointStore`、`PlanStore`；**拒绝**“万能存储接口”。 | 本仓：`src/core/session/*`、`src/core/checkpoint/*`、`src/core/plan_runtime/*`；外部：`codex-rs/thread-store/src/store.rs`、`LangGraph` checkpointer/store 拆分 | 五类数据的写频率、读模式和一致性需求不同，强行揉成一接口只会过宽。 | 未入选：单一 `Store` trait；拒因：难以表达 append-only transcript 和 blob/object store 的差异。 | 数据长得不一样，就别逼它们穿同一件衣服。 |
| B5 transcript 后端 | transcript 继续单文件还是分段 | **采用** append-only segment + tail index 语义，local backend 可映射回 JSONL；**拒绝**每次 rewrite 整个对象。 | 本仓：`src/core/session/transcript.rs`、`resume_index.rs`；外部：`LangGraph` blobs/metadata split、`OpenClaw` live event windows | 云端对象存储和数据库都更适合 append/segment，而不是频繁整文件重写。 | 未入选：远端也继续整文件 JSONL rewrite；拒因：大对象 rewrite 成本高，冲突处理差。 | 本地单文件还能忍，云端整对象重写会越来越疼。 |
| B6 hydrate 策略 | 连接一来就全量 hydrate 还是按需 | **采用** `metadata first, tail hydrate on demand`；**拒绝**连接建立就拉满整段 transcript。 | 本仓：`src/core/session/manager/context.rs`、`chat-resume-hydration.md`；外部：`LangGraph` thread resume、`codex-rs` delayed unload | 先快速拿到足够展示/开跑的状态，再在需要时拉更多历史，是最省成本的。 | 未入选：订阅即全量恢复；拒因：大量只看列表不运行的会话会白白消耗 IO。 | 刚打开会话先别把整本书搬出来，先翻到最近几页够用就行。 |
| B7 回放语义 | reconnect 时只靠内存缓存还是要落存储回放 | **采用** gateway 短缓存 + durable replay 双层；**拒绝**只有短缓存。 | 本仓：Phase A `seq` / `deliveryClass`；外部：`openclaw/live-events.ts`、`LangGraph` interrupt/resume | 短断线靠缓存快，长断线靠 durable transcript 稳。 | 未入选：只有内存 replay cache；拒因：网关一重启就断历史。 | 近的东西从内存里补，远的东西从仓库里补。 |
| B8 本地兼容 | 云端模块是不是另起一套协议 | **采用** `src/api/serve/stdio adapter` 继续复用同一 command/event schema；**拒绝**云端与本地各养一套 wire。 | 本仓：`src/api/serve/types.rs`、`schema.rs`；外部：`codex-rs` export/schema fixtures | 同语义多 transport，是最省客户端维护成本的方式。 | 未入选：本地和云端分叉两套协议；拒因：IDE、SDK、Web 会各自漂移。 | 只是部署环境不同，不该变成两门语言。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| B1 Gateway transport | HTTP 命令入口、WS 事件订阅、stdio adapter 复用同一 schema | `src/cloud/gateway/transport_*` | 见本文 §8.2 / §8.3 | 先把门修出来。 |
| B2 Placement & sticky routing | `PlacementStore`、resolver、generation CAS、drain-aware reroute | `src/cloud/gateway/placement.rs`、`src/cloud/storage/placement_store.rs` | 见本文 §8.1 / §8.2 | 确定每个会话归谁管。 |
| B3 Storage traits | 五类 store trait、本地 backend、SQLite backend、Postgres/S3 接口约束 | `src/cloud/storage/*` | 见本文 §8.1 | 把“存什么”和“存哪儿”分开。 |
| B4 Worker hydrate/evict | worker shard runtime、hydrate pipeline、tail replay、idle evict | `src/cloud/worker/*` | 见本文 §8.2 | 让 worker 真能接手会话。 |
| B5 Replay & subscriptions | subscription index、replay cursor、cache + durable replay | `src/cloud/gateway/subscriptions.rs`、`replay.rs` | 见本文 §8.3 | 断了能续，不是断了就忘。 |

#### 3.2.1 B1：把 `serve` 语义抽成 transport-agnostic gateway

推荐统一的语义层：

- `CommandService`
  - `start_turn`
  - `follow_up`
  - `interrupt`
  - `set_visibility`
  - `subscribe`
- `EventService`
  - `publish_live`
  - `replay_from`
  - `snapshot_for_reconnect`

在 Phase B 里：

- `stdio` 只是 `transport_stdio.rs`
- WebSocket 只是 `transport_ws.rs`
- HTTP 命令只是 `transport_http.rs`

业务逻辑不该知道自己是被 stdio、HTTP 还是 WS 调进来的。

#### 3.2.2 B2：Placement 要有 authoritative generation

`PlacementRecord` 推荐字段：

- `sessionKey`
- `workerId`
- `generation`
- `leaseUntil`
- `state`（`active / draining / migrating`）

迁移规则：

1. gateway 先 CAS 把 placement 从 `worker-A@g=10` 改成 `worker-B@g=11`
2. worker-A 停止接收该会话新命令
3. worker-B 完成 hydrate
4. gateway 开始把 live subscription 指向 worker-B

没有 generation，就没法判断“谁是最新 owner”。

#### 3.2.3 B3：Storage trait 最小拆分

推荐的 trait 边界：

```text
SessionCatalogStore
  - load_session_summary
  - update_title
  - update_current_pointer

TranscriptStore
  - append_event_segment
  - read_tail
  - read_from_seq
  - compact

BlobStore
  - put_blob
  - get_blob
  - delete_blob

CheckpointStore
  - put_pending_write
  - commit_checkpoint
  - load_latest_checkpoint

PlanStore
  - load_plan
  - save_plan
  - save_todos
```

本地 backend 映射建议：

- `SessionCatalogStore` -> `sessions.json` / SQLite
- `TranscriptStore` -> 现有 JSONL + sidecar
- `BlobStore` -> `tool-results/`
- `CheckpointStore` -> 现有 shadow git
- `PlanStore` -> `plans/*.plan.md` + `todos/*.todo.md`

#### 3.2.4 B4：Hydrate 不再直接等于“读本地文件”

Phase B 的 hydrate pipeline 推荐：

1. 读 `SessionCatalogStore` 获取 summary、title、last checkpoint、plan anchor
2. 读 `TranscriptStore.read_tail(sessionKey, anchor)` 获取最近有效 slice
3. 读 `PlanStore` 获取 plan/todo 状态
4. 若有 workspace/sandbox snapshot，则取 snapshot metadata
5. 构建 `HotSessionRuntime`

这样本地 backend 和云端 backend 共用同一流程，只是读取介质不同。

#### 3.2.5 B5：Replay 采用“双层补流”

第一层：gateway 内存短缓存

- 处理短断线
- 低延迟
- 不查存储

第二层：durable replay

- 处理网关重启、长断线、迁移后重连
- 通过 `TranscriptStore.read_from_seq()` + snapshot 补齐

客户端流程：

1. 提交 `ReplayCursor(lastSeenSeq)`
2. gateway 先尝试短缓存
3. 若缓存不够，返回 `replay_required`
4. gateway 拉 durable store，发 snapshot + tail events

## 4. 协议

### 4.1 `SubscribeRequest`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKeys` | `string[]` | 是 | - | WS 订阅 | 订阅哪些会话 | 我想看哪些会话。 |
| `lastSeen` | `object[]` | 否 | `[]` | reconnect | 每个会话的 `sessionKey + seq` | 我上次看到哪儿。 |
| `includeSnapshot` | `bool` | 否 | `true` | reconnect / first open | 是否先发快照 | 要不要先给我一份当前全貌。 |

### 4.2 `ReplayCursor`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | replay | 会话键 | 哪个会话。 |
| `seq` | `u64` | 是 | - | replay | 客户端最后确认看到的事件号 | 你已经收到哪一条了。 |
| `runId` | `string` | 否 | - | replay / dedupe | 当前活跃 run | 有助于定位飞行中的那一轮。 |

### 4.3 `PlacementRecord`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | placement | 会话主键 | 谁。 |
| `workerId` | `string` | 是 | - | placement | 当前 owner worker | 归谁管。 |
| `generation` | `u64` | 是 | - | placement | 版本号 | 谁是最新 owner。 |
| `state` | `string` | 是 | - | placement | `active/draining/migrating` | 现在是在稳态还是迁移中。 |
| `leaseUntil` | `string` | 是 | - | placement | 续租时间 | 多久内这个绑定有效。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | Phase B 职责 | 说人话 |
|-------------|--------------|--------|
| `src/cloud/gateway/transport_http.rs` | 命令面入口、幂等响应、错误规范化 | 下单入口。 |
| `src/cloud/gateway/transport_ws.rs` | 长连接、订阅、心跳、重连 | 看实时大屏。 |
| `src/cloud/gateway/placement.rs` | resolve sticky worker、drain-aware reroute | 知道该找哪台 worker。 |
| `src/cloud/gateway/replay.rs` | 短缓存补流、durable replay fallback | 断了之后接上。 |
| `src/cloud/worker/shard_runtime.rs` | worker 级入口，管理一批热会话 | 这一台机器上的会话总管。 |
| `src/cloud/worker/hydrate.rs` | 从 store 构建 hot runtime | 把冷会话重新热起来。 |
| `src/cloud/storage/transcript_store.rs` | 统一 append/read/replay 语义 | transcript 不再等于某个本地文件。 |
| `src/cloud/storage/placement_store.rs` | authoritative placement 记录 | 谁管谁有据可查。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `cloud.gateway.listen_addr` | `127.0.0.1:0` 本地 / 明确 service addr 云端 | gateway 监听地址 | 云端前门开在哪。 |
| `cloud.gateway.max_subscriptions_per_conn` | `256` | 单连接订阅数上限 | 一条连接别无限挂。 |
| `cloud.gateway.replay_cache_events` | `2048` | 每会话短缓存事件数 | 短断线优先走内存补。 |
| `cloud.worker.max_hot_sessions` | 按内存预算 | 单 worker 热会话上限 | 这台机子能热着多少会话。 |
| `cloud.storage.catalog_backend` | `sqlite` / `postgres` | catalog backend | 元数据存哪。 |
| `cloud.storage.blob_backend` | `fs` / `s3` | blob backend | 大对象放哪。 |
| `cloud.placement.lease_ms` | `30000` | placement 心跳租期 | 多久续一次“我还活着”。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 条件 | 对外表现 | 恢复策略 | 说人话 |
|------|------|----------|----------|--------|
| `placement_conflict` | generation CAS 失败 | 返回重试或 reroute | 重新 resolve placement | 有人比你更晚更新了归属。 |
| `replay_gap` | 短缓存不覆盖 `lastSeenSeq` | 返回 `replay_required` | 走 durable replay | 内存补不上了。 |
| `hydrate_timeout` | worker 恢复热态超时 | 订阅进入等待或失败 | 迁移到其他 worker / fallback cold open | 这台机子热不起来。 |
| `store_unavailable` | DB / object store 异常 | 命令降级为只读或失败 | circuit break + retry | 仓库出故障了。 |

## 8. 测试矩阵（验收）

### 8.1 单元测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| consistent hash + sticky | 相同 `sessionKey` 稳定落同 worker | 会话别乱漂。 |
| placement generation CAS | 并发迁移下只有最新 generation 生效 | 归属更新不打架。 |
| `TranscriptStore` local backend 与 current JSONL 行为对齐 | append/read_tail/replay 语义一致 | 先保证本地 backend 不失真。 |

### 8.2 集成测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| gateway + 两台 worker | 会话 sticky 到同一 worker，命令/事件全链路正确 | 三层真的串起来。 |
| worker drain | 新命令自动转新 worker，旧 worker 不再接单 | 换机时别掉会话。 |
| cold open + tail hydrate | 首次订阅不做全量 transcript 扫描 | 打开会话要快。 |

### 8.3 E2E 测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| WebSocket 断线重连 | 短断线靠缓存、长断线靠 durable replay，最终 UI 状态一致 | 断了能续上，不漏也不重。 |
| 本地 stdio adapter 与云端 HTTP/WS 语义一致 | 同一命令和事件字段都能消费 | 本地和云端说同一种话。 |
| 会话迁移中审批 | `control_request` 不丢、不重复，回包仍能命中正确 run | 迁移时最怕审批串线。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| Phase B 把 storage trait 设计得太像数据库 ORM | backend 难切换，语义被实现绑架 | trait 只表达业务动作，不泄漏 SQL/FS 细节 | 接口别长成某个数据库的脸。 |
| sticky routing 不稳 | hydrate 频繁、沙箱频繁迁移 | placement generation + lease + drain 策略 | 会话老换机器会很痛。 |
| replay 语义不严 | UI 出现重复消息或丢尾部 | 强制 `seq` 测试、snapshot+tail 明确边界 | 补流设计要像财务对账一样严。 |

## 10. 历史决策 / 跨文档修订

1. Phase B 继承 Phase A 的 `SessionMailbox` 和 `runId/seq`，不是重新定义。
2. Phase B 首先把 `storage/` 抽成 trait，再决定后端默认值，因此本地 FS/SQLite 仍是合法 backend，而不是被立刻废弃。
3. Phase B 仍然不处理完整多租户运营策略；那是 [`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md) 的范围。
