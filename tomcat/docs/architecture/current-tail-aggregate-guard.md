# Current-Tail Aggregate Guard：阶段二预防型上下文减负方案

本文档是阶段二 `current-tail aggregate guard` 的开发前定稿方案，补足 [`context-management.md`](./context-management.md)、[`agent-loop.md`](./agent-loop.md)、[`plan-runtime.md`](./plan-runtime.md) 之间尚未成文的“mid-turn current-tail guard”主链。当前仓库**尚未**落地 `current_tail_guard.rs`、mid-turn history apply/recompact orchestration、single-branch-summary collapse + keepalive 等目标态；本文中以 `【目标态】` 标注新增类型、字段、测试与文件。计划文档保留调研与排期，本文只保留**已拍板的行为边界、协议与交付**。

本文按 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 主路径编排。上位与相邻方案：[`context-management.md`](./context-management.md)、[`agent-loop.md`](./agent-loop.md)、[`plan-runtime.md`](./plan-runtime.md)、[`tools/read.md`](./tools/read.md)。

**时间点钉死**：

- **「每次工具循环结束、发下一次 LLM 前」**：专指 `reasoning_loop` 的一次 tool round 完成后，`tool_dispatcher` 已把本轮 assistant/tool 消息 push 到局部 `messages`，并已调用 `ctx_state.on_message_appended(...)` 记账，但下一次 `llm.chat_stream(...)` 还**没有**开始构造 `ChatRequest` 的那个时刻。
- **不是** 下一个 user turn 进入时的外层 `check_before_request`。
- **也不是** reasoning loop 最终 assistant 回复后、user turn 全部结束的时刻。

**说人话**：这篇文档讲的是“车已经在当前轮里装了很多货，下一脚油门准备再发 LLM 之前，先称重、减负、必要时中途打 checkpoint”，不是传统的“下一轮用户再来时压老历史”。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. One-Glance Map（文件职责总览）](#6-one-glance-map文件职责总览)
- [7. 调度时序（运行时图）](#7-调度时序运行时图)
- [8. 状态机](#8-状态机)
- [9. 配置与环境变量](#9-配置与环境变量)
- [10. 错误模型 / 截断 / 警告](#10-错误模型--截断--警告)
- [11. 测试矩阵（验收）](#11-测试矩阵验收)
- [12. 风险与应对](#12-风险与应对)
- [13. 历史决策 / 跨文档修订](#13-历史决策--跨文档修订)

---

## 1. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
| --- | --- | --- | --- | --- |
| **current tail** | 当前 reasoning 回合里新长出来的尾巴 | `reasoning_loop` 局部 `messages[agent.start_idx..]`；`Agent::start_idx` / `context_tail_start` | A/B/C 只允许扫描和改写这段；**不能**只盯 `ContextState.messages` 老前缀；split 成功后必须同步挪 `start_idx` 与 `context_tail_start` | 这篇方案处理的是“眼前这轮新堆起来的货”，不是老仓库。 |
| **mid-turn aggregate precheck** | 当前轮里每次 tool round 结束后、下次 LLM 请求发出前的称重与路由 | `reasoning_loop.rs` 中 `tool_dispatcher` 返回与下一次 `llm.chat_stream(...)` 之间；【目标态】`AggregatePrecheckDecision` | 每个 tool round 都要跑；只负责测量和决策，**不直接改写消息**；`working_set_tokens >= 0.90 * budget` 只打黄灯日志，不直接分流 | 每跑完一拨工具都要过磅一次。 |
| **working_set_tokens** | 下一次 LLM 请求整份 prompt 的估算总重 | `ContextState::estimated_token_count()`；`last_api_usage` + `post_usage_appended_chars` | 是 A 的主判据；B/C 改写消息后必须同步修正相关计数，否则下一次称重会失真 | 看的是整车多重，不是某一个箱子多重。 |
| **context_budget_tokens** | Prompt 可用上限 | `ContextState.context_budget_tokens`，来自 `context_window - max_output_tokens` | 本文统一使用这个名字；**不再**使用旧草案名 `promptBudgetBeforeReserve`；A 不再额外扣一次 output reserve | 红线已经扣过输出位了，不要再扣第二遍。 |
| **route：`fits` / `reduce_current_tail` / `collapse_to_branch_summary`** | A 的三路裁决结果 | 【目标态】`CurrentTailRoute` enum | `fits` = 直接继续；`reduce_current_tail` = 进 B；`collapse_to_branch_summary` = 进 C；不允许“明知超重还硬发一次试试” | 先决定放行、先减负，还是直接把整份工作集折成一条摘要。 |
| **COMPACTABLE_TOOLS** | 当前 tail 里允许进入 reduction 的工具白名单 | 【目标态】`{"read", "search_files", "bash", "task_output"}` | 首版只对白名单工具的 `tool_result` 做 placeholder；白名单外永不改写；后续扩集单独评估 | 先只动最常把 current tail 堆胖的几类读取/日志结果。 |
| **transcript message rewrite helper** | reduction 后，经 `session/transcript` 模块提供的 helper 尝试回写对应 `tool_result` message | `transcript.rs` / `session_impl.rs` 暴露按 `msg_id` 重写 message 的 helper；helper 内部自行负责安全写盘策略 | reduction 不再引入 ledger；**当前轮内存减负不依赖 helper 成功**；helper 成功时 reload 更接近运行时真相 | 减负先在眼前这轮生效；能顺手把 transcript 也补齐最好，但别反过来卡住当前轮。 |
| **single branch_summary collapse** | 当 B 仍压不下来时，把 apply 后的整份 working messages 折成一条 `branch_summary` | 复用既有 `BranchSummaryEntry` + `apply_boundary` 语义；`covered_start_id` 到 `covered_end_id` 覆盖整个工作集 | 不新增 `current_tail.split_turn` 事件；不保留 suffix verbatim；成功后 `messages` 中只剩一条摘要消息，并同步更新边界与 token 基线 | 真压不下来，就别再切前半/后半了，直接整份工作集折成一条摘要。 |
| **keepalive snapshot** | 由本地代码生成并拼进单条 `branch_summary` 的执行态保活块 | 【目标态】`KeepaliveSnapshot`，来源于 `PlanRuntime` + plan file + `ContextState.latest_plan_event` | 不是 LLM 自由概括；至少要带 `mode`、`active_plan_path`、当前 step / pending work；reload 与下一轮 prompt 都能看到同一份保活真相 | 不能只让模型“记住现在在干啥”，得把执行态写死。 |
| **eager append** | tool result 执行完就立即落 transcript，而不是等整轮结束 | `tool_dispatcher.rs` + `SessionManager::append_message*` 路径 | B/C 不能只改局部 `messages`；凡是要跨 reload 维持语义，reduction 应调用 transcript helper best-effort 回写，C 应复用既有 `branch_summary + apply_boundary` | Tomcat 不是纯内存试玩，货一到手就先入账。 |

本文统一用 `context_budget_tokens` 指代 prompt 预算上限；旧草案里的 `promptBudgetBeforeReserve` 不再出现在正式方案中。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 这类问题真正是什么

```text
单条超大结果（阶段一已挡）
    │
    ├─ 一条 read 直接把窗口顶爆
    └─ 解法：单窗护栏 / 裸读门 / L0 落盘 preview

累计 current-tail 超重（本文主战场）
    │
    ├─ 每条都不算大，但同一轮 read/search/bash/task_output 越堆越重
    ├─ 老历史 compaction 往往 no-op，因为 middle 空或 tail 受保护
    └─ 解法：mid-turn precheck -> aggregate reduction -> split current turn
```

**说人话**：阶段一拦“大石头”，阶段二处理“很多中等箱子慢慢把楼板压塌”。后者不在历史区，而是在**正在执行的当前轮**。

### 2.2 常见实现横向对比

> 说明：外部 agent 的代码路径以阶段二计划调研中已核实的仓库/符号为准；本机部分参考 worktree 当前未完整展开，无法逐个本地打开的行会显式写“计划调研”。

| 竞品 | 形态 | 关键设计 | 我们借鉴的点 | 说人话 |
| --- | --- | --- | --- | --- |
| **openclaw** | mid-turn preemptive compaction | `src/agents/embedded-agent-runner/run/preemptive-compaction.ts` + `tool-result-truncation.ts`：发 prompt 前先算 overflow，再看 tool result 总减重空间 | precheck 时机、aggregate budget、批量 replacement 骨架、负向/混合 tail 测试口径 | 最像“发车前先称整车，再决定卸哪几箱”。 |
| **hermes-agent** | tail-preserving compression | `agent/context_compressor.py`：估算发送前预算、保留 tail 原文、middle 为空时压老历史会 no-op | 不把 current tail 与历史 middle 混为一谈；最近几步尽量保真 | 它提醒我们：眼前这轮变胖时，压老历史常常救不了火。 |
| **pi-mono / pi_agent_rust** | split-turn compaction | 计划调研中的 `find_cut_point` / `compact` / `isSplitTurn` 路线：turn-prefix summary + suffix verbatim | split current turn 的兜底形态；切点必须尊重消息边界 | 真压不下来时，别瞎删，得把前半段打成 checkpoint。 |
| **cc-fork-01** | micro-compact / safe 白名单 | 计划调研中的 `microCompact` / `COMPACTABLE_TOOLS`：按总量清旧 tool result，只对白名单动手 | “按总量而不是按单条”减负；safe-class 白名单化 | 先挑能动的箱子动，别一刀切。 |
| **codex** | pre-request context handling | 计划调研中的 mid-turn compaction / 注入时截断工具输出 | “这类问题不必等 turn 结束后再想起来处理” | 当前线程已经变胖，就在当前线程处理。 |
| **Tomcat 当前基线** | L0-L3 + preheat | [`context-management.md`](./context-management.md)：L0/L1/L2/L3 主要面向超大结果、老历史、下一 user turn 前的预热与边界切换 | 保留既有 L0-L3，不重做；阶段二补一条**mid-turn current tail** 新支线 | 旧系统会压历史，但还没有一条专门对付“当前轮越跑越胖”的链路。 |

### 2.3 为什么选这条路线，不选其他路线

1. **压力点在 mid-turn，不在 turn 末尾。** 如果等最终 assistant 回复后再处理，已经白白发出过载请求了。
2. **历史 compaction 不是 current-tail guard。** `ContextState.messages` 与局部 `messages[start_idx..]` 在 Tomcat 里不是同一视角，不能混看。
3. **减负必须可恢复。** `read`、`search_files`、只读 `bash` 都有不同 replay 路径，不能把所有 tool result 当普通长字符串切一刀。
4. **长 plan 场景需要 split + keepalive。** 只做 reduction 不足以保证执行态连续性；只做 summary 又会把当前 step / pending work 压没。
5. **阶段二与阶段三必须分开。** 本期先把“发请求前避免超载”做对；`finish_reason` 驱动的 same-turn recovery 留给后续反应型阶段。

---

## 3. 目标与设计原则

### 3.1 观察指标表（与 §11 验收一一对应）

| 目标 | 观察指标（落地后可核对） | 说人话 |
| --- | --- | --- |
| **G1 提前发现 current-tail 超重** | 每次 tool round 结束后，在下一次 `llm.chat_stream(...)` 之前都会运行 precheck；many-medium-reads 场景在发请求前进入分流，而不是先撞 API | 车还没开出去，就先知道超没超载。 |
| **G2 总量优先，不看单条阈值** | A 的主判据是 `overflow_tokens = max(0, working_set_tokens - context_budget_tokens)`；单条 10K/128KB 不再决定是否触发阶段二 | 单箱合规不代表整车合规。 |
| **G3 reduction 只改 `COMPACTABLE_TOOLS`** | 首版仅 `read`、`search_files`、`bash`、`task_output` 进入 reduction；按“旧的一半 -> 再旧的一半”波次直接置 placeholder | 能砍的砍，先砍旧半区。 |
| **G4 reduction 与 C 路径都有明确持久化语义** | reduction 通过 transcript helper best-effort 回写；C 路径复用既有 `branch_summary` + `apply_boundary`；写盘成功时 reload 仍能得到同样的 reduced tail / 单条摘要 | 不要求为了 reload 一致性牺牲当前轮减负，但也不放弃持久化路径。 |
| **G5 split 不丢执行态** | split 后 `PlanRuntime` 仍能恢复到正确 `mode`，且下一步 `current_step` / `pending_work` 可继续推进 | 压完 token 以后还能接着干活。 |
| **G6 配置漂移收口** | `keep_recent_turns` 真正驱动 `truncation.rs` 保护区；`compaction_turns` 不再出现在 config type / allowlist / catalog / 测试里 | 配置写了就真生效，死配置直接删掉。 |
| **G7 阶段边界不混淆** | 阶段二不消费 `finish_reason`，也不做 same-turn recovery；实现链路固定为 `precheck -> reduction -> split -> keepalive -> next prompt` | 先把发车前整理货物做满，再谈车已经抛锚后的补救。 |

### 3.2 非目标

| 非目标 | 推给 / 理由 | 说人话 |
| --- | --- | --- |
| **`finish_reason` 消费与 same-turn 空响应 / PTL 续轮** | 推给阶段三独立方案；依赖阶段二减负能力先落地 | 本期不做“抛锚后补救”，只做“别开着超载车上路”。 |
| **广义通用 tool-result 裁剪器** | 首版只做 `read` / `search_files` / 只读 `bash`；其余工具语义差异太大 | 先做最常见、最安全的集合。 |
| **adaptive read paging** | 属于更激进的工具层自适应分页；等 aggregate guard 稳定后再考虑 | 先把按总量减负做对，再谈更聪明的自动切页。 |
| **重做 L0-L3 / preheat 既有语义** | 本文只补 stage-2 current-tail 支线，保留 `context-management.md` 主线 | 不是重写上下文系统，而是在中间补一条新链路。 |
| **用户可见的 raw finish_reason / raw guard 诊断展示** | 阶段二只需日志与内部事件；CLI/transcript 展示规范留给阶段三 | 用户只看结果，不看 wire 字段。 |

**说人话**：这篇文档不是要把 Tomcat 全部 context-management 推倒重来，而是给“当前轮中途越跑越胖”的场景补一条专用防线。

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表（维度取舍）

> 说明：第三列 **`决策`** 只写最终裁决；落地点、交付物与阶段边界全部放在 **[§4.2](#42-实施点按阶段拆分)**。`取自` 优先写“本仓代码 + 外部 agent 证据”；对纯本仓漂移修正行，会显式标注“无并列外部正例”。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **R1 触发时机** | guard 应该在什么时候称重 | **采用** `reasoning_loop` 内的 mid-turn precheck：每次 tool round 结束、下一次 `llm.chat_stream(...)` 前执行。 | 本仓：`../../src/core/agent_loop/reasoning_loop.rs`、`../../src/core/agent_loop/tool_dispatcher.rs`；外部：`openclaw + src/agents/embedded-agent-runner/run/preemptive-compaction.ts` | 设计：把 precheck 钉在 tool round 与下一次 LLM 之间；理由：只有这个点同时看得到本轮新增消息、又能拦住过载请求。 | 仅复用外层 `check_before_request` / 仅在最终 assistant 回复后再压缩；拒因：它们看不到当前轮 tool 堆积，发现问题太晚。 | 真正该过磅的点，是这拨工具跑完、下一脚油门还没踩下去的时候。 |
| **R2 预算口径** | A 的红线怎么定义 | **采用** `context_budget_tokens = context_window - max_output_tokens`；**拒绝**再扣一次 output reserve。 | 本仓：`../../src/core/session/manager/types.rs`、[`context-management.md`](./context-management.md)；外部：`openclaw + preemptive-compaction.ts`（预算/overflow 思路） | 设计：直接使用 `ContextState.context_budget_tokens`；理由：Tomcat 已在配置层扣过输出位，再扣一次会双重保守。 | 沿用旧草案名 `promptBudgetBeforeReserve` 或额外手动 reserve；拒因：概念不清且会把 guard 做得过早、过紧。 | 红线已经画好了，别自己又往里缩一圈。 |
| **R3 reduction 范围与策略** | B 应该动哪些消息、按什么顺序动 | **采用** 先复用现有 `apply_boundary` 吃掉已完成的历史摘要，再对 apply 后历史消息整体再压一遍，最后才对 `current tail` 的 `COMPACTABLE_TOOLS` 做“旧的一半 -> 再旧的一半”波次 placeholder。 | 本仓：`../../src/core/compaction/apply.rs`、`../../src/core/compaction/truncation.rs`、`../../src/core/agent_loop/tool_dispatcher.rs`、`../../src/core/session/manager/types.rs`；外部：`openclaw + tool-result-truncation.ts`、`cc-fork-01 + COMPACTABLE_TOOLS`（计划调研） | 设计：先把仓里已经准备好的历史减负吃干净，再重新压一轮历史，最后才动当前轮；理由：这样更贴合“先榨干历史空间，再对现场动刀”的收益顺序。 | 一上来只扫 current tail / 只压老历史 / preview + placeholder 两段策略；拒因：要么白白放过现成历史收益，要么会 no-op，或引入没必要的状态机复杂度。 | 先把已经备好的老货和历史整一遍，再去动眼前这轮的箱子。 |
| **R4 reduction 持久化语义** | B 改完消息后如何跨 reload 维持 | **采用** `session/transcript` 模块提供的 message rewrite helper：按 `msg_id` 尝试回写对应 `tool_result` message；**不以写盘成功作为内存减负前提**。 | 本仓：`../../src/core/session/transcript.rs`、`../../src/core/session/manager/context.rs`、`../../src/api/chat/commands/cmd_restore.rs` | 设计：guard 只调用 helper，不直接操心 JSONL 改写细节；helper 成功时 reload 更接近运行时真相，失败时只降级为“当前轮减负有效、reload 可能回退”。 | 只改局部 `messages` / append-only reduction ledger / 以写盘成功 gate 当前轮减负；拒因：前者 reload 会复活原文，后两者都把阶段二主目标让位给持久化复杂度。 | 先把眼前这轮车减下来；能顺手把 transcript 同步掉最好，但别倒过来拿写盘卡主流程。 |
| **R5 C 路径形态** | B 不够时，如何兜底 | **采用** 单条 `branch_summary + keepalive snapshot`：复用既有 `BranchSummaryEntry` + `apply_boundary`，把 apply 后的整份 working messages 全量折成一条摘要。 | 本仓：`../../src/core/compaction/preheat.rs`、`../../src/core/compaction/apply.rs`、`../../src/core/session/manager/context.rs`、`../../src/core/plan_runtime/mod.rs` | 设计：不再发明 `current_tail.split_turn` 事件，不再保留 suffix verbatim；理由：既然 B 已经把历史和 current tail 都尽力压过，剩下唯一稳定路径就是整份工作集一把折成一条 branch_summary。 | 自定义 `current_tail.split_turn` 事件 / 落普通 user message / 再引入 cut-point 选择；拒因：实现复杂度高，且与现有 `branch_summary + apply_boundary` 成熟路径重复造轮子。 | 真压不下来，就别切前缀后缀了，直接复用现有 branch_summary 语义整份折起来。 |
| **R6 执行态保活** | C 后 plan/runtime 状态靠谁保 | **采用** 本地 deterministic keepalive footer，数据来自 `PlanRuntime + plan file + latest_plan_event`。 | 本仓：`../../src/core/plan_runtime/mod.rs`、`../../src/core/session/manager/context.rs`；外部：`hermes-agent` resume continuity（计划调研） | 设计：LLM 只总结语义进展，本地代码单独拼出 keepalive；理由：`mode`、`active_plan_path`、当前 step / pending work 属于运行态真相，不能让模型自由发挥。 | 只依赖 LLM summary 自己“记住现在做到哪”；拒因：长 plan 下最容易把真正的执行态压没。 | 不能只让模型写摘要，得把“现在到底在执行什么”明确写死。 |
| **R7 配置漂移修正** | 阶段二要不要顺手清理 `keep_recent_turns` / `compaction_turns` | **采用** 同批收口：`keep_recent_turns` 接线到 `truncation.rs`，默认改为 `5`；删除 `compaction_turns`。 | 本仓：`../../src/infra/config/types/context.rs`、`../../src/core/compaction/truncation.rs`、`../../src/core/tools/config_tool/allowlist.rs`、`../../src/core/tools/contract/catalog.rs`；外部：无并列外部正例（纯本仓漂移修正） | 设计：在同一批 current-tail/context 修改里修正配置真相；理由：避免继续保留“配置存在但 runtime 不读”的双份事实。 | 维持现状、等以后再修；拒因：阶段二正好要改这片代码，再拖只会继续误导配置读者与工具目录。 | 既然都在动这片上下文治理代码，就把没生效的旋钮一起修掉。 |
| **R8 阶段边界** | 阶段二要不要顺带做 `finish_reason` recovery | **采用** 阶段二只做预防型 `precheck -> reduction -> collapse`；`finish_reason` 消费与 same-turn recovery 留到阶段三。 | 本仓：`../../src/core/agent_loop/reasoning_loop.rs`、`../../src/core/agent_loop/types.rs`；外部：`codex` / 计划调研中的 pre-request compaction 对比 | 设计：先把“别发出过载请求”做满；理由：把预防与反应混做，会让阶段二失焦，还会诱导“先裸重试再说”。 | 同期把 same-turn 续轮一起做；拒因：scope 混淆，且没有减负能力的重试价值很低。 | 阶段二先别让车超载上路，阶段三再管车已经抛锚怎么办。 |

### 4.2 实施点按阶段拆分

> 下列阶段是交付边界，不强制要求分成 4 个 PR；若单次合入，也必须按这 4 个边界验收。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-CTA：mid-turn precheck** | 在 `reasoning_loop` 内新增 current-tail precheck 与三路路由；交付物：`AggregatePrecheckDecision` / `CurrentTailRoute` / 黄灯日志 | `../../src/core/agent_loop/reasoning_loop.rs`、`../../src/core/agent_loop/accessors.rs`、【目标态】`../../src/core/agent_loop/current_tail_guard.rs` | `agent_loop_tests.rs::current_tail_precheck_routes_before_next_chat_stream`、`current_tail_guard_test.rs::precheck_routes_to_fits_reduce_and_collapse` | 先把“什么时候称重、怎么分流”钉死。 |
| **PR-CTB：apply + history recompact + tail reduction** | 先 apply 已完成历史摘要，再对 apply 后历史整体再压一遍，最后做 `COMPACTABLE_TOOLS` 旧半区波次 placeholder；交付物：通过 transcript helper best-effort 回写对应 `tool_result` message | 【目标态】`../../src/core/agent_loop/current_tail_guard.rs`、`../../src/core/compaction/apply.rs`、`../../src/core/session/manager/context.rs`、`../../src/core/session/transcript.rs`、`../../src/core/compaction/truncation.rs` | `current_tail_guard_test.rs::reduction_applies_ready_history_before_tail_rewrite`、`current_tail_guard_test.rs::history_recompact_runs_before_tail_reduction`、`hydrate_test.rs::tail_reduction_helper_sync_survives_reload_when_write_succeeds` | 先把已有历史收益吃干净，再去砍当前轮。 |
| **PR-CTC：single branch_summary collapse + keepalive** | 生成单条 `branch_summary`、拼接 keepalive snapshot、复用现有 `apply_boundary` 全量折叠 working messages；交付物：reload 后等价恢复 | `../../src/core/compaction/preheat.rs`、`../../src/core/compaction/apply.rs`、`../../src/core/plan_runtime/mod.rs`、`../../src/core/session/manager/context.rs`、【目标态】`../../src/core/agent_loop/current_tail_guard.rs` | `current_tail_guard_test.rs::collapse_to_single_branch_summary_when_reduction_still_overflows`、`hydrate_test.rs::single_branch_summary_collapse_rehydrates_cleanly`、`plan_runtime_test.rs::execution_keepalive_snapshot_survives_collapse` | 真压不下来，就整份折成一条摘要，但不能把执行态丢了。 |
| **PR-CTD：配置清理 + 文档/测试** | `keep_recent_turns` 默认值改 `5` 并接线；删除 `compaction_turns`；同步 allowlist、catalog、README、架构文档与测试矩阵 | `../../src/infra/config/types/context.rs`、`../../src/core/compaction/truncation.rs`、`../../src/core/tools/config_tool/allowlist.rs`、`../../src/core/tools/contract/catalog.rs`、`../../src/core/README.md`、本文 | `context_cfg_test.rs::default_keep_recent_turns_is_5`、`truncation_test.rs::compact_tool_results_respects_keep_recent_turns_config`、文档同步 | 配置别再骗人，字和代码一起收口。 |

#### 4.2.1 PR-CTA：mid-turn precheck

- **入口位置**：`tool_dispatcher::run_tool_calls(...)` 返回之后、下一次 `llm.chat_stream(...)` 之前。
- **主流程**：
  1. 取 `working_set_tokens = ctx_state.estimated_token_count()`；
  2. 取 `current_tail = messages[agent.start_idx..]`；
  3. 计算 `overflow_tokens`；
  4. 调用 `current_tail_guard::estimate_reducible_tokens(ctx_state, messages, agent.start_idx)`；
  5. 按 `fits / reduce_current_tail / collapse_to_branch_summary` 路由；
  6. B/C 改写完成后，**重新** precheck，一次确认已回到预算内。
- **实现约束**：
  - A 只做测量和路由，不直接改写消息；
  - 不引入新的 `block_on`；
  - `working_set_tokens >= 0.90 * context_budget_tokens` 只打诊断日志，不改变控制流。

```text
tool_dispatcher done
      |
      v
working_set_tokens = ctx_state.estimated_token_count()
      |
      v
overflow_tokens = max(0, working - budget)
      |
      +--> overflow == 0 --------------------> fits -> next llm.chat_stream
      |
      +--> overflow > 0 -> ask current_tail_guard "先 apply 历史、再压历史、再压 tail，最多能减多少？"
                                  |
                  +---------------+---------------+
                  |                               |
            enough reducible                 not enough
                  |                               |
                  v                               v
       reduce_current_tail              collapse_to_branch_summary
```

**说人话**：A 是路由器，不是刀子。它先看整车超了多少，再问“先把现成历史收益吃掉、再压一轮历史、最后再动这轮尾巴，能不能救回来”，能救就进 B，救不回来才进 C。

#### 4.2.2 PR-CTB：apply + history recompact + tail reduction

- **阶段顺序**：
  - **Step 0：先 apply**。若仓里已有已完成未消费的历史 `branch_summary`，优先复用现有 `apply_boundary` 路径把它吃进当前工作集；
  - **Step 1：再压一轮 apply 后历史**。对 apply 后的历史 `messages[..agent.start_idx]` 再跑一轮既有历史压缩；目标是先榨干老消息，不急着先动 current tail；
  - **Step 2：最后才压 current tail**。仍超重时，收集当前仍未 placeholder 的 compactable `tool_result`，按消息顺序 old-first 排列；
  - **Step 3**：若当前还有 `n` 个候选，则选最老的 `max(1, floor(n / 2))` 个，直接改成现有 `TOOL_RESULT_PLACEHOLDER`；
  - **Step 4**：每完成一个阶段都立即重称；历史 apply / 历史再压 / tail reduction 任一阶段已回到预算线内就停止；
  - **Step 5**：若历史已尽力、tail 也已减到无更多收益仍超预算，则升级到 C。
- **current tail 候选识别**：
  - 只在 tail 阶段扫描 `current tail`；
  - 以“assistant 的 tool_call + 对应 tool message”为最小单位；
  - 首版 `COMPACTABLE_TOOLS` 固定为 `read`、`search_files`、`bash`、`task_output`；
  - `task_output` 纳入原因：它本质上是后台 bash 日志 tail 的只读结果面，最容易在长编译/长测试场景里把 current tail 堆胖，且重放线索已经在上一条 tool_call 的 `task_id` / `since` 里；
  - 未来候选里，`list_dir` 可以再讨论；`config_get` / 写工具 / plan 工具本期不纳入。
- **placeholder 口径**：
  - 直接复用现有 `TOOL_RESULT_PLACEHOLDER`；
  - 不在 placeholder 文本里重复抄 `path` / `query` / `offset` / `limit` / `task_id` / `since`；
  - 上一条 assistant tool_call 已保留这些 args，模型若真要重读/重新拉日志，可顺着 tool_call 自己再调工具。
- **持久化规则**：
  - 每条被改写的 tool message，先更新局部 `messages` 与计数；
  - 随后调用 `session/transcript` 模块提供的 helper，按 `msg_id` best-effort 回写 transcript 对应 `MessageEntry.message.content`；
  - helper 失败只记 `warn` / metrics，不回滚本轮减负；是否进入 C 只看**当前重称结果**。
- **计数回写**：
  - 若 B 只动了 tail，占位后更新 `messages` / `estimate_context_chars` / `post_usage_appended_chars`；
  - 若 B 动到了 apply 后历史消息，则直接 `ctx_state.invalidate_api_usage()`，后续称重暂时退回 chars fallback；
  - B 完成后统一 recheck。

```text
apply ready branch_summary
   |
   v
still overweight?
   |
   +--> no  -> recheck -> fits
   |
   +--> yes -> recompact applied history
   |
   v
still overweight?
   |
   +--> no  -> recheck -> fits
   |
   +--> yes -> scan current tail + filter COMPACTABLE_TOOLS
   |
   +--> wave 1 / wave 2 ... oldest unreduced half -> placeholder
   |
   +--> still overweight after all stages -> collapse_to_branch_summary
```

**说人话**：B 不是上来就砍 current tail，而是**先把仓里已经准备好的历史收益吃掉，再把 apply 后历史整体压一轮，最后才对 current tail 下刀**。这样做完如果还超，就别再搞更细的 cut-point 了，直接进 C。

#### 4.2.3 PR-CTC：single branch_summary collapse + keepalive

- **触发条件**：
  - B 已经依次做完 apply、apply 后历史再压、current tail reduction；
  - 重称后仍超预算。
- **生成方式**：
  - 复用既有 structured summary 家族，直接对 **B 处理后的整份 working messages** 生成一条摘要；
  - 同一条摘要文本末尾拼接 `Execution Keepalive` 区块；
  - `KeepaliveSnapshot` 来源固定为：`PlanRuntime.mode()`、`active_plan_path()`、`mode().active_plan_id()`、plan file frontmatter.todos（EXEC / Pending 取 `in_progress` 或第一个 `pending`；Planning 取 `active_planning_plan_id + session_todos`）、`ContextState.latest_plan_event`。
- **持久化与应用**：
  - 不新增 `current_tail.split_turn` 自定义事件；
  - 直接复用既有 `BranchSummaryEntry` + `apply_boundary` 语义；
  - `covered_start_id` 取当前 working messages 首条，`covered_end_id` 取当前 working messages 末条；
  - apply 成功后，局部 `messages` 中只保留一条 `ChatMessage::compaction_summary(...)`；
  - 同步更新 `agent.start_idx`、`agent.context_tail_start`；
  - 调用 `ctx_state.invalidate_api_usage()`。

```text
working messages after B
      |
      v
generate one structured summary
      +
build keepalive snapshot
      |
      v
one branch_summary text
      |
      v
apply_boundary
      |
      v
[single compaction_summary]
```

**说人话**：C 出场就说明 B 已经把历史和当前轮都尽力压过了。这个时候别再切 prefix / suffix，也别新造事件，**直接把整份工作集折成一条 `branch_summary + keepalive snapshot`**，复用现有成熟的 `branch_summary/apply_boundary` 路径就够了。

#### 4.2.4 PR-CTD：配置清理 + 文档/测试

- `keep_recent_turns` 默认值已调到 `5`，并真正传到 `compact_tool_results(..., config)`；
- `compaction_turns` 从 `ContextConfig`、config write allowlist、catalog 文案、单测与 README 移除；
- 同步更新：
  - [`context-management.md`](./context-management.md)：补“current-tail guard 是 mid-turn 支线，不是下一 user turn 的 preheat”；
  - [`agent-loop.md`](./agent-loop.md)：补 `tool_dispatcher -> current-tail precheck -> next llm.chat_stream`；
  - [`plan-runtime.md`](./plan-runtime.md)：补 keepalive snapshot 来源与 single-branch-summary collapse 保活边界。

**说人话**：配置、代码、文档要一起收口，不然新的 guard 做出来了，旁边的文档和配置还在讲旧世界。

---

## 5. 协议（入参 / 出参 / Schema）

> **单一事实源（目标态）**：
>
> - transcript 包装层：`../../src/core/session/transcript.rs::CustomEntry`
> - current-tail payload / helper：`【目标态】 ../../src/core/agent_loop/current_tail_guard.rs`
> - keepalive 数据来源：`../../src/core/plan_runtime/mod.rs` + `../../src/core/session/manager/context.rs`

### 5.1 `AggregatePrecheckDecision`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `route` | `string enum` | 是 | 无 | A 输出 | `fits` / `reduce_current_tail` / `collapse_to_branch_summary` | 这次是放行、先减负，还是直接整份折成一条摘要。 |
| `workingSetTokens` | `integer` | 是 | 无 | A 输出 | 当前 prompt 总重，来自 `ContextState::estimated_token_count()` | 整车现在多重。 |
| `contextBudgetTokens` | `integer` | 是 | 无 | A 输出 | Prompt 红线，等于 `context_window - max_output_tokens` | 这车最多能多重。 |
| `overflowTokens` | `integer` | 是 | `0` | A 输出 | `max(0, working - budget)` | 超载了多少。 |
| `maxReducibleTokens` | `integer` | 是 | `0` | A 输出 | 先 apply 已完成历史摘要、再压 apply 后历史、最后把 tail 候选都减到最轻时，最多能腾出的 token | 历史和当前轮全都尽力时，最多能减多少。 |
| `compactableCandidateCount` | `integer` | 是 | `0` | A 输出 | 当前 tail 可参与 reduction 的候选数 | 真到 tail 阶段时，这轮有多少箱子能动。 |
| `firstWaveCandidateCount` | `integer` | 是 | `0` | A 输出 | tail reduction 第一刀会被 placeholder 的候选数 | 第一刀到底砍多少箱。 |
| `yellowLampOnly` | `boolean` | 是 | `false` | A 输出 | `>= 0.90 * budget` 但尚未超线时为 `true`，仅用于诊断日志 | 先亮黄灯提醒一下。 |

示例（内存态，不写 transcript）：

```json
{
  "route": "reduce_current_tail",
  "workingSetTokens": 271800,
  "contextBudgetTokens": 272000,
  "overflowTokens": 1800,
  "maxReducibleTokens": 5400,
  "compactableCandidateCount": 5,
  "firstWaveCandidateCount": 2,
  "yellowLampOnly": false
}
```

### 5.2 reduction transcript helper 契约（非 transcript 事件）

reduction 不再追加 `CustomEntry`；它通过 `session/transcript` 模块提供的 helper 尝试回写对应 `tool_result` message。内部 helper 建议接收如下结构：

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `messageId` | `string` | 是 | 无 | rewrite helper | 被改写的 tool message `msg_id` | 说明要改哪条工具结果。 |
| `toolName` | `string` | 是 | 无 | rewrite helper / audit | `read` / `search_files` / `bash` / `task_output` | 哪种工具的结果。 |
| `waveIndex` | `integer` | 是 | `1` | rewrite helper / metrics | 第几轮二分波次 | 第几刀。 |
| `placeholderText` | `string` | 是 | `TOOL_RESULT_PLACEHOLDER` | rewrite helper | 要写回 transcript 的文本；局部 `messages` 同步使用同一文本 | 直接换成哪句占位符。 |
| `originalChars` | `integer` | 是 | 无 | audit / metrics | 原始字符数 | 原来多重。 |
| `rewrittenChars` | `integer` | 是 | 无 | audit / metrics | 改写后字符数 | 现在多重。 |

示例：

```json
{
  "messageId": "msg_tool_42",
  "toolName": "search_files",
  "waveIndex": 1,
  "placeholderText": "[Previous tool result replaced to save context space]",
  "originalChars": 14820,
  "rewrittenChars": 47
}
```

helper 规则：

1. `current_tail_guard` 先提交局部 `messages` 与计数回写；
2. helper 按 `messageId` 找到目标 `TranscriptEntry::Message`；
3. helper 保留 `id` / `parent_id` / `timestamp` / `tool_call_id` 等字段不变，仅更新 `message.content`；
4. helper 失败只返回诊断结果，不回滚已生效的当前轮减负。

### 5.3 单条 `branch_summary + keepalive snapshot` collapse（复用既有 `BranchSummaryEntry`）

C 不新增 `CustomEntry`；它直接复用 [`context-management.md`](./context-management.md) 既有 `BranchSummaryEntry + apply_boundary` 语义。阶段二只额外增加以下约束：

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `coveredStartId` | `string` | 是 | 无 | collapse / apply | 当前 working messages 首条消息 id | 从哪里开始整份折叠。 |
| `coveredEndId` | `string` | 是 | 无 | collapse / apply | 当前 working messages 末条消息 id | 到哪里为止整份折叠。 |
| `summaryText` | `string` | 是 | 无 | transcript / prompt | 单条 `branch_summary` 的正文，包含结构化摘要和 keepalive 区块 | 最终写进那一条摘要里的全部内容。 |
| `collapseReason` | `string enum` | 是 | 无 | audit | `reduction_insufficient_after_history_and_tail` | 为什么已经进入最后兜底。 |

示例（写入的是既有 `branch_summary` 行，不是新事件）：

```json
{
  "coveredStartId": "msg_u_10",
  "coveredEndId": "msg_tool_58",
  "summaryText": "## Structured Summary\n- 已先 apply 历史摘要、再压一轮历史并完成 current tail 减负，但整体仍超预算。\n\n## Execution Keepalive\n- mode: executing\n- active_plan_path: /Users/me/.tomcat/plans/current-tail.plan.md\n- active_plan_id: plan_76e2ac5a\n- current_step: 实现 current_tail_guard.rs 的 precheck 与 route\n- pending_work: 补 hydration 与 branch_summary collapse reload 语义\n- latest_plan_event: plan.build",
  "collapseReason": "reduction_insufficient_after_history_and_tail"
}
```

应用规则：

1. `coveredStartId` / `coveredEndId` 必须覆盖 **B 完成后的整份 working messages**；
2. 写盘复用既有 `BranchSummaryEntry` / `apply_boundary` 路径，不新增 `current_tail.split_turn`；
3. apply 成功后局部 `messages` 中只保留一条 `ChatMessage::compaction_summary(summaryText)`；
4. C 成功路径必须调用 `ctx_state.invalidate_api_usage()`。

### 5.4 `render_branch_summary_with_keepalive(...)` 与 `build_keepalive_snapshot(...)`

推荐渲染结果：

```text
## Structured Summary
... LLM generated summary over the post-B working messages ...

## Execution Keepalive
- mode: executing
- active_plan_path: ...
- active_plan_id: ...
- current_step: ...
- pending_work: ...
- latest_plan_event: ...
```

约束：

- `Structured Summary` 复用既有 structured summary 家族；
- `Execution Keepalive` 的数据来源固定为 `PlanRuntime.mode()`、`active_plan_path()`、`mode().active_plan_id()`、plan file frontmatter.todos（EXEC / Pending 取 `in_progress` 或第一个 `pending`；Planning 取 `active_planning_plan_id + session_todos`）、`ContextState.latest_plan_event`；
- `Execution Keepalive` 可沿 `Execution_Keepalive_SUMMARIZATION_PROMPT` 的格式口径渲染，但**数据真相只来自本地状态**；
- 任何 C 成功路径都要调用 `ctx_state.invalidate_api_usage()`。

---

## 6. One-Glance Map（文件职责总览）

```text
┌──────────────────────────────────────────────────────────────────────┐
│ src/infra/config/types/context.rs                                   │
│ • keep_recent_turns 目标默认 5                                       │
│ • 删除 compaction_turns                                             │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/tools/config_tool/allowlist.rs / contract/catalog.rs       │
│ • 删除 context.compaction_turns                                      │
│ • 保留 / 暴露 context.keep_recent_turns                              │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/compaction/truncation.rs                                   │
│ • L1 保护区改读 config.keep_recent_turns                             │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/agent_loop/tool_dispatcher.rs                              │
│ • tool result eager append                                            │
│ • on_message_appended(...) 记账                                       │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/agent_loop/accessors.rs                                    │
│ • 暴露 partial_messages = messages[start_idx..]                      │
│ • 为 current tail 视角提供稳定边界                                   │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/agent_loop/reasoning_loop.rs                               │
│ • 每次 tool round 后触发 mid-turn aggregate precheck                 │
│ • route = fits / reduce_current_tail / collapse_to_branch_summary    │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                 ┌─────────────┴─────────────┐
                 ▼                           ▼
┌────────────────────────────────┐  ┌──────────────────────────────────┐
│ src/core/agent_loop/current_   │  │ src/core/compaction/preheat.rs  │
│ tail_guard.rs 【目标态】       │  │ • generate branch_summary text    │
│ • precheck helper              │  │ • 复用 structured summary 模板    │
│ • apply ready history          │  └────────────────┬─────────────────┘
│ • history recompact            │                   │
│ • tail reduction waves         │                   ▼
│ • collapse trigger             │  ┌──────────────────────────────────┐
└────────────────┬───────────────┘  │ src/core/plan_runtime/mod.rs     │
                 │                  │ • build_keepalive_snapshot()     │
                 │                  │ • mode / active_plan_path / todo │
                 │                  └────────────────┬─────────────────┘
                 │                                   │
                 ▼                                   ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/compaction/apply.rs / session/transcript.rs                │
│ • apply_boundary                                                     │
│ • reduction message rewrite helper                                   │
│ • branch_summary 行写入 / boundary apply                             │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/core/session/manager/types.rs / context.rs                       │
│ • ContextState::estimated_token_count / invalidate_api_usage         │
│ • helper 成功时 reload 读到 placeholder                               │
│ • hydrate 复用既有 branch_summary 语义                                │
└──────────────────────────────┬───────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ tests/                                                                │
│ • agent_loop current_tail_guard tests                                 │
│ • hydrate/reload tests                                                │
│ • config/truncation drift cleanup tests                               │
└──────────────────────────────────────────────────────────────────────┘
```

阅读顺序：

1. 先看 `tool_dispatcher.rs -> reasoning_loop.rs`，明确“为什么 guard 一定在 mid-turn”。
2. 再看 `current_tail_guard.rs`，理解 A/B/C 的测量、历史 apply/recompact、tail reduction 与 collapse 逻辑。
3. 最后看 `apply.rs / transcript.rs / context.rs / plan_runtime/mod.rs`，确认改写如何跨 reload 维持且不把执行态压没。

**说人话**：自顶向下就是“配置收口 -> 工具结果入账 -> 当前轮过磅 -> 先吃历史收益 -> 再压历史 -> 再压 tail -> 还不够就整份 collapse -> reload 还能恢复同一份真相”。

---

## 7. 调度时序（运行时图）

### 7.1 fits / reduction 路径

```text
LLM                tool_dispatcher        reasoning_loop       current_tail_guard    apply/transcript/context
 |                        |                     |                     |                        |
 | -- tool_calls -------> |                     |                     |                        |
 |                        | -- 执行工具 ------->|                     |                        |
 |                        | -- push tool msg -->|                     |                        |
 |                        | -- on_message_appended ------------------->|                        |
 |                        |                     | -- precheck -------> |                        |
 |                        |                     | <--- route=fits -----|                        |
 |                        |                     | -- next chat_stream -------------------------------->|
 |                        |                     |                     |                        |
 |                        |                     | -- precheck -------> |                        |
 |                        |                     | <--- route=reduce ---|                        |
 |                        |                     | -- apply ready history ----------------------------->|
 |                        |                     | -- recheck --------> |                        |
 |                        |                     | -- history recompact ------------------------------->|
 |                        |                     | -- recheck --------> |                        |
 |                        |                     | -- tail reduction -->|                        |
 |                        |                     |                     | -- rewrite memory/counters
 |                        |                     |                     | -- best-effort transcript sync
 |                        |                     | -- recheck --------> |                        |
 |                        |                     | <--- route=fits -----|                        |
 |                        |                     | -- next chat_stream -------------------------------->|
```

### 7.2 collapse 路径

```text
tool_dispatcher     reasoning_loop      current_tail_guard      preheat/LLM         plan_runtime      apply/transcript/context
      |                    |                    |                    |                    |                       |
      | -- round done ---> |                    |                    |                    |                       |
      |                    | -- precheck -----> |                    |                    |                       |
      |                    | <--- route=collapse|                    |                    |                       |
      |                    | -- summarize full working set ---------->|                    |                       |
      |                    |                    | <--- summary -------|                    |                       |
      |                    |                    | -- keepalive -------------------------> |                       |
      |                    |                    | <--- snapshot ------------------------ |                       |
      |                    |                    | -- append branch_summary ------------------------------------->|
      |                    |                    | -- apply_boundary ------------------------------------------->|
      |                    |                    | -- invalidate_api_usage ------------------------------------>|
      |                    | -- recheck ------> |                    |                    |                       |
      |                    | <--- route=fits ---|                    |                    |                       |
      |                    | -- next chat_stream ------------------------------------------------------------->|
```

### 7.3 控制流伪码

```text
after tool_dispatcher returns:
    decision = precheck(ctx_state, messages, start_idx)

    match decision.route:
        fits:
            continue next llm.chat_stream

        reduce_current_tail:
            apply_ready_history_if_present(...)
            recompact_history_after_apply(...)
            reduce_current_tail_in_waves(...)
            decision2 = precheck(...)
            if decision2.route == fits:
                continue
            goto collapse_to_branch_summary

        collapse_to_branch_summary:
            summary = generate_branch_summary_over_working_messages(...)
            keepalive = build_keepalive_snapshot(...)
            append branch_summary
            apply_boundary
            move start_idx/context_tail_start to summary
            invalidate_api_usage()
            decision3 = precheck(...)
            assert decision3.route == fits or fail closed
```

**说人话**：这条链路的关键不是“压一次就完”，而是**改写后必须重新称重**。只有真的降回预算线内，下一次 LLM 请求才允许发出去。

---

## 8. 状态机

```text
┌────────────┐  precheck   ┌──────────────┐  route=fits   ┌────────────────────┐
│    idle    │────────────▶│   measuring  │──────────────▶│ ready_for_next_llm │
└────────────┘             └──────┬───────┘               └────────────────────┘
                                  │
                                  │ route=reduce_current_tail
                                  ▼
                          ┌──────────────┐ apply / recompact / tail ┌──────────────┐
                           │   reducing   │──────────────────────▶│  rechecking  │
                           └──────┬───────┘                       └──────┬───────┘
                                  │                                      │
                                  │ still overweight / route=collapse    │ route=fits
                                  ▼                                      ▼
                          ┌──────────────┐ branch_summary+keepalive ┌────────────────────┐
                          │  collapsing  │────────────────────────▶│ ready_for_next_llm │
                           └──────┬───────┘                      └────────────────────┘
                                  │
                                  │ summary fail / append-or-apply fail / recheck still overflow
                                  ▼
                           ┌──────────────┐
                           │ failed_closed│
                           └──────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
| --- | --- | --- | --- | --- |
| `idle` | tool round 完成 | `measuring` | 计算 `working_set_tokens` / `overflow_tokens` | 一拨工具跑完，开始称重。 |
| `measuring` | `overflow_tokens == 0` | `ready_for_next_llm` | 无 | 不超重，直接发下一次 LLM。 |
| `measuring` | `overflow > 0 && reducible enough` | `reducing` | 生成 `ReductionPlan` | 超了，但还能先减负。 |
| `reducing` | apply / 历史再压 / tail reduction 完成 | `rechecking` | apply 历史收益、重压历史、改 tool message、回写计数，并 best-effort 调 transcript helper | 先把历史和当前轮都尽量减下来。 |
| `rechecking` | `overflow == 0` | `ready_for_next_llm` | 无 | 减完确实变轻了。 |
| `rechecking` | 仍超重，且 B 已无更多收益 | `collapsing` | 生成单条 `branch_summary + keepalive` | 局部减负救不回来，只能整份折叠。 |
| `collapsing` | `branch_summary` 生成并 apply 成功 | `ready_for_next_llm` | 写 branch_summary、apply boundary、挪边界、`invalidate_api_usage()` | 整份工作集已经安全折成一条摘要。 |
| `reducing` / `collapsing` | 落盘失败 / summary 失败 / recheck 仍超重 | `failed_closed` | 返回错误，不发送过载请求 | 宁可这次失败，也不裸发一份明知超重的请求。 |

**说人话**：阶段二的状态机只有一个原则：**要么确认减下来了再发，要么失败收口，不能“先发出去试试看”。**

---

## 9. 配置与环境变量

总则：**env > config > 默认**。本方案**不新增**环境变量，只调整 `context` 配置项的解释与默认值。

| 变量 | 取值 | 含义 | 优先级 | 说人话 |
| --- | --- | --- | --- | --- |
| `[context] keep_recent_turns` | `usize >= 0`；默认 `5` | L1 `compact_tool_results` 的 protected zone 大小 | config | 最近几轮老历史先别 placeholder。 |
| `[context] compaction_turns` | **删除** | 阶段二后不再存在于 `ContextConfig` / allowlist / catalog | — | 死配置直接删。 |
| `[context] context_window` | 模型窗口大小 | 与 `max_output_tokens` 一起决定 `context_budget_tokens` | config | 总共多大车厢。 |
| `[context] max_output_tokens` | 最大输出 token | 从总窗口中扣除，得到 prompt 预算上限 | config | 给模型留出的回话空间。 |
| `[context] compaction_model` | 模型名 | `generate_turn_prefix_checkpoint()` 复用此模型 | config | split 做摘要沿用现有压缩模型。 |
| `[context] compaction_max_tokens` | `usize` | split prefix 摘要与 preheat 一样受此上限约束 | config | checkpoint 摘要自己也别太胖。 |

**说人话**：阶段二不再引入新的“神秘旋钮”。真正变化只有两个：`keep_recent_turns` 要真生效，`compaction_turns` 要彻底消失。

---

## 10. 错误模型 / 截断 / 警告

```text
precheck 未超线
  -> 正常继续（无 warning）

precheck 黄灯（>= 90% budget, 但未超线）
  -> 仅 diagnostic log

reduction 成功
  -> apply 历史 / 历史再压 / tail 改写 + recheck

B 已尽力仍超预算
  -> 生成单条 branch_summary + keepalive，复用 apply_boundary

C 成功
  -> append/apply branch_summary + invalidate_api_usage + recheck

C summary 失败 / branch_summary append-or-apply 失败 / recheck 仍超线
  -> Err（fail closed），不发送已知超重请求

reload 时 branch_summary 覆盖区间过期（id 对不上）
  -> warn + 走既有 stale apply 处理

工具结果不在 COMPACTABLE_TOOLS
  -> 原文保留（非错误）
```

| 情况 | 归一化结果 | 具体动作 | 说人话 |
| --- | --- | --- | --- |
| `working_set_tokens >= 0.90 * budget && overflow == 0` | 诊断黄灯 | `tracing::debug!/warn!`；不改控制流 | 快到线了，先亮黄灯提醒。 |
| reduction transcript helper 失败 | 非致命持久化降级 | 保留本次内存减负结果；记 `warn` / metrics；不回滚、不立即转 C | 单据一时没改成，不影响先把这轮车减下来。 |
| B 已完成 apply + 历史再压 + tail reduction 仍超预算 | 正常升级分流 | 停止继续做局部减负，转 `collapse_to_branch_summary` | 历史和尾巴都压过了，还不够，就整份折成一条摘要。 |
| C summary 生成失败 | 致命 guard 失败 | 返回错误给上层 attempt；不发送过载请求 | 单条摘要打不出来，这趟就别硬发。 |
| `branch_summary` append / `apply_boundary` 失败 | 致命 guard 失败 | 返回错误；局部 `messages` 不提交 C 结果 | 摘要没真正 apply 成功，就不算真的切好了。 |
| hydrate / apply 遇到 stale `covered*Id` | 容忍 warning | 复用既有 stale apply 处理；必要时删 `branch_summary` 行并跳过恢复 | 覆盖区间对不上现在的货单，就按已有 branch_summary 规则收口。 |
| `bash` / `task_output` 已进入 `COMPACTABLE_TOOLS` 但模型可能想继续追日志或重跑命令 | 风险提示 | reduction 只做 placeholder，不自动重放；`bash` 仍受既有 gate / confirm 约束，`task_output` 继续沿用 `task_id` / `since` 语义；特别大的输出优先复用 persisted preview helper | 可以把 shell/日志结果折起来，但不能让系统替模型偷偷重跑命令。 |

**说人话**：阶段二宁可“这次 guard 失败并报错”，也不能“明知超重还发一把看看”。**

---

## 11. 测试矩阵（验收）

> 状态说明：本文为开发前定稿，除文档本身外，其余测试均为目标态 **PENDING**。

| 维度 | 用例 / 编号 | 状态 | 说人话 |
| --- | --- | --- | --- |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::precheck_routes_to_fits_reduce_and_collapse`【目标态】 | PENDING | 三条路由都要锁死。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::many_medium_reads_trigger_reduce_before_next_chat_stream`【目标态】 | PENDING | 证明抓到的是“累计超载”，不是单条超大。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::reduction_applies_ready_history_before_tail_rewrite`【目标态】 | PENDING | 先吃现成历史收益，再动 tail。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::history_recompact_runs_before_tail_reduction`【目标态】 | PENDING | apply 后历史要先再压一遍。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::reduction_rewrites_oldest_half_first`【目标态】 | PENDING | 第一刀必须先砍旧半区。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::reduction_runs_next_wave_when_first_wave_not_enough`【目标态】 | PENDING | 一刀不够就继续下一刀。 |
| 单元 | `src/core/agent_loop/tests/current_tail_guard_test.rs::compactable_tools_only_rewrite_read_search_bash_task_output`【目标态】 | PENDING | 只有白名单工具会被改，`task_output` 也在其中。 |
| 单元 | `src/core/session/manager/tests/context_state_test.rs::rewrite_local_tail_delta_updates_post_usage_appended_chars`【目标态】 | PENDING | 改短了消息，称重底账也要一起变。 |
| 单元 | `src/infra/config/tests/context_cfg_test.rs::default_keep_recent_turns_is_5_and_compaction_turns_removed`【目标态】 | PENDING | 配置真相要锁死。 |
| 集成 | `tests/agent_loop_tests.rs::current_tail_overflow_enters_reduction_before_next_chat_stream`【目标态】 | PENDING | 真正的集成链路上，先减负再发 LLM。 |
| 集成 | `src/core/session/manager/tests/hydrate_test.rs::tail_reduction_helper_sync_survives_reload_when_write_succeeds`【目标态】 | PENDING | helper 写盘成功时，reload 后 reduced tail 不能复活成原文。 |
| 集成 | `src/core/session/manager/tests/hydrate_test.rs::single_branch_summary_collapse_rehydrates_cleanly`【目标态】 | PENDING | 单条 branch_summary collapse 要能稳定回盘。 |
| 集成 | `src/core/plan_runtime/tests/keepalive_snapshot_test.rs::execution_keepalive_snapshot_survives_collapse`【目标态】 | PENDING | 压成一条摘要以后还能继续推进 plan。 |
| 负向 | `tests/agent_loop_tests.rs::history_only_compaction_still_overflows_without_mid_turn_precheck`【目标态】 | PENDING | 证明旧路径真的不够。 |
| 文档 | `docs/architecture/current-tail-aggregate-guard.md` 本文定稿 | ✅ 2026-05-30 | 先把方案真相钉住。 |
| 文档 | `context-management.md` / `agent-loop.md` / `plan-runtime.md` 交叉引用同步 | PENDING | 相邻文档别继续讲旧世界。 |

### 11.1 目标 → 测试映射

| 目标 | 锁死它的测试 / 机制 | 状态 | 说人话 |
| --- | --- | --- | --- |
| G1 | `many_medium_reads_trigger_reduce_before_next_chat_stream` | PENDING | 发车前就要拦住。 |
| G2 | `precheck_routes_to_fits_reduce_and_collapse` | PENDING | 看整车，不看单箱。 |
| G3 | `reduction_applies_ready_history_before_tail_rewrite` + `history_recompact_runs_before_tail_reduction` + `reduction_rewrites_oldest_half_first` + `reduction_runs_next_wave_when_first_wave_not_enough` + `compactable_tools_only_rewrite_read_search_bash_task_output` | PENDING | 历史和 tail 的顺序、刀法都要锁死。 |
| G4 | `tail_reduction_helper_sync_survives_reload_when_write_succeeds` + `single_branch_summary_collapse_rehydrates_cleanly` | PENDING | helper 写盘成功或 C 成功时，关掉再开也得还是同一份减负结果。 |
| G5 | `execution_keepalive_snapshot_survives_collapse` | PENDING | 压完 token 不能把 plan 压没。 |
| G6 | `default_keep_recent_turns_is_5_and_compaction_turns_removed` | PENDING | 配置要跟代码一个口径。 |
| G7 | 文档约束 + `history_only_compaction_still_overflows_without_mid_turn_precheck` | PENDING | 阶段边界要清楚，旧路子也得证明不够。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
| --- | --- | --- | --- |
| **B 改完消息但不回写计数** | precheck 连续误判超重，guard 形同失效 | tail-only reduction 时统一做 `messages` / `estimate_context_chars` / `post_usage_appended_chars` 三处 delta；动到历史时直接 `invalidate_api_usage()`；补专门单测 | 文本瘦了，体重秤也得一起变。 |
| **内存减负与 transcript 发散** | 当前轮已减下来，但 reload 后可能暂时看回原文 | reduction 始终先改内存与计数；同时通过 helper best-effort 回写 transcript，并打指标观察失败率；C 仍保持 `branch_summary + apply_boundary` 成功后才提交 collapse 结果 | 先保当前轮不超重，再尽量把账补齐。 |
| **B apply/历史再压后仍沿用旧 usage 底账** | 预算系统性偏大，后续 guard 误判 | 只要 B 动到 apply 后历史消息，立即 `ctx_state.invalidate_api_usage()`；C 也同样 invalidate | 只要动了老消息，就别继续拿旧秤砣算重量。 |
| **`COMPACTABLE_TOOLS` 选太宽** | 破坏推理语义或让模型过度依赖重跑旧工具 | 首版固定 `read` / `search_files` / `bash` / `task_output`；其它工具默认不纳入；是否把 `list_dir` 等加入单独评估 | 白名单先小一点，别一上来什么都往里塞。 |
| **单条 branch_summary collapse 丢执行态** | 压完 token 后 plan 续不下去 | `summaryText` 必须拼接 keepalive snapshot；数据来源固定为 `PlanRuntime + plan file + latest_plan_event`；补专门集成测 | 摘要可以瘦，但执行态不能丢。 |
| **branch_summary stale apply** | reload 或 apply 时覆盖区间对不上，导致恢复异常 | 复用现有 stale apply 处理：删行、warn、跳过恢复；测试覆盖 stale 路径 | 旧摘要偶尔失效可以接受，但别因此崩掉。 |
| **阶段二和阶段三实现边界混淆** | 代码职责漂移，难以验证 | 文档、测试和 PR 命名都以“预防型”为准；`finish_reason` recovery 独立排期 | 先把一件事做对，再做下一件。 |
| **配置与文档继续漂移** | 用户读到假旋钮，工具目录误导 | 阶段二合入时同步清理 `allowlist`、`catalog`、`README` 与本文交叉引用 | 配置、代码、文档必须同一份真相。 |

---

## 13. 历史决策 / 跨文档修订

### 13.1 已被本方案取代的路线

| 历史路线 | 当前结论 | 说人话 |
| --- | --- | --- |
| ~~继续沿用 `promptBudgetBeforeReserve` 一类旧预算命名~~ | → **否**：正式方案统一使用 `context_budget_tokens` | 名字和口径都得统一，别再让预算概念左右互搏。 |
| ~~只靠单条阈值或只压老历史就能解决 current-tail 爆窗~~ | → **否**：阶段二必须引入 mid-turn aggregate precheck | 当前轮越跑越胖时，压老历史常常不顶用。 |
| ~~B 只改内存 `messages`，完全不尝试持久化~~ | → **否**：仍应调用 transcript helper 尝试回写；但**不以成功作为当前轮减负前提** | 先减负，再尽量补账，顺序别反。 |
| ~~B 走 append-only reduction ledger~~ | → **否**：实现复杂度过高，本期直接 rewrite transcript message 行 | 当前仓已有安全 rewrite 路径，就别再多造一套账本。 |
| ~~C 新增 `current_tail.split_turn` 事件并保留 suffix verbatim~~ | → **否**：复用现有 `branch_summary + apply_boundary`，直接整份 working messages collapse | 现成成熟路径能做的，就别再为 split 新造一套。 |
| ~~阶段二顺手把 `finish_reason` recovery 一起做掉~~ | → **否**：阶段三独立处理 | 预防型和反应型要拆开做。 |

### 13.2 跨文档修订清单

| 文档 | 需修订内容 | 状态 | 说人话 |
| --- | --- | --- | --- |
| [`context-management.md`](./context-management.md) | 补一段“current-tail guard 是 mid-turn 支线，不是下一 user turn preheat”，并同步 `keep_recent_turns` / `compaction_turns` 口径 | PENDING | 别让旧文档继续把阶段二说成历史压缩。 |
| [`agent-loop.md`](./agent-loop.md) | 在 `tool_dispatcher -> next llm.chat_stream` 之间补 current-tail precheck / reduction / collapse 钩子 | PENDING | agent-loop 总图里得看见这条新车道。 |
| [`plan-runtime.md`](./plan-runtime.md) | 补 keepalive snapshot 的数据来源与单条 branch_summary collapse 保活边界 | PENDING | plan-runtime 要说明它为什么会被阶段二读取。 |
| `../../src/core/README.md` | 更新 `keep_recent_turns` 默认值与移除 `compaction_turns` | PENDING | README 也得讲新真相。 |

一句话总结：阶段二 `current-tail aggregate guard` 不是只做一层“压 current tail”，而是在 `reasoning_loop` 的 tool round 之间，先称整车、再 **apply 历史 + 重压历史 + 压 current tail**，真压不下来就把 **整份 working messages** 折成单条 `branch_summary + keepalive snapshot`。
