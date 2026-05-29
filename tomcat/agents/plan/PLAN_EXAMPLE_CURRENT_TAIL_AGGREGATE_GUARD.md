---
name: Current-Tail Aggregate Guard
overview: 阶段二处理 aggregate current-tail overflow：在每次工具循环结束、发 LLM 前做 mid-turn aggregate precheck；若当前轮可降载 tool result 总量已逼近预算，就先对 current tail 做按总量的批量减负；仍不够时 split current turn，并显式保活 plan mode / plan runtime / 当前步骤 / pending work。同批接线 keep_recent_turns（默认 5）并删除无效的 compaction_turns。
todos:
  - id: wire-keep-recent
    content: L1 保护区改读 config.keep_recent_turns（默认 5），删除 M_PROTECTED_TURNS；同步 context-management.md / core README
    status: pending
  - id: remove-compaction-turns
    content: 删除 context.compaction_turns（ContextConfig、allowlist、catalog 文案、单测）
    status: pending
  - id: mid-turn-precheck
    content: reasoning_loop 每次工具循环结束、发 LLM 前 aggregate precheck（工作集总量 + current turn tool result 累计 vs 扣输出预留后预算）；撞线则进入减负分支而非正常推进
    status: pending
  - id: aggregate-reduction
    content: current_tail_guard：按总量降载 current tail——较老 replay-safe 结果批量 preview/placeholder/引用+续读提示，保留最近几步原文；不可降载项不硬截
    status: pending
  - id: split-turn
    content: reduction 仍不够或大量不可简单截断时 split current turn（prefix checkpoint summary + suffix verbatim）；必须保活 plan mode/runtime/当前 step/pending work
    status: pending
  - id: state-keepalive
    content: split/reduction 路径保活执行态：plan mode、plan runtime、当前 plan step、关键 pending work；避免 summary 后压没正在执行的 plan
    status: pending
  - id: tests
    content: 单测/场景：keep_recent_turns、medium read 累积 precheck、mixed tail 只降 replay-safe、split turn、长 plan 执行连续性、仅压老历史时仍会爆窗的负向用例
    status: pending
isProject: false
---

# Current-Tail Aggregate Guard（阶段二：precheck + aggregate reduction + split turn）

本文是**阶段二计划**。它解决的是阶段一故意没有处理的那类问题：不是单个 `read` 窗口太大，而是**当前轮执行很久，很多中等 tool result 累积**，把上下文从 10% 一路堆到 90%+，最后在发下一次 LLM 请求时撞线。

说人话：阶段一处理“一块大石头”，阶段二处理“很多中等箱子慢慢把楼板压塌”。

## 1. 背景

本章说明为什么阶段一不够、为什么老历史摘要救不了 current turn、以及为什么配置清理要和第二阶段一起做。

说人话：这里先把“为什么必须上第二阶段”讲透，不然很容易误以为阶段一的 128KB 护栏就已经全解决了。

### 1.1 阶段二要解决的到底是什么

| 问题类型 | 压力源 | 典型表现 | 为什么阶段一不够 | 说人话 |
| --- | --- | --- | --- | --- |
| `single huge read overflow` | 单个 `read` 窗口过胖 | 一窗就把上下文顶爆 | 阶段一已用 128KB 护栏处理 | 一块大石头，门口先挡掉 |
| `aggregate current-tail overflow` | 很多中等 tool result 累积 | 每条都不过单条阈值，但总和会在下一次发 LLM 前撞线 | 阶段一只管单窗护栏，不会处理“多箱累计超载” | 很多中等箱子单个不违规，但合在一起把楼板压塌 |
| `long-running plan overflow` | 执行一个 plan 很久，current turn 内积累大量读/搜/只读 shell 输出 | 压缩一旦做错，就把 `plan mode` / 当前 step / runtime 状态压没 | 阶段一不做 split / state keepalive | 不是光压 token，还得保证“正在执行什么”别丢 |

### 1.2 为什么老历史压缩救不了 current turn 膨胀

| 误区 | 专业描述 | 正确结论 | 说人话 |
| --- | --- | --- | --- |
| “把老历史压成一条 summary 就行” | 当压力源在 current turn、middle 为空或 tail 受保护时，老历史摘要往往 no-op | 真正要处理的是 **working current tail**，不是只盯 `ctx_state.messages` 的老前缀 | 问题发生在眼前这轮，就先处理眼前这轮 |
| “单条阈值够用” | 每条 tool result 可能都小于单条阈值，但总和仍然超预算 | 必须引入 **aggregate budget** 而不是只看单条大小 | 单个箱子都合规，不代表一车货不超载 |
| “等空响应/PTL 再补救” | 真正发出过载请求后，已经浪费一轮模型调用，还可能进入空 assistant / 退化终局 | 更合理的是**发模型前**先做 precheck | 快撞墙了就先踩刹车，不是撞上去再说 |

### 1.3 为什么配置清理和阶段二同批

| 配置/实现漂移 | 当前状态 | 阶段二动作 | 说人话 |
| --- | --- | --- | --- |
| `keep_recent_turns` | 配置存在，但 L1 保护区仍用硬编码 `5` | 接线到 [`truncation.rs`](../../src/core/compaction/truncation.rs)，默认值改为 `5` | 配置既然写着“保护最近几轮”，那就该真生效 |
| `compaction_turns` | 有配置、有 allowlist，但无生产引用 | 直接删除字段、默认值、allowlist、catalog 文案 | 没接线的死配置别继续放着误导人 |

说人话：阶段二本来就要动上下文治理和 current-tail 这块，所以顺手把“写在配置里但根本没生效”的两处漂移一起收掉，是最省心的时机。

## 2. 竞品/方案对比

本章只讨论和 aggregate current-tail overflow 真正相关的做法，不再重复阶段一已经落地的单窗护栏。

说人话：第二阶段的关键词不是“大窗”，而是“累计”“预检”“按总量减负”“必要时 split”。

### 2.1 竞品能力对比

| Agent | 对 aggregate overflow 最相关的能力 | 我们借什么 | 不直接照搬什么 | 说人话 |
| --- | --- | --- | --- | --- |
| `openclaw` | `aggregateReducibleChars`、`buildAggregateToolResultReplacements`、每轮工具循环后的 mid-turn precheck | precheck 时机、aggregate budget、批量 replacement、负向/混合尾部测试口径 | 它的完整 runtime 架构与全套 guard 细节 | 它最像“还没发请求就先看这车货会不会超载” |
| `hermes-agent` | `_prune_old_tool_results` + `protect_tail_tokens` + middle 为空则 no-op | “老历史摘要救不了 current tail”这件事、保留 tail 原文的思路 | 它的 transcript scaffold 与恢复脚手架 | 它告诉我们：不要把 current tail 和历史 middle 混成一类 |
| `pi-mono` / `pi_agent_rust` | `isSplitTurn` / turn-prefix summary + suffix 原文 | split current turn 兜底方案 | 其完整 compaction worker 体系 | 真压不下来时，就把当前轮前半段打成 checkpoint，后半段原文保留 |
| `cc-fork-01` | `microCompact` / time-based microcompact，按总量清旧 tool result | “按总量而不是按单条”减负这一点 | 其更粗粒度的老结果清空策略 | 它证明“总量超预算”本来就是一个独立问题 |
| `codex` | mid-turn compaction、注入时截断工具输出 | “发请求前处理 current thread”这件事 | 它更偏 fail-fast 与不同工具契约 | 它说明这类问题不必等 turn 结束后才想起来处理 |

### 2.2 方案层对比

| 方案 | 适用场景 | 优点 | 局限 | 本期结论 | 说人话 |
| --- | --- | --- | --- | --- | --- |
| 只靠单条阈值截断 | 单个大结果 | 简单 | 对很多 medium result 的累计爆窗无效 | **不够** | 单条都不大，照样能一起把你压死 |
| 只压老历史 | middle 很肥、current tail 较轻 | 对历史包袱有效 | current tail 膨胀时常常 no-op | **不够** | 眼前这轮在变胖，压老历史救不了现在 |
| mid-turn aggregate precheck | 每轮工具执行后、下一次发 LLM 前 | 最早发现“这次请求已经要撞线” | 需要 working tail 视角与预算估算 | **采用** | 先称重，再决定要不要继续装车 |
| aggregate current-tail reduction | 很多 replay-safe medium result | 不必等单条超阈值，直接按总量减负 | 需要区分可/不可安全降载 | **采用** | 不看单箱，看这一车里哪些箱子能先改成轻包装 |
| split current turn | reduction 后仍不够，或不可降载项太多 | 给 current turn 一个真正的兜底 | 会触发 summary 与状态保活问题 | **采用** | 真压不下来，就把前半段打包成 checkpoint，后半段原文继续握在手里 |
| 只做重试，不先减负 | 空响应 / PTL 后 | 实现看似简单 | 常常只是白白再打一轮请求 | **不采用** | 不先减负就重试，很多时候只是换个姿势再撞墙 |

## 3. 评审 QA（每题先专业描述，再说人话）

本章收口第二阶段最容易争议的定义和边界。

说人话：下面这些问题，是实现 aggregate guard 时最容易“看起来合理、实际上会做偏”的点。

### Q1. 为什么必须在“每次工具循环结束、发下一次 LLM 前”就 precheck？

**专业描述**

- aggregate overflow 的关键不是“这轮结束后很胖”，而是“**下一次请求发出前**已经超预算”。
- 如果等到空响应 / PTL / turn 结束再补救，已经浪费了一轮模型调用，也更难分辨是单窗还是累计问题。

**结论**

- precheck 的触发时机固定为：**每次工具循环结束、准备发下一次 LLM 请求之前**。

**说人话**

- 车已经快超载了，就该在发车前称重，不是开出去半路爆胎了再说。

### Q2. 为什么单条超过 X 才截断不够？

**专业描述**

- aggregate overflow 的定义就是：每条 tool result 都可能低于单条阈值，但它们的**总和**足以让本轮下一次请求撞线。

**结论**

- 第二阶段必须引入 **aggregate budget**，而不是继续沿用“每条单看”的思路。

**说人话**

- 单个箱子都不超重，不代表一整车不超载。

### Q3. 为什么要区分“可安全降载”和“不可降载”？

**专业描述**

- `read`、`search_files`、大 `bash` 只读输出这类 replay-safe / preview-safe 结果，适合改成 preview / placeholder / 引用。
- 有副作用、不可重放、必须保留原文语义的工具结果，如果硬截断，可能直接破坏当前执行语义。

**结论**

- 第二阶段只对 replay-safe / preview-safe 项做 aggregate reduction；不可安全降载项保留原文。

**说人话**

- 能重新拿回来的，先瘦身；拿不回来的，就别乱砍。

### Q4. 为什么最近几步原文必须保留？

**专业描述**

- 当前执行的局部状态往往集中在 current turn 的最近若干步：刚读到的上下文、刚搜到的结果、刚形成的局部判断。
- 如果把尾部最近几步也一起改成 placeholder，模型下一次推理会丢掉“手边刚看到的东西”。

**结论**

- aggregate reduction 必须**优先处理较老的 current-turn tool result**，最近几步原文继续保留。

**说人话**

- 可以先压老一点的箱子，但手里刚拿着要用的几箱，不能也一起打包走。

### Q5. 为什么 reduction 不够时还要 split current turn？

**专业描述**

- 有两种场景 aggregate reduction 可能不够：
  1. replay-safe 项不够多
  2. 不可安全降载项太多
- 这时若不 split current turn，就没有第二层兜底手段。

**结论**

- 第二阶段把 **prefix checkpoint summary + suffix verbatim** 作为 reduction 失败后的正式兜底。

**说人话**

- 能轻包装的都轻包装了还不够，那就得把前半段先打包寄存，后半段继续拿在手里。

### Q6. 为什么状态保活是硬性要求？

**专业描述**

- 第二阶段直接作用在“正在执行的 current turn”，且目标场景包含“执行一个 plan 很久”。
- 如果 split / summary 之后把 `plan mode`、当前 step、runtime 状态、pending work 一起压没，表面上是“压缩成功”，实质上是“换一种方式卡死”。

**结论**

- `state-keepalive` 不是配套优化，而是 split-turn 路径的**硬性验收项**。

**说人话**

- 不能只把 token 压瘦，却把“做到哪一步、接下来干什么”一起压没。

### Q7. 为什么 `keep_recent_turns` 接线和删 `compaction_turns` 放阶段二？

**专业描述**

- 阶段二本身就会改 current-tail / compaction / context config 这块的行为与文档。
- 与其继续保留“配置存在但没生效”的漂移，不如在同一轮一起收口。

**结论**

- `keep_recent_turns` 接线与 `compaction_turns` 删除，作为阶段二的同批配置清理动作。

**说人话**

- 既然都已经在动这片上下文治理代码了，就顺手把配置漂移一起修掉，别留死配置继续骗人。

## 4. 决策表

本章把第二阶段的取舍直接压成实现决策。

说人话：下面这张表就是阶段二的拍板版，告诉实现时到底怎么做 current-tail guard。

| 议题 | 主要参考 | 决策 | 具体方案 / 落点 | 说人话 |
| --- | --- | --- | --- | --- |
| mid-turn aggregate precheck | `openclaw` | **采用（阶段二）** | [`reasoning_loop.rs`](../../src/core/agent_loop/reasoning_loop.rs) 在每次工具循环结束、发下一次 LLM 前做预检 | 先称重，再决定要不要继续发车 |
| aggregate budget | `openclaw`、`cc-fork-01` | **采用（阶段二）** | 判断 current turn 内**可降载结果总量**是否已超预算，而不是只看单条 | 不再只盯单箱重量，而是看整车 |
| replay-safe / preview-safe 批量降载 | `openclaw`、`hermes-agent` | **采用（阶段二）** | [`current_tail_guard.rs`](../../src/core/agent_loop/current_tail_guard.rs) 对较老的 safe 项做 preview / placeholder / 引用 | 能重新取回来的先变轻装 |
| 最近几步原文保留 | `hermes-agent`、`openclaw` | **采用（阶段二）** | reduction 从旧到新进行，最近几步原文不动 | 手边刚在用的东西不能一起压走 |
| split current turn | `pi-mono`、`pi_agent_rust` | **采用（阶段二）** | reduction 不够时，prefix → checkpoint summary，suffix → verbatim | 真压不下来，就把前半段打包寄存 |
| 状态保活 | `cc-fork-01`、计划运行态约束 | **采用（阶段二）** | summary / split 前后保住 `plan mode`、当前 step、runtime、pending work | 不是只压 token，还得保证还能继续干活 |
| `keep_recent_turns` 接线 | 现有实现漂移 | **采用（阶段二）** | [`truncation.rs`](../../src/core/compaction/truncation.rs) 改读 `config.keep_recent_turns`；默认值 `5` | 配置写了就要真生效 |
| 删除 `compaction_turns` | 现有实现漂移 | **采用（阶段二）** | 从 [`context.rs`](../../src/infra/config/types/context.rs)、allowlist、catalog、单测中移除 | 没接线的死配置别继续留 |
| adaptive read paging | `openclaw` | **后续增强** | 不在阶段二第一版纳入 | 先把按总量减负做对，再谈更聪明的自动切页 |

### 4.1 实施点 + 小结

本小结按 [PLAN_SPEC.md](./PLAN_SPEC.md) 的要求写清楚：

- 涉及的文件与模块
- 实现思路
- 依赖的现有接口或需新建的接口
- 预期的测试要点

阶段二涉及上下文与计划运行态，规格单一来源为：

- [`../../docs/architecture/context-management.md`](../../docs/architecture/context-management.md)
- [`../../docs/architecture/plan-runtime.md`](../../docs/architecture/plan-runtime.md)
- `read` 续读提示相关口径参考 [`../../docs/architecture/tools/read.md`](../../docs/architecture/tools/read.md)

说人话：下面不是泛泛谈“要做个 guard”，而是把 guard 放到具体文件和调用链里，避免实现时又回到口头方案。

#### 4.1.0 One-Glance Map（ASCII）

```text
┌────────────────────────────────────────────────────────────────────┐
│ src/infra/config/types/context.rs                                 │
│ • keep_recent_turns 默认 5                                        │
│ • 删除 compaction_turns                                           │
└──────────────────────────────┬─────────────────────────────────────┘
                               │
                               ▼
┌────────────────────────────────────────────────────────────────────┐
│ src/core/compaction/truncation.rs                                 │
│ • L1 保护区改读 config.keep_recent_turns                          │
└──────────────────────────────┬─────────────────────────────────────┘
                               │
                               ▼
┌────────────────────────────────────────────────────────────────────┐
│ src/core/agent_loop/accessors.rs                                  │
│ • 暴露/明确 working messages tail 视角                            │
└──────────────────────────────┬─────────────────────────────────────┘
                               │
                               ▼
┌────────────────────────────────────────────────────────────────────┐
│ src/core/agent_loop/reasoning_loop.rs                             │
│ • 每次工具循环结束后做 aggregate precheck                         │
│ • 决定 fits / reduce_current_tail / split_current_turn            │
└──────────────────────────────┬─────────────────────────────────────┘
                               │
              ┌────────────────┴────────────────┐
              ▼                                 ▼
┌───────────────────────────────────┐   ┌───────────────────────────┐
│ src/core/agent_loop/current_tail_ │   │ src/core/compaction/      │
│ guard.rs（新）                    │   │ preheat.rs                │
│ • replay-safe 分类               │   │ • 复用/抽 checkpoint      │
│ • aggregate reduction            │   │   summary 模板            │
└────────────────┬──────────────────┘   └──────────────┬────────────┘
                 │                                      │
                 ▼                                      ▼
┌────────────────────────────────────────────────────────────────────┐
│ src/core/session/manager/context.rs / plan_runtime/mod.rs         │
│ • split-turn reload 语义                                           │
│ • plan mode/runtime/current step/pending work 保活                │
└──────────────────────────────┬─────────────────────────────────────┘
                               │
                               ▼
┌────────────────────────────────────────────────────────────────────┐
│ tests / 对应单测模块                                              │
│ • keep_recent_turns 接线                                           │
│ • medium tail precheck / mixed tail / split turn / long plan       │
└────────────────────────────────────────────────────────────────────┘
```

阅读顺序建议：

1. 先看 `reasoning_loop.rs` 的 precheck 入口，明确 guard 的触发时机。
2. 再看 `current_tail_guard.rs` 与 `preheat.rs`，明确“先减负、再 split”的链路。
3. 最后看 `context.rs` / `plan_runtime/mod.rs`，确认 summary/split 后执行态还能继续推进。

说人话：先搞清什么时候踩刹车，再看怎么减负，最后看减负后是不是还能继续开。

#### 4.1.1 实施点 A：mid-turn aggregate precheck

| 项 | 内容 | 说人话 |
| --- | --- | --- |
| 涉及的文件与模块 | [`../../src/core/agent_loop/reasoning_loop.rs`](../../src/core/agent_loop/reasoning_loop.rs)、[`../../src/core/agent_loop/accessors.rs`](../../src/core/agent_loop/accessors.rs) | 这部分定义“什么时候先别发 LLM” |
| 实现思路 | 在每次工具循环结束、准备发下一次 `llm.chat_stream` 前，估算工作集总量、current turn tool result 总量、`promptBudgetBeforeReserve`；输出 `fits` / `reduce_current_tail` / `split_current_turn` | 先称重，再决定正常走还是进减负分支 |
| 跨异步边界约束 | precheck 本身只做估算与决策，不引入新的 `block_on`；仍在现有 async loop 内完成 | 别为了称重再起一套阻塞调用 |
| 依赖的现有接口或需新建的接口 | 可新增 `estimate_current_tail_pressure`、`decide_current_tail_reduction` 内部 helper；复用现有 working messages / start_idx 边界 | 外部协议不动，内部多一个“本轮发车前检查器” |
| 预期的测试要点 | many medium reads 场景触发 precheck；未撞线场景不误触发；仅压老历史、不做 precheck 的负向用例仍会爆窗 | 重点是“该踩刹车时踩，不该踩时别乱刹” |

#### 4.1.2 实施点 B：aggregate reduction

| 项 | 内容 | 说人话 |
| --- | --- | --- |
| 涉及的文件与模块 | [`../../src/core/agent_loop/current_tail_guard.rs`](../../src/core/agent_loop/current_tail_guard.rs)（新） | 这部分负责“当前轮哪些箱子可以先轻装上路” |
| 实现思路 | 识别 replay-safe / preview-safe 结果；按“从旧到新”批量改写为 preview / placeholder / 引用+续读提示；返回释放量与改写清单 | 先从不那么关键、还能重新拿回来的结果开刀 |
| 依赖的现有接口或需新建的接口 | 可新增 safe-classification helper、reduction result struct；`read` 的续读提示口径对齐 `read.md` | 需要一个清楚的“哪些能砍、砍成什么样”的内部协议 |
| 预期的测试要点 | mixed tail 中只降 replay-safe；最近几步原文保留；不可降载结果不被误改写 | 重点是“砍得对”，不是“砍得猛” |

#### 4.1.3 实施点 C：split current turn + 状态保活

| 项 | 内容 | 说人话 |
| --- | --- | --- |
| 涉及的文件与模块 | [`../../src/core/compaction/preheat.rs`](../../src/core/compaction/preheat.rs)、[`../../src/core/session/manager/context.rs`](../../src/core/session/manager/context.rs)、[`../../src/core/plan_runtime/mod.rs`](../../src/core/plan_runtime/mod.rs) | 这部分负责“真压不下来时怎么打 checkpoint，且别把执行态丢了” |
| 实现思路 | reduction 不够时，对 current turn prefix 生成 checkpoint summary，suffix 保留 verbatim；summary 前后显式校验 `plan mode` / 当前 step / runtime / pending work | 前半段寄存，后半段继续拿着，而且别忘了自己做到哪一步 |
| 跨异步边界约束 | 若 split-turn summary 需要 LLM 参与，必须在现有 async 链路中 `await` 或用明确的 `spawn` 边界；禁止在 async 上下文里嵌 `block_on` | 别为了做 summary 再把 Tokio 边界搞乱 |
| 依赖的现有接口或需新建的接口 | 复用/抽出 `preheat` 的 structured summary 模板；可能新增 split-turn checkpoint 语义与 keepalive helper | 不要重复造 prompt，但要把 current-turn 专属语义补齐 |
| 预期的测试要点 | reduction 不够自动 split；reload 后语义稳定；long plan 执行中 split 后仍能继续推进 | 重点是“压完还能干活”，不是“压完看起来很短” |

#### 4.1.4 实施点 D：配置清理

| 项 | 内容 | 说人话 |
| --- | --- | --- |
| 涉及的文件与模块 | [`../../src/infra/config/types/context.rs`](../../src/infra/config/types/context.rs)、[`../../src/core/compaction/truncation.rs`](../../src/core/compaction/truncation.rs)、[`../../src/core/tools/config_tool/allowlist.rs`](../../src/core/tools/config_tool/allowlist.rs)、[`../../src/core/tools/contract/catalog.rs`](../../src/core/tools/contract/catalog.rs) | 这部分负责把“配置写了但没生效”的坑一起填掉 |
| 实现思路 | `keep_recent_turns` 默认值改为 `5` 并接线到 L1 保护区；删除 `compaction_turns` 及其相关文案/测试 | 该生效的配置真生效，死配置就删掉 |
| 依赖的现有接口或需新建的接口 | 复用现有 `ContextConfig`；无需新 public API | 多数是清理，不是新开大口子 |
| 预期的测试要点 | `keep_recent_turns=2` 时保护区缩小；`compaction_turns` 从类型、allowlist、文案中移除 | 重点是“配置别再骗人” |

### 4.2 小结

- 第二阶段的核心不是“重试”，而是**先在发请求前发现 current tail 已经超载，然后按总量减负**。
- 真正的执行顺序是：**precheck → aggregate reduction → split turn → 保活执行态 → 再发 LLM**。

说人话：阶段二不是“再试一次”的计划，而是“在再试之前，先把当前轮这一车货整理到能安全上路”的计划。

## 5. 测试方案与验收

本章定义阶段二如何证明 aggregate guard 真正工作、且不会把长执行 plan 压坏。

说人话：第二阶段不是看“压没压短”，而是看“有没有在发请求前及时减负、减负后还能继续执行”。

### 5.1 测试方案

| 层级 | 目标 | 主要用例 | 说人话 |
| --- | --- | --- | --- |
| 配置测试 | 校验配置清理 | `keep_recent_turns` 接线；删除 `compaction_turns` | 先把配置漂移收干净 |
| 预检测试 | 校验撞线判断 | many medium reads 触发 precheck；未撞线不误触发 | 该踩刹车时踩，不该踩时别乱踩 |
| reduction 测试 | 校验 replay-safe 批量减负 | mixed tail 只改 safe 项，最近几步保留 | 能砍的砍，不能砍的别乱动 |
| split-turn 测试 | 校验 reduction 不够时的兜底 | prefix summary + suffix verbatim | 真压不下来时，要有第二套方案 |
| 状态测试 | 校验 plan 连续性 | long-running plan 中 split/reduction 后仍可继续 | 压完之后还能继续干活 |
| 负向测试 | 证明旧路径不够 | 仅压老历史、不做 precheck 仍会爆窗 | 证明我们不是多写了一套没用的 guard |

### 5.2 详细测试矩阵

| 场景 | 断言 | 主要文件 | 说人话 |
| --- | --- | --- | --- |
| `keep_recent_turns=2` | L1 只保护最近 2 turns | `truncation` / config 相关测试 | 配置写 2 就真只保护 2 |
| `compaction_turns` 清理 | 类型、allowlist、catalog、单测均不再出现 | config / allowlist / catalog 测试 | 死配置别继续在仓库里装活着 |
| many medium reads | 单条都不过阈值，但总量撞线 → precheck 触发 reduction | `reasoning_loop` / current tail 测试 | 重点是“累计超载”被提前识别 |
| mixed tail | `read` / `search_files` 被减负；不可降载项原文保留 | `current_tail_guard` 测试 | 能砍的砍，不能砍的别误伤 |
| reduction 不够 | 自动 split current turn；suffix 原文保留 | split-turn 相关测试 | 压不下来就切 current turn，不要硬撑 |
| long-running plan | split/reduction 后仍保持 `Executing`，当前 step / runtime / pending work 可继续推进 | `plan_runtime` / context 测试 | 证明压缩不是“把计划压没了” |
| 负向基线 | 仅调 L0/L1 压老历史，不做 precheck → 应仍能复现 current-tail 爆窗 | 应用层或回放测试 | 证明新 guard 不是可有可无 |

### 5.3 验收标准

| 验收项 | 通过标准 | 说人话 |
| --- | --- | --- |
| precheck 有效 | 在发请求前识别 current-tail 累计超载，并进入减负分支 | 别等撞墙后才说“好像太重了” |
| reduction 正确 | 只改 replay-safe 项，最近几步原文保留 | 该瘦身的瘦身，不该动的别乱动 |
| split 兜底有效 | reduction 不够时能切 current turn，而不是继续硬发超载请求 | 真压不下来时，要有第二层兜底 |
| 执行态不丢 | split / summary 后 plan mode / runtime / 当前 step / pending work 仍能继续 | 压完还能继续干活，才算真成功 |
| 配置漂移清理完成 | `keep_recent_turns` 生效；`compaction_turns` 清理干净 | 配置别再骗人 |

### 5.4 暂不纳入本期验收

| 范围 | 原因 | 去向 | 说人话 |
| --- | --- | --- | --- |
| adaptive read paging | 属于更激进的工具层自适应分页 | 更后续增强 | 先把 aggregate guard 做对，再谈更聪明的自动切页 |
| post-compaction loop guard | 依赖更完整的重试/压缩循环观测 | 更后续增强 | 先把主链路跑通，再补“防打转”保险丝 |
| 非 `read` / `search_files` / 只读 `bash` 的广义通用裁剪器 | 会扩展太多工具语义 | 后续评估 | 先做最常见、最安全的 replay-safe 集合 |
