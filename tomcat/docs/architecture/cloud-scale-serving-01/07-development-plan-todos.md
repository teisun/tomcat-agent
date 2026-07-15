# 研发计划、WBS 与里程碑 DoD

> 本文是 [`01-overview.md`](./01-overview.md) 的执行计划分册，把 Phase A / B / C 拆成可排期、可分工、可验收的工作分解结构。
> 关联文档：[`02-phase-a-session-mailbox-hot-cold.md`](./02-phase-a-session-mailbox-hot-cold.md)、[`03-phase-b-gateway-shard-storage-trait.md`](./03-phase-b-gateway-shard-storage-trait.md)、[`04-phase-c-cluster-multitenant.md`](./04-phase-c-cluster-multitenant.md)、[`05-sandbox-workspace-isolation.md`](./05-sandbox-workspace-isolation.md)、[`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md)。
>
> 本文不是“再讲一遍架构”，而是把架构翻译成：
>
> - 里程碑
> - 工作流
> - 依赖关系
> - 交付物
> - DoD（Definition of Done）

---

## 文首导读：方案导图集

### 阅读顺序建议

1. 先看 **A.1 抽象路线图**：掌握整个项目先后顺序。
2. 再看 **A.2 具体工作流图**：看清不同工作流之间的依赖。
3. 再看 **B 里程碑状态机**：看清每个里程碑从“规划”到“完成”要经过哪些状态。
4. 最后进入 **§3.2 详细 WBS**：这是具体可执行的任务清单。

### A.1 抽象 ASCII 路线图

```text
M0 基线与合同
  └─ 指标 / fixture / schema / baseline / feature flags
      ↓
M1 Phase A 单机整改
  └─ SessionMailbox / HeatState / TurnScheduler / stdio compat
      ↓
M2 存储抽象与本地后端
  └─ Storage traits / local backend conformance
      ↓
M3 Phase B 多 worker
  └─ gateway / placement / replay / ws/http adapter
      ↓
M4 沙箱与 workspace
  └─ provider / overlay / egress / credentials / pool
      ↓
M5 Phase C 控制面
  └─ quota / model budget / drain / failover / jetstream
      ↓
M6 上线与 GA
  └─ soak / chaos / canary / regional / default-on
```

读图导读（说人话）：这条路线图故意不是按“代码目录”排，而是按**风险收敛顺序**排。先补基线和合同，再改单机内核，再抽存储，再上多 worker，再补沙箱和控制面，最后才谈 GA。原因很朴素：越靠前的里程碑，越是在给后面所有里程碑降风险。

### A.2 具体 ASCII 工作流依赖图

```text
      [M0 基线与 fixture]
          │        │
          │        ├──────────────┐
          ▼        ▼              ▼
 [M1 SessionMailbox] [M1 HeatState] [M1 Scheduler]
          │        │              │
          └────────┴──────┬───────┘
                           ▼
                   [M2 Storage traits]
                           │
         ┌─────────────────┼──────────────────┐
         ▼                 ▼                  ▼
 [M3 Gateway]      [M3 Placement]      [M3 Replay]
         │                 │                  │
         └────────┬────────┴──────────┬──────┘
                  ▼                   ▼
         [M4 Sandbox provider]   [M5 Control plane]
                  │                   │
                  └──────────┬────────┘
                             ▼
                    [M6 Soak / Chaos / GA]
```

读图导读（说人话）：这里有两个很关键的依赖。第一，**Storage trait 先于多 worker**，否则会在网关、worker、回放、checkpoint 四处同时耦合本地目录结构。第二，**沙箱与控制面并行推进，但都必须在 GA 前完成**：因为控制面没有沙箱只会不安全，沙箱没有控制面只会不好运营。

### B. 里程碑状态机

```text
┌──────────┐ plan ok ┌──────────┐ impl start ┌──────────┐ tests pass ┌──────────┐
│ planned  │────────▶│ ready    │───────────▶│ in_dev   │───────────▶│ verifying│
└────┬─────┘         └────┬─────┘            └────┬─────┘            └────┬─────┘
     │ blocked             │ blocked               │ blocked               │ fail
     ▼                     ▼                       ▼                       ▼
┌──────────┐         ┌──────────┐            ┌──────────┐           ┌──────────┐
│ blocked  │         │ blocked  │            │ blocked  │           │ rework   │
└──────────┘         └──────────┘            └──────────┘           └────┬─────┘
                                                                          │ fix
                                                                          ▼
                                                                    ┌──────────┐
                                                                    │ done     │
                                                                    └──────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `planned` | 前置条件满足 | `ready` | 分配 owner、锁定交付物 | 这活可以开干了。 |
| `ready` | 开始编码/改造 | `in_dev` | 开启 feature flag、写测试骨架 | 真正进实施。 |
| `in_dev` | 测试与文档齐备 | `verifying` | 进入集成、负载、评审 | 不只是代码过了，还要整体过。 |
| `verifying` | DoD 满足 | `done` | 合入主线或进入下一里程碑 | 这一关算结案。 |
| 任意态 | 前置缺失/关键失败 | `blocked` 或 `rework` | 记录 blocker、冻结 promote | 不要带病推进。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `Milestone` | 一组可独立验收的里程碑 | `M0`~`M6` | 必须有 entry criteria、DoD 和 owner | 大阶段关口。 |
| `Workstream` | 某里程碑下按主题拆开的工作流 | `mailbox`、`storage`、`sandbox` 等 | 跨团队协作时可并行，但依赖必须写清 | 一个里程碑里的几条并行线。 |
| `Blocker` | 当前阶段无法继续推进的前置问题 | issue / risk / missing infra | 不允许隐性 blocker 带病继续 | 卡住就要明确说卡在哪。 |
| `DoD` | Definition of Done | 测试、文档、指标、灰度门槛 | 不满足 DoD 不能称完成 | “做完”得有标准。 |
| `Runbook` | 运营处置脚本与步骤文档 | `ops/runbooks/*` | 发布、回滚、故障处理都必须有 | 真出事时照着做的手册。 |

## 2. 竞品 / 选型对比（调研）

| 参考 | 文件 | 对排期拆分的启发 | 说人话 |
|------|------|------------------|--------|
| `LangGraph` | checkpoint / conformance 体系 | 存储抽象和行为一致性要早做，不要等接完多后端再补 | backend 一多，再补 conformance 会很痛。 |
| `Codex` | schema/export + thread manager | 协议 fixture 与 thread/session 模型值得先落，再扩 transport | 合同先稳，后面改动更不容易漂。 |
| `OpenClaw` | placement / drain / admission | 控制面与运营能力不要拖到最后一个礼拜 | 真正难的是运营，不是把代码写出来。 |

## 3. 落地选型与实施（已定稿）

### 3.0 章节编排

先给出路线裁决，再给出详尽 WBS。本文的 WBS 故意拆得细，是为了让团队后续可以直接拿去排 sprint 和 owner。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| W1 先后顺序 | 先上多 worker，还是先修单机模型 | **采用** 先 Phase A、后 Phase B/C；**拒绝**跳过单机整改直接分布式化。 | 本仓：Phase A 现状问题；外部：Codex / LangGraph 都先有清晰单机会话/线程边界 | 基础模型不对时，分布式只会把问题放大。 | 未入选：直接做 gateway/worker；拒因：会把错误的 EventBus 和预算语义复制到更多进程。 | 地基没打好，别先盖楼。 |
| W2 存储抽象时机 | trait 什么时候抽 | **采用** 在 Phase B 前抽 `Storage traits`；**拒绝**等多 worker 全接完再抽。 | 本仓：`src/core/session/*` 当前本地耦合；外部：LangGraph / Codex 存储边界经验 | 如果先写多 worker，再抽存储，返工面会覆盖所有路径。 | 未入选：Phase C 再抽；拒因：耦合会扩散到 replay、checkpoint、placement。 | 先把接口画出来，再接后端。 |
| W3 沙箱时机 | 安全隔离什么时候开始 | **采用** Phase B 即建 `SandboxProvider` 抽象，Phase C 完善运营；**拒绝**等上线前再补隔离。 | 本仓：primitive 能力很强；外部：云端 agent 托管常识 | 安全边界越晚补，返工越大。 | 未入选：最后补；拒因：会反向推翻 workspace、checkpoint、credential 设计。 | 安全不是上线前贴胶带。 |
| W4 测试工件 | fixture / baseline / chaos 何时做 | **采用** M0 就建 baseline 和 fixture，Phase C 前必须完成 chaos/soak；**拒绝**“功能差不多了再补测试”。 | 本仓：协议、replay、quota 都很易漂；外部：Codex fixture、LangGraph conformance | 越早把合同固化，越少返工。 | 未入选：最后补；拒因：会把协议漂移和恢复语义问题拖到最晚暴露。 | 测试工件不是结尾装饰，而是前置护栏。 |
| W5 上线方式 | 是不是一次性切流 | **采用** dark/shadow/canary 渐进式；**拒绝**big bang。 | 本仓：本地和云端双模式并存；外部：OpenClaw drain/admission 经验 | 这套改造面太大，不适合硬切。 | 未入选：一次性替换；拒因：回滚成本过高。 | 慢慢放，不赌国运。 |

### 3.2 实施点总表

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| M0 基线与合同 | baseline、schema fixtures、golden replay、feature flags 骨架 | `tests/fixtures/*`、`scripts/bench/*`、serve schema | 见 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) §8.1 | 先有标尺。 |
| M1 Phase A 单机整改 | mailbox、heat state、scheduler、兼容层 | `src/api/serve/*`、`src/infra/event_bus/*` | 见 `02-phase-a...` / `06...` §8.2 | 先把单机脑子理顺。 |
| M2 存储抽象 | store traits、本地 backend、conformance | `src/cloud/storage/*`、`src/core/session/*` | 见 `03-phase-b...` / `06...` §8.3 | 先把存储边界立起来。 |
| M3 Gateway/Worker | HTTP/WS/stdio adapter、placement、replay、worker runtime | `src/cloud/gateway/*`、`src/cloud/worker/*` | 见 `03-phase-b...` | 多 worker 真正跑起来。 |
| M4 沙箱与 workspace | provider、overlay、egress、credentials、pool | `src/cloud/sandbox/*` | 见 `05-sandbox...` | 安全和执行工位补齐。 |
| M5 控制面与多租户 | quota、model budget、drain、failover、JetStream | `src/cloud/control_plane/*`、`src/cloud/bus/*` | 见 `04-phase-c...` | 正式进入托管服务。 |
| M6 上线与 GA | soak、chaos、canary、runbook、default-on | `ops/*`、`deploy/*` | 见 `06...` §8.4~§8.6 | 真正安全地上生产。 |

#### 3.2.1 M0：基线与合同

##### 交付物

- [ ] `serve` 现状 benchmark 报告
- [ ] schema fixtures（stdio command/event/control）
- [ ] golden replay fixtures（正常、审批、中断、失败）
- [ ] Phase A/B/C feature flag 预留
- [ ] 文档目录与 README 导航骨架

##### 详细 Todo

- [ ] 补 `emit_sync`、writer、hydrate、queue 深度指标
- [ ] 记录现状 16 session / 16 agent limit 的基线表现
- [ ] 固化 `runId/seq` 新字段样例
- [ ] 设计 replay fixture 文件格式
- [ ] 为后续新模块保留 `cloud/*` 目录规划

##### DoD

- [ ] 基线报告可重复运行
- [ ] fixture 已进入 CI
- [ ] feature flags 名称与语义冻结

#### 3.2.2 M1：Phase A 单机整改

##### 交付物

- [ ] `SessionMailbox`
- [ ] `SessionHandle + HotSessionRuntime`
- [ ] `HeatState` 与 idle unload
- [ ] `TurnScheduler`
- [ ] 兼容现有 stdio 客户端的新字段

##### 详细 Todo

- [ ] 从 `SessionSlot` 中拆出轻量摘要与重 runtime
- [ ] 引入 `DeliveryClass`
- [ ] 将用户可见事件改走 mailbox
- [ ] 保留 EventBus 仅用于内部钩子
- [ ] 拆分 `max_live_sessions / max_hot_sessions / max_running_turns`
- [ ] root session handle 不再长期占 running budget
- [ ] 为 queued turn 生成 `queuePosition`
- [ ] 为所有 turn 生成 `runId`
- [ ] 为所有对外事件加 `seq`
- [ ] writer 支持 coalesce 与 drop notice
- [ ] idle unload 支持 `hot -> warm -> cold`
- [ ] 旧客户端忽略新字段仍正常工作

##### DoD

- [ ] 单机负载回归通过
- [ ] stdio 兼容通过
- [ ] 基线对比证明 fan-out 与内存曲线改进

#### 3.2.3 M2：存储抽象

##### 交付物

- [ ] `SessionCatalogStore`
- [ ] `TranscriptStore`
- [ ] `BlobStore`
- [ ] `CheckpointStore`
- [ ] `PlanStore`
- [ ] 本地 backend 和 conformance suite

##### 详细 Todo

- [ ] 从 `sessions.json` 行为抽出 catalog 接口
- [ ] 从 JSONL / sidecar 行为抽出 transcript 接口
- [ ] 规范大对象（tool-results / diff）的 blob 接口
- [ ] 让 shadow git 作为 `CheckpointStore` backend 之一
- [ ] 将 plan / todo 文件行为包到 `PlanStore`
- [ ] 编写 backend conformance fixtures
- [ ] local FS backend 适配 current layout
- [ ] SQLite backend 作为多 worker 前过渡实现

##### DoD

- [ ] local backend 与旧行为一致
- [ ] conformance suite 覆盖 append/read_tail/replay/checkpoint
- [ ] 文档与 trait 一致

#### 3.2.4 M3：Gateway / Worker

##### 交付物

- [ ] `transport_http`
- [ ] `transport_ws`
- [ ] `transport_stdio`
- [ ] `PlacementStore`
- [ ] worker hydrate/replay runtime

##### 详细 Todo

- [ ] 提炼统一 `CommandService` / `EventService`
- [ ] 建立 `SubscribeRequest` / `ReplayCursor`
- [ ] gateway subscription index
- [ ] placement sticky resolver
- [ ] generation/CAS 迁移规则
- [ ] worker shard runtime
- [ ] durable replay fallback
- [ ] stdio adapter 复用同一 schema
- [ ] reconnect / migration E2E 验证

##### DoD

- [ ] 2~5 worker staging 可稳定运行
- [ ] WS/HTTP 与 stdio 语义一致
- [ ] placement 不抖动

#### 3.2.5 M4：沙箱与 workspace

##### 交付物

- [ ] `SandboxProvider` trait
- [ ] `HostSandboxProvider`
- [ ] `GvisorSandboxProvider`
- [ ] overlay workspace
- [ ] `EgressPolicy`
- [ ] `CredentialBundle`
- [ ] prewarm pool

##### 详细 Todo

- [ ] 定义 `SandboxLeaseSpec`
- [ ] base snapshot 生成逻辑
- [ ] overlay diff flush / restore
- [ ] checkpoint 与 workspace snapshot 对齐
- [ ] 扩展 `net_guard` 作为策略 preflight
- [ ] runtime 网络限制接入
- [ ] 短期凭证签发 / 吊销
- [ ] CPU / memory / disk / proc limit 接入
- [ ] suspend / resume / destroy 生命周期
- [ ] 跨 tenant 隔离 E2E 验证

##### DoD

- [ ] 两会话文件系统完全隔离
- [ ] 私网 / 非法出站被 runtime 拒绝
- [ ] suspend/resume 不破坏 workspace 一致性

#### 3.2.6 M5：控制面与多租户

##### 交付物

- [ ] `PlacementService`
- [ ] `QuotaService`
- [ ] `ModelBudgetService`
- [ ] JetStream bus adapter
- [ ] drain / quarantine 流程

##### 详细 Todo

- [ ] tenant config schema
- [ ] 连接/热会话/turn/token/sandbox 配额模型
- [ ] authoritative placement records
- [ ] worker heartbeat
- [ ] drain orchestration
- [ ] failover orchestration
- [ ] replay watermark 与 ack
- [ ] JetStream subject 规划
- [ ] 自动 quarantine 触发条件
- [ ] 运营指标和 dashboard

##### DoD

- [ ] 多租户公平性验证通过
- [ ] worker failover 与 drain 可重复演练
- [ ] 模型预算超限时系统有序降级

#### 3.2.7 M6：上线与 GA

##### 交付物

- [ ] shadow 流量能力
- [ ] opt-in / canary / regional rollout 配置
- [ ] auto rollback
- [ ] runbook
- [ ] soak / chaos 报告

##### 详细 Todo

- [ ] 设计 `RolloutPlan`
- [ ] canary tenant 白名单管理
- [ ] gate breach 自动阻止 promote
- [ ] 一键 rollback flag
- [ ] 24h soak
- [ ] 72h soak（如目标规模需要）
- [ ] worker kill chaos
- [ ] object store 抖动 chaos
- [ ] bus 抖动 chaos
- [ ] sandbox pool 耗尽 chaos
- [ ] 发布 / 回滚 / 故障处置 runbook

##### DoD

- [ ] 所有 SLO gate 达标
- [ ] canary 与 regional 均稳定
- [ ] runbook 经演练可执行

## 4. 协议

### 4.1 `WorkItem`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `id` | `string` | 是 | - | WBS | 唯一任务 ID，如 `M3-WS-04` | 任务编号。 |
| `milestone` | `string` | 是 | - | WBS | 所属里程碑 | 属于哪一关。 |
| `workstream` | `string` | 是 | - | WBS | mailbox / storage / sandbox 等 | 属于哪条线。 |
| `dependsOn` | `string[]` | 否 | `[]` | WBS | 依赖任务 ID | 做它前要先做谁。 |
| `deliverable` | `string` | 是 | - | WBS | 产出物 | 最终交什么。 |
| `dod` | `string[]` | 是 | - | WBS | 完成定义 | 怎样才算结束。 |

### 4.2 `MilestoneDefinition`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `name` | `string` | 是 | - | 规划 | `M0`~`M6` | 哪个里程碑。 |
| `entryCriteria` | `string[]` | 是 | - | 规划 | 进入前提 | 这关开工前要满足什么。 |
| `exitCriteria` | `string[]` | 是 | - | 规划 | 退出条件 | 这关完工要满足什么。 |
| `ownerGroup` | `string[]` | 是 | - | 规划 | 责任团队 | 谁来负责。 |

### 4.3 `DependencyEdge`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `from` | `string` | 是 | - | 规划 | 前置任务 | 谁先做。 |
| `to` | `string` | 是 | - | 规划 | 后置任务 | 谁后做。 |
| `type` | `string` | 是 | - | 规划 | `hard/soft` | 是硬依赖还是软依赖。 |

## 5. 文件职责总览（One-Glance Map）

| 文件 / 模块 | 职责 | 说人话 |
|-------------|------|--------|
| `docs/architecture/cloud-scale-serving-01/*` | 架构与执行计划文档集 | 先把图纸画全。 |
| `tests/fixtures/*` | 协议 / replay / conformance 样本 | 把合同固化。 |
| `tests/serve_phase_a/*` | 单机整改测试 | 先测单机。 |
| `tests/cloud_phase_b/*` | gateway/worker/store 测试 | 再测分层。 |
| `tests/cloud_phase_c/*` | 多租户与 failover 测试 | 最后测运营。 |
| `ops/runbooks/*` | 发布/回滚/故障操作手册 | 上线时照着做。 |

## 6. 配置与环境变量

| 配置项 | 默认建议 | 说明 | 说人话 |
|--------|----------|------|--------|
| `feature.phase_a_mailbox` | `false` | Phase A feature flag | 先灰度。 |
| `feature.phase_b_gateway` | `false` | Gateway/Worker 路由开关 | 多 worker 路径开关。 |
| `feature.phase_c_control_plane` | `false` | 控制面 authoritative 开关 | 多租户治理开关。 |
| `rollout.current_stage` | 环境配置 | 当前灰度阶段 | 现在放量到哪。 |

## 7. 错误模型 / Blocker 约定

| 类别 | 定义 | 处理方式 | 说人话 |
|------|------|----------|--------|
| `hard_blocker` | 缺基础设施或前置接口，后续任务无法继续 | 冻结后续 dependent work | 真的卡死了。 |
| `soft_blocker` | 可并行推进，但会影响效率或验收 | 记录并定期 review | 先记着，不至于停工。 |
| `rollout_blocker` | SLO/chaos/soak 未过，禁止 promote | 自动阻止灰度升级 | 没过线就别放量。 |

## 8. 验收与 DoD 总表

| 里程碑 | 最低 DoD | 说人话 |
|--------|----------|--------|
| M0 | baseline + fixtures + flags 完整 | 没有标尺不准开工。 |
| M1 | 单机模型正确、兼容不炸、指标改善 | 单机脑子先理顺。 |
| M2 | store traits 与本地 backend 稳定 | 数据边界先立住。 |
| M3 | gateway/worker/replay 真跑通 | 分层不是纸上谈兵。 |
| M4 | 沙箱隔离与 workspace 恢复成立 | 安全底线补齐。 |
| M5 | 多租户、公平性、drain/failover 成立 | 真正像托管服务。 |
| M6 | soak/chaos/canary 全过 | 才配叫 GA。 |

## 9. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| WBS 拆分不够细 | 实施时漏任务或互相等待 | 保持 milestone 下 workstream 级任务可追踪 | 粗计划最容易漏活。 |
| 过早并行过多工作流 | 集成时爆炸 | 通过 `DependencyEdge` 明确硬依赖 | 不是所有线都能同时开。 |
| 文档与实现脱节 | 团队按旧理解开发 | 每个里程碑完结时更新对应文档与 fixture | 图纸不能落灰。 |

## 10. 历史决策 / 跨文档修订

1. 本文刻意把测试、灰度、回滚也算进 WBS，因为对云端规模化 Serving 来说，它们不是“收尾工作”，而是主线工作。
2. 若未来资源有限需要裁剪范围，优先裁掉“更远期的 provider class / 区域级优化”，不要裁掉 M0、M1、M2 这种地基工作。
3. 本文与 [`06-test-strategy-and-rollout.md`](./06-test-strategy-and-rollout.md) 是一对：后者定义怎么验，本文定义谁先做什么再去验。
