# `reviewer`：审稿 subagent 派生契约（非 LLM 工具）

> **重要**：`reviewer` **不是** LLM 工具，**也不进** catalog。它是一个**子 Agent**，当前已统一为 `ReviewKind::{Plan, Code}` 两种运行形态：`create_plan` 写完 `PlanFile` 后派发 `ReviewKind::Plan`，`update_plan` 命中 `all_completed` 后在 verifier 前派发 `ReviewKind::Code`（若 rounds 未耗尽）。两者都走**内部 Rust API**（`internal subagent dispatch`，对标 [codex `run_codex_thread_one_shot`](https://example/codex_delegate)）同步派发，与 LLM-facing 的 [`dispatch_agent`](../multi-agent.md) 工具互补。本文档定义 reviewer 的派生契约：派发入口、`allowed_tools`、system prompt 模板、输出契约、`allow_review_edit`（仅 Plan kind 使用）行为、并发 / abort 语义。

本文档是 **B 类**：`docs/architecture/tools/`，承接 [`plan-runtime.md`](../plan-runtime.md)、[`multi-agent.md`](../multi-agent.md) §14（基础设施）、[`create-plan.md`](./create-plan.md)（派发入口）。**实现以仓库代码为准**。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：reviewer 是一个内部子 Agent，模型看不到也不能调。`ReviewKind::Plan` 负责 `create_plan` 之后的计划审稿，结论落到 `plan.review` + `create_plan.review`，仍由用户决定何时 `/plan build`；`ReviewKind::Code` 负责 EXEC 收口时的只读代码审查，结论落到 `plan.code_review` + `update_plan.code_review`，`verdict=pass` 时同回合直连 verifier，非 `pass` 时把问题交回主 Agent 修。只有 Plan kind 会使用 `allow_review_edit`：它可通过 [`update_plan`](./update-plan.md) 调整 plan todos，或通过 `edit` 改 `PlanFile.body` 正文；**唯一硬边界是不能 raw 改 frontmatter**。Code kind 则**完全只读**：不能改 plan、不能改代码、不能改 todos，只能返回结构化结论。**任何 kind 下 reviewer 都不能调 `create_plan`**（防递归套娃 + 职责单一）。

> **实现提示**：下文若未显式写 `ReviewKind`，历史描述默认指 `ReviewKind::Plan`；以 §5 的差异表与仓库代码为当前口径。

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
| **`ReviewSummary`** | 审稿输出摘要 | `struct ReviewSummary { findings: Vec<Finding>, summary: String, changes_summary: String, applied_changes: bool }`；`findings.severity ∈ {nit, suggestion, concern}` | reviewer 在最终消息中以 `<review>` fenced block 给出；runtime 解析后落 `transcript.plan.review` + 回填到 `create_plan` tool result | 挑刺清单 + 审稿意见 + 修改总结（无修改则 `changes_summary = "none"`）。 |
| **`allow_review_edit`（runtime 内部参数）** | 是否允许 reviewer 修订 PlanFile（增量改 todos、修改正文） | `bool`；由 `create_plan` 派发时**在代码里**传入 reviewer dispatch；**不**作为 LLM 可见工具入参 | `false`（默认）：reviewer 只读 + 个人 scratchpad（`{read, grep, find, todos}`）+ 输出摘要；`true`：附加 `{update_plan, edit}`——前者改 frontmatter `todos[]`，后者可改 plan 正文任意段；**任何模式都不附加 `create_plan`**，且 frontmatter raw edit 仍拒绝 | 让 reviewer 能动笔还是只挑刺，由代码决定；动笔时通过结构化 `update_plan` + 通用 `edit` 双通道。 |
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
| **本仓库 `reviewer`（D 方案）** | **internal subagent dispatch**（同 codex） | **同步阻塞** | **调用方硬编码 + `allow_review_edit` 旗标**：默认 `{read, grep, find, todos}`；true → 附加 `{update_plan, edit}`；**永不含 `create_plan`** | **顾问形态：`ReviewSummary` + transcript `plan.review`；不当 gate** | **`allow_review_edit=true` 时通过 `update_plan` 改 frontmatter todos + `edit` 改 plan 正文（frontmatter 除外）** | 取 codex 内部派形态；废弃 verdict gate；让 reviewer 通过同一套结构化工具落地修订。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| RV1 派发入口 | LLM tool / 内部 Rust 函数 / 通用 dispatch + role | 内部 Rust 函数（同 codex）。 |
| RV2 同步 vs 异步 | 同步阻塞 / 异步事件回调 | 同步阻塞，`ReviewSummary` 与 `create_plan` 同条 tool result 返回。 |
| RV3 `allowed_tools` 形态 | 入参 / 调用方硬编码 / runtime 默认 | 调用方硬编码（不暴露 LLM）。 |
| RV4 输出契约 | 二态 verdict（gate） vs 摘要顾问 | **顾问形态**：`findings[] + summary + applied_changes`；不做 gate。 |
| RV5 改稿权 | 只读 / 可写计划 / 可写代码 | 可写计划**正文任意段**（runtime 内部参数控制）；frontmatter raw 仍不可写；不可写代码。 |
| RV6 与 `dispatch_agent` 关系 | 复用 schema / 完全独立 / 共享基础设施 | 独立 schema、共享 §14 基础设施。 |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§11） | 说人话 |
|----|------|------------------|--------|
| G1 | reviewer 不进 LLM catalog；不出现在任何工具 schema 中 | `reviewer_not_in_catalog` | 模型调不到 reviewer。 |
| G2 | 派发走 `internal subagent dispatch`；复用 `multi-agent.md` §14 基础设施 | `reviewer_uses_internal_dispatch_via_agent_registry` | 内部 API 派，复用注册表。 |
| G3 | `allowed_tools` 在调用方代码硬编码（默认 `{read, grep, find, todos}`；`allow_review_edit=true` 附加 `{update_plan, edit}`，其中 `edit` 仅限 `~/.tomcat/plans/*.plan.md` 且 raw frontmatter diff 一律拒绝；**永不**含 `create_plan`） | `reviewer_default_allowed_tools_no_create_plan`、`reviewer_subagent_blocks_bash_checkpoint_write_frontmatter` | 工具白名单写死，frontmatter 永远不许 raw 改；`create_plan` 永远不暴露给 reviewer。 |
| G4 | 输出契约是 `ReviewSummary`（摘要顾问），**不做 verdict gate**；进 EXEC 永远由用户 `/plan build` | `reviewer_summary_does_not_gate_exec`、`build_command_ignores_review_summary` | reviewer 不挡进度，进 EXEC 用户拍。 |
| G5 | `allow_review_edit=true` 时 reviewer 通过 `update_plan` 修改 frontmatter `todos[]`（受 `update_plan` 本身的门控约束）；通过 `edit` 改 plan 正文任意段；frontmatter raw → tool error | `reviewer_edit_preserves_frontmatter`、`reviewer_cannot_touch_frontmatter_raw`、`reviewer_can_use_update_plan_when_allowed` | 改 frontmatter 走 update_plan，正文段走 edit，分工清晰。 |
| G6 | `max_review_rounds` 默认 1；超限不阻塞、不切 mode，只追加 warning 到 transcript | `reviewer_respects_max_review_rounds_advisory` | 审太多次也不卡进度，只提醒人工看一眼。 |
| G7 | 父 Agent abort 触发 reviewer abort（CascadeAbort） | `reviewer_aborts_on_parent_cascade` | 父停子也停。 |
| G8 | reviewer 摘要写 `transcript.plan.review` 自定义事件；不写 `PlanFile` frontmatter | `reviewer_summary_lands_in_transcript_plan_review`、`reviewer_does_not_write_frontmatter` | 审稿结果只进对话流水，不污染计划元数据。 |

**说人话（§3 总览）**：reviewer 是 create_plan 内部的子 Agent，不进工具表、工具名单硬编码、输出是顾问摘要（**不**当 gate）、可选直接改 plan 正文、父 abort 级联、摘要写 transcript 不写 frontmatter。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| reviewer 作为 LLM 工具 | §13 已否决 | 别暴露给模型。 |
| reviewer 写/改代码 | 不含 write/bash；`edit` 仅作用于 `~/.tomcat/plans/*.plan.md` 且 raw frontmatter 仍被守卫拒绝 | 只能看计划/仓库，不能改代码。 |
| reviewer 再派 sub-reviewer | `role = leaf` | 不能再派子 Agent。 |
| reviewer 决定 PlanFile.mode | 由 `/plan build` 命令 + runtime 派生（completed / pending）共同维护 | reviewer 不动 frontmatter 也不动 mode。 |

---

## 4. 派发入口与决策表

### 4.1 落地选型决策表

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| **R-DispatchEntry**（派发入口） | **internal subagent dispatch**（内部 Rust API，不进 catalog；对标 codex `run_codex_thread_one_shot`） | LLM-facing tool 易被乱调；通用 `dispatch_agent` schema 暴露过多权限边界细节 | 内部派，对标 codex。 |
| RV2 同步 vs 异步 | 同步阻塞 await；`ReviewSummary` 与 `create_plan` 同条 ToolResult 返回 | 异步派会让 LLM 在没拿到摘要的情况下盲改 | 必须同步带回摘要。 |
| RV3 `allowed_tools` 形态 | 调用方代码硬编码：默认 `{read, grep, find, todos}`；`allow_review_edit=true` 附加 `{update_plan, edit}`（`edit` `tool_exec` 守卫：路径 ⊆ `~/.tomcat/plans/*.plan.md` 且 frontmatter diff 必须为空）；**任何模式都不含 `create_plan`** | LLM 入参形态会绕过 catalog；通用 schema 失去针对性；reviewer 持有 `create_plan` 会无限套娃 | 白名单写死；frontmatter raw 改永远拦死；frontmatter todos 走 `update_plan` 通道。 |
| RV4 输出契约 | **顾问摘要**：`ReviewSummary { findings[], summary, applied_changes }`；**不**做 verdict gate；不切 mode | verdict 二态被用户决策为「reviewer 仅辅助」 | 不挡进度。 |
| RV5 改稿权 | `allow_review_edit` runtime 内部参数显式控制；**不**作为 LLM 可见工具入参 | 暴露给 LLM 会引导模型主动开权限；交由代码决定保持边界 | 改稿权代码说了算。 |
| RV6 与 `dispatch_agent` 关系 | 共享 `multi-agent.md` §14 `AgentRegistry` / `spawn_depth` / `CascadeAbort`；不共享 schema | 共享 schema 会污染 `dispatch_agent` 的 `subagent_type` 枚举位 | 共用底座，不共用工具 schema。 |
| RV7 spawn_depth 计费 | reviewer 派发**算入** `spawn_depth`（与 `dispatch_agent` 计费一致） | 不计费会让 reviewer 嵌套绕过深度限制 | 算一层嵌套深度。 |
| RV8 `subagent_type` 枚举位 | reviewer 在内部使用 `SubagentType::Reviewer`；该值**不**出现在 `dispatch_agent` schema enum | LLM 不应知道有「reviewer」这种角色 | 内部枚举，模型看不见。 |
| RV9 摘要落点 | `transcript.plan.review` 自定义事件 + 同步回填 `create_plan` tool result.review | 写 frontmatter 会让 frontmatter 字段膨胀；用户也不需感知 `review_status` 之类字段 | 走 transcript，frontmatter 干净。 |
| **RV10 OOD 三层** | `ChatContextRegistry`（按 `session_key`）+ 进程级 `AgentRegistry`（按 `session_id`，[`multi-agent.md` §14.3.2](../multi-agent.md#1432-agentregistry进程级) / §14.3.2.1）+ 短命 `AgentLoop`（栈上拥有） | 全塞进 `ChatContext` 无法做跨会话并发上限与 CascadeAbort；与 §14.3.0 MA1/MA2/MA3 决策一致 | 管家、登记处、工人三层分离。 |
| **RV11 ChatContext 不持 Agent 表** | `ChatContext` 只持 `root_session_id` + `TodosRuntime` + `PlanRuntime` + 共享 `Arc` 服务（详见 [`plan-runtime.md` §6.4](../plan-runtime.md#64-chatcontext-持有关系)） | `HashMap<SubagentType, Agent>` 与「每 spawn 新建 + 跑完即 drop」语义冲突 | 聊天室不养一群 Agent 对象。 |
| **RV12 reviewer 无长期 Runtime** | 每次 `create_plan` 成功 → transient spawn → await → drop handle；状态落 `PlanRuntimeState.last_review_summary` + transcript `plan.review` | 单独 `ReviewerRuntime` 长生命周期对象多余，与 `max_review_rounds` advisory 语义对不齐 | 审稿员用完即走，不养专职岗位。 |
| **RV13 `dispatch_reviewer` 归属** | `PlanRuntime::dispatch_reviewer(allow_review_edit) -> ReviewSummary`（详见 §4.3） | 游离的 `review::dispatch_reviewer` 自由函数与 plan 编排语义割裂，难做 advisory lock 顺序与 memory reconcile | 派审稿归 PlanRuntime 管。 |
| **RV14 写盘锁顺序** | `write_plan` 完成并**释放 plan advisory lock 之后**再 `spawn_subagent_internal`；reviewer 内 `update_plan` / `edit` 自行 acquire 同一份 lock | 父持锁 await 子 → 死锁；reviewer 调 update_plan 时会再次 acquire | 先落盘锁再放审稿进来。 |
| **RV15 审稿后内存 reconcile** | reviewer 通过 `update_plan` 改了 target plan 后，父 `PlanRuntime` 在 await 返回时**重读 PlanFile** 刷新 `state.todos`（与 [`plan-runtime.md` §6.3](../plan-runtime.md#63-planruntimeplanexec-路径) 派生逻辑保持一致） | 父内存仍是旧 todos，下一步 LLM 看到过期快照 | 审稿改完要刷新管家手里的副本。 |
| **RV16 `AgentLoopConfig` 透传** | 落实 `subagent_type` / `parent_session_id` / `spawn_depth`（[`multi-agent.md` §14.3.3 / §14.4.2.1](../multi-agent.md#1433-agentloopconfig-扩展)，Phase 2/3 PENDING） | 不透传 `subagent_type` 则 `tool_exec.edit` / `tool_exec.create_plan` 守卫读不到「调用方是 reviewer」，RV-D 无法生效 | loop 配置里要带「我是审稿员」。 |

### 4.2 实施点（拟定）

> 与 [`plan-runtime.md`](../plan-runtime.md) **PR-PLC** 对齐；由 [`create-plan.md`](./create-plan.md) **CP-D** 调用；当前代码 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **RV-A** | `PlanRuntime::dispatch_reviewer(allow_review_edit) -> ReviewSummary`（取代游离 `review::dispatch_reviewer`，详见 §4.3 / RV13）+ `AgentRegistry::spawn_subagent_internal`；不进 catalog；**交付**：方法 API + 写盘锁顺序（RV14）+ memory reconcile（RV15） | `src/api/chat/plan_runtime/mod.rs`（`PlanRuntime` 上的方法）+ `src/api/chat/plan_runtime/review.rs`（仅保留 prompt / 解析 helpers） | 见 §11：`reviewer_not_in_catalog`、`reviewer_uses_internal_dispatch_via_agent_registry`（PENDING） | 派审稿归 PlanRuntime 管，free function 只剩 prompt/parser。 |
| **RV-B** | `REVIEWER_SYSTEM_PROMPT` + `allowed_tools` 硬编码（`allow_review_edit` 分支）：默认 `{read, grep, find, todos}`；true → 附加 `{update_plan, edit}`；**永不含 `create_plan`**；**交付**：`SubagentType::Reviewer` | `review.rs`、`src/core/agent_loop.rs` | 见 §11：`reviewer_default_allowed_tools_no_create_plan`、`reviewer_can_use_update_plan_when_allowed`、`reviewer_subagent_blocks_bash_checkpoint_write_frontmatter`（PENDING） | 工具白名单写死，默认只读 + todos；改稿走 update_plan + edit。 |
| **RV-C** | 解析子 Agent 最终消息为 `ReviewSummary { findings[], summary, changes_summary, applied_changes }`（`<review>` block）；**不**驱动 mode 转移；**交付**：`ReviewSummary` + transcript 事件写入 | `review.rs`、`src/api/chat/plan_runtime/mod.rs`（拟定）、`src/infra/transcript/...`（既有） | 见 §11：`reviewer_summary_does_not_gate_exec`、`reviewer_summary_lands_in_transcript_plan_review`（PENDING） | 摘要进 transcript，不动 mode。 |
| **RV-D** | reviewer 调 `edit`/`create_plan` 时跳过递归内联 reviewer；`tool_exec.edit` 拦截 reviewer 进程：路径限制 + frontmatter diff guard；**依赖** RV16（`AgentLoopConfig.subagent_type` 透传，否则守卫读不到调用方画像）；**交付**：`parent_subagent_type` 检测 + `reviewer_body_diff_guard` | `tool_exec.rs` + `review.rs` | 见 §11：`reviewer_create_plan_does_not_redispatch_reviewer`、`reviewer_edit_preserves_frontmatter`（PENDING） | 审稿员可改正文，但仍不能越过 frontmatter 边界。 |
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
- **双保险**：`resolve_internal_tools(parent_catalog, allowed_tools)` 与父 catalog 取交集；`tool_exec.edit` 在 `parent.subagent_type == Reviewer` 路径上额外执行 frontmatter diff guard（见 RV-D）；`tool_exec.update_plan` 在 reviewer 上下文中继承自身的 mode-aware 门控（target.mode=completed 拒绝；跨 session 改 executing 拒绝），无需额外守卫。

```text
  allow_review_edit?
     │        │
    false    true
     │        │
     ▼        ▼
 read/     read/ + edit（仅 ~/.tomcat/plans/*.plan.md，且 frontmatter diff 为空）
  grep/find grep/find
```

**说人话**：默认只能看；开了改稿权可以直接改 plan 正文，但 frontmatter 还是碰不到。

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
  2. 计算前后 diff，**只要** frontmatter 有变化就 tool error；正文各段允许调整；
  3. 写盘走与 `create_plan` 同一份 advisory lock。

**说人话**：审稿员就算给了改稿权，也只能在 plan 文件里动正文，frontmatter 还是 runtime 专属，不许 raw 改。

#### 4.2.5 RV-E：abort、轮次与深度

- **交付**：`spawn_depth = parent + 1`；超 `MAX_SPAWN_DEPTH` 在 **dispatch** 前拒（若未来允许 reviewer 链式调用）；`max_review_rounds` 超限 → **warning + 不阻塞**（不切 mode，不拒绝 `create_plan`，留 `transcript.plan.review.warning`）。
- **abort**：父 turn / root handle 的 `CancellationToken` 被 cancel 后，reviewer 子 token 立即跟着取消 → `create_plan` 仍返回成功但 `ToolResult.review` 标 `aborted`，文件保留。

**说人话**：审稿算一层子 Agent，卡住能 abort，审太多次只是 warning，不挡用户后续 `/plan build`。

### 4.3 派发入口（API 形态）

`dispatch_reviewer` 是 `PlanRuntime` 上的**方法**（不是游离的 free function），与 [`plan-runtime.md` §6.3](../plan-runtime.md#63-planruntimeplanexec-路径) 的派生逻辑同源管理；`review.rs` 只保留 prompt 构造与 `<review>` 解析等 helpers。`parent: &AgentHandle` 在调用时从父 loop 在 `AgentRegistry` 中的条目解析得到。

```rust
impl PlanRuntime {
    pub async fn dispatch_reviewer(
        &self,
        plan_id: &PlanId,
        allow_review_edit: bool,        // runtime 内部参数；非 LLM 入参
    ) -> Result<ReviewSummary> {
        // RV14：先释放 plan advisory lock 再派发；本方法只在 write_plan 完成后调用。
        debug_assert!(!self.holds_plan_lock(plan_id));

        let plan   = self.state.read_plan(plan_id)?;            // 已落盘版本
        let parent = self.deps.agent_registry.get(&self.session_id())
            .ok_or(PlanError::ParentHandleMissing)?;

        let allowed_tools: &[&str] = if allow_review_edit {
            &["read", "grep", "find", "todos", "update_plan", "edit"]
        } else {
            &["read", "grep", "find", "todos"]
        };

        let cfg = AgentLoopConfig {
            session_id:        format!("{}:sub:reviewer:{}", parent.session_id, Uuid::new_v4()),
            parent_session_id: Some(parent.session_id.clone()),
            spawn_depth:       parent.spawn_depth + 1,
            subagent_type:     SubagentType::Reviewer,   // RV16
            role:              Role::Leaf,
            tool_definitions:  resolve_internal_tools(&self.deps.parent_catalog, allowed_tools),
            max_turns:         REVIEWER_MAX_TURNS,
            ..Default::default()
        };

        let initial = build_review_prompt(&plan, allow_review_edit);

        // RV15：await 返回后重读 PlanFile，刷新 state.todos。
        let summary = self.deps.agent_registry
            .spawn_subagent_internal(&self.spawn_deps(), &parent, cfg, vec![initial])
            .await?
            .try_into_review_summary()?;

        if summary.applied_changes {
            self.state.reload_from_disk(plan_id)?;
        }
        Ok(summary)
    }
}
```

> 详细的子 `AgentLoop` 所有权与寿命语义见 [`multi-agent.md` §14.4.2.2 子 AgentLoop 的所有权与生命周期](../multi-agent.md#1442-子-agentloop-的所有权与生命周期)；本节不重复展开。

### 4.4 与 `multi-agent.md` 的关系

详见 [`multi-agent.md`](../multi-agent.md) §14.6.1：

- 共享：`AgentRegistry` / `spawn_depth` / `MAX_SPAWN_DEPTH` / `CascadeAbort` / `SubAgentStart`/`End` 事件。
- 不共享：`dispatch_agent` 工具 schema 中的 `subagent_type` 枚举（reviewer 用 `SubagentType::Reviewer` 内部枚举位）。

**说人话**：走 `spawn_subagent_internal`，和 dispatch_agent 用同一套注册表/深度/级联中止，但 LLM 永远调不到 reviewer。

### 4.5 OOD 与双注册表（reviewer 的「嵌套但不开新 ChatContext」）

> 决策依据 RV10–RV16。子 `AgentLoop` 所有权 / 寿命 / `new` 点的**完整**论述见 [`multi-agent.md` §14.4.2.2 子 AgentLoop 的所有权与生命周期](../multi-agent.md#1442-子-agentloop-的所有权与生命周期)；本节仅给出 reviewer 视角的三层简图，不重复展开。

#### 三层 OOD 简图（reviewer 视角）

```text
   ┌────────────────────────── 进程级（双注册表，互不持有） ──────────────────────────┐
   │                                                                                   │
   │   ChatContextRegistry                                AgentRegistry                │
   │   key = session_key                                  key = session_id             │
   │   value = Arc<ChatContext>                           value = Arc<AgentHandle>     │
   │       ▲                                                  ▲                        │
   └───────┼──────────────────────────────────────────────────┼────────────────────────┘
           │                                                  │
           │ 一个 session_key 对应一个 ChatContext              │ Phase 2/3 起，父/子 loop 跑时各登记一行
           ▼                                                  ▼
   ┌────────────────────────────── 会话壳（每会话一个，长寿） ────────────────────┐
   │  ChatContext {                                                              │
   │     root_session_id,            // 与 AgentRegistry 关联的锚点              │
   │     TodosRuntime, PlanRuntime,                                              │
   │     Arc<dyn LlmProvider> / Arc<dyn PrimitiveExecutor> / Arc<EventBus>       │
   │  }                                                                          │
   │  ★ 不持子 AgentLoop / 不持 subagents[]（RV11）                                │
   └─────────────────────────────────────────────────────────────────────────────┘
                       │
                       │  user input / /plan build / create_plan tool
                       ▼
   ┌────────────────────────── 短命执行循环（栈上拥有，跑完即 drop） ────────────────┐
   │  父 AgentLoop（chat_loop 栈帧拥有）                                              │
   │      │                                                                          │
   │      │  create_plan::write_plan 落盘 → 释放 advisory lock（RV14）                │
   │      ▼                                                                          │
   │  PlanRuntime::dispatch_reviewer（RV13）                                          │
   │      │                                                                          │
   │      └── AgentRegistry::spawn_subagent_internal                                  │
   │              ├─ register(child handle, cancel_token)                            │
   │              ├─ new child AgentLoop（SubagentType::Reviewer, role=Leaf, RV16）    │
   │              ├─ child.run([build_review_prompt(plan)]).await   ← reviewer 在此跑 │
   │              ├─ unregister(child handle)                                        │
   │              └─ child AgentLoop drop                                            │
   │      │                                                                          │
   │      ▼                                                                          │
   │  await 返回 ReviewSummary →（若 applied_changes）父 PlanRuntime 重读 PlanFile（RV15）│
   └─────────────────────────────────────────────────────────────────────────────────┘
```

#### 为什么不是 `ChatContext ⊃ { Agent, subagents[] }`

| 误设方案 | 不可行原因 | 落地决策 |
|----------|-----------|----------|
| `ChatContext.agents: HashMap<SubagentType, Agent>` | 与「每轮新建 `AgentLoop` + 跑完 drop」（[`multi-agent.md` §14.4.2.2](../multi-agent.md#1442-子-agentloop-的所有权与生命周期)）冲突；会跨 turn 泄漏子 Agent 状态 | RV11：`ChatContext` 不持 Agent 表 |
| 单独的 `ReviewerRuntime` 长生命周期对象 | reviewer 跨调用**无**残留状态；`rounds` / `last_review_summary` 已落 `PlanRuntimeState` + transcript | RV12：transient spawn |
| 在 `ChatContext` 之上加 `ChatContextRegistry` 同时也挂 Agent 树 | 一张表既要按会话索引壳又要按运行 id 索引 handle，语义错位 | RV10：双注册表，正交 |

#### reviewer 是「嵌套」但不是「新开 ChatContext」

| 维度 | reviewer 实际行为 |
|------|-------------------|
| **并发** | 父 session 的 `create_plan` 同步 await 子 loop；与 codex one-shot 一致 |
| **嵌套** | `spawn_depth = parent + 1`；`session_id = "{root}:sub:reviewer:{uuid}"`；`parent_session_id = Some(root)` |
| **session 边界** | **一个** `session_key`（同一个 `ChatContext`）+ **两个** `session_id`（父/子 `AgentLoop`）+ 子 Agent **独立 transcript** |
| **新 ChatContext？** | ❌ **否**：reviewer 复用父的 `ChatContext.{TodosRuntime, PlanRuntime, Arc<dyn …>}`；只在 `AgentRegistry` 加一条 handle |

**说人话（§4.5）**：reviewer 是同一间聊天室里临时叫起来的审稿员，登记在「访客登记处」（`AgentRegistry`）而不是另开一间聊天室（`ChatContext`）；管家（`ChatContext`）的备忘录（`PlanRuntime`）借给他用，干完就走，备忘录有改动管家立刻重读一次。

---

## 5. system prompt / allowed_tools / 输出契约 / allow_review_edit

### 5.1 reviewer system prompt（常量内容）

当前实现按 `ReviewKind` 分两套 prompt：

| `ReviewKind` | prompt 常量 | 主要职责 | 说人话 |
|--------------|-------------|----------|--------|
| `Plan` | [`review.rs::REVIEWER_SYSTEM_PROMPT`](../../../src/api/chat/plan_runtime/review.rs) | 审 plan 质量；可在 runtime 允许时改 plan；输出顾问摘要 | 开工前审稿。 |
| `Code` | [`review.rs::CODE_REVIEW_SYSTEM_PROMPT`](../../../src/api/chat/plan_runtime/review.rs) | 审 EXEC 期间的代码实现；必须给 `verdict`；提醒“结论交给主 Agent” | verifier 前的代码二审。 |

当前 catalog 把 grep / find 合并为 `search_files`，故文案统一以 `read / search_files / list_dir ...` 描述工具；`max_turns` 由 dispatcher 传入，Plan/Code 两种 kind 当前都使用 64。

### 5.2 allowed_tools

| `ReviewKind` | `allowed_tools` | 备注 | 说人话 |
|--------------|-----------------|------|--------|
| `Plan` | `{read, search_files, list_dir, todos, update_plan, edit}` | `update_plan` 只允许走它自身的 plan/todo 门控；`edit` 仅限 `~/.tomcat/plans/*.plan.md` 且 raw frontmatter diff 为空 | 可以修 plan，但不能碰仓库代码。 |
| `Code` | `{read, search_files, list_dir, bash}` | `bash` 不额外做细粒度白名单，但仍受 permission / timeout / 审计约束；`tool_exec` 与 `safety` 额外拦截全部写工具 | 只读看代码，可跑命令取证。 |

> **三条铁律**：
> 1. **永远不含 `create_plan`**——避免 reviewer 套娃 / 重定计划；reviewer 只挑刺与落地建议，不重写整盘。
> 2. **Plan kind 不含 `bash`，Code kind 不含任何写工具**——两侧边界相反但都硬编码在 runtime。
> 3. **双保险**：`AgentRegistry::spawn_subagent_internal` 构造子 catalog 时与父 catalog 取交集；`tool_exec` 与 `safety` 在 reviewer 上下文中还有二次守卫；即使 `allowed_tools` 错误传入了不存在的工具名，runtime 也会过滤。

### 5.3 输出契约（`ReviewSummary`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `kind` | `ReviewKind` (`plan \| code`) | 本次 reviewer 的运行形态 | 这是 plan 审稿还是 code 审查。 |
| `findings` | `Vec<Finding { severity, area, note }>` | 离散挑刺清单；`severity ∈ {nit, suggestion, concern}`；`area` 为自由短标签（不限 plan 专用枚举） | 一条条挑刺记下来。 |
| `summary` | `String`（≤ 600 字符） | 审稿意见总评（发现了什么、整体判断） | 审稿意见一句话。 |
| `changes_summary` | `String` | 本轮实际修改内容与理由；只读或未改时解析为 `"none"` | 改了什么、为什么改。 |
| `applied_changes` | `bool` | 本轮 reviewer 是否调用过任一写工具（`edit` / `update_plan` 等，由 runtime 守卫范围） | 有没有真动过笔。 |
| `verdict` | `Option<String>` | 仅 `Code` 时使用：`pass \| fail \| partial \| aborted`；`Plan` 永远为 `None` | code review 给个明确结论。 |
| `rounds` | `u32` | 同一 PlanFile 累计被 review 的轮次（含本次） | 这是第几轮审稿。 |

`ReviewSummary` 仍**不**直接携带 mode 转移指令，也**不**含 `accepted`/`rejected` 标记；runtime 只会对 `Code` kind 的 `verdict` 做轻量规范化，再决定是同回合直连 verifier 还是回主 Agent 修复。

### 5.4 摘要落点：tool result + transcript 双通道

- `ReviewKind::Plan`：runtime 解析 `ReviewSummary` 后写 `transcript.plan.review`，并同步回填 `create_plan` 的 `ToolResult.review`。
- `ReviewKind::Code`：runtime 在 `normalize_for_code_review_result()` 后写 `transcript.plan.code_review`，并同步回填 `update_plan` 的 `ToolResult.code_review`；若 rounds 用尽则补一条 `plan.code_review.warning`。
- 两种 kind 都保证 **tool result 与 transcript 口径一致**；Code kind 的 `verdict=pass` 回合里，同一条 `update_plan` tool result 会同时带 `code_review` 与 `verify`。

- `ReviewKind::Plan` 的 transcript 事件结构示意：

```jsonc
{
  "type": "plan.review",
  "plan_id":   "<plan_id>",
  "rounds":    1,
  "findings":  [/* ... */],
  "summary":   "...",
  "changes_summary": "none",
  "applied_changes": false,
  "session_id": "<reviewer subagent session_id>"
}
```

- **不**写 `PlanFile.body` 中的审稿块，也**不**写 `PlanFile` frontmatter；frontmatter 仍不引入 `review_status` / `last_review` 字段。
- `Plan` kind 与进 EXEC 解耦：是否 `/plan build` 仍由用户决定。
- `Code` kind 与 EXEC 收口绑定：非 `pass` 时主 Agent 需要根据 `code_review` 结论 reopen 旧 todo 或新增修复 todo，再次 complete。

### 5.5 `allow_review_edit` / `ReviewKind` 行为

| 维度 | `ReviewKind::Plan` | `ReviewKind::Code` | 说人话 |
|------|--------------------|--------------------|--------|
| `allow_review_edit` 是否生效 | ✅ 生效；当前实现固定允许 | ❌ 不生效；Code kind 始终只读 | 只有 plan 审稿能动 plan。 |
| 改 frontmatter `todos[]` | ✅ 通过 `update_plan` | ❌ | 改计划待办只属于 Plan kind。 |
| 改 plan 正文任意段 | ✅ 通过 `edit`（frontmatter 守卫仍在） | ❌ | Code kind 连 plan 正文都不能动。 |
| 改 frontmatter 其它字段（mode / session_* / plan_id / created_at / goal） | ❌ | ❌ | runtime 专属字段始终不许碰。 |
| 改用户代码（仓库其他路径） | ❌ | ❌ | reviewer 两种 kind 都不改仓库代码。 |
| `bash` | ❌ | ✅ | code reviewer 可以跑只读命令取证。 |
| reviewer turn 内可调 `create_plan`？ | ❌ | ❌ | reviewer 永远不重建计划。 |
| `ReviewSummary.applied_changes` | 可能为 `true` | 运行时强制为 `false` | code review 只提结论不动手。 |
| 非 `pass` 后续动作 | 与 EXEC 无关，仍由用户决定是否 build | 主 Agent 读取 `code_review` 后 reopen / 新增 todo，再 complete | 代码审查打回后由主 Agent 接力修。 |

---

## 6. One-Glance Map

| 路径 | 职责 | 说人话 |
|------|------|--------|
| `src/api/chat/plan_runtime/review.rs`（拟定） | reviewer 派发入口；`allowed_tools` 硬编码；`allow_review_edit` 透传；`ReviewSummary` 解析 | 派生 + 解析审稿结果。 |
| `src/core/agent_registry.rs`（既有/拟扩展） | `spawn_subagent_internal(...)`；`SubagentType::Reviewer` 内部枚举位；递归内联 reviewer 抑制（`parent.subagent_type == Reviewer` 时跳过） | 内部 spawn + 防套娃。 |
| `src/core/agent_loop.rs`（既有） | `AgentLoopConfig` 含 `subagent_type` / `role` 字段；reviewer 初始化时挂 `REVIEWER_SYSTEM_PROMPT` | 挂审稿提示词。 |
| `src/api/chat/plan_runtime/tool_exec.rs`（既有/拟扩展） | reviewer 进程下 `edit` 二次守卫：路径白名单 + frontmatter diff guard | 改稿落点守门员。 |
| `src/api/chat/plan_runtime/file_store.rs`（拟定） | reviewer 调 `edit` 时复用同一份 advisory lock 与 round-trip 逻辑 | 改稿走同一套写盘。 |
| `src/infra/transcript/...`（既有） | `SubAgentStart` / `SubAgentEnd` 事件；`plan.review` 自定义事件写入；UI 可显示 reviewer 进度（可选） | transcript 落审稿摘要。 |

**阅读顺序（说人话）**：`create_plan` 落盘 → `review.rs` 派 internal reviewer → `agent_registry` spawn → reviewer 跑完 → 解析 `ReviewSummary` → 写 `transcript.plan.review` + 回填 `create_plan` tool result.review。

---

## 7. 调度时序

```
父 Agent (PLAN 模式)
  │
  └─ tool_call("create_plan", { goal, draft, todos })
        │
        ▼
   create_plan 工具 (file_store::write_plan → 落盘成功；advisory lock 释放 — RV14)
        │
        ▼
   PlanRuntime::dispatch_reviewer(plan_id, allow_review_edit=runtime_default)
        │
        ▼
   AgentRegistry::spawn_subagent_internal(cfg, build_review_prompt(plan))
        │
        ├──── SubAgentStart 事件
        │
        │  reviewer subagent 运行：
        │   - read / grep / find 调研、todos 自己记调研步骤
        │   - 若 allow_review_edit=true：
        │       * update_plan 改 PlanFile.frontmatter.todos[]（受其本身门控）
        │       * edit 改 PlanFile.body 正文（路径白名单 + frontmatter diff guard）
        │     （tool_exec.edit guard 拒绝 frontmatter 的 raw 修改）
        │   - 输出 final assistant message：
        │     <review>findings: ... summary: ... changes_summary: ...</review>
        │
        ├──── SubAgentEnd 事件
        ▼
   ReviewSummary { findings, summary, changes_summary, applied_changes, rounds }
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
   （任意轮次后）用户敲 /plan build <plan_id/path>
        │
        ▼
   PlanRuntime 切 mode=executing；与 review 摘要无关
```

**说人话**：用户只见 create_plan 一次调用；背后起审稿子 Agent，读完计划/仓库（可选直接改正文），把摘要塞回同一条 tool result + transcript；mode 不变；进 EXEC 由用户 `/plan build` 拍。

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
| 嵌套发生位置 | 在 `AgentRegistry`（子 `session_id` 多一条 `AgentHandle`）；**不**创建第二个 `ChatContext` / 不动 `ChatContextRegistry`（RV10–RV11，详见 [`multi-agent.md` §14.4.2.2](../multi-agent.md#1442-子-agentloop-的所有权与生命周期)） | 嵌套只在访客登记处加一行，不开新聊天室。 |

### 8.3 abort 语义

- 父 Agent 收到 abort → `CascadeAbort` cancel root token，reviewer 子 `CancellationToken` 立即跟着取消，并在 reasoning / tool await 间隙退出。
- reviewer abort / 超时 / 解析失败 → `create_plan` **仍返回成功**（落盘已成功），但 `ToolResult.review` 标 `aborted: true` 且 `summary = "<aborted>"`，transcript 追加 `plan.review.warning` 摘要；**不**切 mode，**不**让 `/plan build` 失败。

**说人话**：单次审稿：Pending→Running→Returned 或 Aborted；父停则子停；审稿挂了/超时也不挡用户后续 `/plan build`，摘要标个 aborted 就行。

---

## 9. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_REVIEWER_MAX_TURNS` | `64` | reviewer subagent 最大 reasoning 轮次（映射 `AgentLoopConfig.max_tool_rounds`） | 审稿最多 64 轮推理；transcript 落实际 `reviewer_turns_used`。 |
| `TOMCAT_REVIEWER_MODEL` | 继承父 Agent | 显式覆写 reviewer 使用的模型 | 可单独指定审稿模型。 |
| `TOMCAT_PLAN_MAX_REVIEW_ROUNDS` | `1` | 单个 `PlanFile` 累计 reviewer 派发轮次软上限；超限只 warning，不阻塞 | 软上限，超了只提醒。 |
| ~~`TOMCAT_REVIEWER_DEFAULT_ALLOW_EDIT`~~ | ~~`false`~~ | **已删除**：`allow_review_edit` 在实现层固定为 `true`，不再接受环境变量/配置项 | 改稿权写死，不可配。 |
| `TOMCAT_REVIEWER_SYSTEM_PROMPT_OVERRIDE_PATH` | 未设 | 测试用：从指定文件读取 system prompt 覆写默认常量 | 单测可换审稿提示词。 |

---

## 10. 错误模型 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| reviewer 子 Agent 异常退出 | `create_plan` 仍返回成功；`ToolResult.review = { aborted: true, summary: "<stderr 摘要>" }`；transcript `plan.review.warning` 追加 | 审稿员挂了也不挡进度。 |
| reviewer 输出不符合 `<review>` 格式 | `ToolResult.review = { aborted: true, summary: "<格式错误，最后一条消息片段>" }`；不静默猜 `ReviewSummary` | 格式不对就 aborted，别瞎猜。 |
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
| 单元：catalog 排除 | `reviewer_not_in_catalog`（[`prod_reviewer.rs`](../../../src/api/chat/plan_runtime/prod_reviewer.rs)） | DONE | reviewer 必须不出现在任何 LLM schema。 |
| 单元：复用 §14 基础设施 | `reviewer_uses_internal_dispatch_via_agent_registry` | 由真 LLM E2E `inprocess_full_plan_path_with_real_llm` 间接覆盖；CLI 黑盒已收缩为 planning-only / exec-only smoke | 走 AgentRegistry，不走 dispatch_agent。 |
| 单元：allowed_tools 收紧 | `reviewer_default_allowed_tools_no_create_plan`、`resolve_internal_tools_filters_to_allowed`、`reviewer_blocks_non_whitelisted_tool`、`reviewer_blocks_create_plan_subagent` | DONE | reviewer 不许碰危险工具，永不含 create_plan，默认含 todos。 |
| 单元：摘要落点 | `reviewer_summary_lands_in_transcript_plan_review`、`reviewer_writes_warning_event_on_second_round` | DONE | 摘要进 transcript，含 `reviewer_turns_used` / `reviewer_turns_limit` / `reviewer_stop_reason`。 |
| 单元：改稿守卫 | `reviewer_body_diff_guard_allows_body_change`、`reviewer_body_diff_guard_rejects_frontmatter_change`、`reviewer_subagent_must_target_plan_files` | DONE | 改稿可动正文任意段；路径外或 frontmatter 越界直接拒。 |
| 单元：防递归 / 防套娃 | `reviewer_blocks_create_plan_subagent`（[`tool_exec.rs`](../../../src/core/agent_loop/tool_exec.rs)） | DONE | reviewer 不能调 create_plan。 |
| 单元：max_review_rounds | `reviewer_round_count_warns_after_threshold`、`reviewer_writes_warning_event_on_second_round` | DONE | 超限只 warning；事件 `plan.review.warning` 含 rounds。 |
| 单元：CascadeAbort | `parent_turn_token_cancel_propagates_to_spawned_child_tokens`、`dispatch_reviewer_releases_plan_lock_before_spawn`（间接） | DONE | 父 abort 子也得 abort。 |
| 单元：max_turns 默认 64 | `reviewer_max_turns_default_is_64` | DONE | 默认 64，transcript 落实际 turns。 |
| 单元：输出契约非 gate | `create_plan_succeeds_even_when_reviewer_aborts` | DONE | reviewer aborted 不挡 `create_plan` 落盘 / `/plan build`。 |
| 集成：真 LLM | `inprocess_full_plan_path_with_real_llm`（[`tests/plan_real_llm_inprocess_tests.rs`](../../../tests/plan_real_llm_inprocess_tests.rs)） | DONE（需 `OPENAI_API_KEY`） | 主 LLM + 真 reviewer 子 LLM 跑完 PLAN→EXEC→Completed。 |
| 集成：CLI 黑盒 | `cli_planning_path_with_real_llm` / `cli_exec_resume_path_with_real_llm`（[`tests/plan_real_llm_cli_e2e.rs`](../../../tests/plan_real_llm_cli_e2e.rs)） | DONE（需 `OPENAI_API_KEY`） | 两条窄 smoke：一条真 `create_plan`，一条 seeded `/plan build` wiring smoke（`--resume` + EXEC prompt + session 绑定）；full completion 交给 inprocess。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| reviewer hang 不收敛 | 高 | `max_turns = 8` + `max_review_rounds = 1`（软上限） + `CascadeAbort` | 卡住要能 abort。 |
| reviewer 输出格式漂移 | 中 | 明确 `<review>` fenced block（含 `changes_summary`）+ 严格解析 + 失败时标 `aborted`，不静默猜 | 别静默猜摘要。 |
| reviewer 误改用户代码 | 高 | `allowed_tools` 默认不含 `write` / `bash` / `dispatch_agent`；`allow_review_edit` 仅放开 `edit` + `update_plan`，且 `tool_exec` 各自二次守卫 | 默认只读；改稿只能改计划文件或 frontmatter todos。 |
| reviewer 越权调 `create_plan` 套娃 | 高 | `allowed_tools` 任何模式都不含 `create_plan`；`tool_exec.create_plan` 在 `subagent_type == Reviewer` 路径上直接 tool error（双保险） | 工具白名单 + 入口拦截。 |
| reviewer 越界改 frontmatter | 中 | `tool_exec.edit` frontmatter diff guard 强制拒绝；transcript 记 warning | 越界 error。 |
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
| ~~reviewer 改稿仅能写 `## Review` 段~~ | **否**：当前设计允许 `edit` 改 plan 正文任意段；只保留 frontmatter raw 不可改这条硬边界；frontmatter todos 继续走 `update_plan`。 | 改稿不是写审稿区，而是直接修正文。 |
| ~~reviewer 改稿独占 `edit_plan_review_section` 内部工具~~ | **替代（D 方案）**：直接用通用 `edit`（路径白名单 + frontmatter diff guard）+ `update_plan`（结构化）；不再造一个专用工具。 | 复用通用工具 + 双重守卫。 |
| ~~reviewer `allowed_tools` 默认 `{read, grep, find}`，true 时附加 `edit`~~ | **替代（D 方案）**：默认 `{read, grep, find, todos}`（`todos` 写自己的 `.todo.md` 无副作用，方便 reviewer 列调研步骤）；`allow_review_edit=true` 附加 `{update_plan, edit}`；**任何模式都不含 `create_plan`**（防套娃 + 职责单一）。 | 默认加 todos 个人 scratchpad；改稿权多给 update_plan。 |
| ~~`create_plan` 在 reviewer 上下文里 `tool_exec.create_plan` 跳过 dispatch_reviewer 即够~~ | **替代（D 方案）**：双保险——`allowed_tools` 永不含 `create_plan` + `tool_exec.create_plan` 在 reviewer 路径直接 tool error。 | 双保险防套娃。 |
| ~~把 reviewer 做成 long-lived 后台 daemon~~ | **否**：每次 `create_plan` 派一个；运行结束即销毁；状态由 `transcript.plan.review` 持久化。 | 每次写完派一个，用完即毁。 |
| ~~`max_review_rounds` 超限切 `Planning` + 阻塞~~ | **否**：超限只 warning，不阻塞 `create_plan` 也不阻塞 `/plan build`；摘要照常落 transcript。 | 软上限，超了只提醒。 |
| ~~`ChatContext` 内嵌 Agent 注册表（`subagents[]` / `HashMap<SubagentType, Agent>`）~~ | **否**（RV10–RV11）：双注册表正交——`ChatContextRegistry` 按 `session_key` 持壳，进程级 `AgentRegistry` 按 `session_id` 持 handle；子 `AgentLoop` 永远栈上拥有、跑完 drop。详见 [`multi-agent.md` §14.3.2.1 / §14.4.2.2](../multi-agent.md#14321-chatcontextregistry-vs-agentregistry-分工)。 | 聊天室不管 Agent 表，访客登记处管。 |
| ~~`ReviewerRuntime` 作为长生命周期对象挂在 `ChatContext`~~ | **否**（RV12）：reviewer 跨调用无残留状态；`rounds` / `last_review_summary` 已落 `PlanRuntimeState` + transcript `plan.review`。 | 审稿员不设专职岗位。 |
| ~~游离 `review::dispatch_reviewer` 自由函数~~ | **替代**（RV13）：派发归 `PlanRuntime::dispatch_reviewer` 方法；`review.rs` 仅保留 prompt 构造 / `<review>` 解析 helpers。 | 派审稿归 PlanRuntime 管。 |

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
- 任务卡：[T2-P1-002.md](../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)
- 完成后代码验证（verifier，与 reviewer 分拆）：[plan-exec-code-verification.md](../plan-exec-code-verification.md)

**说人话**：reviewer 由 `create-plan.md` 内部触发；dispatch 通用子 Agent 看 [`multi-agent.md`](../multi-agent.md)。
