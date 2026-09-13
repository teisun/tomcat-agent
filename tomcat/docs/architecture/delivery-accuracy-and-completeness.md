# 交付准确率与完整性：当前完成门禁

## 2026-09 交付描述与执行证据分离

`TodoItem.content` 回答“原先承诺做什么”；`TodoItem.evidence[]` 回答“实际如何验证或交付”。
两者不能在 EXEC 中互相替代：

```text
approved work content ──冻结──▶ 完成门禁 / review 范围可追溯
                                  │
status transition ──evidence─────┘
  ├─ tests and observed output
  ├─ delivered file / endpoint
  └─ explicit limitation or handoff
```

因此已有 work todo 的 upsert content 改动会被拒，并指向 `set_status.evidence`；planning/pending
仍允许改稿。完成而没有 evidence 仅发 warning（保证旧调用可恢复），不伪造验证通过。

review 预算耗尽时不再把 P0 和 P1 混为一谈：残余 P0 立即 handoff 给用户，残余仅 P1 才进入
Acceptance。用户发新消息是重新开始无人值守 review 预算的明确边界，未解决 findings 不会被清除。
若审计显示 P1 放行也产生可避免的严重漏检，应收紧放行策略而非丢弃审计事件。

### R3 事件触发的口径修正（2026-09-13）

一次被取消的 review 曾把 gate 的 `in_progress` 半开态写到磁盘，并让后续执行无路可走；
另一次预算耗尽则把 P0 当 P1 一样自动放行。这推翻了“半开态可在下次补救”和“轮次上限
可替用户接受风险”两个前提。现在 gate 的启动只保留在当前进程：review 用
`InFlightReview` lease，Acceptance 用进程内启动标记；磁盘只保存 `pending` 或
`completed`。lease 被 Drop 会留下 aborted 终态，P0 则保留 pending gate 并 handoff。

跨轮 reviewer 复用持久化的轮次、基准时间、open findings 和已裁决 findings，而不续跑旧
子 Agent 对话；每一个 reviewer 也拥有独立 `ReadFileState`，其首次 read 不会被另一轮短路。
若全量 verify 仍反复被缩窄，或结构化账本不足以支持增量 review，再重新评估计划命令清单
对账或运行时主动执行命令，而不是现在提前引入它们。

> 状态：已实现（以当前工作树为准）。
> 适用：代码变更计划的 EXEC 收口。字段、调用协议与完整决策以 [plan-exec-code-verification.md](plan-exec-code-verification.md) 为权威来源。
> 规范：[ARCHITECTURE_SPEC.md](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。相邻方案：[tools/reviewer.md](tools/reviewer.md)、[plan-runtime.md](plan-runtime.md)、[permission-system.md](permission-system.md)。
> 本文 `## 1`–`## 10` 分别对应 ARCHITECTURE_SPEC §1–§10。

## 摘要

当前实现把“代码计划完成”定义为可复核的运行时事实，而不是 Agent 的完成声明：已绑定 workspace root 的工作树中存在代码 diff 时，计划必须先通过 P0/P1 code review，再提交由后台 bash 任务账本核验的新鲜绿构建证据。预算耗尽的 P0 必须交还用户，不能自动完成。证据为 `{command, task_id}`，运行时反查任务的精确命令、`exit_code=0` 和启动时间是否晚于最新代码编辑。

本文是交付可靠性总览；[plan-exec-code-verification.md](plan-exec-code-verification.md) 是协议和实现顺序的单一事实源。旧版 `TodoKind`/research evidence、自动 `cargo check`/`tsc`、以及“code reviewer 后派发 verifier”的说法均不再描述当前实现。

**说人话**：待办全打勾不等于交付完成。改过代码时，要先审出没有大问题，再拿出一条实际跑成功、而且比最后一次改代码更晚启动的验收命令收据。

---

## 文首导读：方案导图集

阅读顺序建议：

1. **A.1 抽象总图**：完成为何只认代码、时间和任务账本等事实。
2. **A.2 具体总图**：这些事实在当前 Tomcat 中由谁采集、保存和裁决。
3. **B. 状态机**：todo 已完成为何仍可能保持 `executing`。

### A.1 抽象 ASCII 总图

完成门禁覆盖“所有 todo 已完成”后的最后一段。它不试图验证研究结论，也不预设某一种语言或编译器；在 workspace root 已绑定时，它只对当前工作树中的代码路径强制 review 与项目相关的验收命令。

```text
主 Agent 标记最后一个 todo completed
                  │
                  ▼
        ┌───────────────────────┐
        │ Git 代码 diff 过滤     │
        │ 路径 + 扩展名 + mtime  │
        └─────────┬───────┬─────┘
                  │       │
        无代码 diff       有代码 diff
                  │       │
                  ▼       ▼
             completed  ┌─────────────────────────────────────────────┐
                        │ 已保存的 review/build 是否覆盖最新代码？       │
                        └──────┬───────────────────────────────┬──────┘
                               │是                             │否
                               ▼                               ▼
                          completed                   P0/P1 code review
                                                           │
                                 ┌─────────────────────────┴────────────────────┐
                                 │未裁决 P0/P1                                  │无 P0/P1
                                 ▼                                               ▼
                           EXEC：修复/申辩                              verify skill + background bash
                                                                         │
                                                                         ▼
                                                         registry: command/task_id/exit/fresh?
                                                                         │
                                                                     completed
```

**说人话**：系统先看“这次有没有改代码”。没有就直接结束；有就必须让审查和实际命令结果都追上最后一次编辑。

### A.2 具体 ASCII 总图

`tools/plan_tool/update_plan.rs::execute_for_tool` 串联收口；它通过 `plan_runtime/code_reviewer.rs` 取得 diff 与最新 mtime，通过 PlanFile frontmatter 保存门禁状态，通过 `BashTaskRegistry` 验收命令事实。

```text
update_plan
  │
  ├─ collect_code_diff_context()
  │    └─ git diff --name-only HEAD + git ls-files --others
  │       → is_code_path() → newest_edit_mtime_ms
  │
  ├─ frontmatter
  │    ├─ code_review_pass / code_review_pass_at_ms
  │    ├─ green_build_pass / green_build_evidence[]
  │    └─ completion_gate_cycles
  │
  ├─ dispatch_code_reviewer()
  │    └─ Finding::tier(): P0/P1 block; P1 may be accepted as wontfix
  │
  └─ require_green_build_pass()
       ├─ load_skill("verify") discovers suitable commands
       └─ BashTaskRegistry validates task_id
            ├─ exact command
            ├─ Finished + exit_code=0
            └─ started_at >= newest_edit_mtime
```

**说人话**：skill 不是裁判，它只指导 Agent 找项目自己的检查；裁判是运行时，它会去后台任务账本核对“真的跑过、真的成功、时间也对”。

### B. 状态机

```text
┌───────────┐ final todo complete ┌──────────────────┐ review pass ┌────────────────────┐
│ executing │────────────────────▶│ review required  │─────────────▶│ green build needed │
└─────▲─────┘                     └───────┬──────────┘              └─────────┬──────────┘
      │                                   │ P0/P1 / no pass                    │ valid evidence
      │                                   ▼                                    ▼
      └──── repair or add todo ────┌───────────┐                         ┌───────────┐
                                   │ executing │                         │ completed │
                                   └───────────┘                         └───────────┘
```

| 当前状态 | 条件 | 目标状态 | 运行时动作 | 说人话 |
|----------|------|----------|------------|--------|
| `executing` | workspace root 已绑定且无代码 diff | `completed` | 不触发代码门禁。 | 文档等非代码交付不必硬跑编译。 |
| `executing` | review 或 build 对当前 mtime 失效 | `executing` | 清空旧门禁，进入补审或补建。 | 改代码会让旧凭据作废。 |
| `executing` | P0/P1 未解决 | `executing` | 回传 `code_review`，主 Agent 修复或仅对 P1 申辩。 | 大问题没解决就继续干。 |
| `executing` | review 已通过但无有效 build 证据 | `executing` | `BadArgs` 指引加载 skill 和提交 task ID。 | 审完还得实际跑检查。 |
| `executing` | 证据通过账本核验 | `completed` | 持久化 evidence 并写完成事件。 | 收据核验过，才真完成。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| **代码 diff** | 已绑定 workspace root 后，Git 已跟踪变更与未跟踪文件中被 `is_code_path` 认可的路径。 | `CodeDiffContext` | `.rs`、`.ts(x)`、`.js(x)`、`.py`、`.go`、`.java`、`.sh`、`.sql`、`.vue` 等进入门禁；非代码路径不进入。 | 改代码才需要代码验收。 |
| **新鲜门禁** | review/build 的时间覆盖当前代码最新修改。 | frontmatter 时间/证据与 `newest_edit_mtime_ms` | 代码修改会使 `code_review_pass`、`green_build_pass` 和 evidence 失效。 | 旧绿灯不能盖新代码。 |
| **绿构建证据** | 后台 bash 任务的可复核收据。 | `GreenBuildEvidence` | 精确命令、唯一 task ID、`Finished(exit_code=0)`、启动时间新鲜。 | 后台任务跑成功才作数。 |
| **P0/P1 门禁** | P0/critical/blocker 和 P1/major finding 阻止完成。 | `review::Finding::tier` | P2/unknown 归为不阻塞；reviewer 的 `pass` 文本不能覆盖 P0/P1。 | 严重问题一票否决。 |
| **P1 申辩** | 主 Agent 接受 P1 取舍的书面记录。 | `dispute_findings` 与 runtime disputed findings | 只允许 `wontfix` 且理由非空；P0 仅在已 handoff 且用户发新消息确认后可申辩；修复必须改代码后复审。 | P1 可以解释为什么不改；P0 必须先交给用户裁决。 |
| **完成周期上限** | 已完整验证后再改代码时的重验次数限制。 | `completion_gate_cycles` | 默认 3，最小 1；到顶会以 warning 完成，而非产生新通过证据。 | 防无限重验，但不会把旧收据洗成新收据。 |

本文的“收口”是 `update_plan` 中所有 todo 已完成后计算 `derived_completed` 的调用，不是 Agent 返回自然语言总结的时刻。

---

## 2. 竞品 / 选型对比（调研）

仍与现行实现相关的外部证据只有一条原则：**没有真实命令与输出，不能把验证写成 PASS**。

| 参考实现 | 路径 / 符号 | 仍然借鉴 | 当前未采用 | 说人话 |
|----------|-------------|----------|------------|--------|
| `cc-fork-01` | `cc-fork-01/src/tools/AgentTool/built-in/verificationAgent.ts::VERIFICATION_SYSTEM_PROMPT`。 | 每个 PASS 必须有 `Command run` 与 `Output observed`；验证执行者不改项目。 | cc-fork 的独立 verification agent、文本 `VERDICT` 和后台子 Agent 生命周期。 | 保留“跑过才算”，但用任务账本替代子 Agent 的口头裁决。 |

旧版有关 GenericAgent 的 `[VERIFY]`、Codex Guardian、语言固定命令表、`TodoKind` 研究证据与 internal verifier 的横向比较，不再决定当前代码路径，故未保留为架构依据。

---

## 3. 落地选型与实施（已定稿）

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|---------------|--------|
| G1 收口范围 | 哪些完成需要代码门禁？ | **采用** 已绑定 workspace root 的 Git 路径与扩展名过滤；**拒绝** todo 分类或模型自述。 | Tomcat `plan_runtime/code_reviewer.rs::{collect_code_diff_context,is_code_path}`；cc-fork-01 `verificationAgent.ts` 接收 changed files。 | 设计：识别 tracked/untracked 代码路径并取最新 mtime。理由：工作树是可复核事实，非代码交付不应被无关构建阻塞。 | **未入选**：旧 `TodoKind`/research evidence。**拒因**：当前 `TodoItem` 没有这些字段，且它们不能说明是否改过代码。 | 用 Git 判断要不要验，不猜 todo 的意图。 |
| G2 绿构建 | 如何证明当前代码真正验过？ | **采用** managed `verify` skill + `BashTaskRegistry` 硬核验；**拒绝**文本 PASS 和固定 `cargo check`。 | Tomcat `assets/skills/verify/SKILL.md`、`skill/builtin.rs::materialize_builtin_skills`、`update_plan.rs::require_green_build_pass`；cc-fork-01 `verificationAgent.ts` Command/Output 契约。 | 设计：skill 发现项目命令，后台任务提供 command/task/status/time，runtime 保存核验后快照。理由：既适配多语言/项目脚本，又不信模型自述。 | **未入选**：自动 cargo/tsc 检查或旧 `VerifySummary` 放行。**拒因**：前者不覆盖项目验收，后者不在当前收口链路。 | 项目自己决定怎么测，运行时决定测没测真。 |
| G3 review 与争议 | 什么问题阻塞、合理取舍怎样表达？ | **采用** P0/P1 阻塞，P1 可 `wontfix` 申辩；**拒绝**仅看 reviewer verdict。 | Tomcat `review.rs::Finding::tier`、`update_plan.rs::{blocking_findings,prepare_disputes}`；cc-fork-01 `verificationAgent.ts` 的有意行为复核要求。 | 设计：运行时按 severity 做最终判断，把接受的 P1 取舍注入下一轮审查。理由：避免 “pass + P1” 矛盾，也不让已确认取舍无限重报。 | **未入选**：所有 finding 阻塞或所有 finding advisory。**拒因**：前者会被 P2 死锁，后者漏掉交付风险。 | 大问题要修；小问题不挡；P1 必须留下接受理由。 |
| G4 终止循环 | 重验反复失效时如何停？ | **采用** 持久化重验周期上限；**拒绝**无限重跑或把上限当新证据。 | Tomcat `file_store.rs::completion_gate_cycles`、`update_plan.rs::prior_gate_cycles_exhausted`、`runtime.rs::PlanConfig`；cc-fork-01 `verificationAgent.ts` 强调实际检查而非叙述。 | 设计：默认最多 3 个重新 review→build 周期；到顶 `completed + warning`。理由：显式结束空转，同时保留“本轮没有新绿构建”的事实。 | **未入选**：不设上限或自动重置所有凭据。**拒因**：前者不可终止，后者伪造 freshness。 | 到顶可以结束，但必须明说这是带警告的取舍。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| P1 diff 与 freshness | 代码路径筛选、mtime、持久化 pass/evidence/cycle 字段。 | `plan_runtime/code_reviewer.rs`、`plan_runtime/file_store.rs`、`update_plan.rs`。 | `green_build_gate_blocks_completion_until_pass`；mtime/cycle 边界：PENDING。 | 每份绿灯都对应一份具体代码。 |
| P2 code review | read-only reviewer、P0/P1 判定、P1 申辩与 handoff。 | `plan_runtime/{code_reviewer.rs,review.rs}`、`update_plan.rs`。 | `code_review_rounds_exhaustion_{hands_off_with_p0,unconditionally_advances_to_acceptance_with_p1}_residual`；`code_review_downgrades_uncited_plan_mismatch_but_keeps_cited_p1`。 | 预算内审查先挡住大问题；预算用尽后 P0 交还，只有 P1 能进入 acceptance。 |
| P3 verify 证据 | 内置 skill 物化、后台命令与账本准入。 | `skill/builtin.rs` + `assets/skills/verify/**`、`tools/primitive`、`update_plan.rs`。 | `green_build_gate_blocks_completion_until_pass`。 | skill 会找检查，账本会验收据。 |
| P4 收束防逃避 | 纯文本结束时的继续指令、review/build 不完整时保持 EXEC。 | `agent_loop/turn_finalize.rs::completion_guard_instruction`。 | `agent_loop/tests/completion_guard_test.rs` 的计划未收口 guard 覆盖。 | 没验完不能只写一句“完成了”。 |

---

## 4. 协议（入参 / 出参 / Schema）

完整字段表、调用样例和任务账本校验顺序见权威文档 [plan-exec-code-verification.md §4](plan-exec-code-verification.md#4-协议入参--出参--schema)。这里仅钉死交付门禁的外部接口边界。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `dispute_findings[]` | object[] | 否 | `[]` | 接受 P1 风险 | `ref`、`area`、`resolution:"wontfix"`、`reason`。 | 记录为什么接受 P1。 |
| `green_build_pass` | boolean | 否 | 缺省 | 验收命令完成后 | 传 `true` 时必须同时给有效 evidence。 | 申请把真实验收写进计划。 |
| `green_build_evidence[]` | `{command,task_id}[]` | 条件必填 | `[]` | 同上 | 实际命令和后台任务 ID；运行时再补存时间与退出码。 | 给运行时一张可查的收据。 |

```jsonc
{
  "ops": [],
  "green_build_pass": true,
  "green_build_evidence": [
    { "command": "<exact background command>", "task_id": "<finished task id>" }
  ]
}
```

---

## 5. 文件职责总览（One-Glance Map）

```text
update_plan.rs
  ├─ 收口编排、P0/P1 过滤、P1 申辩
  ├─ freshness / cycle 决策
  └─ BashTaskRegistry 证据核验
        │
        ├──► file_store.rs
        │      └─ persist review/build pass、evidence、cycles
        │
        ├──► code_reviewer.rs + review.rs
        │      └─ Git diff、mtime、read-only P0/P1 findings
        │
        └──► assets/skills/verify/SKILL.md
               └─ 发现项目检查 → 后台 bash → task_id

turn_finalize.rs
  └─ 绿构建缺失时阻止 text-only 收束
```

专业上，所有完成判定收敛到 `update_plan`，所以门禁状态不会散落在 prompt、reviewer 文本或 UI 中。

**说人话**：一处做最终判定，其他组件只负责提供事实，避免“每层都以为别人验过”的漏洞。

---

## 6. 配置与环境变量

当前设计使用 `[plan]` 配置，优先级为**配置文件 > 默认值**，没有对应环境变量。

| 键 | 默认 | 含义 | 说人话 |
|----|------|------|--------|
| `[plan].max_code_review_rounds` | `4` | 收口可派发 code review 的最大次数；`0` 为跳过 review；预算用尽时残余 P0 handoff，残余仅 P1 才带入 acceptance，代码绿构建仍要求。 | 四轮给三次真实修复机会，同时保留无人值守的明确上限。 |
| `[plan].max_completion_gate_cycles` | `3` | 已完整通过后代码又变时，最多重验的 review→build 周期；运行时最小钳制为 `1`。 | 防止修修验验没完。 |

旧 `[plan].verify_gate` 不控制本文完成路径：当前 `update_plan` 不自动调旧 verifier。

---

## 7. 错误模型 / 截断 / 警告

```text
workspace root 已绑定且代码路径为空   → completed
P0/P1 未裁决且预算未尽                 → executing + code_review result
review 已通过但没有 green evidence   → Err(BadArgs) + load_skill 指引
task 不存在 / 未完成 / 非零 / 不新鲜  → Err(BadArgs)，仍 executing
task 账本全部核验通过                 → persist evidence + completed
review 基础设施重试耗尽               → executing + handoff
review 轮次耗尽 + 残余 P0              → executing + handoff
review 轮次耗尽 + 残余仅 P1            → review gate pass + 残余清单 + acceptance
重验周期到顶（此前完整通过过）        → completed + warning
```

没有 workspace root 时，`code_gate_required=false`，因此不会要求绿构建证据；生产路径应绑定 workspace root，才能完整启用代码路径过滤与 mtime freshness。

**说人话**：正常 review 用尽预算时，P0 也要交还用户，只有 P1 转入客观验收；所有代码改动仍必须有真实绿构建凭据。

---

## 8. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| P0/P1 判定 | `tools::plan_tool::tests::code_review_test::only_p0_p1_block_completion_even_when_reviewer_says_pass` | ✅ 当前工作树 | 严重 finding 优先于模型 verdict。 |
| 绿构建账本 | `tools::plan_tool::tests::code_review_test::green_build_gate_blocks_completion_until_pass` | ✅ 当前工作树 | 没有有效后台收据不能完成。 |
| review 失败 / 预算耗尽 | `tools::plan_tool::tests::code_review_test::code_review_rounds_exhaustion_{hands_off_with_p0,unconditionally_advances_to_acceptance_with_p1}_residual` | ✅ 当前工作树 | 预算内 finding 先阻塞；耗尽时 P0 handoff、P1 才放行 review gate 并进入客观验收，不会偷标 completed。 |
| 旧 verifier 不接管 | `tools::plan_tool::tests::verify_test::update_plan_does_not_dispatch_dormant_verifier_even_when_attached` | ✅ 当前工作树 | 文档不会误称还有自动 verifier。 |
| mtime 与周期上限 | 独立 direct tests | PENDING | 应补齐“编辑使证据过期”和“周期到顶 warning”的精确边界。 |
| 文档 | 本文与 [plan-exec-code-verification.md](plan-exec-code-verification.md) | ✅ 2026-08-10 | 高层与权威设计不两张皮。 |

---

## 9. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| 声明代替执行 | 高 | 反查 `BashTaskRegistry`，校验命令、状态、退出码和 mtime。 | 不信嘴，只信收据。 |
| 改代码后沿用绿灯 | 高 | 新代码使 review/build 状态和 evidence 失效；删除文件以当前时间作下界。 | 最后改一行也得重验。 |
| reviewer 结论自相矛盾 | 高 | 以 `Finding::tier()` 的 P0/P1 为准。 | 严重问题不能被“pass”遮住。 |
| 有意识的 P1 取舍反复出现 | 中 | 只允许有理由的 `wontfix`，在下轮 reviewer brief 注入已接受取舍。 | 接受了就留档，别每轮重吵。 |
| 门禁循环 | 中 | review 轮次、基础设施重试、completion cycles 和文本 guard 均有上限。 | 卡住时总有明确出口。 |
| 周期上限被当作新绿灯 | 高 | `completed + warning`，但不恢复已失效的 `green_build_pass`/evidence。 | 可以带风险结束，不能假装刚验过。 |

---

## 10. 历史决策 / 跨文档修订

- ~~research todo 必须有 URL/`file:line` evidence 才能 completed~~ → **否**：当前 schema 没有 `TodoKind` 或 research evidence；本门禁只覆盖代码交付。
- ~~运行时收口自动跑 `cargo check`/`tsc --noEmit`~~ → **否**：当前由 `verify` skill 根据用户/plan、文档、manifest、CI 和变更范围发现项目实际检查。
- ~~code review pass 后自动派发 verifier 子 Agent，`verify_gate` 决定完成~~ → **否**：旧 verifier 是保留资产，`update_plan` 当前不派发；由 skill + `BashTaskRegistry` 替代。
- ~~review 耗尽时 best-effort completed~~ → **否**：保持 `executing` 并 handoff。仅已完整通过后的重验周期上限允许 warning 完成。
- **跨文档修订**：具体协议、状态顺序与代码落点以 [plan-exec-code-verification.md](plan-exec-code-verification.md) 为准；本文只保留交付准确率层面的原则与边界。

---

## 一句话总结

交付准确不靠“我认为完成了”：当前代码变更计划必须由 P0/P1 review 和任务账本中的新鲜成功命令共同证明；周期上限可以停止无穷重验，但只会留下明确 warning，不能制造假的绿构建结果。
