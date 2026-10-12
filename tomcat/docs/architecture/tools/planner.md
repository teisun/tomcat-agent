# `planner`：PLAN 模式与 EXEC 模式整体规范（非 LLM 工具）

> **重要**：`planner` **不是** LLM 工具，**也不是** subagent；它是**会话模式（PLAN Mode）的整体规范**，本文同时收口执行中计划（Executing）。`/plan` 命令族、模式激活与退出、请求级 reminder、稳定工具目录、handler policy、UI 模式标识、CLI prompt 渲染与 `current_mode()` 查询函数都在本文。LLM 始终看到同一份内置工具目录；`create_plan`、[`update_plan`](./update-plan.md)、[`ask_question`](./ask-question.md) 与 [`todos`](./todos.md) 是否可执行由 handler 和路径策略强制判断。

本文档是 **B 类**：`docs/architecture/tools/`，承接 [`plan-runtime.md`](../plan-runtime.md) 的运行时编排，与 [`create-plan.md`](./create-plan.md) / [`ask-question.md`](./ask-question.md) / [`todos.md`](./todos.md) / [`reviewer.md`](./reviewer.md) 协同。**实现以仓库代码为准**。

## 2026-08 生效契约：没有 EXEC 会话模式

> 本节优先于后文遗留的 `PlanState` / EXEC 历史设计；后文只保留为演进记录。

`AgentMode` 只有 `Chat | Plan`。`planning / pending / executing / completed` 是计划文件的生命周期，绝不表示会话模式。

| 命令 / 事件 | 会话模式 | 计划文件 | transcript |
|---|---|---|---|
| `/plan` | `Chat → Plan` | 不改 | `session.agent_mode.changed { agentMode: "plan" }` |
| `/plan exit` | `Plan → Chat` | 不改 | `session.agent_mode.changed { agentMode: "chat" }` |
| `/plan build [target]` | 若当前 Plan，则 `Plan → Chat` | `planning|pending → executing` | `plan.build`；必要时一条 mode changed |
| 用户取消执行 | 保持 Chat | `executing → pending` | `plan.pending` |
| 所有 todo 收口 | 保持 Chat | `executing → completed` | `plan.complete` |

Build 不再“进入 EXEC”。它是从 Plan 回到 Chat、并开始推进一份执行中计划。执行提醒的判据是 `executing_plan_id()`，不是会话模式；planner reminder 的判据才是 `mode() == AgentMode::Plan`。两份 reminder 已在每次请求的 ephemeral tail 中生成，既不写入 system prompt，也不写入持久历史。

CLI 显示 `u[Chat]>` 或 `u[Plan]>`，Chat 绑定 executing/pending 计划时显示 `u[Chat·plan:executing]>` / `u[Chat·plan:pending]>`。

## 2026-05 Active Binding 补充（历史）

当前 `/plan` 命令族的稳定契约如下：

- `/plan build` 现在允许省略目标；默认源顺序为 `active_planning_plan_id -> Pending { id } -> active_plan_path`；
- `/plan exit` 允许 `Planning / Pending -> Chat`；若当前是 **idle 的 `Executing`**，则先把盘上的 plan `executing -> pending`，再退回 `Chat`，用于救回已卡死会话；
- `/plan build` 是唯一会写 active binding 的动作；
- `Completed` 不是稳定可见模式：all-completed 后会立即回到 `Chat(retain)`，CLI prompt 仍显示 `u[Chat]>`；
- 文档中凡提到 `PlanMode`，实现名请以 `PlanState` 为准；`mode.rs` 已由 `state.rs` 接管。
- 下文若仍出现 `plan.enter / plan.exit / plan.complete / plan.pending`、`active_plan_id`、`u[Plan:completed]>` 等旧草稿词汇，均以上述稳定契约与仓库代码为准。

### 当前命令行为速记

| 命令 | 现行行为 |
|------|----------|
| `/plan` | 进入 `Planning`；不写盘，不写事件。 |
| `/plan build [target]` | 若省略 target，默认源顺序为 `active_planning_plan_id -> Pending { id } -> active_plan_path`；成功后写 binding + `plan.build`。 |
| `/plan exit` | `Planning / Pending` 直接回 `Chat`；`Executing + idle` 先 `demote_to_pending_on_cancel()` 再回 `Chat`；不清 retain 字段。 |

### 当前可见状态

对用户与 agent 装配来说，稳定态只有 `Chat / Planning / Pending / Executing`；`Completed` 只在 `update_plan` 收口到 `finalize_completed_to_chat()` 之间瞬时存在。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：PLAN 模式是会话开关，Executing 是 PLAN 结束后用户拍板开干的一份计划状态——`/plan` 进 PLAN、`/plan build <plan_id/path>` 回 CHAT 并推进计划；正常 `/plan exit` 从 `Planning / Pending` 直接回 CHAT，若某个会话已经卡在 idle 的执行中计划，也允许把它先降回 `pending` 再退 CHAT。每次请求都会把对应 reminder 追加到 ephemeral tail；工具目录不随模式切换，真正调用时由 handler、路径白名单和 frontmatter 规则拦截。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. mode 激活 / 退出与命令族](#5-mode-激活--退出与命令族)
- [6. 提示词尾部与稳定 catalog](#6-提示词尾部与稳定-catalog)
- [7. UI 模式标识、`current_mode()` 与 CLI prompt](#7-ui-模式标识current_mode-与-cli-prompt)
- [8. 调度时序](#8-调度时序)
- [9. 状态机](#9-状态机)
- [10. 配置与环境变量](#10-配置与环境变量)
- [11. 错误模型 / 警告](#11-错误模型--警告)
- [12. 测试矩阵（验收）](#12-测试矩阵验收)
- [13. 风险与应对](#13-风险与应对)
- [14. 历史决策](#14-历史决策)
- [15. 关联文档](#15-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **PLAN 模式（Planner Mode）** | 会话被切到「规划」语义下 | `PlanRuntime.mode == Planning` | 由本地命令 `/plan` 进入；非 LLM 工具；非 subagent | 一种会话模式。 |
| **Executing（计划生命周期）** | 推进 `PlanFile` 待办的执行态 | `PlanRuntime.executing_plan_id()` | 由本地命令 `/plan build <plan_id/path>` 开始；仍处于 Chat 会话模式 | 正在开干的计划，不是第三种模式。 |
| **CHAT 模式** | 默认普通聊天 | `PlanRuntime.mode == Chat` | 使用稳定工具目录；handler 决定某次调用是否允许 | 不在规划的日常，也可能绑定一份执行中计划。 |
| **`/plan` 命令族** | 控制 PLAN/EXEC 模式与计划闭环的本地 slash | `src/api/chat/commands/cmd_plan.rs`（拟定） | 解析在本地完成，不丢给 LLM；`/plan` / `/plan exit` / `/plan build <plan_id/path>` 三条 | 用户控会话流的把手。 |
| **PLANNER_SYSTEM_REMINDER** | PLAN 请求末尾的 `<system_reminder kind="planner">` | 进程内常量 + ephemeral-tail 装配 | 仅在 `Planning` 期间存在；不进入持久历史 | 规划模式提示词。 |
| **EXECUTOR_SYSTEM_REMINDER** | 绑定执行中计划的请求末尾 `<system_reminder kind="executor">` | 进程内常量 + ephemeral-tail 装配 | 仅在存在 executing plan 时出现；不进入持久历史 | 执行提醒。 |
| **CLI prompt helper** | 基于 `PlanState` 统一渲染 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` 与对应 agent prompt | `src/api/chat/prompt.rs` | `Completed` 对用户侧折叠为 `Chat`；CHAT agent prompt 保持 `agent.<id>>` | 显示层统一走 helper。 |
| **EXEC 首轮上下文** | `/plan build` 后直接进入执行回合；runtime 依赖当前 PlanFile、system reminder 与工具结果维持上下文 | runtime 装配 | 不再注入 `<plan_meta>` | 进入 EXEC 后靠现有上下文收口，不额外塞一段 plan_meta。 |
| **稳定 catalog** | runtime 每轮下发同一份内置工具数组 | `src/core/plan_runtime/catalog.rs` | 模式和计划生命周期不改变 tools；handler policy / path gate 决定调用是否允许 | 缓存前缀不抖，安全检查不变。 |
| **写盘路径白名单（PLAN 模式专用）** | PLAN 期 `write/edit` 只能写 `~/.tomcat/plans/*.plan.md` | `tool_exec` 在 PLAN 模式校验 path | 其它路径硬拒；其它模式无此白名单（仅 frontmatter 拦截） | 规划阶段只许动计划文件。 |
| **`current_mode()`** | 查询当前会话 PLAN 状态的 Rust API | `PlanRuntime::mode(&self) -> PlanState` | UI、tool handler、prompt helper 与 reminder 注入都查这个；不改变 tools 数组 | 模式的事实源就是 runtime。 |
| **mode 指示器（UI）** | 状态行 / 标题栏中的模式标签 | UI 渲染层 | `Chat → [CHAT]`；`Planning → [PLAN]`；`Executing → [EXEC plan_id=…]`；`Completed → [DONE plan_id=…]`；`Pending → [PENDING plan_id=…]` | 让用户一眼看到现在哪个模式。 |

---

## 2. 竞品 / 选型对比（调研）

### 2.1 PLAN 模式的典型形态

```text
┌───────────────────────────────────────────────────────────────────────┐
│  PLAN 模式在主流 agent 里大致三种形态                                 │
├──────────────────────┬───────────────────────────────────────────┤
│  LLM-facing tool     │  cc-fork-01：EnterPlanMode / ExitPlanMode  │
│                      │  让模型自己决定进出，可被脑补滥用            │
├──────────────────────┼───────────────────────────────────────────┤
│  本地 slash + 系统提示│  codex：/plan + system prompt             │
│                      │  用户控流程，模型守提示词                   │
├──────────────────────┼───────────────────────────────────────────┤
│  subagent            │  hermes：planner role 子 Agent              │
│                      │  子 Agent 上下文隔离但回路重                │
└──────────────────────┴───────────────────────────────────────────┘
```

**说人话**：让模型自己进出 PLAN（cc-fork）会被滥用；用 subagent（hermes）回路太重；本仓采用「本地 slash + 请求尾部 reminder + handler policy」，既由用户控制流程，也保持缓存前缀稳定。

### 2.2 常见实现横向对比

| 来源 | PLAN 形态 | 进入方式 | catalog 是否动态 | 退出方式 | 说人话 |
|------|-----------|----------|------------------|----------|--------|
| **cc-fork-01** | LLM tool | LLM 自调 `EnterPlanMode` | 否（约定） | LLM 调 `ExitPlanMode` 或用户中断 | 模型自治，约束弱。 |
| **codex** | 本地 slash + 提示词 | `/plan` slash | 是 | `/plan exit` / `/plan apply` | 收益/复杂度最优。 |
| **hermes-agent** | role 子 Agent | `delegate_task(role='planner')` | 子 Agent 上下文独立 | 子 Agent 完成 | 回路重。 |
| **Cursor 内置** | 模式选择器 | UI 切换 | 是（Plan / Agent / Ask 模式） | UI 切换 | UI 控制更直观。 |
| **本仓库 `planner`** | **本地 slash + ephemeral-tail reminder + 稳定 catalog + CLI prompt helper** | `/plan` | 否 | `/plan exit` 回 CHAT；`/plan build` 回 CHAT 并绑定执行计划；完成 / cancel 自动派生 | handler 与 path gate 负责策略。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| P1 形态 | tool / slash / subagent / role | 选 slash + 提示词 + catalog。 |
| P2 进入方式 | 用户 vs LLM 决定 | 用户控。 |
| P3 catalog 是否动态 | 静态白名单 vs 按 mode 过滤 | 内置目录保持稳定，策略在 handler。 |
| P4 退出方式 | 是否区分 exit / build / 完成 / pending | 多出口对应「不要了 / 进执行 / 自动完成 / 被打断」。 |
| P5 运行时提示 | 全局 system vs 请求尾部 reminder | 局部 reminder，注入 ephemeral tail。 |
| P6 UI 标识 | 是否显示当前模式 | 显示。 |
| P7 CLI prompt | 仅靠 system 提示 vs 可见 prompt 显式显示当前模式 | 统一显示 `u[Plan:*]>` / `agent.<id>[Plan:*]>`。 |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§12） | 说人话 |
|----|------|------------------|--------|
| G1 | PLAN / EXEC 模式由本地 slash 进入，不向 LLM 暴露「Enter/ExitPlanMode」工具 | `plan_enter_is_local_only`、`plan_build_is_local_only` | 只认 slash，不认 LLM tool。 |
| G2 | 每次请求按 PLAN / executing 状态追加对应 `<system_reminder>` 到 ephemeral tail，保持 catalog 不变 | `request_prefix_is_byte_identical_across_turns` | 进状态就有提示词，真调用时收紧工具。 |
| G3 | `current_mode()` 是单一事实源，UI / catalog / tool_exec / prompt helper / reminder 一致引用 | `current_mode_is_single_source_of_truth` | 模式只问 runtime 一处。 |
| G4 | `/plan exit` 只负责 `Planning / Pending -> Chat`，立即解除当前交互态回 CHAT，下一轮装配不再含对应 reminder/prefix | `plan_exit_restores_chat_only_from_planning` | exit 不当 close 用。 |
| G5 | `/plan build` 是 EXEC 唯一入口；前置 `当前 session` 不能处于 `Executing`；若是 `Pending`，默认续跑当前盘，但也允许显式切到另一份 `planning/pending` plan；若仅有 scratchpad todos 未收口则 warning 后继续；目标 PlanFile `mode ∈ {planning, pending}` | `plan_build_rejects_active_executing_plan`、`plan_build_warns_but_continues_with_active_session_todos`、`completed_session_can_build_another_explicit_plan`、`pending_session_can_build_another_explicit_plan` | 用户拍板，工具不替。 |
| G6 | mode 自动派生：全 todos completed → `Completed`；cancel_token → `Pending` | `all_completed_promotes_completed`、`cancel_token_demotes_pending` | 不靠 close 命令，状态自然演化。 |
| G7 | UI 状态行反映 mode：`[CHAT]` / `[PLAN]` / `[EXEC plan_id=…]` / `[DONE plan_id=…]` / `[PENDING plan_id=…]` | `ui_shows_correct_mode_indicator` | 状态行要能看出在哪个模式。 |
| G8 | PLAN/EXEC 模式下 user message 装配阶段加模式前缀；不污染 transcript | `user_message_prefix_only_in_assembly`、`transcript_unchanged_by_prefix` | 每条贴 tag，但不动聊天记录。 |

**说人话（§3 总览）**：PLAN/EXEC 模式靠用户 slash 进出、靠 reminder + prefix 双保险管住模型、靠 `current_mode()` 一处查状态、build 由用户拍、完成/暂停自动派生、UI 要能看见当前模式。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| `EnterPlanMode` / `ExitPlanMode` 作为 LLM 工具 | 已在 §14 否决 | 不让模型自己开关 PLAN。 |
| `/plan close` 命令 | 完成由 runtime 派生；用户不要可以 `/plan exit` 退 PLAN；EXEC 中按 Ctrl+C 自动 pending | 不要 close 命令。 |
| reviewer accepted 自动进 EXEC | reviewer 仅辅助；进 EXEC 必须用户敲 `/plan build` | 不偷偷开干。 |
| 把 reminder 注入 system 或 user message | 所有 `<system_reminder>` 都注入 ephemeral tail | reminder 不改写历史。 |
| 在调用侧手写 prompt 字符串 | prompt 一律由 `src/api/chat/prompt.rs` 生成 | 不让显示文案漂移。 |
| PLAN 模式独占 transcript | 仍写主 session，仅 `plan.enter` / `plan.exit` 标边界 | 不另开一份会话文件。 |
| PLAN 模式复制一整份 tool catalog | 同一份稳定 catalog；按 mode 的 policy 在 handler 执行 | 不维护两套工具表，也不抖缓存前缀。 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表（摘要）

| 维度 | 关切 | 说人话 |
|------|------|--------|
| P1 形态 | slash + ephemeral reminder + stable catalog + prompt helper，非 LLM tool / 非 subagent | 会话开关四件套。 |
| P2 进入 | `/plan` / `/plan build <plan_id/path>` 本地命令 | 用户控进入。 |
| P3 catalog | 稳定目录；`current_mode()` 驱动 handler policy | 按 mode 真裁调用，不隐藏工具。 |
| P4 退出 | `/plan exit`（仅 PLAN）、`/plan build`（PLAN→EXEC）、自动完成 / 自动 pending | 不要 close 命令。 |

完整 R 维度矩阵见 [`plan-runtime.md`](../plan-runtime.md) §4.1。

### 4.2 实施点（拟定）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PM-A** | `/plan` 本地命令族 + `cmd_plan.rs`；`parse.rs` 识别；**交付**：`ChatCommand::Plan` | `src/api/chat/commands/{parse.rs,cmd_plan.rs}`、`cmd_help.rs` | 见 §12：`parse_plan_commands`、`plan_enter_rejects_while_active` | slash 不进 LLM，本地先吃掉。 |
| **PM-B** | `PlanRuntime` API + per-session 实例（详见 [`plan-runtime.md`](../plan-runtime.md) §6）；`enter/exit/build` API；**交付**：`PlanState` / `PlanRuntime` | `src/api/chat/plan_runtime/{mod.rs,state.rs}` | 见 §12：`current_mode_is_single_source_of_truth`、`plan_build_rejects_active_executing_plan`、`plan_build_warns_but_continues_with_active_session_todos`、`completed_session_can_build_another_explicit_plan` | 模式状态一处存。 |
| **PM-C** | `PLANNER_SYSTEM_REMINDER` / `EXECUTOR_SYSTEM_REMINDER` 在每个请求的 ephemeral tail 按状态渲染；稳定 transcript 恢复链路只依赖 `plan.build` 与 plan 工具事件；**交付**：常量 + request 装配 | `src/api/chat/run_loop/workspace_state.rs`、`src/infra/transcript/...` | 见 §12：`request_prefix_is_byte_identical_across_turns` | reminder 上线/下线挂在状态。 |
| **PM-D** | 稳定 tool surface + `tool_exec` policy 校验；PLAN 模式 `write/edit` 路径白名单；frontmatter raw 改硬拦截（详见 [`create-plan.md`](./create-plan.md) §8）；**交付**：白名单常量 | `src/core/plan_runtime/catalog.rs`、`src/core/agent_loop/tool_exec.rs` | 见 §12：`tool_catalog_is_identical_across_all_mode_and_executing_combinations`、`plan_mode_raw_edit_body_allowed_frontmatter_rejected` | catalog 不随 mode 抖动；调工具和写盘时再拦。 |
| **PM-E** | UI 模式指示器 + `current_mode()` 公开；prompt 统一 helper 渲染；**交付**：状态行文案 + prompt helper | UI 层 + `src/api/chat/prompt.rs` | 见 §12：`ui_shows_correct_mode_indicator`、`prompt_helper_renders_plan_modes` | 状态行 / prompt 收口合一。 |

下文按实施点展开**技术要点与示意图**；**命令语义细节见 [§5](#5-mode-激活--退出与命令族)**。

#### 4.2.1 PM-A：slash 命令面

- **交付**：`parse_command` 匹配 `/plan` 子命令（`/plan` / `exit` / `build <plan_id>`）；`dispatch_chat_command` **优先**消费，不进入 LLM user 文本（对齐 `/ckpt`）。
- **重入**：`enter_plan_mode` 前检查 `mode ∈ {Planning, Executing}` → `UsageError`；`build_plan` 前检查 `当前 session` 不得处于 `Executing`；若当前是 `Pending`，无参默认续跑当前盘，显式 target 则允许切到别盘；scratchpad todos 仅 warning。

```text
  user input
      │
      ▼
  parse.rs ──/plan──▶ cmd_plan.rs
      │
      └── 其它 ──▶ runtime 装配（加 prefix）──▶ LLM
```

**说人话**：/plan 像本地控制台命令，绝不混进模型上下文。

#### 4.2.2 PM-B：`PlanRuntime` API 与状态

- **交付**：`enter_plan_mode` / `exit_plan_mode` / `build_plan`；`PlanRuntime` per-session 单实例挂 `ChatContext`；详见 [`plan-runtime.md`](../plan-runtime.md) §6。
- **写入入口**：仅 `cmd_plan.rs`、`tool_exec::create_plan`、`tool_exec::todos`、runtime 自动转移（completed / pending）。

**说人话**：四个 API 是 slash 的真正实现；状态机别旁路写。

#### 4.2.3 PM-C：system reminder 与 transcript 边界

- **交付**：`enter_plan_mode` 后在下一轮请求尾部插入 `PLANNER_SYSTEM_REMINDER`（`<system_reminder kind="planner">`）；`build_plan` 后切换为 `EXECUTOR_SYSTEM_REMINDER`；`exit_plan_mode` / cancel_token / 完成派生后下轮不再注入，也不写入 transcript 或 `ContextState.messages`。
- **事件**：`plan.enter` / `plan.exit` / `plan.build` / `plan.complete` / `plan.pending` 标记 mode 边界（主 transcript 不断开）。

**说人话**：进 PLAN/EXEC 各在请求尾部临时注一段 reminder，出了就自动拿掉；它不改变可缓存的 system 前缀，transcript 仍用事件记号方便回放。

#### 4.2.4 PM-D：稳定 catalog + 写盘双保险

- **交付**：稳定的 `all_tools_with_policy`（见 [§6.2](#62-稳定-catalog-与-handler-policy)）；PLAN 模式 `write/edit` 路径必须在 `~/.tomcat/plans/*.plan.md` 否则 tool error；frontmatter raw 改硬拒（详见 [`create-plan.md`](./create-plan.md) §8）。
- **双保险**：`tool_exec` dispatch 前按 `current_mode()` 与活跃计划再查 policy。

```text
  all modes   →  同一份全工具集（仅 load_skill 按 skills 是否可用调整）
  Planning    →  handler 允许 create_plan；write/edit 仅 ~/.tomcat/plans/*.plan.md
  Executing   →  handler 拒绝 create_plan / ask_question；plan 文件写入继续受路径策略保护
  Chat        →  create_plan handler 给出进入 Plan 模式的指引
  Completed / Pending → 同 Chat；等待 /plan build 续跑时不改变目录
```

**说人话**：每轮不再按 mode 改工具数组；模型仍会在真正调用时被 `tool_exec` 和路径策略拦下，PLAN 期写盘路径白名单 + frontmatter 拦截仍然生效。

#### 4.2.5 PM-E：UI、`current_mode()` 与统一 prompt

- **交付**：`PlanRuntime::current_mode() -> PlanMode` 为唯一事实源；UI 状态行渲染规则见 §7.1；可见 prompt 统一走 `src/api/chat/prompt.rs`（详见 §7.3）；EXEC 首轮不再额外注入 `<plan_meta>`（详见 §7.4）。
- **测试**：`cfg(test)` 下 `__test_set_mode` 覆写。

**说人话**：界面、catalog、prompt、reminder 全都只问 runtime 当前 mode，不各自缓存；进入 EXEC 后也不再额外塞 plan_meta。

---

## 5. mode 激活 / 退出与命令族

### 5.1 命令一览

| 命令 | 解析层 | 副作用 | 前置条件 | 说人话 |
|------|--------|--------|----------|--------|
| `/plan` | 本地 chat 命令解析（不入 LLM） | `mode = Planning`，下一请求尾部渲染 PLANNER reminder；catalog 保持稳定，写盘路径白名单仍由 `~/.tomcat/plans/*.plan.md` 策略拦截，prompt 切 `u[Plan:planning]>` | 当前 session 不在 `Planning / Executing` | 进 PLAN 模式。 |
| `/plan exit` | 本地 | `mode = Chat`；保留 PlanFile 不动（不写盘、不改 frontmatter）；下一请求不再带 planner tail，prompt 复位 CHAT；**不写事件** | `mode ∈ {Planning, Pending}` 可用；其他状态友好提示 | 不要这次规划了，回到 CHAT。 |
| `/plan build <plan_id/path>` | 本地 | runtime 写 session 绑定与 `frontmatter.state = executing`，随后从 Plan 回到 Chat；下一请求由 executor tail 提醒，prompt 显示绑定计划状态；catalog 保持稳定 | 当前 session 未绑定其他 executing plan；若当前是 `Pending`，无参默认续跑当前盘，显式 target 允许切别盘；scratchpad todos 仅 warning；指定的 PlanFile `state ∈ {planning, pending}` | 把审过 / 续跑的计划推到执行态。 |

> **历史命令下线**（详见 §14）：
> - `/plan apply` → `/plan build <plan_id/path>`
> - `/plan close [completed\|cancelled]` → 移除；完成由 runtime 自动派生（全 todos completed），暂停由 cancel_token 自动 pending
> - `/plan show` → 暂缓；用户直接打开 `.plan.md`
> - `/goal` → 暂缓；目标在进入 PLAN 后通过自然对话收敛

### 5.2 模式可调用工具矩阵

| `current_mode()` | 稳定 LLM 工具目录下的 handler policy | `write/edit` 路径约束 | 说人话 |
|-------------------|--------------------------|------------------------|--------|
| `Chat` | 所有稳定内置工具下发；`create_plan` handler 引导先进入 PLAN | 任意路径（写 `~/.tomcat/plans/*.plan.md` 的 frontmatter 仍被 raw 拦截） | 普通 Agent：每个工具都可见，调用是否允许看策略。 |
| `Planning` | 所有稳定内置工具下发；handler 允许 `create_plan` / `ask_question` | `write/edit/hashline_edit/delete` **仅允许** `~/.tomcat/plans/*.plan.md`；frontmatter 仍被 raw 拦截 | 调研、写计划、问用户、用 `update_plan` 调 todos；写工具只能动 plans/。 |
| `Executing` | 所有稳定内置工具下发；handler 拒绝生命周期不允许的 `create_plan` / `ask_question` | **拒绝任何对 `~/.tomcat/plans/*` 的写**（含正文与 frontmatter） | 推进 plan 用 `update_plan`（默认指向 active plan）；plan 文件全禁写。 |
| `Completed` | 同 `Chat` policy（瞬时态，通常不会停留到下一轮） | 同 `Chat` | 计划结束后立即收口回 Chat(retain)。 |
| `Pending` | 同 `Chat` policy | 同 `Chat` | 等待 `/plan build` 续跑。 |

> **关键差异（D 方案 / 2026-08 收口）**：
> 1. 所有内置工具始终可见；`todos` / `update_plan`、`ask_question`、`create_plan` 的允许性由 handler 按会话模式和计划生命周期判断；
> 2. PLAN 模式写工具硬性限制路径（`~/.tomcat/plans/*.plan.md`），离开此目录的任何写一律拒；
> 3. EXEC 模式 plan 文件全禁写（含正文）；`update_plan` 推进 PlanFile，`todos` 维护独立的 session scratchpad；
> 4. `state=completed` 自动派生由 `update_plan` 在 EXEC 触发，与 `todos` 无关。

**说人话**：模式变化不会改模型的工具名单；每次工具调用仍由 handler 和路径白名单真过滤，prompt tail 只负责把当前状态讲清楚。

### 5.3 callable exit function（API 形态）

```rust
impl PlanRuntime {
    pub fn mode(&self) -> PlanState { /* ... */ }

    pub async fn enter_plan_mode(&self) -> Result<()> { /* /plan */ }
    pub async fn exit_plan_mode(&self)              -> Result<()> { /* /plan exit  */ }
    pub async fn build_plan(&self, plan_id_or_path: &str) -> Result<()> { /* /plan build */ }

    // runtime 内部触发（无对应 slash）：
    pub(crate) fn on_all_todos_completed(&self) -> Result<()> { /* mode = Completed */ }
    pub(crate) fn on_cancel_token(&self)        -> Result<()> { /* mode = Pending */ }
}
```

UI / 测试 / 集成层一律通过这三个 API 触发，slash 命令解析层只是它们的薄封装。

**说人话**：slash 只是壳，真正改状态的是三个 Rust API + 两个内部 hook；测试直接调 API 更稳。

---

## 6. 提示词尾部与稳定 catalog

### 6.1 reminder 常量（PLANNER + EXECUTOR）

#### 6.1.1 `PLANNER_SYSTEM_REMINDER`

> 设计参考：[`plan-mode-execution-playbook-T2-P0-001.md`](../../reports/plan-mode-execution-playbook-T2-P0-001.md) §「PLAN 模式行为契约」，按本期决策改造：① 每次只问 2-4 个关键问题；② mermaid 图改为 ASCII 图；③ reviewer 仅辅助、不做 gate。

```rust
pub const PLANNER_SYSTEM_REMINDER: &str = r#"
<system_reminder kind="planner">
You are now in PLAN mode. Behavior contract (12 rules; D-plan):

1.  Mode awareness: the CLI prompt visibly shows `u[Plan:planning]>` while the
    session stays in PLAN mode. Treat that visible mode as the source of truth;
    do not invent alternate state labels.

2.  Goal alignment: the user's objective is the source of truth. If anything is
    ambiguous, ask 2-4 high-leverage questions via `ask_question` (each with
    2-4 structured options) BEFORE drafting a plan. Do not stack more than 4
    questions per turn.

3.  Read-and-verify first: use `read`, `grep`, `find`, `bash` (read-only
    inspection), or `dispatch_agent` (with `subagent_type ∈ {explore, general}`)
    to verify constraints, library versions, file paths, and assumptions BEFORE
    making architectural calls. Do NOT guess.

4.  Catalog awareness: while in PLAN mode the runtime shows `create_plan` +
    `ask_question` + `todos` + `update_plan` on top of the full toolset.
    `write`/`edit` are scoped to `~/.tomcat/plans/*.plan.md` only (path
    whitelist). Any other write target is rejected by the runtime.

5.  Frontmatter is off-limits to raw write/edit. PlanFile YAML is managed by
    four writers: `create_plan` (initial whole-plan draft), `update_plan`
    (incremental `todos[]` edits), runtime (mode / session binding on
    `/plan build`), and auto-derivation (`state=completed` on all-completed,
    `state=pending` on cancel_token). Raw-editing YAML keys returns a tool error.

6.  Draft via `create_plan` for the FIRST draft or a WHOLESALE rewrite:
    provide `goal`, `draft` (free-form markdown body), and `todos[]`. The
    runtime fills the rest of the frontmatter. Do NOT include frontmatter
    fields in your `create_plan` arguments. After this call, the runtime
    internally dispatches a reviewer (advisory only).

7.  Reviewer is advisory, not a gate: every `create_plan` call returns a
    `review_summary`. The summary lands in `transcript.plan.review` and the
    same tool result. It does NOT auto-promote the plan to EXEC; the user
    decides whether to issue `/plan build`.

8.  Revise INCREMENTALLY via `update_plan`: to mark a todo done, add a single
    todo, or rewrite the current todo list in place, call `update_plan` — do
    NOT rewrite the entire plan via `create_plan` for small edits.
    `update_plan` is visible in all modes.

9.  When to use `todos` vs `update_plan`:
    - `todos` writes to your session-local `.todo.md` scratchpad. Use it to
      track your own research / inspection steps (3+ steps) that are NOT part
      of the plan; it never touches the PlanFile.
    - `update_plan` writes to the PlanFile's frontmatter `todos[]`. Use it to
      revise the actual plan.
    In planning, default todo status is `pending` — do NOT mark steps
    `in_progress` until execution actually starts.

10. ASCII diagrams only: when the plan body needs flow/architecture figures,
    use ASCII art (boxes, arrows, indentation). Do NOT emit Mermaid, PlantUML,
    SVG, or any other DSL.

11. Question budget: aim to settle the plan within 1-3 rounds of
    `ask_question`. If the user is clearly engaged in free-form chat, fall
    back to natural-language clarifications rather than spamming
    `ask_question`.

12. To leave PLAN mode, the user issues `/plan exit` (back to CHAT) or
    `/plan build <plan_id/path>` (back to CHAT with an executing plan). Do
    NOT attempt to leave via tool calls. Once the user issues `/plan build`,
    the runtime replaces the request-local reminder; the stable tool catalog
    remains unchanged.
</system_reminder>
"#;
```

- **注入位置**：进入 PLAN 模式时，runtime 在下一轮请求的 ephemeral tail 追加该 `<system_reminder>` 段（**不**改 system prompt 或持久 user message）。
- **退出**：`/plan exit` / `/plan build` 后下一轮不再注入。
- transcript 同步写 `plan.enter` / `plan.exit` 自定义事件。

#### 6.1.2 `EXECUTOR_SYSTEM_REMINDER`

```rust
pub const EXECUTOR_SYSTEM_REMINDER: &str = r#"
<system_reminder kind="executor">
You are in EXEC mode. Your mission: drive the active plan to completion using ANY available tool. Whenever you make progress on a todo, mark it via update_plan; the runtime handles everything else.

1.  Mission first: the visible CLI prompt shows `u[Plan:executing]>` while the
    active plan is running. Advance that active plan using whatever tools you
    need (read / grep / bash / write / edit / search_files / dispatch_agent, etc.).

2.  Update via update_plan only: claim the next todo with `set_status(in_progress)` BEFORE running side-effecting tools; mark `completed` immediately when done; use `cancelled` for steps deliberately skipped. At most three independent todos may be `in_progress` in the same PlanFile. In EXEC mode `plan_id` defaults to the active plan, so you can omit it.

3.  Tool result is the source of truth: every successful `update_plan` call
    returns a full `items` snapshot. You do NOT need to re-read the PlanFile
    to know the current state — trust the snapshot.

4.  Plan file is off-limits to raw write/edit/delete (frontmatter AND body). The runtime rejects any direct write to `~/.tomcat/plans/*.plan.md` in EXEC. Use `update_plan` for progress. If the plan needs structural rewrite, ask the user to exit and re-plan; do NOT try to leave EXEC via tool calls.

5.  Completion is automatic: when ALL todos in the PlanFile flip to
    `completed`, the runtime promotes `mode = completed`, swaps the
    reminder/prompt/catalog back to CHAT, and you do NOT need to "close" the
    plan.
</system_reminder>
"#;
```

- **注入位置**：`/plan build` 完成后，runtime 在下一轮请求的 ephemeral tail 注入；可见 prompt 同步切成 executing 态（详见 §7.3 / §7.4）。
- **退出**：自动 `mode = completed` / `state = pending` 后下轮不再注入。

**说人话**：进 PLAN 多一段 12 条契约提醒；进 EXEC 多一段 6 条精简契约提醒（主旨「推进任务 + 仅 update_plan 改进度 + plan 文件全禁写」）；出了模式 reminder 自动消失。

### 6.2 稳定 catalog 与 handler policy

```rust
// catalog.rs
//
// All built-ins stay present so tools are the first stable prompt-cache prefix.
// Only load_skill varies because it would be nonfunctional without a visible
// skill inventory. Every mode-sensitive decision stays at the handler.
pub fn all_tools_with_policy(allow_load_skill: bool) -> Vec<ToolDefinition> {
    builtin_tool_surface_with_policy(allow_load_skill).function_definitions
}

// create_plan / ask_question execute:
//   handler checks PlanRuntime and returns RejectedInMode { guidance } when
//   the current plan lifecycle does not allow the action.

// PLAN 模式 write/edit 路径白名单
pub fn validate_write_path(mode: PlanMode, path: &Path) -> Result<()> {
    match mode {
        PlanMode::Planning => {
            if !is_plan_file(path) {
                return Err(ToolError::usage(
                    "PLAN 模式下只能写 ~/.tomcat/plans/*.plan.md；其他路径请先 /plan exit 回 CHAT"
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// frontmatter 拦截（详见 create-plan.md §8）
pub fn validate_frontmatter_diff(old: &str, new: &str) -> Result<()> {
    // 解析新旧 frontmatter，做语义 diff
    // 非空 diff → tool error，引导用 update_plan / /plan 命令
}
```

**说人话**：所有 mode 看同一份工具目录，既不会让缓存前缀抖动，也不会暗示工具“不存在”。不靠模型守纪律：真正执行时仍有 handler policy、路径白名单和 frontmatter 拦截。

### 6.3 双保险

即使模型在不允许的生命周期调用某工具，`tool_exec` 和具体 handler 都会用 `current_mode()` / 活跃计划二次校验，并返回可行动的 tool error。

**说人话**：目录保持不变，真调工具时拦一次；写盘前 path 校验 + frontmatter diff 再拦两遍。

---

## 7. UI 模式标识、`current_mode()` 与 CLI prompt

### 7.1 UI 状态行

| `current_mode()` | UI 状态行（TUI / CLI） | 说人话 |
|-------------------|------------------------|--------|
| `Chat` | `[CHAT]` | 普通聊天。 |
| `Planning` | `[PLAN]`（可附 `goal` 缩略 ≤30 字符） | 在规划。 |
| `Executing` | `[EXEC plan_id=<id_短8位> · M:N todos]` | 执行中，显示完成进度。 |
| `Completed` | `[DONE plan_id=<id_短8位>]` | 整盘做完。 |
| `Pending` | `[PENDING plan_id=<id_短8位>]`（提示 `/plan build` 续跑） | 暂停态。 |

> UI 状态行**只读** `current_mode()`，**不**自己缓存 mode；多 session 切换聊天窗口时直接拉新 ChatContext 的状态行。

**说人话**：状态行一眼能看出当前模式 + 关键 plan_id；不维护自己的 mode 副本。

### 7.2 `current_mode()` API

```rust
impl PlanRuntime {
    /// 单一事实源；UI / catalog / tool_exec / prompt helper / reminder 注入 全部通过此函数读取。
    pub fn current_mode(&self) -> PlanMode { self.state.read().mode }
}
```

- 该函数 O(1)，可在每轮上下文构造前频繁调用。
- 测试中可通过 `PlanRuntime::__test_set_mode(...)` 覆写（仅 `cfg(test)`）。

### 7.3 CLI prompt（统一 helper 渲染）

所有对外可见 prompt 都统一走 `src/api/chat/prompt.rs`，而不是在 `run_loop`、命令分支或测试里手写字符串：

| mode | user prompt | agent prompt |
|------|-------------|--------------|
| `Chat` | `u[Chat]>` | `agent.<id>>` |
| `Planning` | `u[Plan:planning]>` | `agent.<id>[Plan:planning]>` |
| `Executing` | `u[Plan:executing]>` | `agent.<id>[Plan:executing]>` |
| `Pending` | `u[Plan:pending]>` | `agent.<id>[Plan:pending]>` |
| `Completed` | `u[Chat]>` | `agent.<id>>` |

**约束**：
- prompt 仅是 CLI / transcript 渲染层的显示文案，不再通过额外 user-message 标签来传达模式。
- transcript 中持久化的原始 user message 保持不变。
- `/plan build` 自动开跑时也复用同一 helper，因此用户会直接看到 `u[Plan:executing]> start building <path>`。

**说人话**：模式文案现在一处生成，用户看到的 prompt 和测试断言都从同一个 helper 出来。

### 7.4 EXEC 首轮上下文

`/plan build` 进入 EXEC 后，runtime 不再额外注入 `<plan_meta>` 块。执行阶段上下文由这几部分共同维持：

- EXEC 对应的 `<system_reminder>`
- 当前 active plan 的磁盘文件与 tool result
- `update_plan` / `todos` 返回的完整快照
- 统一的可见 prompt（`u[Plan:executing]>`、`agent.<id>[Plan:executing]>`）

**说人话**：不再额外塞一段 plan_meta，大部分执行信息都通过计划文件本身、工具结果和可见 prompt 来维持。

```rust
impl PlanRuntime {
    pub fn mode(&self) -> PlanState {
        self.state.read().clone()
    }
}
```

**说人话**：进 EXEC 之后不再额外补一段 plan_meta，执行态靠 plan 文件、工具快照和统一 prompt 共同维持上下文。

---

## 8. 调度时序

### 8.1 `/plan` 进入 PLAN

```text
用户 ──/plan──▶ 本地命令解析
                                              │
                                              ▼
                                  PlanRuntime::enter_plan_mode()
                                              │
                          ┌───────────────────┼───────────────────┐
                          ▼                   ▼                   ▼
                  state.mode = Planning  state.goal = "..."   transcript: plan.enter
                                              │
                                              ▼
                                stable ToolSurface → 同一份 tools 数组
                                              │
                                              ▼
                              下一轮装配：
                                ① request-local ephemeral tail 加 PLANNER_SYSTEM_REMINDER
                                ② prompt 切到 u[Plan:planning]>
                                ③ handler 启用 PLAN 写盘路径白名单 ~/.tomcat/plans/*.plan.md
                                              │
                                              ▼
                                LLM 在 PLAN 模式下推进；调用 read/grep/find/bash(only)/dispatch_agent
                                ask_question / create_plan / raw write/edit 正文
```

### 8.2 `/plan exit` 退回 CHAT

```text
用户 ──/plan exit──▶ exit_plan_mode()
                          │
                  gate: mode == Planning?
                          │ no → 友好提示
                          │ yes
                          ▼
                ┌─────────┼──────────┐
                ▼         ▼          ▼
       mode = Chat  reminder 移除  prompt 复位
                          │
                          ▼
                  catalog 复位 CHAT 集
                          │
                          ▼
                  transcript: plan.exit
```

### 8.3 `/plan build <plan_id/path>` 进入 EXEC（含 pending 续跑）

```text
用户 ──/plan build <id|path>──▶ build_plan()
                                     │
                       gate: 当前 session 不得处于 Executing；Pending 默认续跑当前盘但允许显式切别盘；scratchpad todos 仅 warning?
                                     │ no → 拒绝
                                     │ yes
                                     ▼
                       resolve plan_id_or_path → PlanFile
                                     │
                       gate: PlanFile.state ∈ {planning, pending}?
                                     │ no → 拒绝
                                     │ yes
                                     ▼
                                runtime 5 件事:
                                  ① write frontmatter.session_key/session_id
                                     （pending 续跑覆盖旧值，warning）
                                  ② write frontmatter.state = executing
                                  ③ swap reminder (PLANNER if any → EXECUTOR)
                                  ④ prompt → u[Plan:executing]> / agent.<id>[Plan:executing]>
                                  ⑤ catalog swap (CHAT/PLAN → EXEC)
                                     │
                                     ▼
                              optional record(Manual{plan_build:plan_id})
                                     │
                                     ▼
                              transcript: plan.build { plan_id, session_key, session_id }
                                     │
                                     ▼
                              下一轮 LLM 装配：reminder + executing prompt + catalog 一气切上
```

### 8.4 自动完成 / 自动 pending（runtime 内部）

```text
EXEC 中：
  todos.apply_op(...) 成功 ──▶ all todos completed ?
                                    │ no → 继续 EXEC
                                    │ yes
                                    ▼
                              PlanRuntime.on_all_todos_completed():
                                ① write frontmatter.state = completed
                                ② swap reminder (EXECUTOR → 无)
                                ③ prompt → CHAT
                                ④ catalog swap (EXEC → CHAT)
                                ⑤ transcript: plan.complete

EXEC 中：
  cancel_token / SIGTERM / parent abort ──▶ PlanRuntime.on_cancel_token():
                                              ① write frontmatter.state = pending
                                              ② swap reminder (EXECUTOR → 无)
                                              ③ prompt → CHAT
                                              ④ catalog swap (EXEC → CHAT)
                                              ⑤ transcript: plan.pending
```

**说人话**：`/plan` 命令负责进 PLAN/进 EXEC/退 PLAN 这三件事；完成与暂停由 runtime 内部 hook 自动派生，不靠 close 命令。

---

## 9. 状态机

```
            ┌────────┐
            │  Chat  │◀─────────────────────────────────┐
            └───┬────┘                                  │
                │ /plan                         │
                ▼                                       │
            ┌──────────┐                                │
            │ Planning │── /plan exit ──────────────────┤
            └────┬─────┘                                │
                 │ /plan build <plan_id/path>           │
                 ▼                                      │
            ┌──────────────┐                             │
            │  Executing   │── all todos completed ────▶│
            └──────┬───────┘                             │
                   │ cancel_token / SIGTERM / parent abort│
                   ▼                                     │
            ┌──────────┐  /plan build <plan_id/path>          │
            │ Pending  │────────────────────────▶ Executing
            └──────────┘
                ▲
                │ (cancel during EXEC)
                │
          ┌─────┴──────┐  (no slash to leave)
          │ Completed  │（只读浏览；开新 plan 走 /plan）
          └────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `Chat` | `/plan` | `Planning` | 下一请求尾部注入 PLANNER reminder、prompt 切 `u[Plan:planning]>` / `agent.<id>[Plan:planning]>`；catalog 保持稳定，写盘路径白名单照常执行 | 进入 PLAN 模式。 |
| `Planning` | LLM 调 `create_plan(...)` | `Planning` | tool 内 advisory lock + 写 `PlanFile` + 内部派 reviewer；mode 不变 | 模型写计划。 |
| `Planning` | reviewer 返回 summary | `Planning` | 摘要落 transcript `plan.review`、内存 `last_review_summary` 更新；**不**改 mode、**不**改 frontmatter | 审稿员只挑刺。 |
| `Planning` | `/plan exit` | `Chat` | reminder/catalog/prefix 复位 CHAT；保留 PlanFile 不动；不写事件 | 中途取消规划。 |
| `Planning` | `/plan build <plan_id/path>`（指向当前 session 创建的 plan） | `Executing` | runtime 5 件事；可选 `record(Manual{plan_build:plan_id})` | 现在才算正式开干。 |
| `Chat` | `/plan build <plan_id/path>`（续跑 pending） | `Executing` | 同上 + warning「旧 session 已覆盖」 | 续跑被打断的 plan。 |
| `Executing` | `todos` 更新但未完结 | `Executing` | 更新 frontmatter `todos[]` + panel；返回 full items snapshot | 干活中。 |
| `Executing` | 所有 todo `= completed` | `Completed -> Chat` | 自动写 frontmatter `state=completed`；经瞬时 `Completed` 立即回 `Chat(retain)`；事件仍记为 `plan.update` | 做完了。 |
| `Executing` | cancel_token / SIGTERM / parent abort | `Pending` | 写 frontmatter `state=pending`；reminder/catalog/prompt 复位 CHAT；不追加独立 `plan.pending` 事件 | 被打断转 pending。 |
| `Completed` | 用户开新 plan（`/plan`） | `Planning` | 与 `Chat → Planning` 同 | 开下一盘。 |
| `Completed` | `/plan build <other_plan_id/path>` | `Executing` | 允许显式切到另一份 `planning/pending` plan；不改变无参默认目标 | 从刚完成的一盘直接切到另一盘。 |
| `Pending` | `/plan build`（无参或显式指向当前 pending plan） | `Executing` | 续跑流程 | 续跑。 |
| `Pending` | `/plan build <other_plan_id/path>` | `Executing` | 允许显式切到另一份 `planning/pending` plan；不改变无参默认目标 | 从被打断的一盘直接切到另一盘。 |

完整运行时编排见 [`plan-runtime.md`](../plan-runtime.md) §8。

**说人话**：5 档状态。退出 PLAN 只有 `/plan exit`，进 EXEC 只有 `/plan build`；完成/暂停由 runtime 自动派生，没有 close 命令。

---

## 10. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH` | 未设 | 测试用：从指定文件读取 `PLANNER_SYSTEM_REMINDER` 内容覆写默认常量 | 单测可换提示词文件。 |
| `TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH` | 未设 | 测试用：覆写 `EXECUTOR_SYSTEM_REMINDER` | 同上。 |
| `TOMCAT_PLAN_INDICATOR_DISABLED` | `0` | 测试或非交互场景下隐藏 UI 模式标识 | CI 里可关掉状态行。 |
| `TOMCAT_PLAN_INDICATOR_DISABLED` | `0` | 测试或非交互场景下隐藏 plan prompt/指示器 | 调试单条 prompt 时用。 |

---

## 11. 错误模型 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| `/plan` 时已存在 active 计划 / EXEC | 本地 UsageError，提示 `/plan exit` 或等待执行结束 | 一份 active 计划不能叠两份。 |
| `/plan exit` 时 `mode != Planning` | 本地友好提示「`/plan exit` 仅在 PLAN 模式可用；如需中止执行请等待 cancel_token 或终止进程」 | exit 不当 close 用。 |
| `/plan build` 当前 session 已在 `Executing` | 本地 UsageError | 不允许两份计划同时跑。 |
| `/plan build` 当前 session 仍有 scratchpad todos | warning 后继续 build | 提醒先收口个人 scratchpad，但不拦目标 plan。 |
| `/plan build` 目标 PlanFile `mode ∉ {planning, pending}` | 本地 UsageError | 已 executing / completed 不能再 build。 |
| `/plan build` 目标 PlanFile 找不到 / frontmatter 不可解析 | 本地 UsageError | 文件没问题再 build。 |
| LLM 在不允许的生命周期调用 `create_plan` / `ask_question` | catalog 仍可见；`tool_exec` / handler 返回带下一步指引的 `RejectedInMode` | 不靠隐藏工具，硬调也拦。 |
| LLM 在非 Chat/Executing 模式调用 `todos` | 同上 | Planning/Completed/Pending 不能改 todos。 |
| LLM 在 PLAN 模式 raw `write/edit` 写 `~/.tomcat/plans/*.plan.md` 外路径 | tool error，usage「PLAN 模式仅允许写计划文件正文；如需改其他文件请先 /plan exit」 | 路径白名单。 |
| LLM 在任意模式 raw 改 `~/.tomcat/plans/*.plan.md` frontmatter | tool error，usage「frontmatter 由 todos / `/plan` 命令更新」 | YAML 锁死。 |
| reminder 注入失败（极端 IO） | warning；mode 切换仍生效 | 提示词写失败也别卡死切模式。 |

---

## 12. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：本地解析 | `parse_plan_commands`（待新增） | PENDING | `/plan` 不丢给 LLM。 |
| 单元：运行态尾部 | `request_prefix_is_byte_identical_across_turns` | 已实现 | 权限、PLAN / executing 变化只改变 ephemeral tail。 |
| 单元：prompt helper | `prompt_helper_renders_plan_modes` | DONE | `u[Chat]>` / `u[Plan:*]>` 和对应 agent prompt 都由 helper 统一产出。 |
| 单元：current_mode 单一事实源 | `current_mode_is_single_source_of_truth`（待新增） | PENDING | 不允许多份模式状态。 |
| 单元：catalog 可见集 | `catalog_visible_set_by_current_mode`（待新增） | PENDING | 各模式可见集要锁住。 |
| 单元：PLAN 写盘路径白名单 | `plan_mode_write_path_whitelist`（待新增） | PENDING | 非 plan 文件路径 → tool error。 |
| 单元：frontmatter raw 改硬拒 | `plan_mode_raw_edit_body_allowed_frontmatter_rejected`（待新增） | PENDING | 正文放、YAML 拦。 |
| 单元：exit 仅 PLAN | `plan_exit_restores_chat_only_from_planning`（待新增） | PENDING | EXEC/其他状态 exit 拒。 |
| 单元：build gate | `plan_build_rejects_active_executing_plan`、`plan_build_warns_but_continues_with_active_session_todos`、`completed_session_can_build_another_explicit_plan` | DONE | 前置检查仍严格，但不再误伤 scratchpad todos，且 `Completed` 可显式切新盘。 |
| 单元：全 completed 派生 | `all_completed_promotes_completed`（待新增） | PENDING | 自动 completed。 |
| 单元：cancel_token 派生 | `cancel_token_demotes_pending`（待新增） | PENDING | 自动 pending。 |
| 单元：UI 指示器 | `ui_shows_correct_mode_indicator`（待新增） | PENDING | 状态行要变。 |
| 单元：自动 build prompt | `plan_build_emits_executing_prompt` | DONE | `/plan build` 自动开跑时会显示 executing prompt。 |
| 单元：transcript 不污染 | `transcript_unchanged_by_prompt_rendering` | DONE | 原始消息不因 prompt 渲染被改写。 |
| 集成：PLAN→EXEC 全链路 | `plan_enter_create_plan_review_build_into_executing`（待新增） | PENDING | 进 PLAN → create_plan → reviewer 摘要 → /plan build → EXEC。 |
| 集成：EXEC→Pending→续跑 | `exec_cancel_to_pending_then_build_resume`（待新增） | PENDING | Ctrl+C → pending → /plan build 续跑。 |

---

## 13. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 模型脑补「我已经退出 PLAN 模式」 | 中 | handler + path gate + prompt helper + ephemeral reminder 四重保险 | 光靠 prompt 不够，真执行时必须拦住。 |
| reminder 与全局 system 冲突 | 低 | reminder 作为 `<system_reminder>` 段贴在 ephemeral tail，不修改全局 system | 局部提醒，不动缓存前缀。 |
| 新增工具漏配策略 | 中 | 新增工具必须声明 handler policy；CI 覆盖稳定工具目录与拒绝路径 | 目录始终可见，允许性必须显式实现。 |
| UI 指示器与 runtime 状态不同步 | 低 | UI 仅查 `current_mode()`，不缓存 | 状态行只问 runtime。 |
| prompt 渲染污染 transcript | 中 | prompt 只作用于 CLI 显示；transcript 保留原始消息 | 不污染。 |
| 模型在 EXEC 后续轮缺少额外 plan_meta | 低 | 依赖 active plan 文件 + `update_plan`/`todos` 快照 + executing prompt 持续收口 | 不靠一次性 plan_meta。 |
| `/plan build` 时旧 session 仍在工作 | 中 | 前置检查 `当前 session` 不得处于 `Executing`；pending 续跑时覆盖旧 session 并 warning；用户若显式切到其他 plan，旧 pending plan 保持磁盘 `pending`；scratchpad todos 仅 advisory warning | gate 仍然严格，但不再误伤 session scratchpad。 |
| 测试中频繁手动构造 mode | 低 | 暴露 `cfg(test)` 的 `__test_set_mode` | 单测可直接设 mode。 |

---

## 14. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把 PLAN 模式做成 LLM tool（`EnterPlanMode` / `ExitPlanMode`）~~ | **否**：本地 slash + ephemeral reminder + handler policy + prompt helper。 | 用户控流程，不靠模型自切模式。 |
| ~~把 planner 做成 subagent~~ | **否**：上下文隔离回路过重；PLAN 阶段需要主 Agent 上下文。 | 规划要主会话上下文。 |
| ~~按 mode 动态裁减 catalog~~ | **否**：它会改变请求前缀且让模型误以为工具不存在。目录稳定；handler、路径 gate 和 frontmatter policy 才是安全边界。 | 不靠隐藏工具做授权。 |
| ~~把 `planner.md` 与 `create-plan.md` 合并成一篇~~ | **否**：mode 编排（本文）与 PlanFile 写入器（`create-plan.md`）作用域不同；分篇更易维护。 | 模式归模式，写文件归写文件。 |
| ~~PLAN 模式独占 transcript~~ | **否**：复用主 transcript + `plan.enter` / `plan.exit` 自定义事件标记边界。 | 主会话一条线。 |
| ~~`Idle` 模式名~~ | **替代**：改名为 `Chat`，更直观。 | 默认状态叫 CHAT。 |
| ~~`ReadyToApply` 中间态~~ | **下线**：reviewer 仅辅助、不做 gate；从 `Planning` 直接经 `/plan build` 跳 `Executing`。 | 状态机少一档。 |
| ~~`Cancelled` 状态~~ | **下线**：cancel_token / 进程退出统一记为 `Pending`，可被 `/plan build` 续跑；用户不要 → `/plan exit` 退 PLAN 文件留着。 | 留可续跑余地，不强收口。 |
| ~~`/plan apply` 进执行态~~ | **替代**：改名 `/plan build <plan_id/path>`，承载 5 件事。 | apply 字面不够，build 涵盖更多。 |
| ~~`/plan close [completed\|cancelled]`~~ | **下线**：完成由 runtime 自动派生；不要可以 `/plan exit`；cancel 由 cancel_token 自动 pending。 | 状态自然演化。 |
| ~~`/plan show` 命令~~ | **暂缓**：用户直接打开 `.plan.md` 看。 | 用文件代替命令。 |
| ~~独立 `/goal` 命令~~ | **暂缓**：目标在进入 PLAN 后通过自然对话收敛。 | 简化命令族。 |
| ~~PLAN 模式 catalog = 只读工具 + create_plan + ask_question~~ | **再迭代**：先一度改为「全工具集 + create_plan + ask_question − todos + 写盘路径白名单」；**最终 D 方案**改为「全工具集 + create_plan + ask_question + todos + update_plan + 写盘路径白名单」（`todos` 任何模式可见、`update_plan` 任何模式可见）。 | PLAN 拿到全套：调研 + 计划创建 + 增量改 todos。 |
| ~~`todos` 仅 CHAT/EXEC/Completed/Pending 可见~~ | **替代（D 方案）**：`todos` **任何模式都可见**；它永远只写 `TodoFile`（session 路径），不动 plan。改 plan 内 todos 走新增的 [`update_plan`](./update-plan.md)。 | todos = 个人 scratchpad；plan 内 todos 单独工具管。 |
| ~~mode=completed 自动派生由 `todos` 触发~~ | **替代（D 方案）**：由 [`update_plan`](./update-plan.md) 在 EXEC + target.state==executing + 全 completed 时触发。 | 改 plan 的工具负责派生 mode。 |
| ~~PLAN/CHAT 下用户要求改 plan 内 todos 必须 `create_plan` 整盘重写~~ | **修复（D 方案）**：增量改用 [`update_plan`](./update-plan.md)，任何模式可见；`create_plan` 仅当结构大改时用。 | 小修不必整盘重写。 |
| ~~frontmatter 三方协同（`create_plan` + `todos` + runtime）~~ | **替代为四方（D 方案）**：`create_plan`（整盘初稿）+ [`update_plan`](./update-plan.md)（增量）+ runtime（mode/session）+ 自动派生。 | 四方各管一段。 |
| ~~reminder 注入到 system 或 user message~~ | **否**：所有 `<system_reminder>` 都注入请求级 ephemeral tail。 | reminder 不改写历史。 |
| ~~只靠 reminder 让模型记住 mode~~ | **补充**：CLI 统一显示 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` / `u[Plan:completed]>` 及对应 agent prompt。 | 当前模式对用户和测试都可见。 |
| ~~reviewer verdict 二态做 gate~~ | **否**：reviewer 仅辅助；进 EXEC 由 `/plan build` 拍板。 | 审稿员只挑刺。 |

---

## 15. 关联文档

- 运行时编排：[plan-runtime.md](../plan-runtime.md)（PlanRuntime / TodosRuntime OOD、状态机、5 件事流程）
- 写计划（整盘）：[create-plan.md](./create-plan.md)
- 写计划（增量）：[update-plan.md](./update-plan.md)
- 结构化提问：[ask-question.md](./ask-question.md)
- 会话级待办：[todos.md](./todos.md)
- 审稿子 Agent 契约：[reviewer.md](./reviewer.md)
- 子 Agent 基础设施：[multi-agent.md](../multi-agent.md)
- 标杆写法：[read.md](./read.md)
- 任务卡：[T2-P1-002.md](../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)
- PLAN 模式行为契约参考：[plan-mode-execution-playbook-T2-P0-001.md](../../reports/plan-mode-execution-playbook-T2-P0-001.md)

**说人话**：想深挖写计划、审稿、todos，从上面链到对应工具 spec；模式切换以本文 + `plan-runtime.md` 为准。
