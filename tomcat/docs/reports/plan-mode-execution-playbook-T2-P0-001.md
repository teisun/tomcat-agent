# Cursor PLAN 模式执行步骤复盘 —— 以 T2-P0-001（Agent Loop 模块化拆分）为案例

> 本报告复盘 2026-04-24 在 Cursor PLAN 模式下为 `T2-P0-001 | agent-loop-modularization` 制定开发计划的真实执行流程，提炼为可复用的执行手册。
>
> 读者对象：后续承接 [TASK_BOARD_002 看板](../../agents/TASK_BOARD_002/README.md)（单卡 `tasks/T2-*.md`）其它任务的工程师 Agent（Tom / Jerry / Spike），以及希望理解 Cursor PLAN 模式底层约束与最佳实践的研发人员。
>
> 案例计划产物：`~/.cursor/plans/agent-loop-modularization_e99e067f.plan.md`
>
> **历史说明（2026-05）**：本文中凡提到 `PLAN_SPEC` 的旧章节号、"7 个维度" 等表述，均对应旧版计划规范的当时写法；当前仓库请以最新 `agents/plan/PLAN_SPEC.md` 为准。

---

## 一、大白话先说（30 秒版本）

在 PLAN 模式下写一份好计划，**不是"打开文件就开写"**，而是像医生看病：先挂号（读角色/流程文档）→ 查体（读代码现状）→ 拍片（对比看板字面值和真实值）→ 问诊（向用户确认关键分歧）→ 再复查（读规范文档）→ 最后出诊断书（`CreatePlan`）。

**规则铁律**：PLAN 模式下**不允许**写代码、不允许改配置、不允许 commit。只能做只读探索 + 提问 + 产出计划文件。

---

## 二、PLAN 模式的底层约束（为什么要按这个流程走）

| 约束 | 来源 | 影响 |
|---|---|---|
| 只读工具集 | PLAN 模式的系统提示词 | 任何 `StrReplace` / `Write` / `git commit` 等写动作都会被拒绝；只有 `Read` / `Grep` / `Glob` / `Shell`（只读命令）/ `AskQuestion` / `CreatePlan` 可用 |
| 歧义必问 | 系统提示第 2-5 条 | 遇到"多种合理实现改动范围差很多"时**必须**用 `AskQuestion` 确认；不能自行选择 |
| 小范围预读 | 系统提示第 5 条 | 回答问题前允许 ≤ 5 个文件 / ~20 秒的小预读，但不能做大规模探索 |
| 计划必须经用户确认 | [Dispatcher.md § 4](../../agents/Dispatcher.md) + PLAN 模式第 6 条 | `CreatePlan` 后用户需显式 confirm，才能切到 Agent 模式进入编码 |
| 计划须满足 7 个维度 | [PLAN_SPEC.md § 1](../../agents/plan/PLAN_SPEC.md) | 子项清单 / 目标验收 / 文件思路接口测试 / 实施顺序 / 风险 / 集成 E2E / Todo 总表；缺一不可 |

**后果**：如果跳过预读直接问问题，问出来的问题通常问不到点子上；如果跳过问问题直接写计划，经常会把改动范围猜错（本案例就差点把 ext/dispatcher 也强拆）。

---

## 三、本案例的实际执行步骤（时间线）

### Phase A — 角色与流程对齐（并行预读）

**动作**：一次性并行读 3 份文档：

- [agents/Jerry.md](../../agents/Jerry.md) — 角色能力边界
- [agents/Dispatcher.md](../../agents/Dispatcher.md) — 领任务 → 读上下文 → 制定计划 → 开发 → 完成的 6 步流程
- [agents/TASK_BOARD_002/README.md](../../agents/TASK_BOARD_002/README.md) + [tasks/T2-P0-001.md](../../agents/TASK_BOARD_002/tasks/T2-P0-001.md) — 迭代索引与 T2-P0-001 任务细节

**产出**：确认任务 ID `T2-P0-001`、优先级 P0、分支名 `feature/agent-loop-split`、看板列出的 **7 条子项** 与 **4 条验收标准**。

**经验**：永远**并行**读角色/流程/看板三件套，不要串行 —— 它们互为上下文，单独看每一份都不完整。

### Phase B — 仓库与代码现状摸底（并行）

**动作**：并行执行：

1. `ls src/core/agent_loop/` + `ls src/ext/` + `git status` + `git branch --show-current` + `git log --oneline -5`
2. `wc -l src/core/agent_loop/*.rs` + `wc -l src/ext/**/*.rs`

**关键发现**（看板字面值 vs 真实值的**量化偏差**，参考 [PLAN_SPEC § 7.1](../../agents/plan/PLAN_SPEC.md) 问题描述要素）：

| 项 | 看板写 | 真实值 | 偏差含义 |
|---|---|---|---|
| `run.rs` 行数 | 832 | **948** | 看板是旧值；T2-P0-007 中断/恢复追加 `CancellationToken` / `tokio::select!` 片段后已涨 116 行 |
| `tests.rs` 存在？ | 隐含"待建" | **已 1277 行独立文件** | 历史已按 RUST_FILE_LINES_SPEC §A 独立 |
| `convert.rs` 是否 = `error_classifier.rs`？ | 未明说 | `convert.rs` 74 行，仅 `classify_error` | 要么重命名要么保留，分歧点 |
| `src/ext/dispatcher/` 是否已拆？ | 子项要求"子模块化" | **已拆 5 业务文件 + tests** | 目录化已完成；但 dispatch(390) / ops(345) / session_ops(374) 单文件超 300 |
| 当前分支 | — | `develop @ 519a3a2`（上一任务刚并入） | 起点干净，无未 commit 冲突 |

**经验**：看板字段可能滞后于真实代码（尤其在相邻任务刚合并后）。**先摸底再信看板**。

### Phase C — 关键代码深读（并行大块读取）

**动作**：两轮并行 `Read`：

- 第 3 轮：`run.rs`（949 行全文） + `types.rs` + `mod.rs`
- 第 4 轮：`convert.rs` + `ext/dispatcher/mod.rs` + `ext/dispatcher/dispatch.rs`

**目的**：
- 判断每个看板子项对应的代码段在哪里、多大、依赖什么
- 预估拆分后每个新文件的行数（给"≤ 300 行"验收标准打底）
- 识别"看板子项清单"与"代码现状"之间的**偏差**（例：子项要求新建 `dispatcher.rs`，但与已存在的 `src/ext/dispatcher/` 重名，要注意心智歧义）

**经验**：读代码时要**把每段代码对齐到看板子项**，一对一映射。映射不上的段落（比如本案例里 `handle_overflow_retry` 的 L3 trim 代码块）是"灰色地带"，要在计划里主动说明归属。

### Phase D — 识别关键决策点并提问（核心步骤）

**动作**：用 `AskQuestion` 一次问 2 个关键范围问题：

**问题 1**（`ext_dispatcher_scope`）：`src/ext/dispatcher/` 要不要在本任务里重拆到 ≤ 300？

- 选项 A：同时重拆（工作量翻倍）
- 选项 B：认定已历史拆分完成，仅 status 说明（最小范围）← 用户选
- 选项 C：折中（只拆 dispatch.rs 的巨型 match）

**问题 2**（`rename_convert`）：`convert.rs` 要不要改名为 `error_classifier.rs`（与看板子项字面一致）？

- 选项 A：重命名 ← 用户选
- 选项 B：保留旧名，仅加注释

**为什么必须问**：
- 问题 1 直接决定工作量（翻倍 vs 不翻倍），用户不拍板不能动
- 问题 2 看板字面写的 `error_classifier.rs`，但不改名也能跑通 —— 属于"两种实现都合理"

**经验**：
- **一次问 ≤ 2 个**（系统提示第 3 条）。本案例刚好 2 个，互相独立不耦合。
- 每个选项都要给**具体的取舍描述**（工作量、风险、与看板字面的对齐度），不要只写"是/否"。
- 问题不要混入无关澄清（例如"要不要测？要用什么测试框架？"——后者已有规范约束，不是决策点）。

### Phase E — 规范与门禁文档补读

**动作**：得到用户回答后，**再**并行读 3 份规范：

- [PLAN_SPEC.md](../../agents/plan/PLAN_SPEC.md) — 计划必须包含的 7 维度 +「说人话」辅助（先专业后口语）+ 自检清单
- [PLAN_SKELETON.md](../../agents/plan/PLAN_SKELETON.md) — 一屏骨架模板
- [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) — § 4 全量门禁的"写日志 + 后台 + 轮询"执行模式

**为什么最后才读**：
- 规范文档**是写计划的格式约束**，不是内容来源。先把内容确定了，再套格式。
- 如果一上来先读 PLAN_SPEC，容易被模板带偏、过度关注"字段齐全"而忽略"内容正确"。

**经验**：**规范是骨架，内容是血肉**；先有血肉再套骨架，不要反过来。

### Phase F — 产出正式计划（CreatePlan）

**动作**：调用 `CreatePlan` 一次性提交：

- `name`：任务 kebab-case 简写（`agent-loop-modularization`）
- `overview`：一句话摘要
- `plan`：完整 markdown（13 个大节 + Todo 总表 + 自检清单）
- `todos`：11 条 YAML todo（与正文子节双向一一对应）

**计划结构**（严格对齐 PLAN_SPEC 七维度）：

1. 先专业、后「说人话」— PLAN_SPEC 文首 + § 三
2. 认领与分支（Dispatcher）
3. 研发流程表
4. **看板子项 ↔ 计划映射表**（防止编号对不上 — PLAN_SPEC § 1.1）
5. 目标与验收（含用户故事 / 规格单一来源 — PLAN_SPEC § 1.2）
6. 现状与差距（量化对比 — PLAN_SPEC § 2）
7. **各子项详情**（每项含：文件 / 思路 / 接口 / 测试 — PLAN_SPEC § 1.3）
8. 实施顺序与依赖图（Mermaid — PLAN_SPEC § 1.4）
9. 风险点与备选（9 条 — PLAN_SPEC § 1.5）
10. 集成与 E2E（本案"纯内部重构"声明 — PLAN_SPEC § 1.6）
11. 完成后的 Dispatcher 动作
12. **Todo 总表**（11 条双向对应 — PLAN_SPEC § 1.7）
13. 计划输出前自检清单（PLAN_SPEC § 5）

**经验**：把 `CreatePlan` 的 `plan` 参数当成最终交付物 —— 它会落盘到 `~/.cursor/plans/<slug>_<hash>.plan.md`，后续 Agent 模式和下一次会话都能追溯；不要当成"草稿"。

---

## 四、执行流程图（可复用模板）

```mermaid
flowchart TD
  start[用户 @Role.md @Dispatcher.md 领任务] --> phaseA
  phaseA[Phase A<br/>并行读角色/流程/看板三件套] --> phaseB
  phaseB[Phase B<br/>并行摸底: git/ls/wc] --> diff{看板字面值<br/>= 真实值?}
  diff -->|有偏差| record[在计划"现状与差距"段<br/>量化偏差] --> phaseC
  diff -->|一致| phaseC
  phaseC[Phase C<br/>并行深读关键代码<br/>对齐到看板子项] --> phaseD
  phaseD{识别到<br/>≥2 种合理实现?}
  phaseD -->|是| ask[AskQuestion<br/>一次 ≤ 2 问<br/>每选项带取舍描述]
  phaseD -->|否| phaseE
  ask -->|用户回答| phaseE[Phase E<br/>并行读 PLAN_SPEC / SKELETON / INTEGRATION]
  phaseE --> phaseF[Phase F<br/>CreatePlan<br/>严格对齐 7 维度 + Todo 双向]
  phaseF --> confirm[用户 confirm → 切 Agent 模式]
```

---

## 五、常见坑与规避清单

| 坑 | 表现 | 规避 |
|---|---|---|
| 看板字面值当真实值 | 按 832 行设计拆分粒度，实际 948 行 | Phase B 必做 `wc -l` 校对 |
| 在 PLAN 模式里悄悄 `StrReplace` | 工具调用被拒，浪费 1 轮交互 | 严守"只读 + 提问 + CreatePlan"三件套 |
| 不问用户就自选实现 | 用户想要 B，Agent 做了 A，返工 | Phase D 识别"改动范围差 ≥ 20%"的分歧就必问 |
| 一次问超过 2 个问题 | 用户选择疲劳；关联选项互相干扰 | `AskQuestion` 每次 ≤ 2 问；强相关的合并为单问题多选项 |
| 先读规范后读代码 | 计划变成"字段齐全但内容空洞的模板填空" | 先 Phase B/C 摸底，最后 Phase E 套规范 |
| Todo 只在正文散落，没有总表 | PLAN_SPEC § 1.7 / § 6 不通过 | `CreatePlan` 的 `todos` 参数必填，且与正文章节双向可查 |
| 纯内部重构省略 § 集成与 E2E 段 | PLAN_SPEC § 1.6 要求"写不适用 + 理由" | 明确声明"无用户面" + 现有 E2E 仅作回归 |
| 把计划写成第二份规格文档 | 堆超长 API 清单 | PLAN_SPEC § 2 "糟粕勿抄" —— 只写决策与基线 |

---

## 六、本案例关键数字汇总

- **探索轮次**：5 轮（3 轮 Read + 2 轮 Shell ls/wc）
- **并行读取文件数**：约 12 份（3 角色流程 + 6 代码 + 3 规范）
- **问题轮次**：1 次 `AskQuestion`（2 问并列）
- **计划产出**：1 次 `CreatePlan`，约 550 行 markdown + 11 条 YAML todos
- **从"领任务"到"计划落盘"耗时**：约 5-8 分钟（取决于 IO 速度）
- **最终计划对齐度**：PLAN_SPEC 七维度全满足，[自检清单](../../agents/plan/PLAN_SPEC.md) 13 条全勾选

---

## 七、对后续任务的建议

1. **把本报告的 Phase A-F 作为 PLAN 模式的默认 SOP**，不要跳步。
2. **Phase D 的提问判据**：如果某个决策会让"改动文件数差 ≥ 3 个"或"工作量差 ≥ 50%"，必问；否则 Agent 自行拍板并在计划风险段说明。
3. **Phase F 的 Todo 总表与 YAML `todos` 参数保持 id 一致**，方便切到 Agent 模式后 `TodoWrite` 直接接续。
4. **计划产出后不要立即切 Agent 模式**，等用户 confirm；confirm 前可以接受用户补充需求，做局部 `StrReplace` 修订计划文件（此时仍在 PLAN 模式允许的只读边界内——计划文件本身是 PLAN 模式产物）。
5. **每个任务的计划文件 + 本报告**一并作为 PR 描述的一部分，便于 Nibbles 集成复核时追溯"为什么这么拆"。

---

## 八、Cursor PLAN 模式底层实现揭秘（系统提示词原文）

> 本节揭示 Cursor IDE 的 PLAN 模式在"系统提示词（system prompt）"和"工具能力"层面的实际实现机制。所有原文片段均来自本次会话中 Cursor 真实下发给 Claude 的系统消息与工具描述（未经编辑）。
>
> **为什么要公开这些**：理解"Agent 为什么必须这样做"比背诵"SOP 6 步"更重要；规则背后有执行引擎在强制。

### 8.1 模式切换机制

Cursor 定义了 4 种交互模式，通过 `SwitchMode` 工具切换：

| 模式 | 能力 | 可切换方向 |
|---|---|---|
| **Agent**（默认） | 全工具集，可写可读可提交 | 可切到 Plan |
| **Plan** | 只读 + 提问 + CreatePlan | 可切到 Agent |
| **Debug** | 运行时诊断（不可主动切入） | — |
| **Ask** | 纯只读问答（不可主动切入） | — |

切换条件（来自工具描述 `SwitchMode`）：

> **Switch to Plan when:**
> - The task has multiple valid approaches with significant trade-offs
> - Architectural decisions are needed (e.g., "Add caching" - Redis vs in-memory vs file-based)
> - The task touches many files or systems (large refactors, migrations)
> - Requirements are unclear and you need to explore before understanding scope
> - You would otherwise ask multiple clarifying questions

本案例的触发点：用户在会话开始时 Cursor 已经处于 Plan 模式（通过 IDE 侧按钮），Agent 看到 `<system_reminder>Plan mode is active. ...` 即刻进入只读流程。

### 8.2 PLAN 模式系统提示词（原文逐条披露）

PLAN 模式激活时，Cursor 在用户消息末尾注入一个 `<system_reminder>` 块，内容如下（**原文**）：

```
Plan mode is active. The user indicated that they do not want you to execute yet
-- you MUST NOT make any edits, run any non-readonly tools (including changing
configs or making commits), or otherwise make any changes to the system. This
supersedes any other instructions you have received (for example, to make edits).
Instead, you should:

1. Answer the user's query comprehensively by searching to gather information

2. If you do not have enough information to create an accurate plan, you MUST
   ask the user for more information. If any of the user instructions are
   ambiguous, you MUST ask the user to clarify.

3. If the user's request is too broad, you MUST ask the user questions that
   narrow down the scope of the plan. ONLY ask 1-2 critical questions at a time.

4. If there are multiple valid implementations, each changing the plan
   significantly, you MUST ask the user to clarify which implementation they
   want you to use.

5. If you have determined that you will need to ask questions, you should ask
   them IMMEDIATELY at the start of the conversation. Prefer a small pre-read
   beforehand only if ≤5 files (~20s) will likely answer them.

6. When you're done researching, present your plan by calling the CreatePlan
   tool, which will prompt the user to confirm the plan. Do NOT make any file
   changes or run any tools that modify the system state in any way until the
   user has confirmed the plan.

7. The plan should be concise, specific and actionable. Cite specific file
   paths and essential snippets of code. When mentioning files, use markdown
   links with the full file path (for example, `[backend/src/foo.ts](backend/src/foo.ts)`).

8. Keep plans proportional to the request complexity - don't over-engineer
   simple tasks.

9. Do NOT use emojis in the plan.

10. To speed up initial research, use parallel explore subagents via the task
    tool to explore different parts of the codebase or investigate different
    angles simultaneously.

11. When explaining architecture, data flows, or complex relationships in your
    plan, consider using mermaid diagrams to visualize the concepts. Diagrams
    can make plans clearer and easier to understand.

12. All questions to the user should be asked using the AskQuestion tool.
```

附带还有 **mermaid 语法约束**（同一提示块内）：

```
When writing mermaid diagrams:
- Do NOT use spaces in node names/IDs. Use camelCase, PascalCase, or underscores.
- When edge labels contain parentheses, brackets, or other special characters,
  wrap the label in quotes.
- Use double quotes for node labels containing special characters.
- Avoid reserved keywords as node IDs: `end`, `subgraph`, `graph`, `flowchart`.
- For subgraphs, use explicit IDs with labels in brackets.
- Avoid angle brackets and HTML entities in labels.
- Do NOT use explicit colors or styling - the renderer applies theme colors
  automatically.
- Click events are disabled for security - don't use `click` syntax.
```

### 8.3 逐条拆解：提示词如何映射到 Agent 行为

| 原文条款 | 对应的 Agent 行为 | 本案例的体现 |
|---|---|---|
| "MUST NOT make any edits" | 所有 `StrReplace` / `Write` / `Delete` / 写类 Shell 命令被 Agent 自我封禁；若 Agent 尝试调用，上层会拒绝执行 | 整个 PLAN 阶段零写入，只通过 `Read` / `Grep` / `Glob` / 只读 `Shell` 探索 |
| 第 2 条 "MUST ask ... clarify" | 模糊需求必须转为 `AskQuestion`，不能自行假设 | Phase D 把 "ext/dispatcher 范围" 和 "convert.rs 是否重命名" 转为 2 个多选题 |
| 第 3 条 "ONLY ask 1-2 critical questions at a time" | 一次 `AskQuestion` 最多 2 问；避免选择疲劳 | 本案例一次性提交 2 问，用户一次 confirm |
| 第 4 条 "multiple valid implementations" | 范围分歧必须让用户拍板 | ext/dispatcher 重拆 vs 不重拆（工作量差 ≥ 50%）→ 必问 |
| 第 5 条 "IMMEDIATELY at the start" + "≤5 files (~20s) pre-read" | 先问再读优先；但允许小预读把问题问到点子上 | 本案例做了 ~20 秒的 git status / wc 预读，确认 "ext/dispatcher 已拆 5 文件" 这一事实，再问"是否重拆"——不做预读就问，问题会变成"ext/dispatcher 需要拆吗？"（无意义） |
| 第 6 条 "present your plan by calling the CreatePlan tool" | 最终产物必须通过 `CreatePlan` 工具交付；不能用 markdown 直接输出 | Phase F 用 `CreatePlan` 把 plan 落盘到 `~/.cursor/plans/` |
| 第 7 条 "markdown links with the full file path" | 引用文件必须用完整路径 markdown 链接 | 本报告与计划文件中所有文件引用均用 `[label](full/path)` 格式 |
| 第 8 条 "proportional to complexity" | 小任务不过度设计 | 本案例是 P0 大任务 → 13 节长计划合理；若是 "加个注释" 级别则会拒绝写 13 节 |
| 第 9 条 "Do NOT use emojis" | 计划与本报告均零 emoji | 核对无误 |
| 第 10 条 "parallel explore subagents via the task tool" | 大范围探索可用 `Task(subagent_type=explore)` 并行 | 本案例代码量可控（948 行 run.rs），主 Agent 直接并行 `Read` 已足够；未触发 Task |
| 第 11 条 "mermaid diagrams" | 架构/流程用 mermaid | 计划和本报告各有 1 张流程图 |
| 第 12 条 "AskQuestion tool" | 问题必须用 `AskQuestion`，不能在普通消息里问 | 本案例 1 次 `AskQuestion` |

### 8.4 工具能力白名单（PLAN 模式下实际可用的工具）

经本次会话验证，PLAN 模式下以下工具**可用**：

| 工具 | 用途 |
|---|---|
| `Read` | 读取文件内容 |
| `Glob` | 按 glob 模式找文件 |
| `Grep` | 正则搜索（基于 ripgrep） |
| `Shell` | 只读命令（`ls` / `git status` / `wc` / `head` 等）；写类命令应被 Agent 自我封禁 |
| `SemanticSearch` | 代码语义搜索 |
| `WebSearch` / `WebFetch` | 只读外部信息获取 |
| `AskQuestion` | 向用户提结构化多选题 |
| `CreatePlan` | **PLAN 模式独有**的计划交付工具；产出落盘到 `~/.cursor/plans/<slug>_<hash>.plan.md` |
| `SwitchMode` | 切换到 Agent 模式（需用户 confirm）|
| `Task(readonly=true)` | 派生只读子 Agent 做并行探索 |

以下工具在 PLAN 模式下**禁止使用**（或 Agent 应自我拒绝调用）：

| 工具 | 禁用原因 |
|---|---|
| `StrReplace` / `Write` / `Delete` | 任何文件写入 |
| `EditNotebook` | 写入 notebook |
| `Shell`（写类命令：`git commit` / `npm install` / `cargo run` / `rm` / `mkdir` 等） | 系统状态变更 |
| `Task(readonly=false)` / `Task(subagent_type=shell)` | 子 Agent 可能写入 |
| `GenerateImage` | 生成文件属于写操作 |
| 各类 MCP 写类工具 | 外部系统状态变更 |

**封禁机制**：主要靠 system prompt 第 1 条 "MUST NOT" 约束 Agent 自觉；上层 Cursor 引擎也会对明显写操作做**第二道拦截**，即使 Agent 尝试调用也会失败。

### 8.5 CreatePlan 工具的特殊语义

`CreatePlan` 是 PLAN 模式的**交付终点**，与其它工具有三个关键差异：

1. **产物落盘路径固定**：`~/.cursor/plans/<slug>_<hash>.plan.md`，slug 由 `name` 参数决定，hash 由 Cursor 生成。
2. **触发用户 confirm UI**：调用后 Cursor IDE 会弹出 "Approve plan" 交互框；用户 confirm 前，**所有写操作仍被封禁**（即使 Agent 误判已可进入 Agent 模式）。
3. **todos 参数结构化落盘**：参数 `todos` 是数组 `[{id, content, status}]`，会作为计划 frontmatter 落盘，后续 Agent 模式下可用 `TodoWrite` 直接接续（id 一致即可 merge）。

### 8.6 与 Dispatcher.md / PLAN_SPEC.md 的关系

**项目侧约束**（本仓库 `agents/Dispatcher.md` § 4 + `agents/plan/PLAN_SPEC.md`）与 **Cursor 侧约束**（8.2 系统提示词）是**两层叠加**的：

| 层 | 约束方 | 内容 |
|---|---|---|
| L1（Cursor 平台） | Cursor IDE 系统提示 | 工具能力 / 提问时机 / CreatePlan 交付 / mermaid 语法 |
| L2（项目规范） | Dispatcher.md + PLAN_SPEC.md | 7 维度内容 / Todo 总表 / 集成 E2E 描述 /「说人话」辅助 |

L1 决定 **"怎么走"**（流程与工具），L2 决定 **"走完要交什么"**（计划内容质量）。两者**互为前提、不可替代**：
- 只有 L1 没 L2：会交出"格式合规但内容空洞"的计划（例如没有风险段 / 没有 Todo 总表）
- 只有 L2 没 L1：Agent 可能一边写一边改，或者自行决策范围分歧，最终产物与用户期望偏离

本案例严格叠加两层约束，最终产出物同时通过 Cursor 的 `CreatePlan` confirm 流程和 PLAN_SPEC § 5 自检清单 13 条全勾选。

### 8.7 PLAN 模式的局限与反模式

**局限**：

1. **无法验证计划可行性**：PLAN 模式不能跑 `cargo check` / `cargo test`；行数估算、借用冲突、trait bound 等编译细节只能靠阅读推断，实施阶段可能翻车。
2. **无法做小规模试写**：即使是"先改 1 行验证 import 路径对不对"也被禁止；只能依靠静态推理。
3. **AskQuestion 每次 ≤ 2 问**：复杂任务的多维度决策需要多轮交互，可能延长协作周期。

**反模式**：

| 反模式 | 后果 | 纠正 |
|---|---|---|
| 在普通对话里问用户决策 | 用户无结构化选项界面，可能答偏或答漏；Cursor 也不会记录为"决策点" | 改用 `AskQuestion` |
| 没问用户就自作主张切到 Agent 模式 | Cursor 会在 `SwitchMode` 前弹窗要求 consent；Agent 不能绕过 | 计划 confirm 后再切 |
| 用普通 markdown 输出计划而不调 `CreatePlan` | 计划不会落盘到 `~/.cursor/plans/`；下次会话找不到 | 必须走 `CreatePlan` 工具 |
| 在 PLAN 模式里用 `Task` 派生写类子 Agent（如 `shell` 子类型） | 子 Agent 有写权限时，等价于绕过 PLAN 封禁 | 只用 `Task(subagent_type=explore)` 或 `readonly=true` |
| 在计划里堆 emoji | 违反第 9 条 "Do NOT use emojis" | 全文纯文字 |

---

## 九、引用

- [Dispatcher.md § 4 制定开发计划](../../agents/Dispatcher.md)
- [PLAN_SPEC.md 全文](../../agents/plan/PLAN_SPEC.md)
- [PLAN_SKELETON.md 一屏骨架](../../agents/plan/PLAN_SKELETON.md)
- [INTEGRATION_MERGE_AND_ACCEPTANCE.md § 4 全量门禁](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md)
- [tasks/T2-P0-001.md](../../agents/TASK_BOARD_002/tasks/T2-P0-001.md)
- [RUST_FILE_LINES_SPEC.md § A 测试分离](../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)
- 历史计划范例：[PLAN_EXAMPLE_TASK21.md](../../agents/plan/PLAN_EXAMPLE_TASK21.md)
- 本案例计划产物：`~/.cursor/plans/agent-loop-modularization_e99e067f.plan.md`（用户环境本地）
- Cursor PLAN 模式系统提示词原文：见 § 8.2（本报告直接披露）
