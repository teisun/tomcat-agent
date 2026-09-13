# Plan / Todo 执行完成后的代码验证

## 2026-09 预算耗尽后的编辑与 Todo 证据

### R3 事件触发的更正（2026-09-13）

一次被取消的 reviewer 把 `gate-review: in_progress` 留在磁盘，且旧规则在预算耗尽后会让
P0 跟 P1 一样自动放行。这两件事推翻了“半开态可恢复、预算耗尽一律验收”的假设：

```text
gate 启动命令 ──► 仅进程内 InFlightReview lease ──► 终态一次写盘
                  Drop / 中断 ──► gate 仍 pending + aborted 事件

第 4 轮仍有 P0 ──► handoff 给用户
第 4 轮仅有 P1 ──► 带残余清单进入 Acceptance
```

跨轮 reviewer 不续跑旧对话。`PlanFileFrontmatter` 持久化轮次、基准时间、open findings 与
已裁决 finding；下一轮仅审基准后的 DELTA 并核销 open findings。每个子 Agent 另建
`ReadFileState`，因此 reviewer 的“已读”不会错误短路另一个 reviewer 的首次读取。

未采用的备选保持记录：B7''「运行时执行声明命令」、B13'「续跑 reviewer 对话或交接笔记」。
只有声明清单系统性偏窄且计划审阅没能拦住，或结构化账本不足以让增量评审收敛时，才重新评估
这些更重的机制。

### 验收范围口径：地板 + 梯子 + 棘轮（2026-09-13）

第一轮修正曾把 verify skill 改成「跑项目完整检查集，只多不少」。它与 `system/verification.txt`
的「先窄后宽、无项目证据不默认全 workspace」直接矛盾，而且没有打中真正的漏洞：计划正文的
验收段是散文，运行时读不懂，所以计划写了三套检查、执行只跑四条窄测试也照样放行。7 个参考
实现（codex `AGENTS.md:66-68`、opencode `session/prompt/default.txt:75`、pi `AGENTS.md:31-33`、
cc-fork-01 `constants/prompts.ts:240` 等）没有一个要求默认全量；口径都是「项目 / 用户声明的
命令必跑 + 从改动处向外扩」。现口径：

```text
PLAN 期                                    EXEC 期 [gate] Acceptance
frontmatter.acceptance_commands            ① 地板：声明清单逐条跑，一条一后台任务，命令原样
  = 逐条可运行命令（机器可读）                    运行时第五项核验：每条声明都有对应绿任务，缺一拒绝
plan reviewer 核对：存在？可运行？           ② 梯子：按实际 diff 的影响半径决定要不要往上加
  配得上 todos 触及的范围？                       L0 改动文件的测试 → L1 所属包测试+lint →
用户批准计划                                     L2 依赖它的包 → L3 项目全量检查集
                                           ③ 棘轮：说不清停在哪一级 → 上一级；executing 后清单只增不减
```

政策只写在 `assets/skills/verify/SKILL.md` 第 2 步；`system/verification.txt`、`plan/planner.txt`、
`plan/executor.txt`、`update_plan` 的 acceptance 提示与 plan reviewer brief 只导航到它。
`prompts/tests/load_test.rs::acceptance_scope_policy_lives_only_in_the_verify_skill` 是机械守卫：
「complete check set / scope proportional / Scale the checks / every project check / full acceptance
suite」在模板或 `NextAction::instruction()` 复活即红，梯子关键词只允许出现在 skill。code reviewer
brief 不承担清单核对（用户决定：它只管代码质量）。

声明为空的老计划沿用旧行为并写 `plan.acceptance.commands_undeclared` 事件。若审计显示新计划
普遍不声明，或声明系统性偏窄而 plan reviewer 与用户都没拦住，应在 `build_plan` 对代码计划强制
非空，或按 workspace 依赖图机器化 L1/L2（见计划文档 7.4 推翻条件）。

```text
代码修改发生在 Acceptance 期间
  ├─ review rounds 尚有余额             → review 与 acceptance 都重新变 stale
  ├─ review 已耗尽、green build 尚未通过 → 保持 review 放行和 residual findings
  │                                        → 刷新 review pass 时间、记录 unreviewed_edit
  │                                        → 只要求新的 Acceptance 绿构建证据
  └─ review 已耗尽、旧绿构建已通过       → 只使 acceptance stale
```

这不是跳过验证：预算耗尽后再次打开 review 只会再次耗尽，不能增加审查信息；Acceptance
仍须用最新代码之后启动且零退出的后台任务证明全部改动。`plan.code_review.unreviewed_edit`
是审计事件，残余 P0/P1 finding 不会被清除。若后续发现预算耗尽后的修改常引入本可由 review
发现的问题，应降低 review 预算耗尽放行的范围或提高预算，而不是把旧 finding 静默删除。

### 备案：声明命令持续失败的非进展预算（暂不启用）

当前不对 acceptance 命令设固定重试次数：大型测试集可能需要多轮有意义的修复，固定三次会错误
打断进展。已备案的启用条件是：真实 transcript 出现同一条声明命令连续至少五次失败、仍未交还，
或用户反馈验收阶段空转。届时只对**非进展**收费：同一命令连续三次尝试中，相邻两次没有代码写入，
或虽有写入但失败签名相同（退出码加 stderr 尾部哈希），才写 `plan.acceptance.stuck` 并让
`NextAction` 交还用户。它不新增 frontmatter 字段，可由任务账本、代码 mtime 和进程内观察表得出。
同类先例：`opencode/packages/opencode/src/session/processor.ts::DOOM_LOOP_THRESHOLD` 与
`cc-fork-01/src/utils/hooks.ts::stop_hook_active` 都以“重复而无新信息”为停止条件；这里尚未
启用，是因为 acceptance 命令的单次成本远高于普通工具调用。

PlanFile 的每个 work todo 另有 `evidence: string[]`。EXEC 中 `content` 是已批准的工作描述，
已有 work todo 不可由 upsert 改写；完成时用 `set_status.evidence` 记录命令、输出或交付路径。
未带 evidence 为兼容性保留，但返回 warning；evidence 持久化在 frontmatter 并随 `update_plan`
返回，面板的 Todos Board 不渲染它。

> 状态：已实现（以当前工作树为准）；本文是 EXEC 代码变更收口的权威技术设计。
> 范围：`PlanRuntime`、`update_plan`、code reviewer、内置 `verify` skill、后台 bash 任务账本。
> 规范：[ARCHITECTURE_SPEC.md](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。相邻设计：[plan-runtime.md](plan-runtime.md)、[tools/reviewer.md](tools/reviewer.md)、[delivery-accuracy-and-completeness.md](delivery-accuracy-and-completeness.md)。
> 本文 `## 1`–`## 10` 分别对应 ARCHITECTURE_SPEC §1–§10。

## 摘要

代码变更计划的完成不是 todo 全部勾选就结束：运行时在已绑定 workspace root 时先从 Git 变更路径识别是否有代码 diff；有代码时，必须针对当前变更取得一次 P0/P1 code review 通过，以及一次可复核的绿构建证据。绿构建证据不是模型的文字声明，而是 `BashTaskRegistry` 中一个命令完全匹配、`exit_code=0`、且启动时间不早于最新代码修改时间的后台任务。

本文取代旧版“独立 Verifier 子 Agent + `verify_gate=soft|gate` 收口”的实现口径。`plan_runtime::verify` 与 `plan.verify` transcript 资产仍可单独调用或回放，但 `update_plan` 的完成链路不再派发它；当前验收由受管的 `verify` skill 与运行时证据校验完成。

**说人话**：代码计划交卷前要有两张可核对的凭据：第二双眼睛没发现 P0/P1，以及确实跑过、而且是在最后一次改代码之后跑成功的验收命令。只说“应该能过”不算。

---

## 文首导读：方案导图集

阅读顺序建议：

1. **A.1 抽象总图**：先看完成门禁依赖哪些事实，而不依赖主 Agent 的自述。
2. **A.2 具体总图**：再看 `update_plan`、持久化 frontmatter、reviewer、skill 和 bash 任务账本如何连接。
3. **B. 状态机**：最后看“全部 todo 完成”后为何还可能停留在 EXEC。

### A.1 抽象 ASCII 总图

收口判断只在执行中计划的 todos 全部完成、且本次 `update_plan` 实际推动了收口时发生。已绑定 workspace root 时，代码路径、review 结果、任务账本和文件时间戳是事实源；模型消息不是事实源。

```text
todos 全部 completed
        │
        ▼
┌─────────────────────────────────────────────────────────────┐
│ 代码范围识别：Git 变更路径 + 代码扩展名过滤                    │
│ 事实源：tracked diff 与未跟踪文件；非代码路径不进代码门禁        │
└───────────────┬───────────────────────────────┬─────────────┘
                │无代码 diff                     │有代码 diff
                ▼                                ▼
          completed                      ┌───────────────────────┐
                                         │ 读取持久化门禁状态       │
                                         │ review pass + build pass│
                                         │ 与最新 mtime 比较        │
                                         └───────┬─────────────────┘
                                                 │
            ┌────────────────────────────────────┼─────────────────────────────┐
            │两道门都新鲜                         │review 新鲜、build 未过      │review 未新鲜
            ▼                                    ▼                             ▼
       completed                         verify skill → 后台 bash       read-only code review
                                         → 账本证据核验                   ├─ P0/P1 → EXEC
                                         → completed                      └─ pass → build gate
                                                                              │
                                                                              ▼
                                                                   P1 可书面申辩；P0 必须修
```

**说人话**：没有代码变更时，不凭空要求编译。改了代码时，只有“审过”和“跑过”都覆盖最后一次修改才放行；任何一张凭据旧了，都必须补齐。

### A.2 具体 ASCII 总图

`update_plan::execute_for_tool` 是唯一完成入口。它调用 `code_reviewer::collect_code_diff_context`，并把最终的门禁状态写进同一个 PlanFile frontmatter；`BashTaskRegistry` 才持有命令真实执行的结果。

```text
update_plan(set_status=completed / green_build_pass=true)
        │
        ▼
tools/plan_tool/update_plan.rs::execute_for_tool
        │
        ├─ code_reviewer::collect_code_diff_context(workspace_root)
        │     └─ git diff --name-only HEAD + untracked
        │        → is_code_path → newest_edit_mtime_ms
        │
        ├─ PlanFileFrontmatter（持久化的事实快照）
        │     ├─ code_review_pass / code_review_pass_at_ms
        │     ├─ green_build_pass / green_build_evidence[]
        │     └─ completion_gate_cycles
        │
        ├─ PlanRuntime::dispatch_code_reviewer
        │     └─ P0/P1 机器分级，P1 可 dispute_findings(wontfix)
        │
        └─ require_green_build_pass
              ├─ load_skill(name="verify") 的流程指导
              ├─ BashTaskRegistry::get_info(task_id)
              ├─ exact command + Finished(exit_code=0)
              └─ started_at_unix_ms >= newest_edit_mtime_ms
                       │
                       ▼
                 finalize_plan_completed
```

**说人话**：skill 负责教 Agent 怎样找到合适的检查并把它后台运行；真正决定证据能否用的是运行时从任务账本取回的记录，而不是 skill 的文字输出。

### B. 状态机

`code_reviewing` 和 `green_build_pending` 是由 frontmatter 字段与本次调用推导出的阶段，不是额外写进磁盘的 `PlanFileState`；磁盘状态仍只有 `executing` 或 `completed`。

```text
┌───────────┐ all todos completed  ┌─────────────────────┐ review pass ┌─────────────────────┐
│ executing │─────────────────────▶│ code_reviewing       │─────────────▶│ green_build_pending │
└─────▲─────┘                      └─────┬───────────────┘              └─────────┬───────────┘
      │                                  │ P0/P1 / review failure                    │ verified evidence
      │                                  ▼                                            ▼
      │                            ┌───────────┐                              ┌───────────┐
      └──── reopen/add fix todo ───│ executing │                              │ completed │
                                   └───────────┘                              └───────────┘
```

| 当前状态 | 事件 / 条件 | 目标状态 | 副作用 | 说人话 |
|----------|-------------|----------|--------|--------|
| `executing` | workspace root 已绑定且没有代码 diff | `completed` | 写 `plan.complete` | 文档、配置或计划类交付不被代码门禁误挡。 |
| `executing` | 当前代码的两个持久化门禁都新鲜 | `completed` | 不重复运行 review/build | 同一份代码不反复验。 |
| `executing` | review 未通过或有未裁决 P0/P1，且预算未尽 | `executing` | 写 review 结果，主 Agent 重开或新增修复 todo | 真问题先修，仍有复审预算就继续。 |
| `executing` | review 预算耗尽且残余含 P0 | `executing` | review gate 保持 pending，写 handoff，等待新的用户消息刷新预算 | P0 不能由无人值守流程替用户接受。 |
| `executing` | review 预算耗尽且仅剩 P1 | `executing` | 完成 review gate、持久化残余 finding，进入 green-build acceptance | P1 可作为明确风险继续由客观验收收口。 |
| `executing` | review 新鲜但没有新鲜绿构建证据 | `executing` | `BadArgs` 指引加载 `verify` skill | 审过不等于跑过。 |
| `executing` | 合格后台任务证据已提交 | `completed` | 持久化 `green_build_*` 后完成 | 账本确认命令真成功才放行。 |
| `executing` | 已完整通过后又改代码，重验周期达到上限 | `completed` + warning | 保留失效状态并记录“沿用最后一次通过结果” | 这是防止无尽重验的明确逃逸，不是新的绿构建通过。 |

---

## 1. 术语统一

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| **代码 diff** | 当前工作区相对 `HEAD` 的已跟踪变更加未跟踪文件中，扩展名属于代码集合的路径。 | `CodeDiffContext.changed_code_files` | 过滤 `.rs`、`.ts`、`.tsx`、`.js`、`.py`、`.go`、`.java`、`.sh`、`.sql`、`.vue` 等；删除文件没有 mtime 时用当前时间作保守下界。 | 只有真改代码才触发代码验收。 |
| **review gate 放行** | reviewer 无未裁决 P0/P1，或预算耗尽且仅剩 P1。 | `code_review_pass`、`code_review_pass_at_ms`、`code_review_residual_findings[]`、`code_review_handoff` | 预算内 P0/P1 阻塞；耗尽后 P0 交还用户，P1 才可携残余进 acceptance。 | P0 不能静默带病放行。 |
| **绿构建通过** | 至少一条由运行时核实的后台验收任务覆盖当前代码，且计划声明的每条验收命令都有对应任务。 | `green_build_pass`、`green_build_evidence[]`、`acceptance_commands[]` | 每条证据要有精确命令、任务 ID、启动时间和零退出码；声明清单按 trim + 折叠空白后整串比对，可多不可少。 | 不是“我跑过了”，而是能查到哪条命令什么时候成功，而且计划承诺要跑的一条都不少。 |
| **声明的验收地板** | 计划在 PLAN 期写进 frontmatter 的验收命令清单。 | `acceptance_commands: string[]` | planning / pending 可整体替换；executing 后只许追加（棘轮）；空表示未声明。 | 计划阶段就把「验收要跑什么」写成机器能对账的清单。 |
| **新鲜（fresh）** | review 或验收发生在当前代码最新修改之后。 | `code_review_pass_at_ms`、`GreenBuildEvidence.started_at_ms`、`newest_edit_mtime_ms` | 通过时间必须 `>=` 最新 mtime；代码再改会清空两项 gate。 | 改完代码后，旧绿灯自动失效。 |
| **P1 申辩** | 主 Agent 把某个未决 P1 作为已接受取舍，而非声称已修。 | `dispute_findings[{ref,area,resolution:"wontfix",reason}]` 与 runtime disputed findings | P0 不可申辩；P2 不阻塞也无需申辩；修复必须改代码后复审。 | P1 可以说明“为什么不改”，P0 必须修。 |
| **完成周期** | 已完整通过过一次后，又因代码修改而重新执行的 review → build 门禁次数。 | `completion_gate_cycles` | 仅在“此前 review+build 都通过”的重验中，review 再次通过时加一；`max_completion_gate_cycles` 最小为 1。 | 记录同一计划被反复改、反复验了几轮。 |

“收口”在本文特指：`update_plan` 使执行中计划的 todos 全部完成后，`execute_for_tool` 开始计算 `derived_completed` 的那次调用；它不等同于模型产生一段“已完成”的普通文本。

---

## 2. 竞品 / 选型对比（调研）

只保留仍能解释当前实现的同行证据：**验收必须有实际命令与输出，而不是阅读代码后的口头 PASS**。旧版有关“另起 verifier 子 Agent、四态 `VerifySummary`、soft/gate 配置决定收口”的调研不再用于当前完成路径，故不在本文作为设计依据。

| 参考实现 | 相关证据 | 保留的原则 | 未沿用的部分 | 说人话 |
|----------|----------|------------|--------------|--------|
| `cc-fork-01` | `cc-fork-01/src/tools/AgentTool/built-in/verificationAgent.ts::VERIFICATION_SYSTEM_PROMPT`：每项 PASS 要求 `Command run` 与 `Output observed`，并禁止修改项目。 | PASS 必须建立在实际命令结果上。 | 不在 `update_plan` 内派发 cc-fork 式独立 verifier；Tomcat 用受管 skill + 任务账本做确定性核验。 | 保留“跑过才算过”，不复制另一套子 Agent 生命周期。 |
| `cc-fork-01` | 同文件 `disallowedTools` 与 `background: true`。 | 验收执行者不应借验证名义改项目；长命令应与主循环解耦。 | Tomcat 不接受 verifier 文本 verdict 作为放行条件，而要求当前会话的 `BashTaskRegistry` 任务。 | 命令能后台跑，但凭据仍要由运行时验真。 |

选择“持久化 review/build 状态 + 任务账本证据”而不是恢复旧 verifier 链路，原因是：

1. **门禁可重启核对**：PlanFile 保存通过标志、时间与任务快照，恢复会话仍知道哪次验收覆盖了哪份代码。
2. **证据不可由文本伪造**：`task_id` 反查运行时任务，命令、退出码与启动时间逐项比对。
3. **边界更清楚**：code reviewer 负责找 P0/P1；skill 负责发现/运行检查；`update_plan` 负责最终准入。
4. **不重启过时设计**：`verify.rs` 的旧 Subagent/`VerifySummary` 资产不再是完成链路的事实源。

---

## 3. 落地选型与实施（已定稿）

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|---------------|--------|
| R1 代码范围 | 哪些计划必须经过代码门禁？ | **采用** 已绑定 workspace root 的 Git 路径清单加扩展名过滤和 mtime；**拒绝** 任何 todo 完成即强制构建。 | Tomcat `plan_runtime/code_reviewer.rs::collect_code_diff_context`、`is_code_path`；cc-fork-01 `verificationAgent.ts` “files changed”输入。 | 设计：`git diff --name-only HEAD` 与未跟踪文件合并后筛代码路径。理由：只让实际代码变化承担 review/build 成本，且删除也会使旧证据失效。 | **未入选**：从 todo 内容、`TodoKind` 或模型声明推测代码变更。**拒因**：它们不是工作树事实，且 `TodoKind` 不存在于当前 schema。 | 看 Git 实际变了什么，不看模型把 todo 起了什么名字。 |
| R2 绿构建凭据 | 怎样证明验收命令真的跑过并覆盖最后一次编辑？ | **采用** `BashTaskRegistry` 的后台任务作为唯一凭据；**拒绝** verifier 文本、人工字符串或 cargo-check 专用记录。 | Tomcat `update_plan.rs::require_green_build_pass`、`file_store.rs::GreenBuildEvidence`；cc-fork-01 `verificationAgent.ts` 的 Command/Output 要求。 | 设计：提交 `{command, task_id}`，运行时核验命令、`Finished(exit_code=0)`、freshness 后写入快照。理由：命令执行事实无法由主 Agent 的自然语言伪造。 | **未入选**：旧版 `VerifySummary` 或 `cargo check` 自动运行。**拒因**：前者已不在收口链路，后者不能覆盖项目特定 build/test/UI smoke。 | 让运行时查后台任务的收据，而不是相信一句“测试通过”。 |
| R3 代码审查与申辩 | 什么 finding 阻塞，又如何处理有意识的取舍？ | **采用** 预算内 P0/P1 阻塞、P2 advisory；预算耗尽时 P0 handoff、仅 P1 可进 acceptance。P1 可 `wontfix`，P0 只在用户确认 handoff 后可带理由接受。 | Tomcat `update_plan.rs::{blocking_findings,prepare_disputes}`、`PlanRuntime::refresh_code_review_budget_after_user_message`；`review.rs::Finding::tier`。 | 预算是无人值守的边界，不是替用户接受 P0 的授权；新用户消息才会重开一段预算，open findings 继续随 reviewer brief 传递。 | **未入选**：P0/P1 一律自动通过或所有 finding 永久阻塞。**拒因**：前者越权，后者会死锁。 | 评审跑满不等于 P0 消失；业主回话才可继续。 |
| R4 循环控制 | 代码反复修改导致验收不断失效时怎样避免无穷循环？ | **采用** 持久化 `completion_gate_cycles` 上限（默认 3，最小 1）；**拒绝**无上限重验。 | Tomcat `file_store.rs::PlanFileFrontmatter`、`update_plan.rs::prior_gate_cycles_exhausted`；`infra/config/types/runtime.rs::PlanConfig`；cc-fork-01 `verificationAgent.ts` 对真实命令优先而非无限叙述的约束。 | 设计：先前已完整通过、随后代码变更且重验次数达到上限时，写 warning 后完成。理由：把逃逸显式、可观察，避免 Agent 在同一门禁上空转。 | **未入选**：把上限当作“自动绿灯”，或删除上限。**拒因**：前者误报验收成功，后者不能终止循环。 | 到上限是明确交付取舍：会提示沿用旧结果，但不会把它伪装成新验证。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| P1 代码范围与持久化门禁 | 代码路径筛选、mtime 新鲜度、PlanFile gate 字段与重验周期。 | `plan_runtime/code_reviewer.rs::{collect_code_diff_context,is_code_path}`；`plan_runtime/file_store.rs::{PlanFileFrontmatter,GreenBuildEvidence}`；`tools/plan_tool/update_plan.rs::{code_gates_are_fresh,invalidate_code_gates}`。 | `code_review_pass_completes_without_verifier`；`green_build_gate_blocks_completion_until_pass`。 | 每次最后收口都比较当前代码和已存凭据。 |
| P2 P0/P1 review 门禁 | read-only code review、finding 机器分级、P1 申辩、P0 handoff、跨轮状态落盘。 | `plan_runtime/{file_store.rs,code_reviewer.rs}`；`update_plan.rs::{blocking_findings,prepare_disputes}`。 | `next_action_is_the_single_close_out_decision_source`；P0/P1 预算边界回归。 | P0 交还用户；P1 才可带清单验收；重启后 reviewer 仍知道上一轮的问题与 DELTA。 |
| P3 受管 verify skill | 启动后物化 `verify/SKILL.md`；P0–P5 发现顺序；只允许 bash 与后台任务工具。 | `skill/builtin.rs::materialize_builtin_skills`；`assets/skills/verify/SKILL.md`。 | `plan_runtime::tests::verifier_can_expose_load_skill_when_config_enabled`（skill 暴露策略）；收口入口由 P1/P4 锁定。 | 不把命令写死在 Rust；skill 教模型按项目事实找检查。 |
| P4 账本证据准入 | 精确命令、唯一 task ID、完成零退出码、开始时间新鲜度；合格快照写入 plan。 | `tools/plan_tool/update_plan.rs::require_green_build_pass`；`tools/primitive::BashTaskRegistry`。 | `green_build_gate_blocks_completion_until_pass`。 | 后台任务的真实记录才是绿构建凭据。 |
| P5 收束与循环上限 | 文本收束 guard、review 预算、基础设施重试、重验周期上限和 warning。 | `agent_loop/turn_finalize.rs::completion_guard_instruction`；`infra/config/types/runtime.rs::PlanConfig`；`tools/plan_tool/update_plan.rs::prior_gate_cycles_exhausted`。 | `final_review_round_immediately_hands_off_with_p0_residual`、`final_review_round_immediately_advances_to_acceptance_with_p1_residual`；周期上限直接测试：PENDING。 | 不让模型在未验收时只写总结离开，也不让它无限重跑。 |

---

## 4. 协议（入参 / 出参 / Schema）

单一事实源是 `plan_runtime/file_store.rs::PlanFileFrontmatter`。`update_plan` 的对外参数定义在 `tools/plan_tool/update_plan.rs::UpdatePlanArgs`，schema 由 `tools/contract/catalog.rs` 导出。

### 4.1 PlanFile 持久化字段

| 字段 | YAML 类型 | 必填 | 默认值 | 约束 | 说人话 |
|------|-----------|------|--------|------|--------|
| `code_review_pass` | `boolean` | 否 | `false` | 代码改动比 `code_review_pass_at_ms` 新时清为 `false`；可由无 finding review 或预算耗尽放行置真。 | 当前代码是否已获 review gate 放行。 |
| `code_review_pass_at_ms` | `integer \| null` | 否 | `null` | review gate 放行时写入 Unix 毫秒；必须不早于最新代码 mtime。 | review 放行的时间戳。 |
| `code_review_residual_findings` | `string[]` | 否 | `[]` | 仅预算耗尽放行时保存残余 P0/P1 摘要；任一后续代码编辑会清空。 | 评审没来得及消掉的问题清单。 |
| `green_build_pass` | `boolean` | 否 | `false` | 新代码会清为 `false`；不是模型可自由宣称的结果。 | 当前代码是否真跑过验收。 |
| `green_build_evidence` | `GreenBuildEvidence[]` | 否 | `[]` | 需至少一项 `started_at_ms >= newest_edit_mtime_ms`。 | 留下验收收据。 |
| `completion_gate_cycles` | `integer` | 否 | `0` | 已完整通过后重新 review 成功才递增。 | 已经重验过几轮。 |
| `acceptance_commands` | `string[]` | 否 | `[]`（空不落盘） | 写入时 trim、折叠空白、去空、去重；executing / completed 状态下新列表必须包含旧列表每一项。 | 计划承诺的验收命令清单。 |

```text
GreenBuildEvidence
├─ command: String         # BashTaskRegistry 记录的精确命令
├─ task_id: String         # 当前运行时后台任务 ID
├─ started_at_ms: u128     # 启动时间，用于 freshness
└─ exit_code: i32          # 必须为 0
```

### 4.2 `update_plan` 收口参数

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `dispute_findings` | `array` | 否 | `[]` | 接受 P1 取舍时 | 每项必须有 `ref`、`area`、`resolution:"wontfix"`、非空 `reason`。 | 对 P1 说明为什么故意不修。 |
| `green_build_pass` | `boolean` | 否 | 缺省 | verify skill 的命令都通过后 | 只有 `true` 才进入证据核验；`false` 会清已有 green-build 结果。 | 成功后才申请放行。 |
| `green_build_evidence` | `array` | `green_build_pass=true` 时是 | `[]` | 同上 | `{command,task_id}`；命令须与账本完全一致，任务 ID 不得重复；声明的每条 `acceptance_commands` 都要在此出现。 | 把后台任务收据交给运行时。 |
| `acceptance_commands` | `string[]` | 否 | 缺省（不改动） | PLAN 期修订清单；EXEC 期扩大范围 | 传完整期望列表。planning / pending 整体替换；executing 下缺少任一旧项 → BadArgs 列出缺失命令。`create_plan` 同名字段用于首次声明。 | 改验收清单只能越改越宽。 |

```jsonc
// review 已通过后，提交当前代码的新鲜后台任务证据
{
  "ops": [],
  "green_build_pass": true,
  "green_build_evidence": [{
    "command": "cargo test -p tomcat --lib plan_runtime",
    "task_id": "bash-task-123"
  }]
}
// 成功：plan_state_after = "completed"
// 失败：task 不存在、未结束、exit_code 非 0、命令不一致或早于最新 edit
//       → BadArgs，plan 保持 executing
```

### 4.3 证据核验顺序

`require_green_build_pass` 对每一项 evidence 依次拒绝：空命令、重复 ID、账本无此任务、任务未 `Finished`、非零退出码、命令不一致、或任务早于最新代码编辑。全部条目通过后做第五项对账：`acceptance_commands` 的每一条都要有一条已核验证据与之 `normalize_acceptance_command`（trim + 折叠空白）后相等，缺项 → BadArgs 列出缺失命令，不写 `green_build_pass`；声明为空 → 写 `plan.acceptance.commands_undeclared` 事件后放行。之后才写 `green_build_pass=true` 与快照，`plan.green_build` 事件带 `declared_acceptance_commands`。

**说人话**：不是拿一张任务 ID 就够；它必须是这次会话中那条相同命令、跑完且成功、跑得比最后一次编辑晚；而且计划里承诺要跑的每一条都得在收据里，多跑可以，用一条更窄的命令顶替不行。

---

## 5. 文件职责总览（One-Glance Map）

```text
tomcat/src/core/
├─ tools/plan_tool/update_plan.rs
│    ├─ execute_for_tool: 收口编排与 completed 决策
│    ├─ code_gates_are_fresh / invalidate_code_gates: mtime 门禁
│    ├─ prepare_disputes / blocking_findings: P0/P1 与 P1 申辩
│    └─ require_green_build_pass: BashTaskRegistry 证据准入
│                   │
│                   ▼
├─ plan_runtime/file_store.rs
│    └─ PlanFileFrontmatter{code_review_pass, code_review_residual_findings,
│         green_build_pass, green_build_evidence, completion_gate_cycles}
│                   │
│       ┌───────────┴─────────────┐
│       ▼                         ▼
├─ plan_runtime/code_reviewer.rs  ├─ skill/builtin.rs + assets/skills/verify/**
│    ├─ collect_code_diff_context │    └─ 物化受管 verify skill；指导发现/后台验收
│    └─ build_code_review_prompt  │
│       │                         ▼
│       ▼                    tools/primitive::BashTaskRegistry
├─ plan_runtime/review.rs          └─ 任务实际 command/status/exit/start time
│    └─ Finding::tier / blocks
│
├─ agent_loop/turn_finalize.rs
│    └─ completion_guard_instruction: 未绿构建时强制继续执行
│
└─ infra/config/types/runtime.rs
     └─ [plan] max_code_review_rounds / max_completion_gate_cycles

配套测试：tools/plan_tool/tests/code_review_test.rs
```

专业上，`update_plan.rs` 是准入裁判，`file_store.rs` 是可恢复的门禁记忆，reviewer 和 skill 分别提供审查与命令发现，任务账本提供不可伪造的执行事实。

**说人话**：一条线负责决定能不能完成，一条线保存上次验过什么，reviewer 负责找错，skill 负责知道怎么测，任务账本负责证明测过。

---

## 6. 配置与环境变量

当前门禁的配置优先级是**配置文件 > 默认值**；这两个键没有环境变量覆盖。`PlanRuntime::set_max_completion_gate_cycles` 会把传入值钳制为至少 `1`。

| 键 | 类型 / 默认 | 含义 | 说人话 |
|----|-------------|------|--------|
| `[plan].max_code_review_rounds` | `u32` / `4` | EXEC 收口最多派发多少次 code review；`0` 表示记录为跳过；最后一轮仅余 P1 才进入 acceptance，仍有 P0 则交还用户。 | 默认首轮找错，后续最多三轮核销；要深审可显式提高。 |
| `[plan].max_completion_gate_cycles` | `u32` / `3` | 已完整通过后又改代码时，最多重跑几轮 review → build。 | 防止同一个计划无限重验。 |

`[plan].verify_gate` 及 `PlanRuntime::dispatch_verifier` 是旧 verifier 资产的配置/API；当前 `update_plan` 不读取它来决定完成，不能把它当作关闭或开启本设计的开关。

---

## 7. 错误模型 / 截断 / 警告

```text
workspace root 已绑定且代码路径为空
  → 无代码门禁，完成

有代码 diff + review 尚未通过
  → 保持 executing；P0/P1 或 reviewer 状态写入 code_review 结果

review 已通过 + 未提交绿构建凭据
  → Err(BadArgs)：指导 load_skill("verify")、后台 bash、提交 task_id

提交的凭据不真实或不新鲜
  → Err(BadArgs)：保持 executing，不写 green_build_pass

真实且新鲜的凭据
  → 持久化 evidence，completed

review 技术故障连续超过两次
  → 保持 executing，写 transcript handoff，交还用户决定

review 最后一轮结束
  ├─ 仅剩 P1 → `code_review_pass=true`，残余 finding 写 frontmatter/transcript，进入 acceptance
  └─ 仍有 P0 → review gate 保持 pending，写 handoff 事件并交还用户

此前已完整通过 + 重验周期达到上限
  → completed + warning（不把已失效凭据改写成新绿灯）
```

没有 workspace root 时，`code_gate_required=false`，因此不会要求绿构建证据；当前代码仍可依 review 路径收口。生产路径应绑定 workspace root，才能让代码路径过滤和 freshness 完整生效。

`turn_finalize::completion_guard_instruction` 还会在根 Agent 试图用纯文本结束、且已 review 通过但绿构建未通过时，注入继续指令，要求加载 skill、运行后台命令、提交 `task_id`。注入本身另有 `MAX_COMPLETION_GUARD_INJECTIONS=8` 上限，避免文本循环。

**说人话**：基础设施失败和预算耗尽后的 P0 都会明确交还；只有预算耗尽后仅剩 P1 才转入客观验收，仍必须有新鲜绿构建凭据。只有“已完整验过又反复修改到周期上限”会带 warning 完成，而且不会假装那是新一轮绿构建。

---

## 8. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元 / 收口 | `tools::plan_tool::tests::code_review_test::code_review_pass_completes_without_verifier` | ✅ 当前工作树 | code review 通过后不会调旧 verifier。 |
| 单元 / P0/P1 | `tools::plan_tool::tests::code_review_test::{final_review_round_immediately_hands_off_with_p0_residual,final_review_round_immediately_advances_to_acceptance_with_p1_residual}` | ✅ 当前工作树 | P0/P1 在预算内阻塞；最后一轮结束即 P0 handoff、P1 才带着残余清单进入 acceptance。 |
| 单元 / 绿构建 | `tools::plan_tool::tests::code_review_test::green_build_gate_blocks_completion_until_pass` | ✅ 当前工作树 | 没有任务证据不能完成；合格任务才能完成。 |
| 单元 / review 失败 | `tools::plan_tool::tests::code_review_test::resolved_findings_converge_to_completion_within_review_budget` | ✅ 当前工作树 | 预算内 finding 先回主 Agent 修复并在下一轮核销；预算耗尽时 P0 不会静默完成，P1 才可进入 acceptance。 |
| 单元 / 审查恢复 | `tools::plan_tool::tests::code_review_test::second_review_round_receives_previous_open_findings_and_clears_fixed_ones` | ✅ 当前工作树 | 下一轮只应看到仍未解决的问题。 |
| 单元 / verifier 遗留资产 | `tools::plan_tool::tests::verify_test::update_plan_does_not_dispatch_dormant_verifier_even_when_attached` | ✅ 当前工作树 | 旧 verifier 即使挂载也不进当前收口流。 |
| 关键承诺 / mtime 与周期上限 | 直接覆盖 `code_gates_are_fresh`、失效后重验与 `completion_gate_cycles` 的场景 | PENDING | 设计已实现，但需把这些边界单独锁成测试。 |
| 文档 | 本文与 [delivery-accuracy-and-completeness.md](delivery-accuracy-and-completeness.md) 同步 | ✅ 2026-08-10 | 架构口径不再保留旧 TodoKind/cargo-check 流程。 |

---

## 9. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| 模型伪造“测试已过” | 高 | 不接受文本证据；`require_green_build_pass` 查询 `BashTaskRegistry` 并逐项比对。 | 光说不算，账本说了算。 |
| 旧验收覆盖不了新代码 | 高 | 比较 review 时间、任务启动时间与 `newest_edit_mtime_ms`；代码变化时清空两道 gate。 | 最后改了一行也得重新验。 |
| 代码删除没有可读 mtime | 中 | `collect_code_diff_context` 用当前时间作保守下界。 | 删代码也不能沿用旧绿灯。 |
| reviewer `pass` 与 findings 矛盾 | 高 | 运行时以 `Finding::blocks()` 的 P0/P1 分级为准。 | 结论和问题打架时，看严重问题本身。 |
| P1 反复被 reviewer 重报 | 中 | 匹配 P1 内容的已申辩 finding 注入“已接受取舍”段，禁止重报。 | 已明确接受的取舍不用每轮吵。 |
| review 或 guard 无限循环 | 中 | review 轮次、2 次基础设施重试、`completion_gate_cycles` 和 8 次文本 guard 都有上限。 | 每种循环都有能停下来的出口。 |
| 周期上限被误解为“验收通过” | 高 | 只写 warning 后完成；不恢复 `green_build_pass` 或证据，把该行为在 transcript/结果中显式暴露。 | 到上限是带风险交付，不是假绿灯。 |

---

## 10. 历史决策 / 跨文档修订

- ~~`TodoKind{Task,Research}`、research URL/`file:line` evidence 是完成门禁的一部分~~ → **否**：当前 `TodoItem` 只有 `id/content/status`，没有 `TodoKind` 或 research evidence；它们不属于现行代码验收路径。
- ~~收口时运行 `cargo check` / `tsc --noEmit` 并自动记为通过~~ → **否**：当前门禁不固定命令。`verify` skill 按项目文档、manifest、CI 与变更范围发现 build/test/lint/UI smoke，再由后台任务账本核验。
- ~~`update_plan` 在 code review 后派发 internal verifier，并由 `[plan].verify_gate` 决定是否收口~~ → **否**：`update_plan_does_not_dispatch_dormant_verifier_even_when_attached` 锁定当前行为；旧 verifier 代码保留但不做完成事实源。
- ~~review 预算耗尽即可 best-effort completed~~ → **否**：预算耗尽后的残余 P0 保持 `executing` 并交还用户；只有残余仅 P1 才可进入 acceptance。唯一完成逃逸是“此前完整通过后、代码又变且重验周期到顶”，并且必须携带 warning。
- **跨文档修订**：`delivery-accuracy-and-completeness.md` 只保留本设计在交付可靠性总图中的定位；字段、顺序、协议和验收真相均以本文为准。

---

## 一句话总结

当前代码验证是一个可恢复、可核验的硬收口门禁：代码变更必须先过 P0/P1 review，再由 `verify` skill 驱动后台命令，并让运行时以任务 ID、精确命令、零退出码和 mtime 新鲜度核验绿构建；只有明确的重验周期上限例外会带 warning 完成，绝不把旧结果伪装成新证据。
