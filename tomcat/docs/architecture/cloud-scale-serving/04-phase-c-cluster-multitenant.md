# Phase C：集群、多租户与跨 Worker 恢复

> 父文档：[`01-overview.md`](./01-overview.md)
>
> 前置：
>
> - [`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md) 已让单机具备 mailbox、热/温/冷、run identity 与本地零回退能力
> - [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md) 已让 `serve` 具备 gateway dispatcher、WS transport、`SessionRegistry + SubscriptionRegistry`、`Storage trait`
>
> 本册范围：
>
> - 外部 gateway、worker 池、一致性哈希、再均衡
> - 控制面 DB + 数据面 transcript / blob / checkpoint store
> - 跨 worker rehydrate、灾备恢复、route lease
> - 多租户鉴权、隔离、配额、限流
> - `stream_resumable` 断线重连
> - 按 session / tenant / worker 的可观测与混沌测试

**说人话**：Phase C 才是真正把 Tomcat 从“单机可扩的本地 sidecar”推到“云端多租户基础设施”的那一层。这里关注的不再是单机架构债，而是云上系统最怕的几类问题：谁有权限、谁占资源、谁掉了怎么接、断线怎么补、谁把谁拖垮了怎么定位。

---

## 先看总图：方案导图集

### 阅读顺序建议

1. **A.1 抽象 ASCII 总图**：先看租户、网关、worker、控制面、数据面是怎么分工的。
2. **A.2 具体 ASCII 总图**：再把这套架构落到 Tomcat 未来的 `cloud/*`、`storage/*`、`serve/gateway/*` 落点。
3. **B 状态机**：最后看一个 session 在 worker 宕机、lease 过期、重连恢复时如何跨 worker 复活。

### A.1 抽象 ASCII 总图

```text
Tenant client（IDE / Web / API）
   │
   │  ① auth / quota / rate limit
   ▼
External gateway ring
   ├─ verify tenant identity
   ├─ `session_id -> worker` consistent hash
   ├─ route lease / rebalance coordination
   └─ resumable subscribe(cursor -> snapshot/live)
   │
   │  ② worker 只跑会话，不关心租户接入细节
   ▼
Worker pool
   ├─ SessionRegistry
   ├─ hot/warm/cold residency
   ├─ run scheduler
   └─ cross-worker `resume`
   │
   │  ③ durable state 拆控制面与数据面
   ▼
Control-plane DB
   ├─ tenant / quota / auth binding
   ├─ session metadata / route lease / subscription cursor
   └─ checkpoint index / pending_writes index

Data-plane storage
   ├─ transcript segments
   ├─ checkpoint blobs
   └─ large tool outputs / attachments / artifacts
   │
   │  ④ 全链路可观测
   ▼
Metrics / Trace / Audit
   ├─ by tenant
   ├─ by session
   └─ by worker / route / recovery attempt
```

这张抽象图先钉死云端最核心的两个拆分。第一是 **接入平面** 与 **执行平面** 分离：gateway 负责 auth、配额、连接与路由；worker 负责 session、run、mailbox、恢复。第二是 **控制面存储** 与 **数据面存储** 分离：控制面存“小而频繁更新、强一致、索引型”的内容，数据面存“大而可分段、吞吐优先”的内容。

**说人话**：一上云，最贵的不是“多几个 worker”，而是“别把所有责任都塞进同一个进程和同一个数据库表”。租户、路由、会话、transcript、工具大输出，天生就不是一类东西。

### A.2 具体 ASCII 总图

```text
┌─ [new] src/cloud/gateway/{server,auth,router,reconnect}.rs ─────────────────────────┐
│ • 外部 WS/HTTP gateway                                                               │
│ • JWT / token / tenant 绑定                                                          │
│ • consistent hash + route lease                                                      │
│ • subscribe(cursor) -> snapshot_then_live                                            │
└──────────────────────────────┬───────────────────────────────────────────────────────┘
                               ▼
┌─ [new] src/cloud/control_plane/{tenant,quota,lease,session_meta}.rs ────────────────┐
│ • `tenant_id`、quota profile、rate bucket                                            │
│ • `session_id -> worker` lease                                                       │
│ • subscription cursor / resume metadata                                              │
└───────────────┬──────────────────────────────┬───────────────────────────────────────┘
                │                              │
                ▼                              ▼
┌─ src/api/serve/gateway/* + registry.rs ─────┐  ┌─ [new] src/core/session/storage/{postgres,object_store}.rs ─┐
│ • gateway dispatcher 复用 Phase B 逻辑      │  │ • checkpoints/checkpoint_writes index                          │
│ • SessionRegistry / SubscriptionRegistry    │  │ • transcript segments / blobs / attachments                    │
└───────────────┬─────────────────────────────┘  └───────────────┬───────────────────────────────────────────────┘
                │                                                 │
                ▼                                                 ▼
┌─ [new] src/cloud/recovery/{resume,rebalancer}.rs ───────────────────────────────────┐
│ • lease expiry / worker crash -> route move                                          │
│ • `Command(resume)` 风格 rehydrate                                                   │
│ • pending_writes replay / `upToSeq`                                                   │
└───────────────┬───────────────────────────────────────────────────────────────────────┘
                ▼
┌─ [new] src/cloud/observability/{metrics,trace,audit}.rs ─────────────────────────────┐
│ • by tenant/session/worker/recovery                                                  │
│ • SLO / lag / hydrate latency / resume gap                                           │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张具体图把 Phase C 的新增复杂度都放到了 `cloud/*` 与 shared storage backend 里，而不是继续往 `AgentLoop` 里灌逻辑。Tomcat core 仍然在 worker 内复用，但 gateway、lease、quota、recovery orchestration 和 observability 明确成为云端新层。

**说人话**：到了 Phase C，要新长出来的是“云基础设施器官”，不是“另一套 agent 大脑”。真正会新建很多文件的，是路由、存储、恢复和观测这几层。

### B. 状态机：跨 Worker 恢复与断线续流

```text
                client disconnect / worker crash
┌──────────────┐  route lease lost / stream broken  ┌──────────────┐
│ active_on_A  │───────────────────────────────────▶│ lease_stale  │
└──────┬───────┘                                    └──────┬───────┘
       │ normal heartbeat                                   │ claim route lease
       │                                                    ▼
       │                                              ┌──────────────┐
       │                                              │ recovering_B │
       │                                              └──────┬───────┘
       │                                                     │ load checkpoint + pending_writes
       │                                                     ▼
       │                                              ┌──────────────┐
       │                                              │ rehydrated_B │
       │                                              └──────┬───────┘
       │                                                     │ subscribe(cursor)
       │                                                     ▼
       └────────────────────────────────────────────────▶┌──────────────┐
                                                         │ streaming_B  │
                                                         └──────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `active_on_A` | worker crash / lease timeout | `lease_stale` | route lease 标记失效，gateway 暂停新 live 绑定 | A 挂了，先把“它说了算”这件事撤掉。 |
| `lease_stale` | worker B 成功 claim lease | `recovering_B` | 读取控制面元数据和恢复锚点 | 轮到 B 接手。 |
| `recovering_B` | checkpoint + pending writes 装载完成 | `rehydrated_B` | 重建 hot slot / mailbox / run identity | 会话在 B 身上醒过来。 |
| `rehydrated_B` | client subscribe(cursor) | `streaming_B` | snapshot + `upToSeq` + live 流恢复 | 客户端重新接上这条流。 |

**说人话**：Phase C 最关键的不是“worker 崩了以后另一台也能启动一个新会话”，而是“另一台能把**同一个 session** 接着跑下去，而且客户端能把流接上，不乱序、不丢账”。这就是云端恢复的及格线。

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 / 单一事实源 | 行为约束 | 说人话 |
|------|------|----------------------|----------|--------|
| **Control-plane DB** | 存租户、会话元数据、route lease、quota、cursor 等索引型信息的强一致平面 | `cloud/control_plane/*` + shared DB | 小对象、高频更新、强一致优先 | 管“谁是谁、谁在哪、谁有多少配额”的账本。 |
| **Data-plane storage** | 存 transcript、checkpoint blob、tool artifact、附件等大对象的吞吐平面 | `storage/{postgres,object_store}.rs` / object store | 大对象、分段写、吞吐优先 | 真正的大文件仓库。 |
| **Route lease** | 某个 `session_id` 当前由哪个 worker 持有的可续约所有权 | control-plane lease 表 | 失效前同一 session 只应有一个 active owner | 这个房间现在归哪台机器负责。 |
| **Rebalance** | 在不中断或尽量少中断前提下，把 session 从一个 worker 挪到另一个 worker | `rebalancer.rs` | 必须走显式 lease 迁移与 rehydrate | 主动搬家，不是故障接盘。 |
| **Tenant** | 共享基础设施上的独立安全、配额、审计主体 | tenant 表 / auth token / quota profile | 每个 session、route、blob、metric 都必须能回溯到 tenant | 不是“用户昵称”，而是安全和配额边界。 |
| **Quota profile** | 租户可用的连接数、热会话数、并发 run 数、token/分钟等资源预算 | control-plane quota 表 | 配额超限必须在入口显式拒绝或降级 | 每个租户能吃多少饭。 |
| **`stream_resumable`** | 客户端断线后，凭 cursor + snapshot + `upToSeq` 继续收流的协议能力 | gateway reconnect contract | 不能只靠 live-only 重连；必须可补历史缺口 | 直播断了以后要能从录像接回来。 |
| **Rehydrate** | 根据 durable state 在新 worker 上重建 hot slot 的过程 | recovery / storage trait | 必须可重复、可中断、可审计 | 把冷记录重新装回可运行状态。 |

**说人话**：Phase C 最大的区别，是“会话”不再只是一个聊天条目，而是穿过 auth、quota、routing、storage、observability 的一级实体。只要 `tenant` 和 `route lease` 这两个词没钉死，云端就会越长越乱。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 聚焦本期的借鉴表

| 竞品 / 仓库 | 本期关注点 | 关键设计 | 我们借鉴的点 | 说人话 |
|-------------|------------|----------|--------------|--------|
| **LangGraph** | `Command(resume)` 与 Postgres checkpoint schema | `types.py`、`checkpoint-postgres/.../base.py` | 跨 worker resume 语义、`checkpoints/checkpoint_writes` 表结构思路 | 它告诉我们“恢复”要先有稳定账本，再有执行器。 |
| **OpenClaw** | 控制面 / 数据面拆分 | `docs/refactor/database-first.md` | control-plane DB + data-plane store 分层 | 不是所有状态都该进同一种数据库。 |
| **Tomcat Phase B** | 现有 gateway / storage trait / registries | `serve/gateway/*`、`storage/trait.rs` | 作为 Phase C 的稳定边界起点 | 先有桥，再去上高架。 |

### 2.2 本期最重要的调研结论

1. **`resume` 不是“再发一次 prompt”，而是“根据 durable checkpoint 恢复同一次 run / session 语义”。**  
   这正是 LangGraph `Command(resume)` 最值得借的地方。

2. **控制面与数据面不分，最终一定会互相拖累。**  
   route lease、quota、cursor 这种索引型数据，与 transcript/blob 这种大对象数据完全不是一类负载。OpenClaw 给了一个非常清晰的拆分方向。

3. **多租户隔离必须从入口一路贯穿到存储和指标。**  
   不能只在网关层做 token 校验，然后内部又回到“只靠 session_id 区分”的模式。

**说人话**：到了 Phase C，真正像云的地方不在“有几台 worker”，而在“每条链路都能回答：这是哪个租户的、现在归谁负责、断了怎么续、出了问题看哪里”。竞品给我们的正是这些边界感。

---

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章先钉云端裁决，再落到可执行的实施点。它的重心是“系统边界与恢复语义”，不是“某个 endpoint 长什么样”。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **C1 云端拓扑** | 是继续让每个 worker 自带公网入口，还是引入外部 gateway ring | **采用** 外部 gateway ring + worker pool；**拒绝** “每个 worker 既接流量又跑 session” 的混合拓扑。 | 本仓：`tomcat/src/api/serve/gateway/*`（Phase B 边界）、`tomcat/src/api/serve/registry.rs`；外部：`openclaw/src/gateway/server.impl.ts`、`langgraph` 部署语义 | 设计：gateway 负责 auth / quota / route，worker 负责 session / run；理由：入口与执行职责拆开后，扩容、隔离、观测与故障域都更清晰。 | 未入选：每个 worker 自带入口、靠上层 LB 轮询；拒因：route lease、cursor 恢复、quota 收口都会更难统一。 | 大门口和车间分开，问题才好控。 |
| **C2 路由策略** | 多 worker 下如何保持 session 粘性与可迁移性 | **采用** 一致性哈希 + route lease + 显式 rebalance；**拒绝** 纯轮询或“连接绑定 worker”。 | 本仓：`tomcat/src/api/serve/registry.rs`、`tomcat/src/api/serve/gateway/router.rs`（Phase B）；外部：`openclaw/docs/concepts/queue.md`、`langgraph` 的 thread identity 语义 | 设计：默认 sticky 到单 worker，故障或扩容时通过 lease 迁移；理由：既保留顺序与恢复简单度，又给弹性扩容和故障切换留口子。 | 未入选：只靠 LB 轮询；拒因：同 session 的状态会被切碎，恢复与排障成本陡增。 | 平时固定、出事再搬，是最稳的路由方式。 |
| **C3 存储分层** | transcript、checkpoint、quota、route lease 是否放同一存储 | **采用** control-plane DB + data-plane transcript/blob/object store 分层；**拒绝** 单一大表或继续本地文件直扛。 | 本仓：`tomcat/src/core/session/manager/session_impl.rs`、`tomcat/src/core/session/storage/trait.rs`（Phase B）；外部：`openclaw/docs/refactor/database-first.md`、`langgraph/libs/checkpoint-postgres/.../base.py` | 设计：控制面存索引与租户状态，数据面存大对象与段文件；理由：查询模式、吞吐模式、容量模式完全不同，分层后才能兼顾一致性与成本。 | 未入选：所有内容一把梭进单 DB schema；拒因：大对象与高频索引混放会拖垮成本与性能。 | 小账本和大仓库，本来就不该住一个抽屉。 |
| **C4 恢复语义** | worker 宕机后是“新建会话”还是“恢复原会话” | **采用** 基于 checkpoint + pending_writes 的跨 worker `resume` / rehydrate；**拒绝** 宕机后只允许用户手动重开新会话。 | 本仓：`tomcat/src/core/session/storage/trait.rs`、`tomcat/src/api/chat/run_loop/mod.rs`；外部：`langgraph/libs/langgraph/langgraph/types.py`、`langgraph/libs/checkpoint-postgres/.../base.py` | 设计：worker B 按 lease claim 后读取 durable state，恢复原 session/run 语义；理由：云端系统若不能恢复原会话，只能算“有多台机器”，算不上可靠服务。 | 未入选：sticky forever / worker 死了会话跟着死；拒因：会让长会话、多小时任务、审批中断场景极不可靠。 | 挂了一台机器，房间不能跟着消失。 |
| **C5 多租户边界** | 租户隔离只做在网关层是否足够 | **采用** `tenant_id` 贯穿 auth、quota、route lease、storage key、metrics、audit；**拒绝** 内部只按 `session_id` 区分。 | 本仓：`tomcat/src/api/serve/types.rs`、`tomcat/src/api/serve/control.rs`；外部：`openclaw` 的多连接鉴权经验 + `langgraph` 的 durable thread identity | 设计：tenant 是一级主键，与 session 并列进入控制面和观测；理由：没有端到端 tenant 身份，就无法做真正的配额、隔离和审计。 | 未入选：只在入口验 token，内部全部退回 session-only；拒因：跨租户资源串扰和审计缺失风险很高。 | 谁家的请求、谁家的配额、谁家的事故，都得一路跟着走。 |
| **C6 断线续流** | 重连后是只接 live 流，还是必须可补历史缺口 | **采用** `stream_resumable`：cursor + snapshot + `upToSeq` + live resume；**拒绝** live-only 重连。 | 本仓：`tomcat/src/api/serve/writer.rs`、`tomcat/src/api/serve/types.rs`；外部：`langgraph/libs/sdk-py/langgraph_sdk/schema.py`、`openclaw` 的 subscribe/history 经验 | 设计：客户端带 cursor 重连，服务端回 snapshot 与截止 `upToSeq` 后再接 live；理由：网络抖动和移动端切换是常态，live-only 会导致缺帧或重复帧。 | 未入选：断线后只重新开始收最新事件；拒因：长输出和审批中的会话会出现不可恢复缺口。 | 直播断了以后，要能从录像无缝接回去。 |
| **C7 可观测** | 云端观测只看全局指标是否足够 | **采用** by tenant/session/worker/recovery 的多维指标、trace 和 audit；**拒绝** 只看全局聚合。 | 本仓：`tomcat/src/api/serve/writer.rs`、`tomcat/src/core/agent_registry/mod.rs`；外部：`langgraph` 的恢复语义、`openclaw` 的网关广播诊断思路 | 设计：指标必须能回答“哪个租户、哪个会话、哪次恢复、哪个 worker”；理由：多租户环境里只看全局平均值几乎等于盲飞。 | 未入选：只保留全局吞吐与错误率；拒因：定位 noisy neighbor、恢复抖动和 lease 抖动时完全不够。 | 云上出问题，必须能顺着租户和会话一路查下去。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PC-1 外部 gateway ring** | 公网/内网入口、auth、quota、consistent hash、route lease、rebalance | `[new] src/cloud/gateway/*`、`[new] src/cloud/control_plane/lease.rs` | 新增 `gateway_ring_claims_and_rebalances_route_leases` | 大门口和分片大脑先建起来。 |
| **PC-2 控制面 + 数据面存储** | tenant/quota/session metadata DB；transcript/checkpoint/blob 数据面 | `[new] src/cloud/control_plane/*`、`[new] src/core/session/storage/{postgres,object_store}.rs` | 新增 `control_plane_and_data_plane_split_survives_large_artifacts` | 小账本和大仓库分家。 |
| **PC-3 跨 worker rehydrate / 灾备** | lease 失效接管、checkpoint + pending_writes 恢复、resume orchestrator | `[new] src/cloud/recovery/{resume,rebalancer}.rs`、`storage/*` | 新增 `worker_crash_rehydrates_session_on_peer` | 机器挂了，会话还能接着活。 |
| **PC-4 多租户 auth / quota / 限流** | tenant contract、quota profile、per-tenant limits、审计归属 | `[new] src/cloud/{auth,quota}.rs`、`control_plane/tenant.rs` | 新增 `tenant_quota_isolation_prevents_cross_tenant_noisy_neighbor` | 谁家的流量谁家付账。 |
| **PC-5 `stream_resumable`** | cursor、snapshot、`upToSeq`、live resume、resume gap 告警 | `[new] src/cloud/gateway/reconnect.rs`、`subscription_registry.rs` | 新增 `reconnect_with_cursor_resumes_without_gap_or_duplicate` | 掉线之后能无缝接着看。 |
| **PC-6 可观测与混沌测试** | metrics、trace、audit、chaos drills、SLO | `[new] src/cloud/observability/*`、chaos/load tests | 新增 `chaos_worker_kill_preserves_session_recovery_slo` | 云上系统不能只靠感觉判断稳不稳。 |

#### 3.2.1 PC-1：外部 gateway ring

外部 gateway ring 是 Phase C 的接入平面。它既不是业务运行时，也不是简单反向代理，而是负责把 tenant 身份、route lease、rebalance 和 resumable subscribe 收口到一处。这样 worker 只关注 session/run 本身，不必直接暴露公网接入细节。

#### 3.2.2 PC-2：控制面 + 数据面存储

控制面存储负责强一致小对象：tenant、quota、session 元数据、route lease、cursor、checkpoint 索引。数据面存储负责大对象：transcript 段、checkpoint blob、tool artifact、附件。这个拆分是云端成本和性能都能站住脚的前提。

#### 3.2.3 PC-3：跨 worker rehydrate / 灾备

本期定义的恢复不是“用户自己再问一遍”，而是 worker B 在 claim lease 后，从 durable checkpoint + pending_writes 重建 hot slot，再用 `resume` 语义让会话继续。只有这样，长会话、审批中断和断线续流才真有可靠基础。

#### 3.2.4 PC-4：多租户边界

多租户不是在网关验完 token 就结束，而是 `tenant_id` 进入 route lease、进入 storage key、进入 metrics、进入 audit。只要有一个环节丢了 tenant 身份，配额、公平性和审计就都不成立。

---

## 4. 协议

本期协议重点有两块：tenant contract，以及 resumable stream / route lease 相关字段。

### 4.1 租户与路由字段表

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `tenantId` | `string` | 是 | 无 | auth、session、metrics、storage | 多租户主键 | 这是谁家的请求。 |
| `authToken` | `string` | 是 | 无 | gateway initialize / reconnect | 鉴权凭证 | 没票就别进。 |
| `quotaProfile` | `string` | 是 | 无 | auth / admission | 决定连接、热会话、并发 run、速率限制 | 这个租户按哪套规则吃资源。 |
| `routeLeaseVersion` | `string` | 是 | 无 | route claim / rebalance / recovery | 标识当前 route lease 版本 | 现在哪台 worker 的说法是最新的。 |
| `workerId` | `string` | 是 | 无 | route response / observability | 当前负责该 session 的 worker 标识 | 这个房间归谁负责。 |
| `cursor` | `string` | 否 | `null` | reconnect / subscribe | 客户端最后看到的位置 | 从哪一帧接着看。 |
| `upToSeq` | `u64` | 是（snapshot 响应） | 无 | snapshot_then_live | 快照已覆盖到的最高序号 | 快照补到了哪儿。 |

### 4.2 存储契约字段表

| 字段 / 结构 | JSON / 结构类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|-------------|------------------|------|--------|----------|------|--------|
| `(tenant_id, session_id)` | composite key | 是 | 无 | 所有控制面与数据面对象 | session 的全局归属键 | 先知道是谁家的哪个房间。 |
| `checkpoint_ns` | `string` | 是 | `"default"` | checkpoint | checkpoint 命名空间 | 同会话里可以有不同恢复轨。 |
| `checkpoint_id` | `string` | 是 | 无 | checkpoint / resume | 单个恢复点主键 | 要恢复哪一次状态。 |
| `pending_writes` | `[]WriteOp` | 否 | `[]` | resume | checkpoint 之后、完全广播之前的尾部写操作 | 还没播完但已经要记账的尾巴。 |
| `blob_ref` | `{bucket,key,etag}` | 否 | `null` | tool artifact / transcript segment / attachment | 数据面大对象引用 | 大东西别直接塞控制面。 |

### 4.3 样例

```jsonc
// reconnect / resumable subscribe
{
  "type": "subscribe",
  "requestId": "req-reconnect-7",
  "sessionIds": ["s-9"],
  "cursor": { "s-9": "seq:8831" },
  "mode": "snapshot_then_live",
  "tenantId": "t-42"
}

// 服务端 snapshot 响应（示意）
{
  "type": "response",
  "id": "req-reconnect-7",
  "success": true,
  "payload": {
    "sessionId": "s-9",
    "workerId": "worker-b",
    "routeLeaseVersion": "lease-101",
    "upToSeq": 8848,
    "snapshot": { "messages": [/* ... */] }
  }
}
```

单一事实源：

- tenant / route / reconnect 契约：`[new] src/cloud/gateway/protocol.rs`
- shared durable schema：`[new] src/cloud/control_plane/schema.rs`
- storage backend contract：`[new] src/core/session/storage/trait.rs`

**说人话**：Phase C 协议的本质，是让每次恢复和每次重连都不再靠运气。客户端要能带着 cursor 回来，服务端要能明确回答“你现在归谁管、快照补到哪、接下来从哪开始看 live”。

---

## 5. 文件职责总览（One-Glance Map）

```text
┌─ [new] src/cloud/gateway/{server,auth,router,reconnect}.rs ─────────────────────────┐
│ • 公网 / 内网入口                                                                    │
│ • tenant auth / quota gate                                                           │
│ • consistent hash / route lease / resumable subscribe                                │
└──────────────────────────────┬───────────────────────────────────────────────────────┘
                               ▼
┌─ [new] src/cloud/control_plane/{tenant,quota,lease,session_meta}.rs ─────────────────┐
│ • tenant / quota / route / cursor / session metadata                                 │
│ • 强一致小对象控制面                                                                  │
└───────────────┬──────────────────────────────┬────────────────────────────────────────┘
                │                              │
                ▼                              ▼
┌─ src/api/serve/gateway/* + registry.rs ─────┐  ┌─ [new] src/core/session/storage/{postgres,object_store}.rs ─┐
│ • 延续 Phase B dispatcher                    │  │ • transcript / checkpoint / blob durable backend             │
│ • SessionRegistry / SubscriptionRegistry     │  │ • pending_writes / resume cursor persistence                  │
└───────────────┬─────────────────────────────┘  └───────────────┬───────────────────────────────────────────────┘
                │                                                 │
                └─────────────────────┬───────────────────────────┘
                                      ▼
┌─ [new] src/cloud/recovery/{resume,rebalancer}.rs ────────────────────────────────────┐
│ • worker crash recovery                                                               │
│ • lease claim / rehydrate / replay / rebalance                                        │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       ▼
┌─ src/api/chat/run_loop/mod.rs + core/agent_loop/* ───────────────────────────────────┐
│ • 继续作为 worker 内业务执行核心                                                     │
│ • 被恢复后的 session 仍回到同一套 core                                               │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       ▼
┌─ [new] src/cloud/observability/{metrics,trace,audit}.rs ─────────────────────────────┐
│ • by tenant / session / worker / lease / recovery                                    │
│ • SLO / lag / recovery cost / resume gap                                             │
└──────────────────────┬────────────────────────────────────────────────────────────────┘
                       ▼
┌─ tests: chaos / load / failover / multitenant isolation ─────────────────────────────┐
│ • worker kill / reconnect / quota / noisy-neighbor / storage outage                  │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

这张图的阅读顺序是：先看 gateway 和 control plane 这两个云端新增入口层，再看 storage 与 recovery，最后看 observability 和 chaos tests。它说明了一件事：Phase C 的复杂度主要来自“跨进程、跨机器、跨租户”的编排，而不是来自 `AgentLoop` 本身。

**说人话**：到了这一步，Tomcat 的云端复杂度已经有点像“半个消息系统 + 半个控制平面”。越是这样，越要把每层边界画清楚。

---

## 6. 配置与环境变量

总则：**env > config > 默认**。

| 变量 / 配置 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `[cluster].worker_id` | string | 当前 worker 身份 | env / config | 这台机器叫什么。 |
| `[cluster].ring_members` | list | 一致性哈希成员表 | env / config | 整个 worker 圈里有谁。 |
| `[cluster].rebalance_grace_ms` | `u64` | rebalance / lease handoff 宽限期 | env / config | 搬家时给旧 worker 留多长收尾时间。 |
| `[auth].jwks_url` / `[auth].token_secret` | url / secret | 多租户鉴权配置 | env / config | 拿什么验票。 |
| `[tenant].default_quota_profile` | string | 新租户默认配额 | env / config | 默认饭量。 |
| `[storage].control_plane_dsn` | DSN | 控制面 DB | env / config | 小账本数据库。 |
| `[storage].object_store_bucket` | bucket ref | 数据面对象存储 | env / config | 大仓库地址。 |
| `[stream].resume_retention_ms` | `u64` | cursor / snapshot 保留时长 | env / config | 断线后多久内还能无缝接回。 |
| `[observability].metrics_namespace` | string | 指标命名空间 | env / config | 指标打到哪套表里。 |

**说人话**：云端配置的关键不是“模型名”，而是“身份、路由、数据库、对象存储、恢复窗口、指标归属”这些基础设施参数。

---

## 7. 错误模型 / 截断 / 警告

```text
入口级
  auth failed / quota exceeded
    -> reject at gateway

路由级
  route lease stale / no worker available
    -> retry / degraded / recovery path

恢复级
  checkpoint missing / pending_writes corrupt / resume timeout
    -> session recovery failed + explicit terminal state

流级
  cursor too old / gap beyond retention
    -> snapshot fallback + resume_gap warning
```

| 结局 | 触发条件 | 对外形态 | 说人话 |
|------|----------|----------|--------|
| `quota_exceeded` | tenant 超连接数 / 超热会话数 / 超并发 run / 超速率 | gateway 错误响应 | 资源吃满了要早拒绝。 |
| `lease_stale` | 旧 worker lease 失效但仍尝试发流 | drop stale delivery + metrics + audit | 过期拥有权不能继续说了算。 |
| `resume_gap` | cursor 超出保留窗口或缺段 | snapshot fallback + warning | 接不上精确直播时要明确告诉客户端。 |
| `recovery_failed` | checkpoint / pending_writes 恢复失败 | terminal error + re-open guidance | 会话救不回来要显式失败。 |
| `tenant_isolation_violation` | 跨租户读取或路由异常 | hard fail + audit alarm | 这是安全事故级别。 |

**说人话**：Phase C 的错误模型和本地单机完全不是一个量级。这里最重要的是“谁能立刻拒绝”“谁必须告警”“谁属于安全事故”，不能再把所有错误都看成普通 `AppError`。

---

## 8. 测试矩阵（验收）

| 层级 | 目标 | 锚点（测试函数名 / 文件） | 状态 | 说人话 |
|------|------|---------------------------|------|--------|
| 单元 | consistent hash 在成员变更时稳定重映射有限 session | `cloud::gateway::tests::consistent_hash_minimizes_session_movement_on_member_change` | PENDING | 扩容别把所有房间都洗牌。 |
| 单元 | route lease claim / renew / expire 语义正确 | `cloud::control_plane::tests::route_lease_claim_renew_and_expire` | PENDING | 谁负责这个房间要说得清。 |
| 单元 | quota profile 在不同维度都能早拒绝 | `cloud::quota::tests::tenant_quota_enforces_connections_hot_sessions_and_runs` | PENDING | 多租户限流别只拦一种。 |
| 单元 | storage backend 正确拼 control-plane keys 与 blob refs | `core::session::storage::tests::postgres_backend_persists_checkpoint_and_blob_refs` | PENDING | 控制面和大对象引用别串。 |
| 集成 | worker crash 后可在 peer 上 rehydrate 同 session | `tests/cloud_recovery_e2e.rs::worker_crash_rehydrates_session_on_peer` | PENDING | 最关键的灾备验收。 |
| 集成 | reconnect with cursor 无缺帧、无重帧 | `tests/cloud_resume_e2e.rs::reconnect_with_cursor_resumes_without_gap_or_duplicate` | PENDING | 真正的断线续流验收。 |
| 集成 | rebalance 后 session 从 worker A 平滑迁到 worker B | `tests/cloud_rebalance_e2e.rs::rebalance_moves_session_with_explicit_route_handoff` | PENDING | 主动搬家也要稳。 |
| 集成 | tenant 隔离：不同 tenant 不能读到彼此 session | `tests/multitenant_isolation_e2e.rs::cross_tenant_session_access_is_denied` | PENDING | 这是安全底线。 |
| 混沌 | 杀 worker / 断 DB / 断 object store 时系统进入可预期退化 | `tests/cloud_chaos.rs::chaos_worker_kill_and_storage_partition_emit_expected_signals` | PENDING | 真云端必须经得住事故演练。 |
| 压测 | `10^5~10^6` 冷会话、数百～数千热会话下 tail latency / recovery SLO 稳定 | `tests/cloud_scale_serving_load.rs::million_cold_sessions_with_hot_pool_slo` | PENDING | 这是最终容量答案。 |

**说人话**：Phase C 的测试最重要的已经不是“功能通不通”，而是“出事时系统会不会按预期退化和恢复”。如果没有 failover、quota、chaos、resume 这些用例，云端化就只是演示，不是能力。

---

## 9. 风险与应对

| 风险 | 影响 | 应对策略 | 说人话 |
|------|------|----------|--------|
| route lease 与实际 worker 状态漂移 | 同 session 可能出现双主或黑洞 | lease heartbeat + fencing token + explicit claim/release | 这个房间到底归谁，不能含糊。 |
| control-plane DB 与 data-plane store 更新不同步 | 恢复时找得到索引、拿不到数据或反之 | persist order、幂等写、reconciliation job | 小账本和大仓库要定期对账。 |
| quota 只在入口生效，worker 内部无 tenant 维度 | noisy neighbor 仍会拖垮共享资源 | tenant_id 贯穿 worker metrics / scheduler / storage | 限流不能只限门口。 |
| resumable stream retention 过短 | 移动端 / 弱网重连常出现 resume gap | snapshot fallback + retention 配置 + metrics | 续流窗口太短，用户就会频繁掉档。 |
| recovery 过慢 | worker 故障时会话长时间不可用 | 热路径最小化、checkpoint 粒度、parallel prefetch | 会话能恢复还不够，还得恢复得快。 |
| observability 粒度不够 | 云上故障难定位，SLO 难落地 | by tenant/session/worker/recovery 四维指标强制化 | 只看平均值等于闭眼开车。 |

**说人话**：Phase C 最怕的是“看起来一切都有，但真出事故时不知道谁负责、怎么恢复、为什么慢、谁被谁拖垮”。所以这期必须把 lease、quota、resume、metrics 一起做成系统级能力。

---

## 10. 历史决策 / 跨文档修订

1. Phase C 建立在 Phase B 已有 `Storage trait` 和 gateway dispatcher 的前提上，不重新定义这些边界。
2. 本册首次把 `tenant` 提升为与 `session` 并列的一等实体；从这期开始，任何新增云端能力都必须说明 tenant 归属。
3. 本册是 `cloud-scale-serving/` 系列的终点册；若后续继续细分，例如单独写“quota system”或“route lease protocol”，应由本册负责向下导航。

**说人话**：如果说 Phase A 是还债，Phase B 是搭桥，那 Phase C 就是正式把 Tomcat 推到云上跑，并且要求它像一套真正的多租户基础设施那样可靠和可审计。
