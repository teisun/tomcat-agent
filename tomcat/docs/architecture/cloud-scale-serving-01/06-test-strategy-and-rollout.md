# 测试策略、SLO 与上线回滚方案

> 本文是 [`01-overview.md`](./01-overview.md) 的验收与上线分册，统筹 Phase A / B / C 的测试金字塔、负载模型、灰度路径、回滚机制与风险登记。
> 上位测试规范：`docs/openspec/specs/guides/testing/*`。
>
> 本文回答三件事：
>
> 1. **怎样证明这套云端规模化方案不是“纸上扩容”？**
> 2. **每个阶段必须过哪些硬门槛，才能进入下一阶段？**
> 3. **真正上线时，怎么灰度、怎么观测、怎么回滚，才不把现有本地/IDE 体验打炸？**

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象图**：理解“从单元测试到 GA 上线”的整条验证链。
2. 再看 **A.2 具体图**：理解 Phase A/B/C 各自在哪个环境、用什么工件验证。
3. 再看 **B 状态机**：理解一次功能从 dark launch 到 default-on 的灰度生命周期。
4. 最后看 **§8 测试矩阵**：这里是整套方案的验收总表。

### A.1 抽象 ASCII 总图

```text
设计 / 代码改动
  │
  ▼
单元测试
  │ 结构不变量 / 协议字段 / 幂等 / 限流 / seq
  ▼
集成测试
  │ gateway-worker-store / hydrate / replay / sandbox
  ▼
E2E 测试
  │ IDE / Web / SDK / reconnect / approval / migration
  ▼
负载测试
  │ cold/hot/running 比例 / queue lag / replay / drain
  ▼
混沌与浸泡
  │ kill worker / cut store / exhaust sandbox / long soak
  ▼
灰度发布
  │ dark -> shadow -> opt-in -> canary -> regional -> default-on
  ▼
GA
```

读图导读（说人话）：云端规模化最怕两种假象。第一种是假性能: 小样本看起来挺快，真上量时却因为热/冷比例、断线补流、慢消费者而崩。第二种是假稳定: 正常路径很通，但只要 worker 崩、数据库抖、沙箱池耗尽就雪崩。所以测试链必须从“代码正确”一路推到“线上异常也能撑住”，中间不能跳级。

### A.2 具体 ASCII 总图

```text
Phase A（单机整改）
  local CI / dev machine
  ├─ unit: SessionMailbox / HeatState / TurnScheduler
  ├─ integration: current serve + stdio compatibility
  └─ load: 单机 10k cold / 1k warm / 100 hot

Phase B（多 worker）
  staging cluster
  ├─ integration: gateway + 2~5 workers + local sqlite/fs backend
  ├─ e2e: ws/http + stdio adapter
  └─ load: 50k sessions / 10k connections / 500 running turns

Phase C（多租户）
  pre-prod / prod canary
  ├─ integration: postgres + jetstream + object store + sandbox pool
  ├─ chaos: drain / kill worker / store hiccup / quota spike
  ├─ soak: 24h / 72h sustained
  └─ load: 1M sessions / 100k connections / 10k hot / 1k running
```

读图导读（说人话）：不要一开始就把所有测试都堆到“正式集群演练”。Phase A 的价值之一，就是能在单机环境先把核心模型测透。到了 Phase B，再验证 gateway/worker/storage 边界是否真的成立。到了 Phase C，才有资格做大规模、混沌和灰度。这种层层递进的好处，是每一步失败都能比较快地归因，而不是在最终环境里同时被十几个变量折磨。

### B. 状态机：功能灰度与上线生命周期

```text
┌──────────┐ promote ┌──────────┐ promote ┌──────────┐ promote ┌────────────┐
│ dark     │────────▶│ shadow   │────────▶│ opt_in   │────────▶│ tenant_canary│
└────┬─────┘         └────┬─────┘         └────┬─────┘         └────┬───────┘
     │ rollback            │ rollback            │ rollback             │ promote
     ▼                     ▼                     ▼                      ▼
┌──────────┐         ┌──────────┐         ┌──────────┐          ┌────────────┐
│ off      │         │ off      │         │ off      │          │ regional   │
└──────────┘         └──────────┘         └──────────┘          └────┬───────┘
                                                                      │ promote
                                                                      ▼
                                                                ┌────────────┐
                                                                │ default_on │
                                                                └────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `dark` | 内部测试通过 | `shadow` | 开始镜像生产请求但不影响用户 | 线上跟跑，但先不接管用户流量。 |
| `shadow` | 对照稳定 | `opt_in` | 向内部/白名单租户开放 | 先给愿意试的人用。 |
| `opt_in` | 关键指标达标 | `tenant_canary` | 小比例真实租户接入 | 先让少量真实流量走新路。 |
| `tenant_canary` | 区域稳定 | `regional` | 扩大到一个 region / worker pool | 小步扩大，不一口吃成胖子。 |
| `regional` | SLO 连续稳定 | `default_on` | 成为默认路径 | 这时才算真正上线。 |
| 任意态 | 告警超阈值 / 人工回滚 | `off` | 路由切回旧路径、冻结新迁移 | 出现问题必须能立刻撤。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `Baseline` | 改造前的性能和稳定性基线 | benchmark report / metrics snapshot | 每阶段上线前都要对比它 | 没有基线，就不知道是进步还是退步。 |
| `SLO Gate` | 某阶段必须满足的量化门槛 | config + rollout policy | 过不了 gate 就不能升阶段 | 不是“感觉还行”，而是“数字过线”。 |
| `Shadow Traffic` | 复制真实请求但不让结果影响用户 | edge gateway mirror | 必须避免副作用写入重复执行 | 线上跟跑，先悄悄试。 |
| `Soak Test` | 长时间持续运行验证 | 24h / 72h sustained load job | 主要看内存泄漏、队列积压、配额漂移 | 短跑能赢，不代表马拉松也行。 |
| `Chaos Test` | 主动注入故障 | kill pod / cut store / slow bus | 必须在 staging/pre-prod 先做 | 故意把它搞坏，看是不是还能稳住。 |
| `Rollback Fence` | 回滚边界与禁止事项 | feature flags / migration policy | 不能依赖不可逆变更后再想回滚 | 上线前先想清楚怎么撤。 |

## 2. 竞品 / 选型对比（调研）

| 参考 | 文件 | 可借鉴点 | 说人话 |
|------|------|----------|--------|
| `codex-rs` | `app-server-protocol/src/export.rs` | schema fixture、防止协议漂移 | 协议测试不能靠肉眼。 |
| `LangGraph` | `checkpoint-conformance` 系列 | 存储 / checkpoint 行为一致性测试 | backend 一多，就要 conformance。 |
| `OpenClaw` | drain / admission 相关代码 | 灰度、drain、配额与运营指标的联动 | 上线不是只看单个请求成功。 |

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

本章先裁决“怎么测、测到什么程度算过”，再给出测试工件和上线工件的落点。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| T1 测试金字塔 | 是否主要靠 E2E | **采用** 单元 / 集成 / E2E / 负载 / 混沌分层；**拒绝**只用少量 E2E 兜底。 | 本仓：多模块跨层设计；外部：LangGraph conformance、Codex schema fixtures | 会话、回放、checkpoint、配额这类问题，很多必须在更低层先卡死。 | 未入选：只有 E2E；拒因：定位慢、覆盖不精确、成本高。 | 大问题要大测，小问题要小测，不能一锅炖。 |
| T2 阶段门槛 | 是否只看“功能能跑” | **采用** 每阶段固定 SLO Gate；**拒绝**无量化门槛的主观推进。 | 本仓：Phase A/B/C 分期；外部：成熟服务灰度常规做法 | 分期改造必须配分期验收，否则容易一边扩一边失控。 | 未入选：口头评估“差不多可以了”；拒因：多人协作下最容易失真。 | 每一关都要有及格线。 |
| T3 负载模型 | 压测按什么比例建模 | **采用** `cold / warm / hot / running` 分层建模；**拒绝**只按 QPS 或连接数一个维度压。 | 本仓：Phase A HeatState、Phase C quotas；外部：云端 agent 实际热度分布经验 | 这类系统瓶颈不只在 QPS，更在热会话、回放、审批、模型预算。 | 未入选：只压 HTTP QPS；拒因：测不出真正的热点。 | 这不是普通 API，光看请求数会被骗。 |
| T4 上线路径 | 是否直接 big bang | **采用** dark -> shadow -> opt-in -> canary -> regional -> default-on；**拒绝**一次性切流。 | 本仓：本地/云端双模式共存要求；外部：OpenClaw drain/admission 思路、常规云服务灰度实践 | 这套方案跨协议、跨存储、跨运行时，不适合大爆炸。 | 未入选：一次性替换；拒因：回滚面太大，风险集中。 | 先悄悄试，再一点点放量。 |
| T5 回滚策略 | 迁移和回滚是否允许不可逆 | **采用** 特性开关 + 向后兼容 schema + 双写/双读过渡；**拒绝**先上不可逆数据迁移。 | 本仓：Phase A additive compatibility、Phase B trait backend；外部：Codex schema fixtures、LangGraph conformance | 新旧路径至少一段时间要能并存，才能安全撤回。 | 未入选：先迁完再说；拒因：出了事只能硬扛。 | 先想好退路，再敢上路。 |
| T6 可观测 | 指标只按服务维度看够不够 | **采用** `tenant / sessionKey / worker / model / providerClass` 多维观测；**拒绝**只有全局平均值。 | 本仓：多租户、sandbox、model budget 设计；外部：运营服务常规实践 | 全局平均往往掩盖单租户或单 shard 局部雪崩。 | 未入选：只看全局 p95；拒因：真实问题很可能被平均掉。 | 平均分好看，不代表没人考零分。 |
| T7 故障演练 | 混沌测试是否可选 | **采用** Phase C GA 前必须完成混沌与浸泡；**拒绝**只测 happy path。 | 本仓：failover / drain / replay 设计；外部：OpenClaw live infra、LangGraph durable resume | 这套系统最大的风险恰恰在故障与恢复路径。 | 未入选：不上混沌；拒因：最危险的路永远没走过。 | 真要上云，就别只测风和日丽。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| T-A Baseline & fixtures | 基线报告、schema fixtures、golden replay fixtures | `tests/fixtures/*`、`scripts/bench/*` | 见 §8.1 / §8.2 | 先有对照组。 |
| T-B Phase A suite | mailbox / heat / scheduler / stdio compat | `tests/serve_phase_a/*` | 见 §8.2 | 先把单机整改测透。 |
| T-C Phase B suite | gateway/worker/storage/replay/ws/http | `tests/cloud_phase_b/*` | 见 §8.3 | 多 worker 边界要真成立。 |
| T-D Phase C scale & chaos | quota / drain / failover / bus / sandbox / soak | `tests/cloud_phase_c/*`、`tests/chaos/*` | 见 §8.4 / §8.5 | 运营级问题得专门演练。 |
| T-E Rollout automation | feature flag、shadow、canary、rollback runbook | `ops/runbooks/*`、`deploy/*`、CI/CD 配置 | 见 §8.6 | 上线和回滚也要可执行。 |

#### 3.2.1 T-A：基线和 fixture

必须先沉淀三类工件：

1. **性能基线**
   - 当前 `EventBus emit_sync` 耗时
   - 当前 transcript hydrate bytes / latency
   - 当前 `serve` writer 背压行为
2. **协议 fixture**
   - stdio command/event/control 示例
   - `runId/seq` 新字段 fixture
3. **golden replay fixture**
   - 正常流
   - approval 中断流
   - worker crash 后 pending writes 恢复流

#### 3.2.2 T-B：Phase A 单机验收

Phase A 过线条件建议：

- 与基线相比，在 1k warm + 100 hot 会话场景下，用户可见事件 fan-out CPU 明显下降；
- `follow_up` / `steer` 与旧客户端兼容；
- `hot -> warm -> cold` 迁移可重复且无明显内存泄漏；
- 引入 `runId/seq` 后，旧客户端仍能正常消费。

#### 3.2.3 T-C：Phase B 多 worker 验收

Phase B 过线条件建议：

- gateway + 2~5 workers + 本地 backend 环境下，`HTTP command + WS event + stdio adapter` 语义一致；
- placement sticky 有效，非迁移场景下同会话不会在 worker 间抖动；
- reconnect 时 snapshot + tail replay 一致；
- `Storage trait` 的本地 backend 与预期 durable backend 通过 conformance。

#### 3.2.4 T-D：Phase C 规模与混沌验收

Phase C GA 前必须至少完成：

- 24h soak
- worker drain 演练
- kill worker failover 演练
- object store / DB 抖动演练
- quota spike 演练
- sandbox pool 枯竭演练

#### 3.2.5 T-E：上线自动化

上线和回滚不能靠聊天窗口临时操作。至少要有：

- rollout plan 模板
- canary tenant 白名单配置
- 一键切回旧路径的 feature flag
- runbook：告警触发后 5 分钟内怎么撤

## 4. 协议

### 4.1 `LoadProfile`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `registeredSessions` | `u64` | 是 | - | 压测 | 总会话数 | 系统里总共有多少会话。 |
| `connectedSessions` | `u64` | 是 | - | 压测 | 当前在线连接会话数 | 有多少会话正被看着。 |
| `hotSessions` | `u64` | 是 | - | 压测 | 热会话数 | 真在内存里热着多少。 |
| `runningTurns` | `u64` | 是 | - | 压测 | 同时运行 turn 数 | 真在跑的有多少。 |
| `approvalRate` | `float` | 否 | `0` | 压测 | 需要人工审批的比例 | 有多少会话会卡在审批。 |
| `slowConsumerRate` | `float` | 否 | `0` | 压测 | 慢客户端比例 | 有多少连接收得慢。 |

### 4.2 `SloGate`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `name` | `string` | 是 | - | 灰度/发布 | 门槛名字 | 这是哪一关。 |
| `metric` | `string` | 是 | - | 灰度/发布 | 指标名 | 看哪个数字。 |
| `target` | `number` | 是 | - | 灰度/发布 | 目标阈值 | 多少算过。 |
| `window` | `string` | 是 | - | 灰度/发布 | 统计窗口 | 连续看多久。 |
| `rollbackOnBreach` | `bool` | 是 | - | 灰度/发布 | 是否自动触发回滚 | 爆线后要不要立刻撤。 |

### 4.3 `RolloutPlan`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `feature` | `string` | 是 | - | 灰度/发布 | 功能开关名 | 要放量的是谁。 |
| `stage` | `string` | 是 | - | 灰度/发布 | `dark/shadow/opt_in/canary/regional/default_on` | 现在走到哪一步。 |
| `targetTenants` | `string[]` | 否 | `[]` | 灰度/发布 | 本阶段覆盖的租户 | 这一步给谁用。 |
| `entryGates` | `string[]` | 是 | - | 灰度/发布 | 进入本阶段前要满足的 gate | 升阶段前要先过哪些门。 |
| `rollbackFlag` | `string` | 是 | - | 灰度/发布 | 对应回滚开关 | 出事时撤哪一个闸。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | 职责 | 说人话 |
|-------------|------|--------|
| `tests/fixtures/schema/*` | 协议 schema / d.ts fixture | 协议不漂移的标尺。 |
| `tests/fixtures/replay/*` | golden replay / checkpoint fixture | 恢复路径的对照样本。 |
| `tests/serve_phase_a/*` | 单机模型测试 | 先在本地把模型测透。 |
| `tests/cloud_phase_b/*` | gateway/worker/store 测试 | 多 worker 边界验证。 |
| `tests/cloud_phase_c/*` | quota / failover / soak / load | 运营级验收。 |
| `tests/chaos/*` | 失效注入与恢复 | 故意搞坏系统。 |
| `ops/runbooks/*` | 上线/回滚/告警处置手册 | 值班时照着做。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `test.load.default_profile` | 环境分层定义 | 默认压测剖面 | 每次别自己拍脑袋压。 |
| `rollout.shadow_enabled` | `false` | 是否启用 shadow 流量 | 先跟跑再放量。 |
| `rollout.canary_tenants` | 空 | 白名单租户 | 谁先试。 |
| `rollout.auto_rollback` | `true` | 爆线是否自动撤 | 减少值班手抖。 |
| `observability.high_cardinality_labels` | 明确开关 | 是否打开 `sessionKey` 级 tracing | 调试时开，日常别把指标库炸了。 |

## 7. 错误模型 / 截断 / 警告

| 类别 | 条件 | 对外表现 | 恢复策略 | 说人话 |
|------|------|----------|----------|--------|
| `slo_gate_breach` | 指标连续超过阈值 | 阻止 promote 或自动 rollback | 调查根因，回退或扩容 | 这关没过，不能往前走。 |
| `shadow_divergence` | shadow 路径与主路径结果差异过大 | 挂起 canary | 先修语义漂移 | 新旧两条路说的话不一样。 |
| `fixture_drift` | schema/replay fixture 更新未同步 | CI 失败 | 强制补 fixture 或回退改动 | 协议和恢复语义不能偷偷变。 |

## 8. 测试矩阵（验收总表）

### 8.1 基线采集

| 项目 | 指标 | 目标 | 说人话 |
|------|------|------|--------|
| 当前 EventBus fan-out | `emit_sync_p95_ms` | 得到真实基线值 | 先知道现在有多慢。 |
| 当前 hydrate | `hydrate_bytes_read` / `hydrate_p95_ms` | 得到真实基线值 | 先知道会话恢复有多贵。 |
| 当前 writer 背压 | `drop_count` / `buffer_depth` | 得到真实基线值 | 先知道慢消费者会怎样。 |

### 8.2 Phase A 验收

| 测试层 | 核心场景 | 过线标准 | 说人话 |
|--------|----------|----------|--------|
| 单元 | mailbox / heat / scheduler / seq | 所有结构不变量成立 | 核心逻辑先对。 |
| 集成 | stdio 兼容、idle unload、queue | 旧客户端不炸、冷热切换稳定 | 用户体验别被先搞坏。 |
| 负载 | 10k cold / 1k warm / 100 hot | 相比基线，事件 fan-out 和内存曲线显著改善 | 单机整改必须真有收益。 |

### 8.3 Phase B 验收

| 测试层 | 核心场景 | 过线标准 | 说人话 |
|--------|----------|----------|--------|
| 单元 | placement / storage trait conformance / replay cursor | 本地与云端 backend 语义一致 | 接口抽象不能漂。 |
| 集成 | gateway + worker + storage | reconnect / replay / migrate 全链路稳定 | 多 worker 真能接住会话。 |
| E2E | HTTP + WS + stdio adapter | 三条入口语义一致 | 别每个入口都是不同产品。 |
| 负载 | 50k sessions / 10k conns / 500 turns | queue lag 和 replay latency 达标 | 规模继续放大也要稳。 |

### 8.4 Phase C 验收

| 测试层 | 核心场景 | 过线标准 | 说人话 |
|--------|----------|----------|--------|
| 集成 | tenant quota / model budget / drain | 多租户互不饿死，drain 可控 | 运营能力真成立。 |
| E2E | failover / approval / reconnect | 不丢审批、不丢结局、不乱 replay | 最危险的交互都过关。 |
| 负载 | 1M sessions / 100k conns / 10k hot / 1k running | 关键 SLO 达标 | 这才叫上到目标数量级。 |
| 混沌 | kill worker / object store hiccup / bus hiccup | 自动恢复或清晰降级 | 坏了也得稳。 |
| 浸泡 | 24h / 72h | 无明显泄漏、配额漂移、replay 积压 | 长跑也能扛。 |

### 8.5 推荐 SLO

| 指标 | 目标 | 说人话 |
|------|------|--------|
| `turn_start_to_first_event_p95_ms` | Phase A `< 1500ms`，Phase C `< 2000ms`（冷启动另算） | 用户发话后，至少要很快看到系统动起来。 |
| `cold_open_to_replay_ready_p95_ms` | `< 8000ms` | 冷会话打开别等太久。 |
| `replay_gap_recovery_p95_ms` | `< 3000ms` | 断线重连补流要快。 |
| `lossless_event_delivery` | `99.99%+` | 生命周期和审批类事件几乎不能丢。 |
| `tenant_quota_fairness` | 无单租户饿死其他租户 | 大户不能把小户挤没。 |
| `worker_drain_success_rate` | `99%+` | 升级排空要成功。 |

### 8.6 灰度与回滚

| 阶段 | 动作 | 放量条件 | 回滚条件 | 说人话 |
|------|------|----------|----------|--------|
| `dark` | 仅内部链路跑 | 基础指标齐全 | 任意异常 | 先别给用户。 |
| `shadow` | 镜像真实请求但不对外 | 主/影子差异可接受 | 结果偏差、成本异常 | 线上偷偷跟跑。 |
| `opt_in` | 内部租户/白名单 | SLO 连续稳定 | 任意 gate breach | 先给懂的人试。 |
| `tenant_canary` | 小比例真实租户 | 租户级指标稳定 | 单租户严重异常 | 真用户小规模。 |
| `regional` | 一个 region 默认开启 | 整区指标稳定 | 区域级异常 | 先打一块区域。 |
| `default_on` | 全量默认 | 长周期稳定 | 大事故或长期漂移 | 这时才算成熟。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 没有 golden replay fixture | 恢复路径悄悄漂移 | 把 replay fixture 变成 CI 必测工件 | 恢复语义最怕偷偷变。 |
| 只在小样本上压测 | 上线后才暴露热/冷比例问题 | 按分层负载模型压测 | 测试样本不能太幼稚。 |
| 灰度没有自动 rollback | 值班反应慢，损失扩大 | gate breach 绑定自动撤流 | 出事时速度比分析更重要。 |
| 只看全局平均指标 | 局部雪崩被掩盖 | tenant / shard / worker 多维拆看 | 坏事往往先发生在局部。 |

## 10. 历史决策 / 跨文档修订

1. 本文把测试与上线单独抽出来，是因为云端规模化的最大风险不在“能不能写出来”，而在“写出来以后能不能稳地放出去”。
2. 本文的 SLO 与负载模型会直接约束 [`07-development-plan-todos.md`](./07-development-plan-todos.md) 的排期和 DoD。
3. 若后续团队对目标规模做上调或下调，应优先修改本文的 `LoadProfile`、`SloGate` 和 Phase 验收门槛，再回头调整其它架构分册。
