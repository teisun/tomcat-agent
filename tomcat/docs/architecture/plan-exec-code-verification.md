# Plan / Todo 执行完成后的代码验证（Verifier）技术方案

本文档是 **T2-P1-002 | plan-mode-enhance** 的横向调研 + 落地选型方案，承接 [`plan-runtime.md`](./plan-runtime.md)、[`tools/reviewer.md`](./tools/reviewer.md)、[`tools/read.md`](./tools/read.md) 与 [`ARCHITECTURE_SPEC.md`](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。**实现以仓库代码为准**；文中 **PENDING / PARTIAL** 验收项表示仍有缺口待补。

§4.1 **「决策」**列（裁决结论）；其他表末列 **「说人话」** 与 ARCHITECTURE_SPEC **§14.1** 对齐。

**说人话**：竞品在「计划/todo 做完之后」普遍还有一层**代码验证**（跑测试、对抗性找茬、强模型二审），和 Tomcat 的 **plan 前 reviewer（顾问）** 不是一回事。当前实现已经把 EXEC 收口链路补成 `all_completed -> code review（可跳过） -> verifier -> maybe completed`；本文保留选型背景，同时把现行实现口径补齐。

> **编号对照**：本文 `## 1`–`## 14` 对应 ARCHITECTURE_SPEC 推荐骨架（术语 → 竞品 → 目标 → §4 选型 → 协议 → …）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
  - [4.1.1 决策表 Q&A（FAQ）](#411-决策表-qafaq)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. One-Glance Map](#6-one-glance-map)
- [7. 调度时序](#7-调度时序)
- [8. 状态机](#8-状态机)
- [9. 配置与环境变量](#9-配置与环境变量)
- [10. 错误模型 / 警告](#10-错误模型--警告)
- [11. 测试矩阵（验收）](#11-测试矩阵验收)
- [12. 风险与应对](#12-风险与应对)
- [13. 历史决策](#13-历史决策)
- [14. 关联文档](#14-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **Reviewer（Plan 审稿）** | `create_plan` 后读计划/仓库，给**顾问摘要** | `ReviewSummary(kind=plan)` + `transcript.plan.review` | **不进** EXEC gate；可改 plan（`allow_review_edit`） | 开工前的挑刺员，不当法官。 |
| **Code Reviewer（EXEC 代码审查）** | 全部 todo `completed` 后、Verifier 前，读 diff/代码给**只读结论** | `ReviewSummary(kind=code, verdict=...)` + `transcript.plan.code_review` | **严格只读**；`pass` 同回合直连 verifier；`aborted` 视为 reviewer 不可用并继续 verifier；只有真 findings（`fail/partial`）才回主 Agent 修 | 完工前的代码二审，只提问题不动手改。 |
| **Verifier（验证）** | code review 通过、被跳过、或 `aborted` 后，用**可复现命令**证明交付物可用 | `VerifySummary` + `transcript.plan.verify` | 默认**只读项目** + `bash` 跑检查；输出含 **Command run / Output observed** | 最后的验货员，专找「最后 20%」。 |
| **Mini 验证** | 执行单步后的快速自检 | 主 Agent prompt / SOP（无独立子 Agent） | 如 `file_read` 非空、`exit code == 0` | 每步顺手看一眼，不另开子进程。 |
| **Inline 验证** | 执行过程中穿插跑测试/构建 | 主 Agent `bash` + executor reminder | 不阻塞 `mode=completed` 除非配置 gate | 边干边测，Codex execute 模板风格。 |
| **CI / 脚本验证** | 人/流水线触发的确定性检查 | `pi_agent_rust/verify` → `scripts/e2e/run_all.sh` 等 | **非** LLM 子 Agent | 机器跑全套测试，和对话解耦。 |
| **Verdict gate** | 验证结果阻塞「任务完成」叙事 | `VERDICT: PASS\|FAIL`（GenericAgent）/ Stop hook `preventContinuation`（cc-fork） | Tomcat **默认不做** gate（对齐 reviewer 顾问原则） | 验不过就不让喊完工。 |
| **`mode=completed` 派生** | EXEC 下全部 plan todo `completed` 后 runtime 自动收口 | `update_plan` → `code review?` → `verify` → `set_mode_completed` | code review 只有真 findings（非 `pass` 且非 `aborted`）才先停留 EXEC；`verify_gate=gate` + verify fail 时也停留 EXEC | 勾全后不会立刻回 CHAT，要先过完收口链路。 |

**时间点钉死**：

- **Reviewer 触发点**：`create_plan` 落盘成功后（PLAN 阶段，**早于** `/plan build`）。
- **Code Reviewer 触发点（现行）**：最后一次 `update_plan` 使全部 todo `completed` 之后，若 `code_review_rounds < max_code_review_rounds`（默认 1）则先跑 read-only code review。
- **Verifier 触发点（现行）**：code review `pass` / `aborted` 的同一回合，或 code-review rounds 用尽后，进入 verifier；与 plan reviewer（`create_plan -> review`）**正交**。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 验证在 agent 流水线中的典型位置

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  Plan/Todo 生命周期里，「验证」常出现在三个位置                              │
├────────────────────┬───────────────────────────────────────────────────────┤
│  A. 规划后 / 开工前 │  Tomcat reviewer、codex delegate review、Advisor「开工前」│
│  B. 执行中每步      │  GenericAgent Mini验证、Codex「along the way verify」   │
│  C. 全部 todo 完成后 │  cc-fork Verification Agent、GenericAgent [VERIFY]    │
└────────────────────┴───────────────────────────────────────────────────────┘
```

**说人话**：审稿多半在 A；真正验代码多在 B+C。Tomcat 已有 A，缺 C（可选 B 靠 prompt）。

### 2.0 调研方法与仓库可及性

| 仓库 | 工作区状态 | 精读方式 | 说人话 |
|------|------------|----------|--------|
| **cc-fork-01**、**GenericAgent**、**codex** | 完整工作树 | `ls` / `find` / `rg` / `Read` | 可直接打开源码。 |
| **hermes-agent**、**openclaw**、**pi-mono**、**pi_agent_rust** | 工作区仅 `.git` | `git ls-tree` + `git show HEAD:<path>` | 无检出树，用 git 对象读 HEAD。 |

### 2.2 七仓库精读摘要（证据路径级）

| 竞品 | 验证形态 | 触发时机 | Verifier 设计要点 | Gate? | 说人话 |
|------|----------|----------|-------------------|-------|--------|
| **cc-fork-01** | 内置 **Verification Agent** + **Advisor** + **Stop Hooks** | 主 Agent / Todo 完成流 nudge spawn（见 `QUALITY_MECHANISMS.md` §5）；Advisor 在「声称完成前」 | `cc-fork-01/src/tools/AgentTool/built-in/verificationAgent.ts`：对抗性 system prompt（L10–129）、`disallowedTools` 禁 edit/write/spawn（L139–145）、`background: true`（L138）、输出 **`VERDICT: PASS\|FAIL\|PARTIAL`**（L119–127）+ 每 check 必须 **Command run / Output observed**（L81–115）；`docs/QUALITY_MECHANISMS.md` §5–§7 与源码一致 | Verification 产出 verdict，配合 Stop Hook 可 **preventContinuation**；Advisor 非 gate | 专门找茬子 Agent + 强模型二审 + 脚本 gate 循环。 |
| **codex** | Collaboration **execute** 模板（沿途验证）+ **Guardian AutoReview**（权限审批）+ 内部 one-shot 线程 | Execute：**执行中**分步验证（`collaboration-mode-templates/templates/execute.md` L34–35）；Guardian：**bash/危险操作审批前**，非 plan todo 完工验货 | `codex-rs/collaboration-mode-templates/templates/execute.md`；`codex-rs/core/src/guardian/review.rs` + `review_session.rs`（`run_codex_thread_interactive` 嵌套会话）；plan 项见 `app-server/tests/suite/v2/plan_item.rs`。**注意**：Guardian ≠ 交付物 verifier | Execute 靠 prompt；Guardian gate **审批**；无与 Tomcat `all_completed` 同构的 LLM verifier | 沿途自测 + 审批二审；完工验货主要靠模型自觉与 CI。 |
| **GenericAgent** | `plan_sop.md` **[VERIFY]** + `verify_sop.md` + **runtime 拦截** | plan 模式全部 `[✓]` 后；**代码 gate**：`ga.py` 拦截「声称完成」 | `GenericAgent/memory/plan_sop.md` §107–111、§175–241；`memory/verify_sop.md`（铁律：无工具输出 PASS→无效）；`GenericAgent/ga.py` L459–462：无 `VERDICT`/`[VERIFY]` 则注入「验证拦截」 | **是**（SOP + handler 双保险） | 计划里写死验货步 + Python 层硬拦完工声明。 |
| **hermes-agent** | `delegate_task` 通用子 Agent；**curator ≠ 代码 verifier** | `delegate_task` 由 LLM 决定时机；curator 为**空闲时技能库维护**（`agent/curator.py` 模块 docstring） | `git show HEAD:agent/curator.py`（技能 pin/archive，非跑测试）；`delegate_task` 见 `agent/tool_guardrails.py` / `RELEASE_v0.11.0.md`（orchestrator 深度可配） | 通常非硬 gate | 子 Agent 靠 role；curator 管技能不验交付物。 |
| **openclaw** | **`sessions_spawn`** 子会话 + skills（如 code-review） | **LLM 自调** spawn；无 plan-runtime 级 `all_completed` 钩子（本调研未在 HEAD 发现等价模块） | CHANGELOG / `skills/software-development/requesting-code-review`（技能驱动）；spawn 参数含 `model`/`runtime`/`lightContext` 等 | 否（默认） | 模型自己叫子 agent，边界靠配置与 skill。 |
| **pi-mono** | **subagent 扩展**：Single / Parallel / **Chain** | 用户 slash / 模板触发；`implement-and-review` 为 **worker→reviewer→worker** 链 | `git show HEAD:packages/coding-agent/examples/extensions/subagent/index.ts`（spawn 独立 `pi` 进程）；`prompts/implement-and-review.md`；`agents/reviewer.md`（tools: read,grep,find,ls,bash 只读） | 否 | 实现-审稿-改稿链；无 plan todo 状态机绑定。 |
| **pi_agent_rust** | **`./verify` → `scripts/e2e/run_all.sh`** | 人 / CI / `CODEX_THREAD_ID` 多 agent 隔离 target | `git show HEAD:verify`（exec 转发 run_all.sh；profile quick/ci/full） | **脚本 exit code** | 工程级全量验证，不进对话 plan。 |

### 2.2.1 机制对照（源码摘录级）

#### cc-fork-01：Verification Agent（C 层完工验货标杆）

- **角色定义**：`verificationAgent.ts` L10–12 — 「不是确认能工作，而是试图打破它」；反 **verification avoidance** 与 **前 80% 迷惑**（L12）。
- **工具边界**：`disallowedTools` 含 `FILE_EDIT` / `FILE_WRITE` / `AGENT_TOOL_NAME` 等（L139–145）；仅允许 bash（及会话 MCP）做观测。
- **输出契约**：每项 check 必须 `Command run` + `Output observed`（L81–90）；收尾字面量 `VERDICT: PASS|FAIL|PARTIAL`（L119–127），与 GenericAgent 同构。
- **运行形态**：`background: true`（L138）— 不阻塞主会话 UI，但 verdict 仍供主 Agent / hook 消费。
- **协同**：`docs/QUALITY_MECHANISMS.md` §7 — Stop Hook `preventContinuation` 可在脚本失败时阻止主循环「声称完成」。

#### GenericAgent：plan SOP + handler gate（最强 plan 绑定）

- **计划内嵌**：`plan_sop.md` 强制「验证检查点」与 `[VERIFY]` 步（§二步骤 4、§三终止检查、§四）。
- **验证子 Agent**：`verify_context.json` 只传任务/交付物/必做检查，**不传**执行过程（§四步骤 8）；`verify_sop.md` 要求对抗探测 + 表格化 PASS/FAIL。
- **代码 gate**（非 prompt  alone）：

```459:462:GenericAgent/ga.py
        if self._in_plan_mode() and any(kw in content for kw in ['任务完成', '全部完成', '已完成所有', '🏁']):
            if 'VERDICT' not in content and '[VERIFY]' not in content and '验证subagent' not in content:
                yield "[Warn] Plan模式完成声明拦截。\n"
                return StepOutcome({}, next_prompt="⛔ [验证拦截] 检测到你在plan模式下声称完成，但未执行[VERIFY]验证步骤。请先按plan_sop §四启动验证subagent，获得VERDICT后才能声称完成。")
```

#### codex：三层「review/verify」语义分拆

| 机制 | 文件 | 与 plan/todo 完工的关系 | 说人话 |
|------|------|-------------------------|--------|
| **Execute 沿途验证** | `codex-rs/collaboration-mode-templates/templates/execute.md` L34–35 | 执行里程碑时自检，非独立 verifier 子 Agent | 边做边测，写在协作模式里。 |
| **Guardian AutoReview** | `codex-rs/core/src/guardian/review.rs`、`review_session.rs` | 审批 **危险 shell/网络** 等；`ReviewDecision` 阻塞/放行 **单次工具** | 保安审操作，不是验交付物。 |
| **内部 one-shot 线程** | `run_codex_thread_interactive`（guardian 复用） | 可用于审稿类任务；Tomcat 对标 [`reviewer.md`](./tools/reviewer.md) | 内部开线程，不进 catalog。 |

#### pi-mono：模板链式 review（无 plan runtime）

`implement-and-review.md` 规定 `chain`：worker 实现 → reviewer 读 `{previous}` → worker 应用反馈。reviewer 的 `agents/reviewer.md` 明确 bash **只读**（`git diff`/`log`/`show`），输出 Critical/Warnings/Suggestions 结构 — **顾问式 code review**，无 `VERDICT` gate 字段。

#### pi_agent_rust：CI 脚本 verifier

`./verify` 委托 `scripts/e2e/run_all.sh`，支持 `--profile quick|ci|full`；与 agent 计划状态**解耦**，适合 Tomcat **非目标**「替代 CI」定位。

#### hermes / openclaw：软边界子 Agent

- **hermes**：`curator.py` 维护 agent-created skills 生命周期（auto-archive 等），**不**在 plan 完成后跑 `cargo test`。
- **openclaw**：`sessions_spawn` 由模型发起；质量依赖 skill 文档与 spawn 参数，**无**与 `update_plan`/`all_completed` 等价的统一 runtime 钩子（本仓库 Tomcat 拟补的能力）。

### 2.3 维度词典（V1–V10）

| 维度 | 关切 | 说人话 |
|------|------|--------|
| **V1 与 reviewer 关系** | 是否同一子 Agent | **分拆**：reviewer=计划质量；verifier=交付物可运行。 |
| **V2 触发** | 何时 spawn verifier | **todo 全 completed 时** runtime 自动派发；无 per-plan 关闭开关，soft/gate 由全局配置 `[plan].verify_gate` 控制。 |
| **V3 Gate + FAIL 语义** | 何时挡收工、`verdict` 含义 | **默认不 gate**；仅 `fail` 可挡；`partial`/`aborted` 不挡。 |
| **V4 输出契约** | 结构化 vs 叙述；证据字段 | `<verify>` + `checks[].command`；无命令 PASS 无效。 |
| **V5 工具集** | 只读 vs 可改仓库 | `{read, search_files, list_dir, bash}`；禁 plan/write/edit。 |
| **V6 派发入口** | catalog vs internal | **internal subagent**（同 reviewer）。 |
| **V7 执行中 Mini（PR-V0）** | B 层沿途验证 | prompt + **P0–P6 发现**；不 gate `completed`。 |
| **V8 FAIL 后谁修** | verifier 能否改 plan 状态 | 只写 transcript/摘要；主 Agent 或用户续跑。 |
| **V9 命令发现** | 命令从哪来、是否语言表 | **P0–P6**；manifest 优先；AGENTS.md 可选。 |
| **V10 Verifier 动作链** | C 层子 Agent 跑什么 | 发现→build→test→lint→类型策略→对抗探测→verdict。 |

### 2.4 Tomcat 现状（代码证据）

| 能力 | 现状 | 证据 |
|------|------|------|
| Plan 前审稿 | 已有 `reviewer`，顾问非 gate | `tomcat/src/api/chat/plan_runtime/review.rs`、`tools/reviewer.md` |
| EXEC 完成条件 | 全部 todo `completed` → `code_review?` → `verify` → 视 gate/promote 结果决定是否 completed | `update_plan.rs`：`ops::all_completed` → `derived_completed` → `dispatch_code_reviewer/dispatch_verifier` → `set_mode_completed` |
| 执行中验证 prompt | **弱**：executor 只强调 `update_plan`，**未**要求 build/test | `prompts/executor.txt` |
| 完成后 LLM verifier | **无** | 无 `dispatch_verifier` / `SubagentType::Verifier` |
| 写改前一致性（Mini 验证相关） | **有**：edit 前 `read` 指纹陈旧拦截 | [`tools/read.md`](./tools/read.md) §1 `staleness` — 属 **B 层**工具级自检，非 C 层完工验货 |

```101:115:tomcat/src/api/chat/plan_runtime/tools/update_plan.rs
    let derived_completed = matches!(plan_state_before, PlanFileState::Executing)
        && ops::all_completed(&plan.frontmatter.todos);
    if derived_completed {
        plan.frontmatter.state = PlanFileState::Completed;
    }
    // ...
    if derived_completed {
        runtime.set_mode_completed(target_plan_id.clone());
    }
```

**当前口径（相对竞品）**：Tomcat 已经在 `derived_completed` 处补上 **read-only code reviewer + verifier** 两段收口：code reviewer 负责给主 Agent 提代码问题并决定是否同回合直连 verifier；verifier 继续负责命令级验证与 `verify_gate` 语义。与 cc-fork / GenericAgent 相比，Tomcat 仍保持默认 advisory 风格，但不再是「all_completed 后直接 completed」。

---

## 3. 目标与设计原则

### 3.1 观察指标表

| 目标 | 观察指标（落地后可核对） | 说人话 |
|------|--------------------------|--------|
| **G1 语义分拆** | 文档与代码中 reviewer ≠ verifier；各自独立 prompt / `SubagentType` | 审稿和验货两套词不混。 |
| **G2 可复现证据** | `VerifySummary.checks[]` 每条含 `command` + `output_excerpt`；`result=pass` 但无 `command` 单条 runtime 降为 `skip` + warning；关键 build/test 全部 `skip` 时 `verdict` 降为 `partial` | 没跑命令不算过；单条偷懒先降 skip，关键全 skip 才整单 partial。 |
| **G3 默认不 gate 完工** | `[plan].verify_gate="soft"`（默认）时 `mode=completed` 与 today 一致；verify 失败只写 transcript + tool result | 验挂了也能收工，但留痕。 |
| **G4 可选硬 gate** | `[plan].verify_gate="gate"` 时 FAIL 阻止 `set_mode_completed`，session 保持 EXEC | 要严模式用户自己开。 |
| **G5 工具边界** | verifier 调不到 `create_plan` / `update_plan` / `write` / `edit` | 验货员不能改计划或源码（除非未来单独立项）。 |
| **G6 与 permission 一致** | verifier 的 `bash` 仍走 gate；危险命令被拒时写入 `VerifySummary` | 该问用户还得问。 |

### 3.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 替代 CI / `cargo test` 全量流水线 | 仓库 CI、用户脚本 | 不在 agent 里重造 GitHub Actions。 |
| 合并 reviewer 与 verifier 为一个子 Agent | — | 时机和工具集不同，硬合并更难维护。 |
| LLM 自调 `dispatch_agent(role=verifier)` 进 catalog | — | 和 reviewer 一样走 internal dispatch。 |
| 浏览器 E2E 一等内置 | 未来插件 / MCP | 本期最多 bash 调 playwright CLI。 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表

**`决策`** 列钉本行裁决结论（**SHOULD**），与 ARCHITECTURE_SPEC **§4.1 / §14.1** 同向。下表已吸收 **§4.1.1 FAQ**（FAIL 语义、各 Agent 验货动作、P0–P6 命令发现、PR-V0 Prompt 定稿）；FAQ 保留展开证据与竞品表。

| 维度 | 关切 | 决策 | 取自 | 入选设计 + 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **V1 角色分拆** | reviewer 是否兼 verifier | **采用** 新增 `SubagentType::Verifier` 独立子 Agent；**拒绝** reviewer 兼 verifier。 | `review.rs`；`cc-fork-01/src/tools/AgentTool/built-in/verificationAgent.ts`；`QUALITY_MECHANISMS.md` §5 vs §6 | **设计**：`SubagentType::Verifier` + 独立 VERIFY system prompt（对抗性、禁改仓库）。**理由**：reviewer=规划顾问（可改 plan）；verifier=交付物验货（只读+跑命令），合并会稀释 cc-fork「找茬」强度（§4.1.1 Q2）。 | **未入选**：EXEC 末再跑一轮 reviewer。**拒因**：工具集与时机均冲突。 | 两个子 Agent，各干各的。 |
| **V2 触发点** | 何时 spawn verifier | **采用** `all_completed` 时 `dispatch_verifier` **同步 await**（无 per-plan 关闭开关）。 | `GenericAgent/memory/plan_sop.md` §四；`verificationAgent.ts` L131–132；`update_plan.rs` | **设计**：`ops::all_completed` → `PlanRuntime::dispatch_verifier` 与 `set_mode_completed` 同事务边界；soft/gate 行为由全局 `[plan].verify_gate` 控制。**理由**：对齐 GenericAgent「全 [✓] 后验证」；避免主 Agent 回 CHAT 后才验（§4.1.1 Q2 实现层对照）；frontmatter 零改造保持配置面精简。 | **未入选**：主 Agent / LLM 自觉 spawn；frontmatter `verify` 字段。**拒因**：verification avoidance（cc-fork L12、§4.1.1 Q1）；多一字段无对应需求。 | 勾完 todo 由 runtime 拉验货员，开关只看全局 verify_gate。 |
| **V3 Gate + FAIL** | 是否阻塞 completed；何谓 FAIL | **采用** `[plan].verify_gate` 单一配置项，枚举 `{soft, gate}`，默认 `soft`；**仅** `verdict=fail` 且 `verify_gate="gate"` 时不 `set_mode_completed`；`partial`/`aborted` **不 gate**。 | `reviewer.md` G4；`verificationAgent.ts` L119–127；`verify_sop.md` L62–64；`ga.py` L459–462（GenericAgent 对照） | **设计**：`fail`=test/build/lint 非 0、对抗探测失败、无 `command` 的 PASS（runtime 降级）；`partial`=环境/工具不可用；`aborted`=spawn/parse/父 abort。**理由**：对齐 reviewer 顾问哲学 + cc-fork PARTIAL 语义；避免 flaky/沙箱误杀（§4.1.1 Q1 表）；单配置项最简。 | **未入选**：GenericAgent 式一律 VERDICT gate 完工声明；frontmatter per-plan 覆盖。**拒因**：Tomcat 默认仍 `set_mode_completed` 留痕；frontmatter 字段会让配置面发散。 | 默认只记账；严模式（一项 config 改 `gate`）只卡真 FAIL。 |
| **V4 输出契约** | 结构化 vs 叙述；证据 | **采用** `<verify>` → `VerifySummary`；`checks[]` 含 `command`+`output_excerpt`；`verdict` 四态。 | `verificationAgent.ts` L81–127；`plan_sop.md` L206–217；`verify_sop.md` L10–14 | **设计**：每项 check 必须 **Command run + Output observed**（抄 cc-fork）；解析失败→`aborted`。**理由**：GenericAgent「无工具输出的 PASS=无效」；可挂 UI/测（§4.1.1 Q1/Q2）。 | **未入选**：纯 markdown / pi-mono Critical 列表。**拒因**：无法机器区分「真跑 curl」与读代码 PASS。 | 结构化验货单，没命令不算过。 |
| **V5 工具集** | 能否改代码；bash 谁批 | **采用** `{read, search_files, list_dir, bash}`；**拒绝** `edit`/plan 工具；bash 仍走 **permission**。 | `verificationAgent.ts` L139–145；`QUALITY_MECHANISMS.md` L516–541；Tomcat permission 既有链 | **设计**：与 cc-fork `disallowedTools` 同构；verifier 可跑项目 test，危险命令被拒写入 `VerifySummary`。**理由**：验证=观测+执行，不是第二实施者（§4.1.1 Q2）。 | **未入选**：verifier 可 `edit` 自动修。**拒因**：与 executor 重叠、难审计。 | 验货员只读+跑命令，该问用户还得问。 |
| **V6 派发** | 入口形态 | **采用** `spawn_subagent_internal`；**拒绝** catalog / `dispatch_agent`。 | [`reviewer.md`](./tools/reviewer.md) §2.1 §4.1 | **设计**：`AgentRegistry::spawn_subagent_internal`，复用 cascade abort / depth。**理由**：与 reviewer、codex internal thread 同路径；防 LLM 跳过验货（§4.1.1 Q2 vs openclaw/hermes 软边界）。 | **未入选**：`dispatch_agent(role=verifier)`。**拒因**：模型可能不调或滥用。 | 内部拉子 Agent，模型看不见。 |
| **V7 Mini（PR-V0）** | B 层是否只靠 prompt | **采用** PR-V0：`executor.txt`/`planner.txt` **追加** Mini 段（§4.1.1 合入用 Prompt）；**不** gate `update_plan(completed)`。 | `execute.md` L34–41；`prompts.ts` L211；`plan_sop.md` L146；竞品表 §4.1.1 Q3 | **设计**：每 todo `completed` 前 **1 条** scoped smoke 或 `read` 非空；测不了同轮说明原因。**理由**：对齐 codex「along the way」+ GenericAgent Mini；低成本补 B，与 C 层互补（§4.1.1 Q3）。 | **未入选**：PR-V0 用 runtime 拦 `completed`。**拒因**：与「先 prompt 后 gate」分期一致；避免误杀进度。 | 边做边测，先靠提示词。 |
| **V8 FAIL 后谁修** | verifier 能否改 plan | **采用** FAIL 仅 `VerifySummary`/transcript/tool result；**拒绝** verifier `update_plan` 回滚 todos。Gate 拦下后由**主 Agent 在收到 `verify` tool result 时** `update_plan` 把目标 todo 退回 `in_progress` 或追加新 todo（runtime **不**自动）。 | `plan_sop.md` §步骤10；`update_plan.rs` 进度不变量 | **设计**：verifier 输出修复**建议文案**；todos 仍全 `completed`（gate 失败时 mode 保持 Executing）；主 Agent 后续轮自行 `update_plan` 续跑（同 EXEC 模式正常进度推进路径）。**理由**：进度只由 executor 推；区别于 GenericAgent 自动追加 `[FIX]` 步（§4.1.1 Q1 表）。 | **未入选**：verifier 自动 `pending` / GenericAgent 式修复循环。**拒因**：与 plan-runtime 不变量冲突；修复由主 Agent 或用户 `/plan build` 续跑。 | 验挂了给清单，主 Agent 在下轮把要修的 todo 退回 in_progress。 |
| **V9 命令发现** | 命令从哪来；是否语言矩阵 | **采用** **P0–P6 发现算法**（§4.1.1）；**拒绝** runtime「语言→默认 test」表。 | `verificationAgent.ts` L43–44；`init-verifiers.ts` L27–50；`prompt_builder.py` L1144–1172；`codex/AGENTS.md`；§4.1.1 竞品表 | **设计**：P0 plan/用户 → P1 已注入上下文 → P2 最近 manifest → P3 README → P4 AGENTS/CLAUDE（**仅 P2/P3 无果**）→ P5 读 manifest 确认推断 → P6 `inferred` 一条；EXEC 内 ≤3 次 Discover 缓存。**理由**：无一竞品硬编码 npm/cargo；AGENTS.md **非**前置（§4.1.1 Q3）。 | **未入选**：假定每仓有 AGENTS.md/plan；或 Rust 维护语言表。**拒因**：与调研不符；polyglot/monorepo 易错。 | 先读 manifest，AGENTS 可有可无。 |
| **V10 Verifier 动作** | C 层子 Agent 做什么 | **采用** cc-fork 动作链：发现命令（P0–P6）→ build → test → lint → ≥1 对抗探测 → `verdict`；不分 task_type。 | `verificationAgent.ts` L27–72、L42–47；`verify_sop.md` L29–43；§4.1.1 Q2 表 | **设计**：VERIFY prompt 抄 REQUIRED STEPS + 通用对抗探测要求；输入含 plan 路径、交付物（无 `verify_checks` / `verify_task_type` frontmatter）；**非**重写业务代码。**理由**：与 GenericAgent verify subagent、cc-fork Verification Agent 同构（§4.1.1 Q2）；不引入 task_type 策略表，让模型按发现的命令自动判断。 | **未入选**：仅跑 test 套件即 PASS；pi_agent_rust 式纯脚本无 LLM；按 `verify_task_type` 分支的策略表。**拒因**：无法覆盖「前 80% 迷惑」与 happy-path-only（cc-fork L12）；task_type 字段需要 plan 端额外维护。 | 找茬式验货：跑命令 + 至少一项对抗探测；侧重靠模型按发现的命令自判。 |

> **§4.1 ↔ §4.2**：**V7** → PR-V0（`prompts/*.txt`）；**V9** 由 PR-V0 Prompt 落地（始终走 P0–P6 算法，无 frontmatter 命令模板）；**V2/V4/V5/V10** → PR-V1 `verify.rs` + PR-V2 transcript & tool result 双通道；**V3** → PR-V3 `[plan].verify_gate` 单一配置项（无 frontmatter 字段）。

### 4.1.1 决策表 Q&A（FAQ）

以下三问对应上表 **V3 Gate**、**V2/V4/V5（各 Agent 验货在做什么）**、**V7 PR-V0（Mini 验证怎么做）**。

---

#### Q1：`[plan].verify_gate="gate"` 时什么时候会 FAIL？各 Agent 怎么定义 FAIL？

**Tomcat 拟定语义（C 层 Verifier，`VerifySummary.verdict`）**

| `verdict` | 含义 | `verify_gate="soft"`（默认） | `verify_gate="gate"`（严模式） | 说人话 |
|-----------|------|------------------------------|-------------------------------|--------|
| **`pass`** | 至少 1 条关键 build/test check 有 `command` 且 `result=pass`；其余可为非关键 `skip`（如缺浏览器 / GPU / 可选 lint） | 正常 `set_mode_completed` | 正常 `set_mode_completed` | 至少跑过一条 build/test，其它 skip 可接受。 |
| **`fail`** | 至少一项关键检查 **FAIL**（见下表「FAIL 触发」） | **仍** `set_mode_completed`；FAIL 写入 transcript + `update_plan` tool result | **不**调用 `set_mode_completed`；`PlanFile.mode` 保持 **Executing**，todos 仍全 `completed` | 严模式才卡收工。 |
| **`partial`** | 环境/工具不可用，**不是**「不确定是不是 bug」 | 同 pass 路径收工（留痕） | 同 pass 收工（与 cc-fork PARTIAL 一致；不 gate 避免 infra 误杀） | 测不了，别当 bug 硬 FAIL。 |
| **`aborted`** | spawn 失败、parse `<verify>` 失败、父 abort、超轮次 | 同 pass 路径收工（留痕 + warning） | 不 gate（避免 infra 误杀） | 验货员没跑完，不是交付物坏了。 |

**FAIL 触发（Tomcat runtime + verifier prompt 对齐 cc-fork / GenericAgent）**

| 类别 | 典型条件 | 证据要求 | 说人话 |
|------|----------|----------|--------|
| **硬 FAIL** | `cargo test` / `npm test` / 项目 test 脚本 **非 0 exit** | 必须粘贴 **Output observed** | 测试红了就是 FAIL。 |
| **硬 FAIL** | build / typecheck / lint（若项目配置了）失败 | 同上 | 编不过、类型不过。 |
| **硬 FAIL** | 复现步骤仍复现原 bug（bugfix 类） | curl/脚本输出 | 修了个寂寞。 |
| **硬 FAIL** | API/UI：happy path 之外 **对抗探测** 崩溃（边界/幂等/孤儿 ID 等） | 命令 + 输出 | 只测 200 不够。 |
| **非 FAIL → 单条 `skip`** | check `result=pass` 但缺 `command`（narration only） | runtime 单条改 `skip` + warning；不直接拉整单 FAIL | 单漏命令降级，不诛连。 |
| **非 FAIL → `partial`** | 无 test 框架、起不来 server、缺 GPU/浏览器且 MCP 不可用；**或** 关键 build/test check 全部 `skip`（含上一行降级累积） | verifier 写明原因 | 环境不行 / 全 skip 没真证据，不是代码错也不算过。 |
| **非 FAIL → `aborted`** | permission 拒了全部 bash、子 Agent 崩溃 | note 在 summary | 验货流程断了。 |

**各竞品对 FAIL / 完工 gate 的定义（对比，非 Tomcat 已实现）**

| Agent | FAIL 谁定 | FAIL 典型定义 | 是否阻塞「计划/任务完成」 | 证据 |
|-------|-----------|---------------|---------------------------|------|
| **cc-fork-01** | Verification 子 Agent 输出 **`VERDICT: FAIL`** | build 失败、test 失败、lint 失败、对抗探测发现缺陷；读代码无命令 → 无效 PASS（prompt L93–100） | **可**：主循环 + **Stop Hook** `preventContinuation`（脚本失败则 agent 不能结束）；Verification 本身 `background: true`，verdict 供消费 | `verificationAgent.ts` L42–47、L119–127；`QUALITY_MECHANISMS.md` §5、§7 |
| **GenericAgent** | 验证 subagent 输出 **`VERDICT: FAIL`** | `verify_sop.md` 按产物类型跑检查；**无工具输出的 PASS 视为无效→按 FAIL 处理**（§步骤 10） | **是**：无 VERDICT 不得声称完成（`ga.py` L459–462）；FAIL 进入修复循环，最多 2 轮 | `verify_sop.md` L10–14、L62–64；`plan_sop.md` §四步骤 10 |
| **codex** | 无统一 plan 完工 VERDICT | Execute 模式靠模型「沿里程碑报告 verified what changed」；Guardian 只有 **Allow/Deny**（审批单次危险操作） | Guardian **gate 工具**；**不** gate plan 全完成 | `execute.md` L34–41；`guardian/review.rs` |
| **pi-mono reviewer** | 无 VERDICT；**Critical / Warnings** 列表 | `git diff` + 读文件找 bug/安全/坏味道 | **否**；chain 里 worker 自行改 | `agents/reviewer.md`（git HEAD） |
| **pi_agent_rust** | **shell exit code ≠ 0** | `run_all.sh` 任一步 lint/test/E2E 失败 | **否**（非 agent 状态机） | `verify` → `scripts/e2e/run_all.sh` |
| **openclaw / hermes** | 无标准字段；skill / prompt 约定 | 依赖 `sessions_spawn` 或 `delegate_task` 子会话自由发挥 | **通常否** | 软边界 |

**与 reviewer 的对比（为何默认 `[plan].verify_gate="soft"`）**

- **reviewer**：`findings.severity` 只有 nit/suggestion/concern，**无** pass/fail gate，不挡 `/plan build`。
- **verifier**：面向 **可运行性**，FAIL 在严模式（`verify_gate="gate"`）下才挡 `mode=completed`；默认只留痕，避免 flaky CI、缺 Docker、沙箱禁网把会话卡死在 EXEC。

**说人话**：FAIL =「验货员用命令证明交付物不行」；默认验挂了也照样收工记账；只有开严模式才把收工闸拉上。

---

#### Q2：各 Agent 的 Verifier 都会做哪些动作？怎么实现的？

**共性模式**：几乎都是 **「只读上下文 + 跑命令（+ 可选浏览器 MCP）+ 结构化结论」**，而不是再写一遍业务代码。差异在 **谁触发、工具白名单、输出字段、是否 gate**。

| Agent | 实现形态 | 典型动作序列（逻辑） | 工具 / 运行时 | 输出 | 说人话 |
|-------|----------|----------------------|---------------|------|--------|
| **cc-fork-01 Verification Agent** | 内置 `BuiltInAgentDefinition` + `runAgent` 子循环 | ① 读 CLAUDE.md/README/package 脚本名 → ② **build** → ③ **test suite** → ④ lint/tsc → ⑤ 按变更类型策略（前端起 dev+curl+浏览器 MCP；API curl；CLI 跑边界输入）→ ⑥ **至少 1 项对抗探测** → ⑦ `VERDICT:` | bash + read + MCP；**禁** edit/write/spawn | 每项 `Command run` + `Output observed` + `VERDICT` | 独立找茬子 Agent，prompt 里写满策略表。 |
| **GenericAgent [VERIFY]** | 主 Agent 按 SOP **spawn 验证 subagent**（监察 `output.txt`） | ① 主 Agent 写 `verify_context.json` → ② subagent **必读** `verify_sop.md` → ③ 按 `task_type` 选策略表执行 **file_read / code_run / 浏览器证据** → ④ `result.md` + `VERDICT` → ⑤ 主 Agent 读 VERDICT；FAIL→追加 `[FIX]` 步 | subagent 工具集由 SOP 约定；**handler** 拦未验证完工声明 | `VERDICT` + 表格 checks | 计划里多一步；Python 硬 gate。 |
| **codex Execute** | **协作模式模板**注入 system（非子 Agent） | 主 Agent 每里程碑：**做了什么、验了什么、还剩什么**；原则「step by step, verifying along the way」 | 主会话全套工具 | 自然语言 progress | 没有单独 verifier 进程，靠主模型自觉。 |
| **codex Guardian** | `run_codex_thread_interactive` **嵌套会话** | 把待审批 action +  transcript 发给 reviewer 模型 → JSON assessment → Allow/Deny | 子会话；与 implementer 隔离 | `ReviewDecision` / risk | 审 **这一下** 能不能执行，不是验全任务。 |
| **pi-mono chain** | extension `spawn` 独立 `pi` 进程 | worker 实现 → reviewer `git diff`+读文件 → worker 按 feedback 改 | reviewer: read,grep,find,ls,bash(只读) | Critical/Warnings/Suggestions | 模板工作流，无 plan todo 钩子。 |
| **pi_agent_rust** | **bash 脚本** | lint → lib test → integration → E2E（profile 控制范围） | 无 LLM | exit code + artifact | 纯工程验证。 |
| **openclaw** | LLM 调 **`sessions_spawn`** | 由 skill（如 code-review）描述步骤；子会话隔离 | 入参定 model/runtime/权限 | 子会话消息 | 灵活、无统一 schema。 |
| **hermes** | **`delegate_task(goal, toolsets=...)`** | LLM 定何时派；子 Agent 按 goal 执行 | toolsets **LLM 入参** | 文本 | 通用委派，非专用 verifier。 |
| **Tomcat（拟定 PR-V1）** | `spawn_subagent_internal(SubagentType::Verifier)` | 同 cc-fork：读 plan body Test plan + 仓库脚本（P0–P6 发现）→ bash 跑检查 → 解析 `<verify>` → `VerifySummary` | `{read, search_files, list_dir, bash}`；禁 plan/write/edit | `checks[]` + `verdict` | 与 reviewer 同派发路径，专用验货 prompt。 |

**实现层对照（「代码在哪」）**

```text
触发源                执行体                     结论写回
─────────────────────────────────────────────────────────────
cc-fork    Todo/完成 nudge → AgentTool    → VERDICT 文本 + Stop Hook
GenericAgent  plan_sop §四 → subagent   → result.md + ga.py 拦截
codex Execute   模板拼进 system prompt    → 主会话 turn 文本
codex Guardian  approval 事件 → review_session → Allow/Deny 事件
Tomcat 拟定   update_plan all_completed → verify.rs dispatch → transcript + tool result
```

**说人话**：Verifier 不是新语言运行时，而是 **「另一个只读 Agent + 跑仓库里已有的 test/build 命令」**；Tomcat 拟用和 reviewer 一样的 internal spawn，把 cc-fork 的 prompt 策略抄过来。

---

#### Q3：PR-V0「Mini 验证」具体怎么做？要按语言写死 lint/build/test 吗？

**结论（Tomcat PR-V0 设计）**

| 项 | 决策 | 说人话 |
|----|------|--------|
| **改什么** | 只改 **`executor.txt`**（必做）+ **`planner.txt`**（建议）：在现有 system/reminder 中 **追加段落**，不新增工具、不改 runtime 状态机 | 先靠提示词补 B 层。 |
| **是否按语言写死命令** | **否**。不在 Rust 里维护「Rust=cargo test、Node=npm test」表 | 栈太多，写死会烂。 |
| **命令从哪来** | **P0–P6 发现算法**（竞品调研后定稿，见下「竞品调研：各 Agent 如何发现…」+「命令发现算法」+ **PR-V0 合入用 Prompt**） | 先 manifest/README，AGENTS.md 仅可选一档。 |
| **Mini vs C 层** | Mini = **单 todo 粒度**、**轻量**、**主 Agent 自己跑**；C 层 Verifier = **全 plan 完工后**、**对抗性**、**独立子 Agent** | 前者防堆错，后者防「全勾完其实不能跑」。 |
| **是否 gate `update_plan(completed)`** | **PR-V0 不 gate**（仅 prompt）；可选未来在 tool result 里 **warning**「本步未附验证命令」 | 别一上来卡进度。 |

#### 竞品调研：各 Agent 如何发现 build/test/lint 命令（非语言矩阵）

| Agent | 发现方式 | 是否假定 `AGENTS.md` | 硬编码 `npm test`? | 证据（路径） |
|-------|----------|----------------------|-------------------|--------------|
| **cc-fork-01** | Verifier prompt **显式顺序**：`CLAUDE.md`/`README` → `package.json`/`Makefile`/`pyproject.toml` → implementer 给的 plan/spec；主 Agent：**完工前跑 test/script**，测不了就明说 | 否（CLAUDE.md 四层记忆，项目内**可有可无**） | **否**（读 script 名） | `verificationAgent.ts` L43–44；`constants/prompts.ts` L211、L240 |
| **cc-fork-01** | `/init-verifiers`：**扫描子目录** manifest（`package.json`/`Cargo.toml`/`pyproject.toml`/`go.mod`）、读 scripts、识别 dev server | 否 | 否（检测 npm/yarn/pnpm） | `commands/init-verifiers.ts` L27–50 |
| **codex** | **仓库根 `AGENTS.md`**（若有）写死 `pnpm test`/`just` 等；Execute：**先读 readme 再 quick test**；Plan：**探索优先**，用非破坏性命令摸清事实 | **常见但非必须**（子树可有 scoped `AGENTS.md`） | 否（文档写 `pnpm` 而非默认 npm） | `codex/AGENTS.md` L57–60；`execute.md` L29、L34–41；`plan.md` L41–49 |
| **GenericAgent** | 探索 subagent 记环境；Mini验证 = **轻量**（非全量 test 矩阵）；`verify_sop` 按**产物类型**选动作；FAIL 前查 `CLAUDE.md` | 否 | 否 | `plan_sop.md` L34–38、L146；`verify_sop.md` L29–39、L50 |
| **hermes-agent** | **注入上下文**：`.hermes.md`（walk git root）\| `AGENTS.md`（仅 cwd）\| `CLAUDE.md` \| `.cursorrules` — **first match wins**；子任务在 `delegate_task` **context 里写死** `pytest …` 等 | **仅 cwd 顶层**，无则跳过 | 否（skill/plan 嵌入命令） | `agent/prompt_builder.py` L1144–1172；`skills/.../subagent-driven-development/SKILL.md` L68–78 |
| **openclaw** | 根 `AGENTS.md` **Commands** 节 + `pnpm check:changed` 等；读 **scoped** `AGENTS.md` | 有则必读，无则靠 `package.json`/脚本 | 否（`pnpm` 生态） | `git show HEAD:AGENTS.md` Commands 段 |
| **pi-mono** | reviewer 用 `git diff` + 读文件；**无**统一 test 发现 prompt | 否 | 否 | `agents/reviewer.md` |
| **pi_agent_rust** | `./verify` 脚本固定入口 | N/A | N/A | `verify` → `run_all.sh` |

**共性（Tomcat PR-V0 应对齐）**

1. **先探测、后执行**：用 `read` / `search_files` / `list_dir` 找 manifest 与文档，**禁止**未读就猜 `npm test`/`cargo test`。
2. **上下文文件全是可选增强**：cc-fork 用 CLAUDE.md 四层；hermes **互斥**加载一种 project context；codex/openclaw 的 `AGENTS.md` 是**最佳实践文档**而非每个仓库都有。
3. **范围缩小**：monorepo 只跑**与本步改动目录相关**的 crate/package（codex `cargo test -p <crate>`；openclaw `check:changed` 思想）。
4. **测不了要说**：cc-fork L211、GenericAgent 无效 PASS 规则 — 与 Tomcat 透明报告一致。
5. **不靠 runtime 语言表**：无一竞品在核心 Rust/TS 里维护「语言→默认 test 命令」映射。

---

#### 命令发现算法（Tomcat 拟定 · PR-V0/V1 共用）

**Phase 0 — 会话内缓存（可选，主 Agent 自用）**

进入 EXEC 后，若尚未归纳过本仓库验证命令，允许用 **≤3 次** 只读工具（`search_files`/`read`/`list_dir`）做一次 **Discover**，把结论记在脑中并在首个含代码的 todo 完成摘要里写一行 `Discovered checks: …`。之后 todos **复用**，勿每步重复扫全仓。

**Phase 1 — 发现顺序（严格按序，命中即停本步；文件不存在则跳过）**

| 优先级 | 来源 | 动作 | 说人话 |
|--------|------|------|--------|
| P0 | **本步 / 本 plan 已写明** | `PlanFile.body` 的 Test plan、用户当轮消息（**注**：本期不引入 `verify_checks[]` frontmatter，命令模板若需固化由 plan body 写明） | 计划或用户说了跑啥，先用。 |
| P1 | **已注入的项目上下文** | 若 system 已含 `AGENTS.md`/`CLAUDE.md`/rules 片段，从中取 **Commands / Test / CI** 类条目 | 有注入就用，别假设一定有。 |
| P2 | **变更目录最近的 manifest** | 从本 todo 改动的路径向上找：`package.json`（`scripts.test`/`scripts.build`）、`Cargo.toml`+`[[workspace]]`、`pyproject.toml`、`Makefile`/`justfile`、`go.mod`、`pom.xml` 等 | monorepo 找**最近**一层，不全仓盲跑。 |
| P3 | **文档** | 同目录或根 `README*`、`CONTRIBUTING*`、`docs/**/testing*` | 很多仓只有 README 里写了 `make test`。 |
| P4 | **仓库惯例文件名** | 根或子目录 `AGENTS.md`、`CLAUDE.md`（**仅当 P2/P3 未给出命令时**再 `read`） | 没有就跳过，不报错。 |
| P5 | **VCS / 同类文件推断** | 同目录已有 `*.test.ts`、`tests/`、`__tests__/` 时，结合 manifest 推断 runner；仍须 **读 manifest 确认** 再 `bash` | 猜到了也要读一眼 scripts 再跑。 |
| P6 | **最小推断 smoke** | 仅当 P0–P5 全无：按路径推断**一条**最小命令（例：仅改 `tomcat/` → `cargo test -p tomcat --lib <module>`），并在 `completed` 摘要标注 **`inferred`** | 最后手段，必须标注。 |

**禁止**：无 P0–P5 证据时默认 `npm test`、`cargo test`（全 workspace）、`pytest`（全库）。

**Phase 2 — 选哪条命令跑（Mini vs 全量）**

| 场景 | 选命令原则 |
|------|------------|
| **Mini（每 todo `completed` 前）** | 与**本步 diff 范围**相关的 **1 条** smoke：单 crate / `-p` / 单测 filter / `lint` 子包 / `pnpm --filter`；优先 `scripts` 里带 `test`/`check`/`build` 且耗时短者 |
| **Verifier（C 层，PR-V1）** | 在 Mini 基础上做 **build + 项目 test 入口 + 至少 1 项对抗探测**（抄 cc-fork `verificationAgent.ts` REQUIRED STEPS） |

---

#### PR-V0 合入用 Prompt 规则（调研后定稿）

> 下列英文块拟 **追加** 到 `executor.txt` / `planner.txt`（与现稿 `<system_reminder>` 并存）；**不**改 runtime。合入 PR-V0 时以代码库实际措辞微调。

**`executor.txt` 追加段（Mini verification + command discovery）**

```text
## Mini verification (before update_plan completed)

Before you set any todo to completed, run a quick Mini verification for THAT step only.
Mini verification is not the final Verifier subagent; it is a cheap smoke check so errors do not pile up.

1) Choose what to run
   - Code/config changes: run exactly ONE bash command scoped to files you changed in this step.
   - Docs/plan-only: use read to confirm the deliverable exists and is non-empty.
   - Cannot run (missing deps, sandbox, no test target): do NOT mark completed until you state
     in the same turn WHY you could not run anything. Never imply you tested when you did not.

2) How to pick the command (discovery — do not guess package managers)
   Follow this order; skip missing files; stop when you have a concrete command:
   a) Commands already in the active plan body, plan_meta, or user message.
   b) Commands section in project context already injected in system (AGENTS.md / CLAUDE.md / rules) — if present.
   c) Nearest manifest to your changed paths: package.json scripts, Cargo.toml workspace member,
      pyproject.toml, Makefile, justfile, go.mod, etc. Read the file; do not invent npm/cargo/pytest.
   d) README / CONTRIBUTING near the change or repo root.
   e) Only if still unknown: read AGENTS.md or CLAUDE.md if they exist (many repos do not have them).
   f) Last resort: one minimal inferred command tied to the changed directory (e.g. cargo test -p <crate> …)
      and label it inferred in your completion note.

   FORBIDDEN: defaulting to npm test, cargo test (whole workspace), or pytest (entire tree) without
   reading a manifest or doc in this repo.

3) Scope
   - Prefer the smallest check that exercises your edit (one crate, one package, one test filter).
   - Reuse commands you already discovered earlier in this EXEC session; avoid rescanning the whole repo every todo.

4) When marking completed
   - Briefly record: command run (or read check), exit status / key output, or explicit skip reason.
   - Full adversarial verification still happens after all todos complete (Verifier), when enabled.
```

**`planner.txt` 追加段（plan-time test hints）**

```text
## Verification hints in plans

When drafting todos that touch code, prefer actionable verification over a vague "test everything" finale:
- In Plan body, add a short "Test plan" or per-step note with the command you expect IF you already
  found it during read-only exploration (e.g. from README or package.json). If unknown, write
  "discover test command from manifest during EXEC" — do not invent npm test.
- Split large work so at least one mid-plan todo is "run scoped smoke test" rather than only testing at the end.
```

**planner / executor 分工**

| 阶段 | 谁写命令 | 谁跑命令 |
|------|----------|----------|
| PLAN | 探索后**可写**入 plan body 的 Test plan（若已探测到） | 只读探索，可跑 **非破坏性** dry-run（对齐 codex `plan.md` L27–28） |
| EXEC 每 todo | 复用 plan + Discover 缓存 | 主 Agent `bash`/`read` Mini |
| EXEC 全完成 | — | Verifier 子 Agent（PR-V1） |

**各 Agent 的 B 层（执行中 Mini 验证）— 对照上表**

| Agent | 机制 | 命令从哪来 | 证据 |
|-------|------|------------|------|
| **GenericAgent** | 每步 Mini：非空/exit code | 探索 findings + 当步 SOP | `plan_sop.md` L146 |
| **cc-fork-01** | 主 Agent 完工前 verify | README/脚本；测不了明说 | `prompts.ts` L211 |
| **codex Execute** | along the way + progress 写 verified | readme + 推断 quick test | `execute.md` L29–41 |
| **hermes** | 子任务 context **内嵌**命令 | planner/implementer 写入 delegate | `subagent-driven-development/SKILL.md` |
| **openclaw** | 文档 Commands + changed 范围 | `AGENTS.md` | `AGENTS.md` |
| **Tomcat PR-V0** | 上列 **executor/planner 追加段** | P0–P6 发现算法 | 本文 |

**为何不让 runtime 按语言分发命令？**

见上表「共性」第 5 点。更强约束优先：**plan body 的 Test plan** → **manifest/README** → **注入的 AGENTS/CLAUDE** → 未来只读 `detect_project_commands` 工具（非 PR-V0）。

**说人话**：PR-V0 = 在 prompt 里教模型 **按顺序找命令、只跑一小条、找不到就说**；`AGENTS.md` 只是发现链里靠后的一档，和 plan body 一样**有则用、无则换 manifest/README**。

---

### 4.2 实施点（分期）

| 实施点 | 交付范围（含交付物） | 主要代码落点 | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------|------------------|--------|
| **PR-V0 prompt** | `executor.txt` / `planner.txt` 追加 **§4.1.1 合入用 Prompt**（Mini + P0–P6 发现）；文档同步 | `src/api/chat/plan_runtime/prompts/*.txt` | `executor_prompt_contains_mini_verification_section`、`planner_prompt_contains_test_plan_hint`、`prompt_forbids_default_npm_or_full_workspace_test`、`executor_prompt_mentions_verifier_gate_behavior`、`verifier_system_prompt_contains_contract`、`build_verify_prompt_mentions_discovery_order_and_inferred_rules` | 先靠提示词补 B 层（**V7+V9**）。 |
| **PR-V1 verifier 核心** | `SubagentType::Verifier`、`VERIFY_SYSTEM_PROMPT`、`VerifySummary` 解析、`dispatch_verifier`；**代码常量** `VERIFIER_MAX_TURNS = 64`（与 reviewer 同值，**不进** TOML） | `plan_runtime/verify.rs`（新）、`agent_registry.rs`、`plan_runtime/mod.rs`、`plan_runtime/tools/update_plan.rs` | `verifier_spawned_on_all_completed`、`verifier_blocks_non_whitelisted_tools`、`verifier_max_turns_default_is_64`、`build_summary_from_outcome_marks_turn_budget_cutoff_as_aborted`、`build_summary_from_outcome_keeps_pass_when_limit_is_used_exactly_and_normally` | 勾完 todo 拉验货子 Agent。 |
| **PR-V2 transcript + tool result** | `transcript.plan.verify` 事件落盘；`update_plan` 成功 tool result JSON 增加 `verify: VerifySummary \| null` 字段（双通道回传主 Agent，无 `/plan verify` CLI、无 frontmatter 字段） | `session-storage` 事件序列化、`update_plan.rs` tool result 拼装 | `verify_event_in_transcript`、`verify_event_matches_tool_result_after_normalization`、`update_plan_tool_result_has_verify_field`、`inprocess_full_plan_path_with_real_llm`（full-chain `plan.verify`） | 事件落 transcript，结果同时回主 Agent 当轮 prompt。 |
| **PR-V3 gate 配置** | `[plan].verify_gate: String`（枚举 `soft\|gate`，默认 `"soft"`）；`verify_gate="gate" && verdict="fail"` 时 runtime **不**调 `set_mode_completed`、`PlanFile.mode` 保持 `Executing`、tool result.verify 仍回传主 Agent | `infra/config/mod.rs`、`update_plan.rs` | `verify_gate_soft_does_not_block`、`verify_gate_blocks_completed_on_fail`、`gate_fail_keeps_mode_executing_but_returns_verify`、`verify_gate_allows_completed_on_partial`、`verify_gate_allows_completed_on_aborted`、`main_agent_can_reopen_todo_after_gate_fail`、`gate_fail_then_recomplete_respawns_verifier` | 严模式卡完工，主 Agent 自己退 todo 续跑。 |

### 4.2.1 PR-V1 调度要点（ASCII）

```text
update_plan (last todo → completed)
        │
        ▼
  all_completed(plan.todos)?
        │ no ──▶ 常规返回 snapshot
        │ yes
        ▼
  first write: todos=completed, mode=executing
        │
        ▼
  PlanRuntime::dispatch_verifier(plan)  ── sync await ──▶ raw VerifySummary
        │
        ▼
  normalize_for_gate(raw) ──▶ final VerifySummary
        │
        ├─ write transcript.plan.verify (normalized)
        │
        ├─ config[plan].verify_gate == "gate" && verdict == "fail"
        │     ──▶ 保持 EXEC，不写 completed mode；tool result 仍含 verify
        └─ else ──▶ set_mode_completed + attach verify to tool result
```

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 PlanFile frontmatter（不扩展）

**本期不向 PlanFile frontmatter 新增任何 verify 相关字段**（`verify` / `verify_checks` / `verify_task_type` 全部不引入）。verifier 是否 gate 完工统一由全局配置项 `[plan].verify_gate` 决定（详 §9）；命令模板若需固化，写入 plan body 的 Test plan 段（PR-V0 命令发现算法 P0 即可消费）。

**说人话**：frontmatter 一刀切不动，配置就一项；要严模式就改 `[plan].verify_gate = "gate"`。

### 5.2 `VerifySummary`（单一事实源：runtime normalize 后的最终 JSON）

```yaml
# 子 Agent 最终消息内 <verify> block（yaml 子集）
checks:
  - name: "unit tests"
    command: "cargo test -p tomcat --lib"
    output_excerpt: "test result: ok. 42 passed"
    result: pass   # pass | fail | skip
verdict: pass      # pass | fail | partial | aborted
summary: "≤600 chars"
```

| 字段 | 约束 | 说人话 |
|------|------|--------|
| `checks[].command` | `result=pass` 必填；缺则 runtime 单条降 `skip` + warning；关键 build/test 全 `skip` → `verdict=partial`（详 §10） | 没命令别说过；单条漏不诛连。 |
| `checks[].output_excerpt` | ≤ 500 chars；超长尾部截断（保留命令最后若干行 + `…[truncated]` 尾注） | 输出别撑爆 transcript。 |
| `verdict` | `aborted`：parse 失败 / 达到 `VERIFIER_MAX_TURNS` 预算仍未正常收口 / parent abort；其他四态见 §4.1.1 Q1 | 验货中断。 |
| transcript / tool result | `plan.verify` 事件与 `update_plan.verify` 共用 runtime `normalize_for_gate()` 之后的最终 `VerifySummary` | 审计口径和当轮推理口径一致。 |
| Tool result 挂载 | `update_plan` 成功 JSON 增加 `verify: VerifySummary \| null` | 主 Agent 一眼看见验货结果。 |

### 5.3 Verifier system prompt（设计要点，节选）

对标 `cc-fork-01` verificationAgent（`QUALITY_MECHANISMS.md` §5）：

1. 职责：**不是**确认实现正确，而是**试图推翻**「已完成」声称。
2. 禁止：改项目文件、改 plan、`create_plan`。
3. 每个 check：**Command run** + **Output observed**（粘贴终端输出）。
4. 反合理化：禁止「看起来对」式 PASS（与 cc-fork L563-582 同构）。
5. 通用动作链（不分 task_type）：发现命令（P0–P6）→ build → test → lint → ≥1 对抗探测 → `verdict`；按发现的命令性质（test/build/curl/启动 server）自然决定侧重，不依赖 frontmatter 字段。

---

## 6. One-Glance Map

```text
┌─────────────────────────────────────────────────────────────────────────┐
│ ChatContext.plan_runtime: PlanRuntime                                     │
│   on_update_plan_success()                                              │
│     ├─ ops::all_completed()                                             │
│     ├─ [NEW] dispatch_verifier() → AgentRegistry::spawn_subagent_internal │
│     │         └─ plan_runtime/verify.rs (prompt, parse, build_brief)    │
│     └─ set_mode_completed()  (gated by [plan].verify_gate=="gate" && verdict=="fail") │
├─────────────────────────────────────────────────────────────────────────┤
│ AgentRegistry                                                           │
│   SubagentType::Reviewer  ← create_plan (已有 review.rs)                │
│   SubagentType::Verifier  ← update_plan all completed (新)              │
├─────────────────────────────────────────────────────────────────────────┤
│ tool_exec + permission                                                  │
│   verifier context: bash allowlist / url_like / gate                    │
├─────────────────────────────────────────────────────────────────────────┤
│ transcript                                                                │
│   plan.review (已有)  plan.verify (新)                                    │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 7. 调度时序

```mermaid
sequenceDiagram
  participant User
  participant Main as Main Agent (EXEC)
  participant UP as update_plan tool
  participant PR as PlanRuntime
  participant CR as Code Reviewer subagent
  participant VR as Verifier subagent
  participant TR as Transcript

  User->>Main: 继续执行 todo
  Main->>UP: set_status(completed)
  UP->>PR: all_completed?
  alt all_completed
    alt code_review_rounds < max_code_review_rounds
      PR->>CR: spawn_subagent_internal (sync, kind=Code)
      CR-->>PR: raw ReviewSummary(kind=code)
      PR->>PR: normalize_for_code_review_result(raw)
      PR->>TR: plan.code_review event (normalized)
      alt verdict == pass
        PR->>VR: spawn_subagent_internal (sync)
        VR-->>PR: raw VerifySummary
        PR->>PR: normalize_for_gate(raw)
        PR->>TR: plan.verify event (normalized)
      else verdict != pass
        PR-->>UP: code_review only; keep executing
      end
    else rounds exhausted
      PR->>TR: plan.code_review.warning(reason=rounds_exhausted)
      PR->>VR: spawn_subagent_internal (sync)
      VR-->>PR: raw VerifySummary
      PR->>PR: normalize_for_gate(raw)
      PR->>TR: plan.verify event (normalized)
    end
  end
  PR->>PR: set_mode_completed (unless code_review != pass OR verify_gate=="gate" && verdict=="fail")
  UP-->>Main: snapshot + code_review + verify
```

---

## 8. 状态机

| 状态 | 进入条件 | verifier/code-review 行为 | 说人话 |
|------|----------|--------------------------|--------|
| EXEC | `/plan build` | 中途不自动跑；只在 `all_completed` 时触发收口链路 | 干活中。 |
| EXEC + code_reviewing | `all_completed` 且还有 code-review rounds | runtime 同步 spawn read-only code reviewer | 代码二审中。 |
| EXEC + verifying | code review `pass`，或 rounds 用尽被跳过 | runtime 同步 spawn verifier | 验货中。 |
| COMPLETED | code review 已通过/跳过，且 verifier 不阻断 completed | 留下 `plan.code_review?` / `plan.verify` 事件供回放 | 回 CHAT。 |
| EXEC（code review 真 findings） | code reviewer `verdict != pass` 且 `aborted == false` | **停留** EXEC；主 Agent 收 `tool_result.code_review` 后自行 reopen / 新增 todo 修复 | 代码审查真打回，主 Agent 接力修。 |
| EXEC（gate 失败） | `[plan].verify_gate="gate"` 且 verifier verdict=fail | **停留** EXEC；todos 仍全 completed；主 Agent 收 tool result.verify 后自行 `update_plan` 退 todo | 验货被拦下，主 Agent 自己接力修。 |

---

## 9. 配置与环境变量

> **不进 TOML**：verifier 子 loop 轮次上限 **`VERIFIER_MAX_TURNS = 64`**，与 reviewer 默认 `max_turns` 同值，在 `plan_runtime/verify.rs` 以 `const` 写死，映射 `AgentLoopConfig.max_tool_rounds`；达到预算仍未正常收口 → `verdict=aborted` + `verifier_stop_reason=max_turns`（详 §10）。**不提供** `[plan].verifier_max_turns` 或 env 覆盖，保持配置面精简。

> **frontmatter 零改造**：本期不向 PlanFile frontmatter 新增任何 verify 相关字段（详 §5.1）；verifier soft/gate 行为只看下表 `[plan].verify_gate` 全局配置。

| 键 / env | 类型 | 默认 | 说明 | 说人话 |
|----------|------|------|------|--------|
| `[plan].verify_gate` | string（枚举 `soft \| gate`） | `"soft"` | `soft`：verifier 始终派发，FAIL 仅写 transcript + tool result，不阻塞 `set_mode_completed`。`gate`：FAIL 时 runtime 不调 `set_mode_completed`，PlanFile.mode 保持 Executing，主 Agent 在收到 tool result.verify 后自行 `update_plan` 把目标 todo 退回 `in_progress`（详 §4.1 V8、§10）。 | 一项 config 控全开关：默认软验货，改 `gate` 即严模式。 |

---

## 10. 错误模型 / 警告

| 情况 | 结局 | 说人话 |
|------|------|--------|
| Verifier spawn 失败 | `verdict=aborted`；**不**阻塞 completed（除非 gate） | 验货员没起来也先收工。 |
| Parse `<verify>` 失败 | 同 `verdict=aborted`；`verifier_stop_reason=parse_error` | 格式乱了当没验。 |
| Bash 被 permission 拒 | check `result=fail` 或 `skip` + note；多条全被拒 → `verdict=aborted`（permission 全否） | 命令没跑成记下来。 |
| `result=pass` 但缺 `command` | runtime 单条降 `skip` + warning；关键 build/test 全 skip → `verdict=partial`（与 §5.2 / §3.1 G2 一致） | 没跑命令不算过；单漏一条不诛连。 |
| 子 Agent 达到 `VERIFIER_MAX_TURNS`（64）预算仍未正常收口 | `verdict=aborted`；`verifier_stop_reason=max_turns`；**不**自动重派 | 验太久就停。 |
| Gate 拦下 `mode=completed`（`verdict=fail` 且 `[plan].verify_gate="gate"`） | `PlanFile.mode` 保持 `Executing`；todos 仍全 `completed`；主 Agent 收到 tool result.verify 后 **自行** `update_plan` 把目标 todo 退回 `in_progress` 或追加新 todo 续跑（详 §4.1 V8） | 严模式拦下后，主 Agent 下轮自己退 todo 修。 |
| 无任何 plan body 命令提示 | verifier prompt 走 P0–P6 命令发现算法（manifest/README/AGENTS），从仓库自行推断最小 build + test | 没写命令就按算法自己探。 |

---

## 11. 测试矩阵（验收）

| ID | 场景 | 期望 | 状态 |
|----|------|------|------|
| T-V1 | 最后一个 todo completed + `[plan].verify_gate="soft"`（默认） | spawn verifier；transcript 有 `plan.verify`；`update_plan` tool result JSON 含 `verify` 字段；mode → completed | DONE（`verifier_spawned_on_all_completed`、`verify_event_in_transcript`、`update_plan_tool_result_has_verify_field`、`inprocess_full_plan_path_with_real_llm`） |
| T-V2 | verifier 调用 `write` / `edit` / `update_plan` | tool_exec 拒绝；summary 含失败 check | DONE（`verifier_blocks_non_whitelisted_tools`） |
| T-V3 | check `result=pass` 但无 `command` | 单条降 `skip` + warning；若关键 build/test 全部 `skip` → `verdict=partial`（详 §5.2 / §10） | DONE（`normalize_for_gate_demotes_empty_command_pass_and_partializes_key_checks`、`verify_event_matches_tool_result_after_normalization`） |
| T-V4 | `[plan].verify_gate="gate"` + verdict=fail | `mode` 保持 `Executing`；`set_mode_completed` 未调用；transcript 有 `plan.verify`；tool result 仍含 verify | DONE（`verify_gate_blocks_completed_on_fail`、`gate_fail_keeps_mode_executing_but_returns_verify`） |
| T-V5 | `[plan].verify_gate="soft"` + verdict=fail | `mode` 仍 → completed；FAIL 写 transcript + tool result；不阻塞 | DONE（`verify_gate_soft_does_not_block`） |
| T-V6 | reviewer 与 verifier 同 plan | create_plan 只触发 reviewer；all_completed 只触发 verifier；两者 prompt / SubagentType 不同 | PARTIAL（已有 `plan.review` / `plan.verify` 分别覆盖，但缺一条显式 lifecycle 分离断言） |
| T-V7 | 子 loop 达到 `VERIFIER_MAX_TURNS`（64）预算仍未正常收口 | `verdict=aborted`；`verifier_stop_reason=max_turns`；不重派；gate 不把它当 fail | DONE（`build_summary_from_outcome_marks_turn_budget_cutoff_as_aborted`、`verify_gate_allows_completed_on_aborted`） |
| T-V8 | gate 拦下后主 Agent 下轮 `update_plan(todo_x, in_progress)` | mode 仍 Executing → 主 Agent 推进 → 全部完成后 runtime 自动再 spawn verifier | DONE（`main_agent_can_reopen_todo_after_gate_fail`、`gate_fail_then_recomplete_respawns_verifier`） |

---

## 12. 风险与应对

| 风险 | 应对 | 说人话 |
|------|------|--------|
| 验证命令破坏环境 | bash gate + 沙箱策略；verifier 禁 write | 验货别乱删库。 |
| 耗时过长 | `VERIFIER_MAX_TURNS`（64）+ 单命令 timeout；可 `background` 二期 | 别验一小时。 |
| 与 reviewer 重复读盘 | 独立上下文；brief 只传 plan 路径 + deliverables | 各用各的上下文。 |
| 模型 verification avoidance | prompt 反合理化 + 输出 schema 强制 command 字段 | 专防「嘴上说过了」。 |

---

## 13. 历史决策

| 决策 | 原因 |
|------|------|
| reviewer **不做** EXEC 完工 gate | 已在 [`reviewer.md`](./tools/reviewer.md) / [`plan-runtime.md`](./plan-runtime.md) G4 拍板 |
| 本期 **不** 把 verifier 暴露为 LLM 工具 | 与 reviewer 同：防滥用、边界硬编码 |
| `pi_agent_rust/verify` **不** 内嵌为子 Agent | 它是 CI 脚本，不是对话 verifier |

---

## 14. 关联文档

- 运行时总览：[plan-runtime.md](./plan-runtime.md)
- 审稿（规划后）：[tools/reviewer.md](./tools/reviewer.md)
- 读工具与陈旧检测：[tools/read.md](./tools/read.md)
- 子 Agent 基础设施：[multi-agent.md](./multi-agent.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- 任务卡：[T2-P1-002.md](../agents/TASK_BOARD_002/tasks/T2-P1-002.md)（本卡 **PR-V0～V3** 子项）
- 竞品质量机制原文：`cc-fork-01/docs/QUALITY_MECHANISMS.md`
- cc-fork 验证 Agent 源码：`cc-fork-01/src/tools/AgentTool/built-in/verificationAgent.ts`
- GenericAgent 验证 SOP：`GenericAgent/memory/plan_sop.md` §四、`GenericAgent/memory/verify_sop.md`
- GenericAgent 完工 gate：`GenericAgent/ga.py` L459–462
- Codex Execute 模板：`codex/codex-rs/collaboration-mode-templates/templates/execute.md`
- pi-mono subagent 链：`pi-mono`（git HEAD）`packages/coding-agent/examples/extensions/subagent/`

**说人话**：开工看 reviewer，收工看 verifier（本文）；中间靠 executor prompt 做 Mini验证；机器全量测试仍走 CI / `pi_agent_rust` 的 `./verify` 脚本。

---

**一句话总结**：竞品把「plan/todo 勾完」和「交付物真能用」拆成两阶段 — 后者用**只读 + 跑命令 + VERDICT/证据**的独立 verifier（cc-fork / GenericAgent 最完整）；Tomcat 已有 plan 前 reviewer 与 read 陈旧检测，但 **`all_completed` 仍直接 `mode=completed`**，宜按 §4 增加可选 **internal `SubagentType::Verifier`**，默认顾问不 gate、结构与 cc-fork 对齐。
