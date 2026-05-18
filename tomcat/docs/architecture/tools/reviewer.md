# `reviewer`：审稿 subagent 派生契约（非 LLM 工具）

> **重要**：`reviewer` **不是** LLM 工具，**也不进** catalog。它是一个**子 Agent**，由 [`create_plan`](./create-plan.md) 工具在写完 `PlanFile` 后通过**内部 Rust API**（`internal subagent dispatch`，对标 [codex `run_codex_thread_one_shot`](https://example/codex_delegate)）同步派发，与 LLM-facing 的 [`dispatch_agent`](../multi-agent.md) 工具互补。本文档定义 reviewer 的派生契约：派发入口、`allowed_tools`、system prompt 模板、**输出契约（顾问、非 gate）**、`allow_review_edit`（runtime 内部参数）行为、并发 / abort 语义。

本文档是 **B 类**：`docs/architecture/tools/`，承接 [`plan-runtime.md`](../plan-runtime.md)、[`multi-agent.md`](../multi-agent.md) §14（基础设施）、[`create-plan.md`](./create-plan.md)（派发入口）。**实现以仓库代码为准**。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：reviewer 是一个内部子 Agent，模型看不到也不能调；它由 `create_plan` 在写完计划文件后内部叫起来，读计划、读代码，给出**摘要建议**写进 transcript（`plan.review` 自定义事件）+ 同步返回到 `create_plan` 的工具结果里；它**不**决定能不能进 EXEC——进 EXEC 永远由用户敲 `/plan build <plan_id|path>`。默认 reviewer 还可以用 [`todos`](./todos.md) 给自己列调研步骤（无副作用、不动 plan）。如果 runtime 内部参数 `allow_review_edit=true`（**不**暴露为 LLM 入参），reviewer 还可以：① 用 [`update_plan`](./update-plan.md) 增量改 PlanFile frontmatter 的 `todos[]` / `milestones[]`；② 用 `edit` 改 `PlanFile.body` 的 `## Review` 段（不能动 frontmatter / `## Draft` / `## Goal` / `## Todos Board`）。**任何模式下 reviewer 都不能调 `create_plan`**（防递归套娃 + 职责单一）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 派发入口与决策表](#4-派发入口与决策表)
- [5. system prompt / allowed_tools / 输出契约 / allow_review_edit](#5-system-prompt--allowed_tools--输出契约--allow_review_edit)
- [6. One-Glance Map](#6-one-glance-map)
- [7. 调度时序](#7-调度时序)
- [8. 状态机 / 并发与 abort](#8-状态机--并发与-abort)
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
| **`reviewer`（subagent）** | 由 `create_plan` 内部派发的审稿子 Agent | `AgentRegistry` 中 `SubagentType::Reviewer`；不进 LLM catalog | 同步阻塞；输出 `ReviewSummary`（非 gate） | 子 Agent 形态的审稿员；只挑刺、不当 gate。 |
| **internal subagent dispatch** | 内部 Rust API 形态的子 Agent 派发入口（对标 codex [`run_codex_thread_one_shot`](https://example/codex_delegate)） | `AgentRegistry::spawn_subagent_internal(...)`（拟定） | 与 LLM-facing `dispatch_agent` 工具**互补**：复用 [`multi-agent.md`](../multi-agent.md) §14 基础设施，但不走 schema、不进 catalog | 内部派子 Agent，模型看不到。 |
| **`ReviewSummary`** | 审稿输出摘要 | `struct ReviewSummary { findings: Vec<Finding>, summary: String, applied_changes: bool }`；`findings.severity ∈ {nit, suggestion, concern}` | reviewer 在最终消息中以约定 fenced block 给出；runtime 解析后落 `transcript.plan.review` + 回填到 `create_plan` tool result | 「过/不过」二态被废弃；只给挑刺清单 + 一句话总结。 |
| **`allow_review_edit`（runtime 内部参数）** | 是否允许 reviewer 修订 PlanFile（增量改 todos/milestones、写 `## Review` 段） | `bool`；由 `create_plan` 派发时**在代码里**传入 reviewer dispatch；**不**作为 LLM 可见工具入参 | `false`（默认）：reviewer 只读 + 个人 scratchpad（`{read, grep, find, todos}`）+ 输出摘要；`true`：附加 `{update_plan, edit}`——前者改 frontmatter `todos[]` / `milestones[]`，后者仅限 `## Review` 段；**任何模式都不附加 `create_plan`** | 让 reviewer 能动笔还是只挑刺，由代码决定；动笔时通过结构化 `update_plan` + 正文 `edit` 双通道。 |
| **`/plan build` 与 reviewer 解耦** | 进 EXEC 由用户敲 `/plan build`，与 reviewer verdict 无关 | `PlanRuntime::on_build_command` 不读 `ReviewSummary` | reviewer 在 PLAN 模式期间可能跑多次，每次只追加 transcript 摘要 | reviewer 是顾问，进 EXEC 用户拍板。 |
| **R-DispatchEntry**（决策） | 「reviewer 派发入口走哪里」决策行 | §4.1 决策表 | 入选：internal subagent dispatch（不进 catalog） | 派发位置选定。 |

---

## 2. 竞品 / 选型对比（调研）

### 2.1 reviewer 的典型形态

```text
┌──────────────────────────────────────────────────────────────────────┐
│  reviewer 在主流 agent 里大致三种形态                                │
├──────────────────────┬───────────────────────────────────────────────┤
│  LLM-facing tool     │  openclaw：sessions_spawn 让 LLM 自调           │
│                      │  优点：通用；缺点：模型可乱调                   │
├──────────────────────┼───────────────────────────────────────────────┤
│  内部 Rust 函数      │  codex：codex_delegate.rs 内部调用              │
│                      │  优点：边界清晰、不入 schema；缺点：对外不可见  │
├──────────────────────┼───────────────────────────────────────────────┤
│  通用 dispatch 工具  │  hermes：delegate_task + role                  │
│  + role 参数         │  优点：单工具多角色；缺点：审稿权限边界靠 prompt │
└──────────────────────┴───────────────────────────────────────────────┘
```

**说人话**：审稿要解决入口形态（工具 vs 内部函数）、同步还是异步、工具白名单谁定、输出契约（verdict gate vs 顾问摘要）、能不能改稿五件事。

### 2.2 常见实现横向对比

| 来源 / 形态 | reviewer 入口 | 同步/异步 | `allowed_tools` 形态 | 输出契约 | 改稿权 | 说人话 |
|-------------|---------------|-----------|----------------------|----------|--------|--------|
| **codex** | `run_codex_thread_one_shot`（内部 Rust） | 同步 | 调用方代码硬编码 | 摘要 + 是否阻塞，视调用方 | 视实现 | 内部派，边界硬。 |
| **hermes-agent** | `delegate_task(role='reviewer', toolsets=[...])` | 同步 | LLM 入参 | 文本摘要 | 视 toolsets | LLM 调度，边界软。 |
| **cc-fork-01** | 内部 `VERIFICATION_AGENT_TYPE` | 同步 | 内部硬编码 | 二态 verdict（gate） | 否 | 内部派 + 只读 + gate。 |
| **openclaw** | `sessions_spawn`（LLM tool） | 异步 | LLM 入参 | 摘要 | 视入参 | LLM 调度，权限边界软。 |
| **GenericAgent** | VERIFY/VERDICT 内部流程 | 同步 | 内部硬编码 | 二态 verdict | 否 | 内部派 + 只读 + gate。 |
| **本仓库 `reviewer`（D 方案）** | **internal subagent dispatch**（同 codex） | **同步阻塞** | **调用方硬编码 + `allow_review_edit` 旗标**：默认 `{read, grep, find, todos}`；true → 附加 `{update_plan, edit}`；**永不含 `create_plan`** | **顾问形态：`ReviewSummary` + transcript `plan.review`；不当 gate** | **`allow_review_edit=true` 时通过 `update_plan` 改 frontmatter todos/milestones + `edit` 改 `## Review` 段** | 取 codex 内部派形态；废弃 verdict gate；让 reviewer 通过同一套结构化工具落地修订。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| RV1 派发入口 | LLM tool / 内部 Rust 函数 / 通用 dispatch + role | 内部 Rust 函数（同 codex）。 |
| RV2 同步 vs 异步 | 同步阻塞 / 异步事件回调 | 同步阻塞，`ReviewSummary` 与 `create_plan` 同条 tool result 返回。 |
| RV3 `allowed_tools` 形态 | 入参 / 调用方硬编码 / runtime 默认 | 调用方硬编码（不暴露 LLM）。 |
| RV4 输出契约 | 二态 verdict（gate） vs 摘要顾问 | **顾问形态**：`findings[] + summary + applied_changes`；不做 gate。 |
| RV5 改稿权 | 只读 / 可写计划 / 可写代码 | 可写计划**正文 `## Review` 段**（runtime 内部参数控制）；不可写 frontmatter；不可写代码。 |
| RV6 与 `dispatch_agent` 关系 | 复用 schema / 完全独立 / 共享基础设施 | 独立 schema、共享 §14 基础设施。 |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§11） | 说人话 |
|----|------|------------------|--------|
| G1 | reviewer 不进 LLM catalog；不出现在任何工具 schema 中 | `reviewer_not_in_catalog` | 模型调不到 reviewer。 |
| G2 | 派发走 `internal subagent dispatch`；复用 `multi-agent.md` §14 基础设施 | `reviewer_uses_internal_dispatch_via_agent_registry` | 内部 API 派，复用注册表。 |
| G3 | `allowed_tools` 在调用方代码硬编码（默认 `{read, grep, find, todos}`；`allow_review_edit=true` 附加 `{update_plan, edit}`，其中 `edit` 仅限 `~/.tomcat/plans/*.plan.md` 的 `## Review` 段；**永不**含 `create_plan`） | `reviewer_default_allowed_tools_no_create_plan`、`reviewer_subagent_blocks_bash_checkpoint_write_frontmatter` | 工具白名单写死，frontmatter 永远不许 raw 改；`create_plan` 永远不暴露给 reviewer。 |
| G4 | 输出契约是 `ReviewSummary`（摘要顾问），**不做 verdict gate**；进 EXEC 永远由用户 `/plan build` | `reviewer_summary_does_not_gate_exec`、`build_command_ignores_review_summary` | reviewer 不挡进度，进 EXEC 用户拍。 |
| G5 | `allow_review_edit=true` 时 reviewer 通过 `update_plan` 修改 frontmatter `todos[]` / `milestones[]`（受 `update_plan` 本身的门控约束）；通过 `edit` 改 `## Review` 段；写其它段或 frontmatter raw → tool error | `reviewer_edit_scoped_to_review_section`、`reviewer_cannot_touch_frontmatter_raw`、`reviewer_can_use_update_plan_when_allowed` | 改 frontmatter 走 update_plan，正文段走 edit，分工清晰。 |
| G6 | `max_review_rounds` 默认 1；超限不阻塞、不切 mode，只追加 warning 到 transcript | `reviewer_respects_max_review_rounds_advisory` | 审太多次也不卡进度，只提醒人工看一眼。 |
| G7 | 父 Agent abort 触发 reviewer abort（CascadeAbort） | `reviewer_aborts_on_parent_cascade` | 父停子也停。 |
| G8 | reviewer 摘要写 `transcript.plan.review` 自定义事件；不写 `PlanFile` frontmatter | `reviewer_summary_lands_in_transcript_plan_review`、`reviewer_does_not_write_frontmatter` | 审稿结果只进对话流水，不污染计划元数据。 |

**说人话（§3 总览）**：reviewer 是 create_plan 内部的子 Agent——不进工具表、工具名单硬编码、输出是顾问摘要（**不**当 gate）、可选改 `## Review` 段、父 abort 级联、摘要写 transcript 不写 frontmatter。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| reviewer 作为 LLM 工具 | §13 已否决 | 别暴露给模型。 |
| reviewer 写/改代码 | 不含 write/bash；`edit` 仅作用于 `~/.tomcat/plans/*.plan.md` 的 `## Review` 段 | 只能看计划/仓库，不能改代码。 |
| reviewer 再派 sub-reviewer | `role = leaf` | 不能再派子 Agent。 |
| reviewer 决定 PlanFile.mode | 由 `/plan build` 命令 + runtime 派生（completed / pending）共同维护 | reviewer 不动 frontmatter 也不动 mode。 |

---

## 4. 派发入口与决策表

### 4.1 落地选型决策表

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| **R-DispatchEntry**（派发入口） | **internal subagent dispatch**（内部 Rust API，不进 catalog；对标 codex `run_codex_thread_one_shot`） | LLM-facing tool 易被乱调；通用 `dispatch_agent` schema 暴露过多权限边界细节 | 内部派，对标 codex。 |
| RV2 同步 vs 异步 | 同步阻塞 await；`ReviewSummary` 与 `create_plan` 同条 ToolResult 返回 | 异步派会让 LLM 在没拿到摘要的情况下盲改 | 必须同步带回摘要。 |
| RV3 `allowed_tools` 形态 | 调用方代码硬编码：默认 `{read, grep, find, todos}`；`allow_review_edit=true` 附加 `{update_plan, edit}`（`edit` `tool_exec` 守卫：路径 ⊆ `~/.tomcat/plans/*.plan.md` 且 diff ⊆ `## Review` 段）；**任何模式都不含 `create_plan`** | LLM 入参形态会绕过 catalog；通用 schema 失去针对性；reviewer 持有 `create_plan` 会无限套娃 | 白名单写死；frontmatter raw 改 / `## Draft` / `## Goal` 永远拦死；frontmatter todos/milestones 走 `update_plan` 通道。 |
| RV4 输出契约 | **顾问摘要**：`ReviewSummary { findings[], summary, applied_changes }`；**不**做 verdict gate；不切 mode | verdict 二态被用户决策为「reviewer 仅辅助」 | 不挡进度。 |
| RV5 改稿权 | `allow_review_edit` runtime 内部参数显式控制；**不**作为 LLM 可见工具入参 | 暴露给 LLM 会引导模型主动开权限；交由代码决定保持边界 | 改稿权代码说了算。 |
| RV6 与 `dispatch_agent` 关系 | 共享 `multi-agent.md` §14 `AgentRegistry` / `spawn_depth` / `CascadeAbort`；不共享 schema | 共享 schema 会污染 `dispatch_agent` 的 `subagent_type` 枚举位 | 共用底座，不共用工具 schema。 |
| RV7 spawn_depth 计费 | reviewer 派发**算入** `spawn_depth`（与 `dispatch_agent` 计费一致） | 不计费会让 reviewer 嵌套绕过深度限制 | 算一层嵌套深度。 |
| RV8 `subagent_type` 枚举位 | reviewer 在内部使用 `SubagentType::Reviewer`；该值**不**出现在 `dispatch_agent` schema enum | LLM 不应知道有「reviewer」这种角色 | 内部枚举，模型看不见。 |
| RV9 摘要落点 | `transcript.plan.review` 自定义事件 + 同步回填 `create_plan` tool result.review | 写 frontmatter 会让 frontmatter 字段膨胀；用户也不需感知 `review_status` 之类字段 | 走 transcript，frontmatter 干净。 |

### 4.2 实施点（拟定）

> 与 [`plan-runtime.md`](../plan-runtime.md) **PR-PLC** 对齐；由 [`create-plan.md`](./create-plan.md) **CP-D** 调用；当前代码 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **RV-A** | `review::dispatch_reviewer` + `spawn_subagent_internal`；不进 catalog；**交付**：内部 API | `src/api/chat/plan_runtime/review.rs`（拟定） | 见 §11：`reviewer_not_in_catalog`、`reviewer_uses_internal_dispatch_via_agent_registry`（PENDING） | 只有 create_plan 内部能派审稿员。 |
| **RV-B** | `REVIEWER_SYSTEM_PROMPT` + `allowed_tools` 硬编码（`allow_review_edit` 分支）：默认 `{read, grep, find, todos}`；true → 附加 `{update_plan, edit}`；**永不含 `create_plan`**；**交付**：`SubagentType::Reviewer` | `review.rs`、`src/core/agent_loop.rs` | 见 §11：`reviewer_default_allowed_tools_no_create_plan`、`reviewer_can_use_update_plan_when_allowed`、`reviewer_subagent_blocks_bash_checkpoint_write_frontmatter`（PENDING） | 工具白名单写死，默认只读 + todos；改稿走 update_plan + edit。 |
| **RV-C** | 解析子 Agent 最终消息为 `ReviewSummary { findings[], summary, applied_changes }`；**不**驱动 mode 转移；**交付**：`ReviewSummary` + transcript 事件写入 | `review.rs`、`src/api/chat/plan_runtime/mod.rs`（拟定）、`src/infra/transcript/...`（既有） | 见 §11：`reviewer_summary_does_not_gate_exec`、`reviewer_summary_lands_in_transcript_plan_review`（PENDING） | 摘要进 transcript，不动 mode。 |
| **RV-D** | reviewer 调 `edit`/`create_plan` 时跳过递归内联 reviewer；`tool_exec.edit` 拦截 reviewer 进程：路径限制 + diff 限制在 `## Review` 段；**交付**：`parent_subagent_type` 检测 + `review_section_diff_guard` | `tool_exec.rs` + `review.rs` | 见 §11：`reviewer_create_plan_does_not_redispatch_reviewer`、`reviewer_edit_scoped_to_review_section`（PENDING） | 审稿员改稿只能动审稿区，且不再套审稿。 |
| **RV-E** | `CascadeAbort` + `max_turns` + `max_review_rounds`（advisory）；**交付**：超时/abort 语义 | `src/core/agent_registry.rs`、`review.rs` | 见 §11：`reviewer_aborts_on_parent_cascade`、`reviewer_respects_max_review_rounds_advisory`（PENDING） | 父停子停，审太多轮只是 warning。 |

下文按实施点展开**技术要点与示意图**；**API 签名见 [§4.3](#43-派发入口api-形态)**。

#### 4.2.1 RV-A：`dispatch_reviewer` 与 internal dispatch

- **交付**：`create_plan` 落盘成功后唯一调用点；`AgentRegistry::spawn_subagent_internal(cfg, build_review_prompt(plan))` **同步 await**。
- **事件**：`SubAgentStart` / `SubAgentEnd` 带 `parent_session_id`；UI 可选展示子树进度。
- 派发参数中 `allow_review_edit` 由 `create_plan` 调用方根据 runtime 配置传入；**不**经 LLM 暴露。

```text
  create_plan::write_plan OK
        │
        ▼
  dispatch_reviewer(plan, allow_review_edit, parent_handle)
        │
        ▼
  spawn_subagent_internal ──await──▶ ReviewSummary
        │
        ▼
  ① transcript.plan.review 自定义事件追加
  ② create_plan ToolResult.review 同条返回
  ③ PlanRuntime.mode 保持不变（仍为 planning）
```

**说人话**：用户只见 create_plan；审稿在工具内部同步跑完；摘要进 transcript + 回填 tool result；mode 不动。

#### 4.2.2 RV-B：`allowed_tools` 与 system prompt

- **交付**：`allow_review_edit=false` → `{read, grep, find, todos}`；`true` → 附加 `{update_plan, edit}`（仍无 `bash` / `write` / `create_plan` / `dispatch_agent`）。
- **双保险**：`resolve_internal_tools(parent_catalog, allowed_tools)` 与父 catalog 取交集；`tool_exec.edit` 在 `parent.subagent_type == Reviewer` 路径上额外执行 `review_section_diff_guard`（见 RV-D）；`tool_exec.update_plan` 在 reviewer 上下文中继承自身的 mode-aware 门控（target.mode=completed 拒绝；跨 session 改 executing 拒绝），无需额外守卫。

```text
  allow_review_edit?
     │        │
    false    true
     │        │
     ▼        ▼
  read/     read/ + edit（仅 ~/.tomcat/plans/*.plan.md，且 diff ⊆ ## Review 段）
  grep/find grep/find
```

**说人话**：默认只能看；开了改稿权也只能动审稿区，连 frontmatter 都碰不到。

#### 4.2.3 RV-C：`ReviewSummary` 解析与摘要落点

- **交付**：reviewer 最终消息中以 fenced block 给出固定结构（详见 §5.1）；runtime 严格解析；解析失败 → `create_plan` tool error，附最后一条消息摘要。
- **落点**：解析成功后**同时**：
  1. 追加 `transcript.plan.review` 自定义事件（`session-storage.md` 注册）；
  2. 回填到 `create_plan` 的 `ToolResult.review`；
  3. **不**写 `PlanFile` frontmatter；
  4. **不**修改 `PlanRuntime.mode`（mode 仍为 `planning`）。

**说人话**：摘要进对话流水 + 工具结果；不动计划元数据；不动模式。

#### 4.2.4 RV-D：防递归内联 reviewer + 改稿落点守卫

- **防递归**：`tool_exec::create_plan` 检测 `AgentLoopConfig.subagent_type == Reviewer` → **跳过** `dispatch_reviewer`（实际上 reviewer 的 `allowed_tools` 已不含 `create_plan`，这是双保险）。
- **改稿守卫**：`tool_exec::edit` 在 reviewer 进程下：
  1. `path` 必须 `~/.tomcat/plans/*.plan.md`，否则 tool error；
  2. 计算 diff，**只允许**改动落在 `## Review` 段内；touch 到 frontmatter / `## Goal` / `## Draft` / `## Todos` 一律 tool error；
  3. 写盘走与 `create_plan` 同一份 advisory lock。

**说人话**：审稿员就算给了改稿权，也只能在审稿区里写感想；frontmatter、目标、初稿、todos 全都拦死。

#### 4.2.5 RV-E：abort、轮次与深度

- **交付**：`spawn_depth = parent + 1`；超 `MAX_SPAWN_DEPTH` 在 **dispatch** 前拒（若未来允许 reviewer 链式调用）；`max_review_rounds` 超限 → **warning + 不阻塞**（不切 mode，不拒绝 `create_plan`，留 `transcript.plan.review.warning`）。
- **abort**：父 `abort_signal` → 子 reviewer 间隙检查并退出 → `create_plan` 仍返回成功但 `ToolResult.review` 标 `aborted`，文件保留。

**说人话**：审稿算一层子 Agent，卡住能 abort，审太多次只是 warning，不挡用户后续 `/plan build`。

### 4.3 派发入口（API 形态）

```rust
pub async fn dispatch_reviewer(
    plan: &PlanFile,
    allow_review_edit: bool,            // runtime 内部参数；非 LLM 入参
    parent: &AgentHandle,
) -> Result<ReviewSummary> {
    let allowed_tools: &[&str] = if allow_review_edit {
        &["read", "grep", "find", "todos", "update_plan", "edit"]
    } else {
        &["read", "grep", "find", "todos"]
    };

    let cfg = AgentLoopConfig {
        parent_session_id: Some(parent.session_id.clone()),
        spawn_depth:       parent.spawn_depth + 1,
        subagent_type:     SubagentType::Reviewer,
        role:              Role::Leaf,
        tool_definitions:  resolve_internal_tools(parent_catalog, allowed_tools),
        max_turns:         REVIEWER_MAX_TURNS,
    };

    let initial_msg = build_review_prompt(plan, allow_review_edit);
    AgentRegistry::spawn_subagent_internal(cfg, initial_msg).await
}
```

### 4.4 与 `multi-agent.md` 的关系

详见 [`multi-agent.md`](../multi-agent.md) §14.6.1：

- 共享：`AgentRegistry` / `spawn_depth` / `MAX_SPAWN_DEPTH` / `CascadeAbort` / `SubAgentStart`/`End` 事件。
- 不共享：`dispatch_agent` 工具 schema 中的 `subagent_type` 枚举（reviewer 用 `SubagentType::Reviewer` 内部枚举位）。

**说人话**：走 `spawn_subagent_internal`，和 dispatch_agent 用同一套注册表/深度/级联中止，但 LLM 永远调不到 reviewer。

---

## 5. system prompt / allowed_tools / 输出契约 / allow_review_edit

### 5.1 reviewer system prompt（常量内容）

```rust
pub const REVIEWER_SYSTEM_PROMPT: &str = r#"
You are a strict, read-mostly plan reviewer. You are NOT the user-facing agent.
You are NOT a gate: your output is advisory. The user decides whether to enter
EXEC mode by issuing `/plan build`.

Inputs:
- A PlanFile written under ~/.tomcat/plans/<slug>_<hash>.plan.md.
- Tools you may use:
  * read / grep / find: inspect the repo and the PlanFile.
  {{#if allow_review_edit}}
  * edit: write into the `## Review` section of the same PlanFile ONLY.
    The runtime will reject any diff that touches frontmatter, `## Goal`,
    `## Draft`, or `## Todos`.
  {{/if}}

Output contract (must produce as the final assistant message, exact format):

```
<plan_review>
findings:
  - { severity: nit|suggestion|concern, area: "<goal|draft|todos|other>", note: "<one-line concrete remark>" }
  - ...
summary: <one-paragraph rationale, <= 600 chars>
</plan_review>
```

Rules:
1. Output is advisory. Do NOT phrase your findings as gate verdicts; do NOT
   recommend `/plan build` or its absence. The user decides.
2. Severities: `nit` (style/cleanup), `suggestion` (consider adjusting),
   `concern` (substantive risk worth flagging).
3. Do not invent steps the user didn't ask for. The PlanFile's `goal` is the
   source of truth.
4. Do not modify code; you can only modify the PlanFile (when allowed) and
   ONLY within the `## Review` section.
5. Frontmatter is off-limits. Never emit edits whose diff touches the YAML
   frontmatter block; the runtime will reject such edits.
6. Stay within {{max_turns}} turns; if you cannot decide by then, output what
   you have with a clear summary of what's still unclear and stop.
"#;
```

### 5.2 allowed_tools

| 模式 | `allowed_tools` | 备注 | 说人话 |
|------|-----------------|------|--------|
| `allow_review_edit = false`（默认） | `{read, grep, find, todos}` | reviewer 评价 + 个人 scratchpad；`todos` 只写自己的 `.todo.md`，无副作用 | 只读 + 给自己记调研步骤。 |
| `allow_review_edit = true` | `{read, grep, find, todos, update_plan, edit}` | `update_plan` 仅能动 target 的 `todos[]` / `milestones[]`（受其本身门控）；`edit` 路径 ⊆ `~/.tomcat/plans/*.plan.md` 且 diff ⊆ `## Review` 段 | 给权限就能改 frontmatter todos/milestones + 写 `## Review` 段。 |

> **三条铁律**：
> 1. **永远不含 `create_plan`**——避免 reviewer 套娃 / 重定计划；reviewer 只挑刺与落地建议，不重写整盘。
> 2. **永远不含 `bash` / `write` / `dispatch_agent` / `checkpoint`**——副作用收到最小集。
> 3. **双保险**：`AgentRegistry::spawn_subagent_internal` 构造子 catalog 时与父 catalog 取交集；`tool_exec.edit` 与 `tool_exec.update_plan` 在 reviewer 上下文中各自有守卫；即使 `allowed_tools` 错误传入了不存在的工具名，runtime 也会过滤。

### 5.3 输出契约（`ReviewSummary`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `findings` | `Vec<Finding { severity, area, note }>` | 离散挑刺清单；`severity ∈ {nit, suggestion, concern}` | 一条条挑刺记下来。 |
| `summary` | `String`（≤ 600 字符） | 总评，自由文本 | 一句话总结。 |
| `applied_changes` | `bool` | 本轮 reviewer 是否调用过 `edit` 修改 PlanFile（仅可能落在 `## Review` 段） | 有没有真改过审稿区。 |
| `rounds` | `u32` | 同一 PlanFile 累计被 review 的轮次（含本次） | 这是第几轮审稿。 |

`ReviewSummary` **不**含 `verdict`、**不**含 mode 转移建议、**不**含 `accepted`/`rejected` 标记。

### 5.4 摘要落点：`transcript.plan.review`

- runtime 解析 `ReviewSummary` 后立刻写一条 `transcript.plan.review` 自定义事件（注册见 [`session-storage.md`](../session-storage.md)），结构示意：

```jsonc
{
  "type": "plan.review",
  "plan_id":   "<plan_id>",
  "rounds":    1,
  "findings":  [/* ... */],
  "summary":   "...",
  "applied_changes": false,
  "session_id": "<reviewer subagent session_id>"
}
```

- **同步**回填到 `create_plan` 的 `ToolResult.review` 字段（结构与上一致），让父 Agent 在同一条 tool result 里就拿到摘要，无须再读 transcript。
- **不**写 `PlanFile` frontmatter；frontmatter 仍保持 `mode = planning`、不引入 `review_status` / `last_review` 字段（详见 [`create-plan.md` §5.2](./create-plan.md#52-frontmatter-schema)）。
- 进 EXEC 由 `/plan build` 触发，与 `ReviewSummary` 完全解耦（见 [`plan-runtime.md` §5.1](../plan-runtime.md#51-plan-build-的-5-件事)）。

### 5.5 `allow_review_edit` 行为

| 步骤 | `allow_review_edit = false` | `allow_review_edit = true` | 说人话 |
|------|------------------------------|-----------------------------|--------|
| reviewer 可用工具 | `{read, grep, find, todos}` | `{read, grep, find, todos, update_plan, edit}` | 改稿权一开就多两件武器。 |
| 改 frontmatter `todos[]` / `milestones[]` | ❌（无 `update_plan`） | ✅（通过 `update_plan`） | 改 plan 待办用结构化工具。 |
| 改正文 `## Review` 段 | ❌（无 `edit`） | ✅（`edit` 路径白名单 + 段位守卫） | 审稿区允许动笔。 |
| 改正文 `## Draft` / `## Goal` / `## Todos Board` | ❌ | ❌（`edit` 守卫拦截） | 这些段位 reviewer 永远不动。 |
| 改 frontmatter 其它字段（mode / session_* / plan_id / created_at / goal） | ❌ | ❌（`update_plan` 与 `edit` 各自门控均拦截） | runtime 专属字段不许碰。 |
| 改用户代码（仓库其他路径） | ❌ | ❌（`edit` 路径不在 `~/.tomcat/plans/`） | 用户代码不动。 |
| reviewer turn 内可调 `create_plan`？ | ❌ | ❌（永远不在 `allowed_tools` 内） | reviewer 永远不重建计划。 |
| reviewer 改完后必须给摘要？ | — | ✅（仍要 emit `<plan_review>` 块） | 改完也要总结。 |
| `ReviewSummary.applied_changes` | `false` | reviewer `update_plan` / `edit` 调用 ≥ 1 时为 `true` | 记有没有真改过 plan。 |
| reviewer 越界写其它段或 frontmatter 字段 | — | `tool_exec.edit` / `tool_exec.update_plan` 各自拒绝；reviewer 收 tool error，下一轮自行调整 | 越界就 error。 |

---

## 6. One-Glance Map

| 路径 | 职责 | 说人话 |
|------|------|--------|
| `src/api/chat/plan_runtime/review.rs`（拟定） | reviewer 派发入口；`allowed_tools` 硬编码；`allow_review_edit` 透传；`ReviewSummary` 解析 | 派生 + 解析审稿结果。 |
| `src/core/agent_registry.rs`（既有/拟扩展） | `spawn_subagent_internal(...)`；`SubagentType::Reviewer` 内部枚举位；递归内联 reviewer 抑制（`parent.subagent_type == Reviewer` 时跳过） | 内部 spawn + 防套娃。 |
| `src/core/agent_loop.rs`（既有） | `AgentLoopConfig` 含 `subagent_type` / `role` 字段；reviewer 初始化时挂 `REVIEWER_SYSTEM_PROMPT` | 挂审稿提示词。 |
| `src/api/chat/plan_runtime/tool_exec.rs`（既有/拟扩展） | reviewer 进程下 `edit` 二次守卫：路径白名单 + `## Review` 段 diff guard | 改稿落点守门员。 |
| `src/api/chat/plan_runtime/file_store.rs`（拟定） | reviewer 调 `edit` 时复用同一份 advisory lock 与 round-trip 逻辑 | 改稿走同一套写盘。 |
| `src/infra/transcript/...`（既有） | `SubAgentStart` / `SubAgentEnd` 事件；`plan.review` 自定义事件写入；UI 可显示 reviewer 进度（可选） | transcript 落审稿摘要。 |

**阅读顺序（说人话）**：`create_plan` 落盘 → `review.rs` 派 internal reviewer → `agent_registry` spawn → reviewer 跑完 → 解析 `ReviewSummary` → 写 `transcript.plan.review` + 回填 `create_plan` tool result.review。

---

## 7. 调度时序

```
父 Agent (PLAN 模式)
  │
  └─ tool_call("create_plan", { goal, draft, todos, milestones })
        │
        ▼
   create_plan 工具 (file_store::write_plan → 落盘成功)
        │
        ▼
   review::dispatch_reviewer(plan, allow_review_edit=runtime_default, parent_handle)
        │
        ▼
   AgentRegistry::spawn_subagent_internal(cfg, build_review_prompt(plan))
        │
        ├──── SubAgentStart 事件
        │
        │  reviewer subagent 运行：
        │   - read / grep / find 调研、todos 自己记调研步骤
        │   - 若 allow_review_edit=true：
        │       * update_plan 改 PlanFile.frontmatter.todos[] / milestones[]（受其本身门控）
        │       * edit 改 PlanFile.body 的 ## Review 段（路径白名单 + 段位 diff guard）
        │     （tool_exec.edit guard 拒绝 frontmatter / ## Draft / ## Goal / ## Todos 的修改）
        │   - 输出 final assistant message：
        │     <plan_review>findings: ... summary: ...</plan_review>
        │
        ├──── SubAgentEnd 事件
        ▼
   ReviewSummary { findings, summary, applied_changes, rounds }
        │
        ▼
   ① transcript 追加 plan.review 自定义事件
   ② create_plan ToolResult.review 同条返回
   ③ PlanRuntime.mode 保持 planning（不切 mode）
        │
        ▼
   父 Agent 看到 review 摘要 → 继续 PLAN 对话 / 修正 create_plan
        │
        ▼
   （任意轮次后）用户敲 /plan build <plan_id|path>
        │
        ▼
   PlanRuntime 切 mode=executing；与 review 摘要无关
```

**说人话**：用户只见 create_plan 一次调用；背后起审稿子 Agent，读完计划/仓库（可选改审稿区），把摘要塞回同一条 tool result + transcript；mode 不变；进 EXEC 由用户 `/plan build` 拍。

---

## 8. 状态机 / 并发与 abort

### 8.1 reviewer 单次调用内部子状态

```
┌──────────┐ spawn  ┌──────────┐ emit ReviewSummary ┌──────────┐
│ Pending  │───────▶│ Running  │───────────────────▶│ Returned │
└──────────┘        └─────┬────┘                    └──────────┘
                          │ parent abort / max_turns / parse error
                          ▼
                    ┌──────────┐
                    │ Aborted  │
                    └──────────┘
```

**说人话**：一次审稿调用里，子 Agent 要么正常 emit 出 `ReviewSummary`，要么被父 abort/超时/解析失败打成 Aborted。

### 8.2 并发约束

| 约束 | 处理 | 说人话 |
|------|------|--------|
| 同一 active 计划 | `create_plan` 入口 mutex 串行「写盘 → 派 reviewer → 落 transcript」 | 一份计划同时只审一轮。 |
| 进程级并发上限 | 复用 `MAX_CONCURRENT_AGENTS` / `MAX_CHILDREN_PER_AGENT` | 跟 multi-agent 共用上限。 |
| 嵌套深度 | reviewer 计入 `spawn_depth`，受 `MAX_SPAWN_DEPTH` 约束 | 算一层子 Agent 深度。 |

### 8.3 abort 语义

- 父 Agent 收到 abort → `CascadeAbort` 触发 reviewer `abort_signal`，reviewer 在 reasoning 间隙退出。
- reviewer abort / 超时 / 解析失败 → `create_plan` **仍返回成功**（落盘已成功），但 `ToolResult.review` 标 `aborted: true` 且 `summary = "<aborted>"`，transcript 追加 `plan.review.warning` 摘要；**不**切 mode，**不**让 `/plan build` 失败。

**说人话**：单次审稿：Pending→Running→Returned 或 Aborted；父停则子停；审稿挂了/超时也不挡用户后续 `/plan build`，摘要标个 aborted 就行。

---

## 9. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_REVIEWER_MAX_TURNS` | `8` | reviewer subagent 最大 reasoning 轮次 | 审稿最多 8 轮推理。 |
| `TOMCAT_REVIEWER_MODEL` | 继承父 Agent | 显式覆写 reviewer 使用的模型 | 可单独指定审稿模型。 |
| `TOMCAT_PLAN_MAX_REVIEW_ROUNDS` | `1` | 单个 `PlanFile` 累计 reviewer 派发轮次软上限；超限只 warning，不阻塞 | 软上限，超了只提醒。 |
| `TOMCAT_REVIEWER_DEFAULT_ALLOW_EDIT` | `false` | `dispatch_reviewer` 调用时未显式指定 `allow_review_edit` 时使用的默认值 | 默认改不了审稿区。 |
| `TOMCAT_REVIEWER_SYSTEM_PROMPT_OVERRIDE_PATH` | 未设 | 测试用：从指定文件读取 system prompt 覆写默认常量 | 单测可换审稿提示词。 |

---

## 10. 错误模型 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| reviewer 子 Agent 异常退出 | `create_plan` 仍返回成功；`ToolResult.review = { aborted: true, summary: "<stderr 摘要>" }`；transcript `plan.review.warning` 追加 | 审稿员挂了也不挡进度。 |
| reviewer 输出不符合 `<plan_review>` 格式 | `ToolResult.review = { aborted: true, summary: "<格式错误，最后一条消息片段>" }`；不静默猜 `ReviewSummary` | 格式不对就 aborted，别瞎猜。 |
| reviewer 超 `max_turns` 仍未给摘要 | warning + `ReviewSummary { applied_changes: false, summary: "<超时未给出摘要>", aborted: true }` | 超时只 warning。 |
| reviewer 被父 abort | `ToolResult.review = { aborted: true, ... }`；`PlanFile` 保留 | 用户中断算 aborted。 |
| `allow_review_edit=true` 但 reviewer 越界 raw 写 frontmatter / `## Draft` / `## Goal` / `## Todos Board` | `tool_exec.edit` 返回 tool error；reviewer 收 error 后可在下一轮自我纠正 | 越界就 error。 |
| `allow_review_edit=true` 但 reviewer 用 `update_plan` 改 `target.mode=completed` 的 plan | `tool_exec.update_plan` 拒绝；reviewer 收 error，可改其它 planning/pending 的 plan 或仅给摘要 | 已结案 plan 不让动。 |
| `allow_review_edit=true` 但 reviewer 用 `update_plan` 改了某 plan 的 todos[]，全部 completed | 仅当 reviewer 调用方 mode == Executing 且 target 是同 session 的 active plan 时才触发自动派生 `mode=completed`；reviewer subagent 通常不在 Executing 上下文（由 PLAN 期 `create_plan` 调起），所以**不**会触发 | reviewer 改 todos 不会偷偷收口。 |
| `max_review_rounds` 超限 | warning；摘要照常落 transcript；**不**阻塞 `create_plan` 也**不**阻塞 `/plan build` | 审太多次让人工接手，但不挡进度。 |

---

## 11. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：catalog 排除 | `reviewer_not_in_catalog`（待新增） | PENDING | reviewer 必须不出现在任何 LLM schema。 |
| 单元：复用 §14 基础设施 | `reviewer_uses_internal_dispatch_via_agent_registry`（待新增） | PENDING | 走 AgentRegistry，不走 dispatch_agent。 |
| 单元：allowed_tools 收紧 | `reviewer_subagent_blocks_bash_checkpoint_write_frontmatter`、`reviewer_default_allowed_tools_no_create_plan`、`reviewer_default_allowed_tools_includes_todos`（待新增） | PENDING | reviewer 不许碰危险工具，永不含 create_plan，默认含 todos。 |
| 单元：`update_plan` 接入 | `reviewer_can_use_update_plan_when_allowed`、`reviewer_cannot_use_update_plan_when_disallowed`（待新增） | PENDING | 改稿权一开才放 update_plan。 |
| 单元：输出契约非 gate | `reviewer_summary_does_not_gate_exec`、`build_command_ignores_review_summary`（待新增） | PENDING | reviewer 不挡 `/plan build`。 |
| 单元：摘要落点 | `reviewer_summary_lands_in_transcript_plan_review`、`reviewer_does_not_write_frontmatter`（待新增） | PENDING | 摘要进 transcript，不写 frontmatter。 |
| 单元：改稿守卫 | `reviewer_edit_scoped_to_review_section`、`reviewer_cannot_touch_frontmatter`、`reviewer_edit_outside_plan_dir_rejected`（待新增） | PENDING | 改稿只能动 `## Review` 段。 |
| 单元：防递归 | `reviewer_create_plan_does_not_redispatch_reviewer`（待新增） | PENDING | reviewer 不再触发内联 reviewer。 |
| 单元：max_review_rounds | `reviewer_respects_max_review_rounds_advisory`（待新增） | PENDING | 超限只 warning。 |
| 单元：CascadeAbort | `reviewer_aborts_on_parent_cascade`（待新增） | PENDING | 父 abort 子也得 abort。 |
| 集成：allow_review_edit=true | `reviewer_allow_edit_writes_review_section`（待新增） | PENDING | 给权限就能改审稿区。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| reviewer hang 不收敛 | 高 | `max_turns = 8` + `max_review_rounds = 1`（软上限） + `CascadeAbort` | 卡住要能 abort。 |
| reviewer 输出格式漂移 | 中 | 明确 `<plan_review>` fenced block + 严格解析 + 失败时标 `aborted`，不静默猜 | 别静默猜摘要。 |
| reviewer 误改用户代码 | 高 | `allowed_tools` 默认不含 `write` / `bash` / `dispatch_agent`；`allow_review_edit` 仅放开 `edit` + `update_plan`，且 `tool_exec` 各自二次守卫 | 默认只读；改稿只能改 frontmatter todos/milestones 或审稿区。 |
| reviewer 越权调 `create_plan` 套娃 | 高 | `allowed_tools` 任何模式都不含 `create_plan`；`tool_exec.create_plan` 在 `subagent_type == Reviewer` 路径上直接 tool error（双保险） | 工具白名单 + 入口拦截。 |
| reviewer 越界改 frontmatter / `## Draft` | 中 | `tool_exec.edit` `review_section_diff_guard` 强制拒绝；transcript 记 warning | 越界 error。 |
| reviewer 摘要被误读成 gate | 中 | system prompt 第 1 条 + 输出契约不含 verdict 字段；`/plan build` 不读 `ReviewSummary` | 反复强调不是 gate。 |
| reviewer 与 `dispatch_agent` schema 耦合 | 中 | `SubagentType::Reviewer` 不出现在 `dispatch_agent` schema enum；§14.6.1 边界文档化 | 别污染 dispatch schema。 |
| 多 active 计划并发 review | 中 | `PlanRuntime` mutex 串行化；同一计划同时只一个 reviewer | 同时只审一个。 |
| 模型经验积累让 reviewer 永远「无 concern」 | 低（社会工程层面） | system prompt 强调严格；测试矩阵覆盖「错误计划应产生 concern findings」 | 测要打回场景。 |

---

## 13. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把 reviewer 暴露为 LLM 可见 tool（`review_plan`）~~ | **否**：内部 Rust API 形态；不进 catalog。LLM 应只看到 `create_plan` 的 `review` 字段。 | 模型只见 review 字段。 |
| ~~让 reviewer 走 `dispatch_agent` 工具 schema（`subagent_type='reviewer'`）~~ | **否**：会把 reviewer 的存在暴露给 LLM；`SubagentType::Reviewer` 是内部枚举位，不出现在 `dispatch_agent` schema enum。 | 不占 dispatch 枚举位。 |
| ~~verdict 二态 `{accepted, changes_requested}` 做 gate；`accepted` → `ReadyToApply`，`changes_requested` → 留 `Planning`~~ | **否**：reviewer 仅辅助；输出改为 `ReviewSummary { findings, summary }`，**不**做 gate；进 EXEC 永远由用户 `/plan build` 决定（详见 [`plan-runtime.md` §13](../plan-runtime.md#13-历史决策) `H1`–`H3`）。 | reviewer 当顾问，不当门神。 |
| ~~verdict 多态（approve / block / comment / nit / question）~~ | **否**：直接抛弃 verdict，改用 `findings[].severity ∈ {nit, suggestion, concern}` 的离散清单 + `summary` 自由文本。 | 不留 verdict 概念。 |
| ~~`ReadyToApply` / `Cancelled` 作为 PlanFile.mode 之一~~ | **否**：mode 收敛为 `planning / executing / completed / pending`；reviewer 不再产生 mode 转移；中断走 `pending`（runtime 自动写）。 | 状态机简化。 |
| ~~`review_status` / `last_review` 写进 PlanFile frontmatter~~ | **否**：摘要只落 `transcript.plan.review` + `create_plan` ToolResult.review；frontmatter 保持精简（详见 [`create-plan.md` §5.2](./create-plan.md#52-frontmatter-schema)）。 | frontmatter 干净点。 |
| ~~`apply_changes` 作为 `create_plan` LLM 入参~~ | **否**：reviewer 改稿权改为 runtime 内部参数 `allow_review_edit`，代码传参；LLM schema 无相关字段。 | 改稿权代码决定，模型说了不算。 |
| ~~reviewer 异步派发，verdict 后续轮次回填~~ | **否**：同步 await；`ReviewSummary` 与 `create_plan` 同条 ToolResult 返回，避免 LLM 盲改。 | 必须同步返回。 |
| ~~reviewer 也能修改用户代码（带 write 工具）~~ | **否**：`allow_review_edit=true` 仅放开 `edit` + `update_plan`，且 `tool_exec` 各自二次守卫；用户代码（`~/.tomcat/plans/` 外）永远不动。 | 不写用户代码。 |
| ~~reviewer 改稿可写 `## Goal` / `## Draft` / frontmatter raw~~ | **否**：`review_section_diff_guard` 强制 `edit` diff ⊆ `## Review`；frontmatter 改走 `update_plan`（仅 todos/milestones）。 | 改稿就是审稿区写感想 + 用 update_plan 调待办。 |
| ~~reviewer 改稿独占 `edit_plan_review_section` 内部工具~~ | **替代（D 方案）**：直接用通用 `edit`（路径白名单 + 段位 diff guard）+ `update_plan`（结构化）；不再造一个专用工具，与 `## Review` 段以外的需求（改 todos/milestones）也能落地。 | 复用通用工具 + 双重守卫。 |
| ~~reviewer `allowed_tools` 默认 `{read, grep, find}`，true 时附加 `edit`~~ | **替代（D 方案）**：默认 `{read, grep, find, todos}`（`todos` 写自己的 `.todo.md` 无副作用，方便 reviewer 列调研步骤）；`allow_review_edit=true` 附加 `{update_plan, edit}`；**任何模式都不含 `create_plan`**（防套娃 + 职责单一）。 | 默认加 todos 个人 scratchpad；改稿权多给 update_plan。 |
| ~~`create_plan` 在 reviewer 上下文里 `tool_exec.create_plan` 跳过 dispatch_reviewer 即够~~ | **替代（D 方案）**：双保险——`allowed_tools` 永不含 `create_plan` + `tool_exec.create_plan` 在 reviewer 路径直接 tool error。 | 双保险防套娃。 |
| ~~把 reviewer 做成 long-lived 后台 daemon~~ | **否**：每次 `create_plan` 派一个；运行结束即销毁；状态由 `transcript.plan.review` 持久化。 | 每次写完派一个，用完即毁。 |
| ~~`max_review_rounds` 超限切 `Planning` + 阻塞~~ | **否**：超限只 warning，不阻塞 `create_plan` 也不阻塞 `/plan build`；摘要照常落 transcript。 | 软上限，超了只提醒。 |

---

## 14. 关联文档

- 派发入口（调用方）：[create-plan.md](./create-plan.md)
- 增量改 PlanFile：[update-plan.md](./update-plan.md)
- PLAN 模式整体规范：[planner.md](./planner.md)
- 运行时编排：[plan-runtime.md](../plan-runtime.md)
- 子 Agent 基础设施：[multi-agent.md](../multi-agent.md)（§14 整体；§14.6.1 internal subagent dispatch 边界）
- 会话级待办：[todos.md](./todos.md)
- 结构化提问：[ask-question.md](./ask-question.md)
- 标杆写法：[read.md](./read.md)
- 任务卡：[T2-P1-002.md](../../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)

**说人话**：reviewer 由 `create-plan.md` 内部触发；dispatch 通用子 Agent 看 `multi-agent.md` §14.6.1。
