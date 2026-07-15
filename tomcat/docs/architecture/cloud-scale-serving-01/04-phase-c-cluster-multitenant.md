# Phase C：集群化、多租户治理与跨 Worker 恢复

> 本文是 [`01-overview.md`](./01-overview.md) 的 Phase C 分册，回答“当 Tomcat 真正进入云端托管形态后，如何从能扩展，变成能运营、能隔离、能恢复、能滚动发布”。
> 它建立在 [`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md) 之上，假设 Gateway / Worker / Storage trait 已经存在。
>
> Phase C 的主题不是再发明单机会话模型，而是建立：
>
> - 控制面 / 数据面的边界
> - 多租户鉴权、配额与限流
> - 跨 worker 断点恢复与 replay
> - 滚动发布、drain、自动扩缩容
> - 全局 LLM 预算与运营可观测

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象图**：抓住控制面和数据面分别负责什么。
2. 再看 **A.2 具体图**：理解 Phase C 需要新增哪些服务与部署工件。
3. 再看 **B 状态机**：理解一个会话在 failover / drain / replay 期间如何保持连贯。
4. 最后看 **§3 决策矩阵**：这里钉死多租户、全局预算、消息总线和 K8s 运营模型。

### A.1 抽象 ASCII 总图

```text
客户端 / IDE / Web / SDK
  │
  ▼
┌──────────────────────── Edge Gateway ────────────────────────┐
│ TLS / auth / tenant resolve / request validation / ws fan-in │
└───────────────────────────┬──────────────────────────────────┘
                            │
                            ▼
┌──────────────────────── Control Plane ───────────────────────┐
│ tenant config │ quota service │ placement authority │ drain │
│ model budget │ replay policy │ feature flags │ audit policy │
└───────────────┬───────────────────────────────┬─────────────┘
                │                               │
                ▼                               ▼
┌──────────────────────── Data Plane ──────────────────────────┐
│ Gateway workers │ Session workers │ Sandbox pool │ live bus  │
│ short replay cache │ hot session runtimes │ durable writers   │
└───────────────┬───────────────────────────────┬─────────────┘
                │                               │
                ▼                               ▼
┌──────────────────────── Shared Infra ────────────────────────┐
│ Postgres │ NATS JetStream │ Object Store │ Metrics / Logs / OTel │
└───────────────────────────────────────────────────────────────┘
```

读图导读（说人话）：到了 Phase C，Gateway 也不再是一个单体角色，而要拆成“边缘接入”和“控制决策”两层。原因很简单：接入层要扛连接和 TLS，控制面要记租户配置、配额和 placement，数据面要跑真正的会话执行与事件回放。把这三件事继续混在同一个服务里，迟早会出现“一个热点问题拖垮全局”的运营事故。

### A.2 具体 ASCII 总图

```text
Kubernetes / Cloud Deployment
────────────────────────────────────────────────────────────────────────────
Ingress / API Gateway
  └─ routes:
     - /v1/commands/*      -> edge-gateway deployment
     - /v1/stream          -> edge-gateway deployment

edge-gateway deployment
  ├─ auth / tenant binding
  ├─ ws session multiplexing
  ├─ short replay cache
  └─ forwards to control-plane / worker-gateway

control-plane deployment
  ├─ placement service
  ├─ quota service
  ├─ model budget service
  ├─ drain / rollout coordinator
  └─ feature flag + tenant config API

worker deployment
  ├─ session worker runtime
  ├─ SessionMailbox / TurnScheduler / hydrate
  ├─ sandbox lease client
  ├─ checkpoint writer
  └─ publishes events to JetStream

shared infra
  ├─ Postgres          tenant/session/placement/catalog metadata
  ├─ NATS JetStream    live fan-out / ack / drain signals
  ├─ Object Store      blobs / snapshots / large transcript segments
  └─ Prometheus/OTel   metrics / traces / alerts
```

读图导读（说人话）：Phase B 里的“一个 gateway、多台 worker”到了 Phase C 会被部署成多副本服务，而且它们之间不只是 HTTP 调用关系，还会通过共享 metadata、消息总线和对象存储协作。这里最关键的不是“上了 K8s”，而是**谁说了算**：placement 谁 authoritative、quota 谁 authoritative、failover 谁判定、drain 谁发号施令，这些都必须归到控制面。

### B. 状态机：会话 failover / drain 生命周期

```text
┌──────────┐ drain begin ┌─────────────┐ checkpoint ok ┌────────────┐ hydrate ok ┌──────────┐
│ steady   │────────────▶│ draining    │──────────────▶│ recovering │───────────▶│ replaying│
└────┬─────┘             └────┬────────┘               └────┬───────┘            └────┬─────┘
     │ worker crash            │ timeout / crash               │ hydrate fail             │ replay done
     ▼                         ▼                                ▼                          ▼
┌──────────┐             ┌─────────────┐                  ┌────────────┐             ┌──────────┐
│ failover │────────────▶│ quarantined │                  │ degraded   │             │ steady   │
└──────────┘             └─────────────┘                  └────────────┘             └──────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `steady` | worker drain 开始 | `draining` | 停收新 turn，保留 in-flight replay/approval | 先礼貌地“只出不进”。 |
| `steady` | worker crash | `failover` | 控制面抢占 placement，找新 worker | 机器突然死了也要有人接盘。 |
| `draining` | checkpoint / pending writes 提交成功 | `recovering` | 新 worker 开始 hydrate | 有序迁移就按流程交接。 |
| `recovering` | hydrate 成功 | `replaying` | 给客户端补 snapshot + tail | 新 worker 先补历史再接实时。 |
| `replaying` | replay 完成 | `steady` | 会话重新进入稳态 | 恢复完成，继续服务。 |
| 任意态 | hydrate / replay 失败 | `degraded` 或 `quarantined` | 限制读写、拉响告警、等待人工/自动重试 | 坏了也要可控地坏。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `Control Plane` | authoritative 决策层 | placement/quota/config services | 不跑 AgentLoop，不持有热会话 | 发号施令，不亲自干活。 |
| `Data Plane` | 真正跑会话和转发事件的层 | gateway workers + session workers | 可水平扩容，状态尽量短驻留 | 干活的一线队伍。 |
| `TenantQuota` | 某租户可消耗的连接、热会话、并发 turn、模型 token 预算 | Postgres row + in-memory cache | 必须全局一致，不得只在单 worker 本地记 | 大客户不能把整池水喝干。 |
| `ModelBudget` | 跨 worker 的全局 LLM 调用与 token 节流 | control-plane service + distributed token bucket | 不允许各 worker 各自无上限抢模型 | 模型供应也是共享资源。 |
| `DrainMode` | 某 worker / 某 shard / 某 zone 停止接新流量的状态 | control-plane flag + JetStream signal | `draining` 时不再接新 turn，只清空存量 | 滚动升级时要先“只出不进”。 |
| `ReplayWatermark` | 某客户端 / 某连接已确认消费到的事件边界 | `sessionKey + seq` | reconnect 时 authoritative 以它为准 | 断线续流凭这个对齐。 |
| `Quarantine` | 某 worker / 某会话因异常被隔离 | placement / health state | 不再承接新流量，等待人工或自动恢复 | 某处坏得太怪，就先隔离起来。 |

## 2. 竞品 / 选型对比（调研）

| 参考 | 文件 | 借鉴点 | 不直接照抄的地方 | 说人话 |
|------|------|--------|------------------|--------|
| `OpenClaw` | `gateway-work-admission.ts` | 网关级 drain / admission 控制 | OpenClaw 的整体产品面更宽，Tomcat 可以更聚焦 session serving | 滚动发布和过载保护这套很有价值。 |
| `OpenClaw` | `worker-environments/live-events.ts` | seq ack 窗口、pending bytes | 我们已有 `runId/seq`，可直接嫁接 | 断线补流和背压治理的实战味很重。 |
| `LangGraph` | `checkpoint/README.md`、SDK stream transport | durable interrupt/resume、命令/事件分离 | graph 模型不完全等于 coding agent | 恢复语义成熟，适合借来做 checkpoint 纪律。 |
| `Codex` | `transport.rs`、`thread_state.rs` | 多 transport、thread listener、请求串行域 | Codex 偏本地 app-server，租户治理不足 | 连接和线程模型值得借，控制面要自己补。 |

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章围绕“谁 authoritative、谁发号施令、谁负责恢复”来定稿。Phase C 不再只是技术扩展，而是正式进入托管服务的运维世界。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| C1 控制面 / 数据面边界 | placement、quota、drain 是否由 worker 兼职处理 | **采用** 独立控制面服务 authoritative，数据面只执行；**拒绝**把这些决策散落在各 worker。 | 本仓：Phase B gateway/worker/store 拆层；外部：`OpenClaw` placement/admission 设计、`LangGraph` 云端 transport 分离 | authoritative 决策集中后，重启、迁移、扩缩容才有统一真相源。 | 未入选：worker 本地各自记账和分配；拒因：跨 worker 公平性和恢复一致性无法保证。 | 谁能接单、谁该限流、谁该下线，得有人统一拍板。 |
| C2 多租户配额 | 配额放在本地还是全局 | **采用** tenant 级全局配额服务：连接数、热会话数、并发 turn、LLM token/s、sandbox 并发；**拒绝**只在单 worker 局部限流。 | 本仓：`src/core/llm/openai.rs` 仅本地 semaphore、`src/api/serve/commands.rs` 局部 busy；外部：`OpenClaw` admission、`LangGraph` multitask/stream policy | 多租户托管必须先全局公平，再谈单机会不会过载。 | 未入选：per-worker quota only；拒因：大租户可横向打满全部 worker。 | 不能让某个租户把所有机器都悄悄吃满。 |
| C3 全局模型预算 | 模型并发和 token 预算怎么管 | **采用** 控制面 `ModelBudgetService` + distributed token bucket；**拒绝**只靠每个 provider 自带 semaphore。 | 本仓：`src/core/llm/openai.rs`；外部：`LangGraph` durability/stream discipline、`OpenClaw` admission 思路 | 每个 worker 只知道自己，不知道全局；模型预算必须跨 worker 协调。 | 未入选：继续 `Semaphore(4)` 一类本地限制；拒因：横向扩容后总并发不可控。 | 模型供应是全局水龙头，不能每个工位自己随便开。 |
| C4 实时事件总线 | gateway 和 worker 之间靠什么做 live fan-out | **采用** `NATS JetStream` 作为推荐默认；**保留** `Redis Streams` 为轻量部署备选；**拒绝**只靠 HTTP 直连或数据库轮询。 | 本仓：Phase B `SessionMailbox` / replay 语义；外部：`OpenClaw` live events 窗口、`LangGraph` WS stream 模式 | JetStream 同时满足 durable stream、ack、consumer group 和控制信号广播，适合会话事件流。 | 未入选：DB polling / gateway->worker direct only；拒因：延迟和扩展性都差，failover 时很脆。 | 事件流要有像公交系统一样的专门轨道，不该靠小汽车临时接送。 |
| C5 恢复语义 | worker 挂了后从哪里恢复 | **采用** `checkpoint + pending writes + replay watermark` 三件套；**拒绝**只看 transcript 尾部。 | 本仓：`src/core/checkpoint/*`、`src/core/session/resume_index.rs`；外部：`LangGraph` pending writes、`OpenClaw` seq 窗口 | transcript 能恢复历史，不一定能恢复“哪一步已经执行过”；pending writes 补这块空白。 | 未入选：只靠 transcript；拒因：工具链部分成功时无法安全幂等恢复。 | 历史聊天记录不等于执行恢复点。 |
| C6 滚动发布与 drain | 升级时怎么不停机 | **采用** worker `draining` + placement freeze + replay handoff；**拒绝**杀 Pod 后让会话自然失败。 | 本仓：Phase B placement 语义；外部：`OpenClaw` gateway drain / placement generation | 托管服务的升级不应依赖用户“刷新一下重试”。 | 未入选：裸滚动重启；拒因：中断 in-flight run 和审批会话，用户体验极差。 | 升级时应该像换班，而不是停电。 |
| C7 自动扩缩容 | 按什么指标扩容 | **采用** `hot_sessions`、`queued_turns`、`queue_wait_ms`、`llm_wait_ms`、`sandbox_pool_pressure` 联合驱动；**拒绝**只看 CPU。 | 本仓：Phase A/B 指标体系；外部：`OpenClaw` admission/drain 实战 | CPU 低不代表没问题，LLM 等待、队列积压、沙箱耗尽更关键。 | 未入选：HPA 只看 CPU / 内存；拒因：会错判排队和外部资源瓶颈。 | 很多时候不是机器算不动，而是模型配额或沙箱不够。 |
| C8 鉴权与隔离 | `sessionKey` 级别还是 tenant 级别 | **采用** tenant 级鉴权 + `sessionKey` ownership 校验 + sandbox 级隔离；**拒绝**只做应用层 session 检查。 | 本仓：`net_guard.rs`、`work-dir-and-data-layout.md`；外部：`OpenClaw` auth / server-session-key.ts | 会话归属、凭证发放、文件系统和网络策略必须共同构成隔离边界。 | 未入选：只在业务代码里判断 sessionId；拒因：挡不住运行时越权和配置错误。 | 真隔离不是“看起来分开了”，而是登录、文件、网络都得分开。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| C1 控制面服务 | placement、quota、model budget、drain/rollout API | `src/cloud/control_plane/*` | 见本文 §8.2 / §8.4 | 先把指挥系统建起来。 |
| C2 JetStream live bus | worker 事件发布、gateway 订阅、ack / replay watermark | `src/cloud/bus/*`、gateway/worker adapter | 见本文 §8.1 / §8.3 | 把实时总线正规化。 |
| C3 失效恢复链路 | failover、checkpoint handoff、replay、quarantine | `src/cloud/control_plane/failover.rs`、worker hydrate/replay | 见本文 §8.3 / §8.5 | 机器坏了也能续上。 |
| C4 K8s 运营工件 | deployment、PDB、HPA、drain hook、preStop | `deploy/k8s/*`、`helm/*` | 见本文 §8.4 | 真正能上线运维的配套。 |
| C5 运营可观测 | tenant 仪表盘、SLO、审计、告警规则 | `ops/dashboards/*`、`ops/alerts/*` | 见本文 §8.4 | 让值班的人看得明白。 |

#### 3.2.1 C1：控制面 authoritative 能力

控制面至少要提供四类 authoritative 服务：

1. `PlacementService`
   - `assign(sessionKey)`
   - `migrate(sessionKey, targetWorker?)`
   - `begin_drain(workerId)`
2. `QuotaService`
   - `reserve_turn(tenantId)`
   - `reserve_hot_session(tenantId)`
   - `release_*`
3. `ModelBudgetService`
   - `reserve_model_tokens(tenantId, model, estimatedTokens)`
   - `commit_actual_tokens(...)`
4. `FeatureFlagService`
   - 按 tenant / region / workload 开关新能力

这些服务都不能由任意 worker 本地决定。

#### 3.2.2 C2：JetStream 事件轨道

推荐主题设计：

- `session.live.{shard}`
  - worker -> gateway live events
- `session.control.{region}`
  - drain / placement / invalidate cache
- `session.replay.{tenant}`
  - 可选的异步 replay/snapshot 任務

选择 JetStream 的理由：

- 支持 durable consumer
- 有 ack / redelivery 语义
- 既能做广播控制信号，也能做按 shard/tenant 分组的事件流

小规模部署可落到 Redis Streams，但主设计不以它为前提。

#### 3.2.3 C3：failover 与 quarantine

failover 推荐分两层：

- **自动 failover**：worker crash、心跳超时、pod 被驱逐
- **人工 quarantine**：怀疑某 worker/sandbox/tenant 有异常污染时，禁止继续接流量

恢复顺序：

1. placement 抢占到新 generation
2. 从 durable store 读 checkpoint 和 pending writes
3. hydrate sandbox/workspace snapshot
4. replay 到 `ReplayWatermark`
5. live stream 恢复

若第 2–4 步任一失败，则会话进入 `degraded`，而不是无限重试打爆系统。

#### 3.2.4 C4：Kubernetes 运营约束

建议 Kubernetes 约束：

- `PodDisruptionBudget`：避免同一 shard/zone 同时被驱逐
- `preStop`：向控制面声明 `draining`
- `readinessProbe`：drain 后立即失败，停止接新流量
- `livenessProbe`：只在真的卡死时重启，避免对 `awaiting_user` 会话误杀
- `HorizontalPodAutoscaler`：以 queue lag / hot sessions / llm wait 为主指标

#### 3.2.5 C5：运营指标

必须按 `tenant / region / worker / model / sandbox class` 维度可切片：

- `active_connections`
- `hot_sessions`
- `running_turns`
- `queue_wait_ms`
- `replay_latency_ms`
- `checkpoint_commit_ms`
- `model_budget_wait_ms`
- `sandbox_pool_pressure`
- `drain_duration_ms`
- `quarantine_count`

## 4. 协议

### 4.1 `TenantQuotaSnapshot`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `tenantId` | `string` | 是 | - | 控制面 | 租户标识 | 哪家租户。 |
| `maxConnections` | `u32` | 是 | - | 配额 | 并发连接上限 | 最多能挂多少连接。 |
| `maxHotSessions` | `u32` | 是 | - | 配额 | 热会话上限 | 最多能热着多少会话。 |
| `maxRunningTurns` | `u32` | 是 | - | 配额 | 并发 turn 上限 | 最多能同时跑多少轮。 |
| `modelBudgets` | `object` | 是 | - | 配额 | 各模型 token/s 与 rpm 预算 | 模型预算表。 |

### 4.2 `WorkerHeartbeat`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `workerId` | `string` | 是 | - | 控制面 | worker 唯一标识 | 哪台机器。 |
| `zone` | `string` | 是 | - | 控制面 | 可用区/节点池 | 在哪里。 |
| `hotSessions` | `u32` | 是 | - | 调度 | 当前热会话数 | 这台机子有多忙。 |
| `queuedTurns` | `u32` | 是 | - | 调度 | 当前排队 turn | 队伍有多长。 |
| `sandboxPressure` | `float` | 是 | - | 调度 | 沙箱池压力 | 工位够不够。 |
| `drainState` | `string` | 是 | - | 运维 | `active/draining/quarantined` | 是正常、下线中还是隔离中。 |

### 4.3 `ReplayAck`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `sessionKey` | `string` | 是 | - | live stream | 会话主键 | 哪个会话。 |
| `seq` | `u64` | 是 | - | live stream | 客户端已确认消费到的最高序号 | 你已经收到了哪儿。 |
| `connectionId` | `string` | 是 | - | live stream | 哪条连接发来的 ack | 多连接时区分消费者。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | Phase C 职责 | 说人话 |
|-------------|--------------|--------|
| `src/cloud/control_plane/placement.rs` | authoritative placement、generation CAS、drain 协调 | 会话归属总裁判。 |
| `src/cloud/control_plane/quota.rs` | tenant 级连接 / 热态 / turn / token 配额 | 全局记账本。 |
| `src/cloud/control_plane/model_budget.rs` | 跨 worker 模型预算协调 | 模型水龙头总开关。 |
| `src/cloud/bus/jetstream.rs` | live bus / control bus adapter | 实时轨道。 |
| `deploy/k8s/*` | Deployment、Service、PDB、HPA、preStop hooks | 上线工地蓝图。 |
| `ops/dashboards/*` | 指标看板与容量视图 | 值班视角。 |
| `ops/alerts/*` | SLO、quota、replay、drain 告警 | 出事时第一时间叫人。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `cloud.control_plane.pg_dsn` | 必填 | authoritative metadata DB | 真相源数据库。 |
| `cloud.bus.jetstream_url` | 必填 | live/control bus | 实时总线地址。 |
| `cloud.bus.replay_consumer_ack_ms` | `5000` | consumer ack 超时 | 多久不 ack 算掉线。 |
| `cloud.quota.default_max_connections` | tenant default | 默认连接配额 | 没单独配置的租户也要有底线。 |
| `cloud.model_budget.default_rpm` | model default | 模型默认速率限制 | 模型别被瞬间打爆。 |
| `cloud.deploy.drain_timeout_ms` | `60000` | worker drain 超时 | 最多等多久清空存量。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 条件 | 对外表现 | 恢复策略 | 说人话 |
|------|------|----------|----------|--------|
| `tenant_quota_exceeded` | 租户资源配额耗尽 | 命令拒绝或排队 | 等窗口恢复或人工扩配 | 你家的额度用完了。 |
| `model_budget_exhausted` | 模型全局预算耗尽 | turn 延迟启动 | 排队等待或切备用模型 | 模型水龙头要节流。 |
| `worker_quarantined` | worker 因异常被隔离 | 会话迁走，新命令不再派发给它 | failover 到健康 worker | 这台机子有毒，先别再派活。 |
| `replay_incomplete` | durable replay 仍无法补齐 | 客户端收到 `degraded_snapshot` | 人工干预或全量恢复 | 补流补不全，先给你降级快照。 |

## 8. 测试矩阵（验收）

### 8.1 单元测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| quota token bucket | 跨 worker reserve/release 一致 | 全局预算别算错账。 |
| placement generation CAS | 并发 failover 不会双 owner | 同一会话不能双写。 |
| replay ack window | ack 超时会触发 redelivery 或 replay_required | 掉线补流要有纪律。 |

### 8.2 集成测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 一租户打满配额，另一租户仍可服务 | 租户互不饿死 | 多租户公平要成立。 |
| worker drain + pod 重建 | 无新命令落到 draining worker，旧会话有序迁移 | 升级时不该让人掉会话。 |
| 模型预算耗尽 | admission 延迟而不是全局雪崩 | 模型紧张时也要有秩序。 |

### 8.3 E2E 测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| worker 崩溃时，正在 `awaiting_user` 的会话恢复 | 审批请求不重复、不丢失、回包仍命中正确 run | 最难的场景要能过。 |
| Web 客户端断线重连到另一 edge gateway | replay 后状态与断线前一致 | 前门换了也得接上。 |
| 跨 region / zone failover（如设计支持） | placement 与 replay 正确 | 更大故障也能撑住。 |

### 8.4 负载与容量测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 100k 长连接、10k 热会话、百级并发 turn | edge gateway、control plane、worker 各自 SLO 达标 | 真上量再看是不是站得住。 |
| drain 期间 20% worker 轮换 | SLO 不显著劣化 | 升级不应等于抖动。 |

### 8.5 混沌测试

| 场景 | 断言 | 说人话 |
|------|------|--------|
| 杀 JetStream 节点 / 抖动 Postgres | 控制面与数据面降级可控、告警准确 | 基础设施坏了，也不能瞎。 |
| 随机 quarantine worker | 会话自动迁走、容量下降可见 | 隔离坏节点要像切掉坏电路。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 控制面过于集中 | 变成新的单点 | 控制面自身高可用，缓存只做加速不做真相源 | 指挥部也要有备份。 |
| 全局预算服务本身成为瓶颈 | admission 慢、级联阻塞 | 本地 cache + batch reserve + fallback policy | 记账别比干活还慢。 |
| 多租户隔离只做“业务字段隔离” | 运行时仍可能串租户 | 与 sandbox、凭证、网络策略联动 | 纸面隔离不算隔离。 |

## 10. 历史决策 / 跨文档修订

1. Phase C 把 Phase B 的 placement 从“gateway 内部能力”提升为“控制面 authoritative 服务”。
2. Phase C 明确选择 `NATS JetStream` 作为推荐默认 live bus，但保留轻量替代，不把产品绑死在某一家中间件上。
3. Phase C 的多租户与全球预算策略，会反向约束 [`05-sandbox-workspace-isolation.md`](./05-sandbox-workspace-isolation.md) 的资源配额与凭证模型。
