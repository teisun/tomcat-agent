# `planner`：PLAN 模式与 EXEC 模式整体规范（非 LLM 工具）

> **重要**：`planner` **不是** LLM 工具，**也不是** subagent；它是**会话模式（PLAN Mode）的整体规范**，本文同时收口 EXEC 模式（Executing）。`/plan` 命令族、模式激活与退出、系统提示词注入、catalog 动态过滤白名单、UI 模式标识、user message 模式前缀、`current_mode()` 查询函数都在本文。LLM 在 PLAN 模式中实际能调用的写动作工具是 [`create_plan`](./create-plan.md)（整盘）+ [`update_plan`](./update-plan.md)（增量）+ [`ask_question`](./ask-question.md)；EXEC 模式的写动作工具是 [`update_plan`](./update-plan.md)（主力）+ [`todos`](./todos.md)（session-local scratchpad）。

本文档是 **B 类**：`docs/architecture/tools/`，承接 [`plan-runtime.md`](../plan-runtime.md) 的运行时编排，与 [`create-plan.md`](./create-plan.md) / [`ask-question.md`](./ask-question.md) / [`todos.md`](./todos.md) / [`reviewer.md`](./reviewer.md) 协同。**实现以仓库代码为准**。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：PLAN 模式是会话开关，EXEC 模式是 PLAN 结束后用户拍板开干的状态——`/plan "<objective>"` 进 PLAN、`/plan exit` 退出回 CHAT、`/plan build <plan_id|path>` 进 EXEC；完成由 runtime 自动派生（全 todos completed），中断由 cancel_token 自动转 pending。进 PLAN/EXEC 模式后，runtime 给 LLM **在 system 区段尾部**注一段 reminder、把 catalog 切到模式集、给每条 user message 加 `[mode: PLAN]` / `[mode: EXEC plan_id=…]` 前缀。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. mode 激活 / 退出与命令族](#5-mode-激活--退出与命令族)
- [6. 系统提示词与 catalog 动态过滤](#6-系统提示词与-catalog-动态过滤)
- [7. UI 模式标识、`current_mode()` 与 user message prefix](#7-ui-模式标识current_mode-与-user-message-prefix)
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
| **PLAN 模式（Planner Mode）** | 会话被切到「规划」语义下 | `PlanRuntime.mode == Planning` | 由本地命令 `/plan "<objective>"` 进入；非 LLM 工具；非 subagent | 一种会话模式。 |
| **EXEC 模式（Executing）** | 推进 `PlanFile` 待办的执行态 | `PlanRuntime.mode == Executing` | 由本地命令 `/plan build <plan_id\|path>` 进入；catalog 切 EXEC 集；首轮注入 plan 全文 | 真正开干的模式。 |
| **CHAT 模式** | 默认普通聊天 | `PlanRuntime.mode == Chat` | catalog = 全工具集 + `todos` − `create_plan` | 不在规划也不在执行的日常。 |
| **`/plan` 命令族** | 控制 PLAN/EXEC 模式与计划闭环的本地 slash | `src/api/chat/commands/cmd_plan.rs`（拟定） | 解析在本地完成，不丢给 LLM；`/plan "<obj>"` / `/plan exit` / `/plan build <plan_id\|path>` 三条 | 用户控会话流的把手。 |
| **PLANNER_SYSTEM_REMINDER** | 进入 PLAN 模式时注入到 transcript **system 区段尾部**的 `<system_reminder kind="planner">` | 进程内常量 + 装配阶段注入 | 仅在 `Planning` 期间存在；切换后下一轮装配不再注入 | 规划模式提示词。 |
| **EXECUTOR_SYSTEM_REMINDER** | 进入 EXEC 模式时注入到 **system 区段尾部**的 `<system_reminder kind="executor">` | 进程内常量 + 装配阶段注入 | 仅在 `Executing` 期间存在；切换后下一轮装配不再注入 | 执行模式提示词。 |
| **user message mode prefix** | PLAN/EXEC 模式下给每条 user message 前缀加 `[mode: PLAN]` / `[mode: EXEC plan_id=…]` | runtime 在 LLM 请求装配阶段贴；详见 §7.3 | CHAT 不贴；不污染 transcript JSONL | 每条消息都贴模式 tag。 |
| **EXEC 首轮 user meta** | `/plan build` 后第一轮注入 `<user_meta kind="plan_body">` 含 PlanFile 全文 | runtime 装配；只注入一次 | pending 续跑也算一次首轮 | 进 EXEC 先把全貌喂给模型。 |
| **catalog 动态过滤** | runtime 在每轮上下文构造前根据 `current_mode()` 决定 LLM 可见工具集 | `src/api/chat/plan_runtime/catalog.rs`（拟定） | CHAT/EXEC：全工具集 + `todos` − `create_plan` − `ask_question`；PLAN：全工具集 + `create_plan` + `ask_question` − `todos` + 写盘路径白名单 | 模式决定可见集。 |
| **写盘路径白名单（PLAN 模式专用）** | PLAN 期 `write/edit` 只能写 `~/.tomcat/plans/*.plan.md` | `tool_exec` 在 PLAN 模式校验 path | 其它路径硬拒；其它模式无此白名单（仅 frontmatter 拦截） | 规划阶段只许动计划文件。 |
| **`current_mode()`** | 查询当前会话 PLAN 状态的 Rust API | `PlanRuntime::current_mode(&self) -> PlanMode` | UI / catalog / tool_exec / user prefix / reminder 注入都查这个 | 模式的事实源就是 runtime。 |
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

**说人话**：让模型自己进出 PLAN（cc-fork）会被滥用；用 subagent（hermes）回路太重；用「本地 slash + 系统提示词 + catalog 动态过滤」三件套是收益/复杂度最优。

### 2.2 常见实现横向对比

| 来源 | PLAN 形态 | 进入方式 | catalog 是否动态 | 退出方式 | 说人话 |
|------|-----------|----------|------------------|----------|--------|
| **cc-fork-01** | LLM tool | LLM 自调 `EnterPlanMode` | 否（约定） | LLM 调 `ExitPlanMode` 或用户中断 | 模型自治，约束弱。 |
| **codex** | 本地 slash + 提示词 | `/plan` slash | 是 | `/plan exit` / `/plan apply` | 收益/复杂度最优。 |
| **hermes-agent** | role 子 Agent | `delegate_task(role='planner')` | 子 Agent 上下文独立 | 子 Agent 完成 | 回路重。 |
| **Cursor 内置** | 模式选择器 | UI 切换 | 是（Plan / Agent / Ask 模式） | UI 切换 | UI 控制更直观。 |
| **本仓库 `planner`** | **本地 slash + 系统提示词 + catalog 动态过滤 + user message prefix** | `/plan "<objective>"` | 是 | `/plan exit` 回 CHAT；`/plan build` 进 EXEC；完成 / cancel 自动派生 | 三件套 + prefix 注入。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| P1 形态 | tool / slash / subagent / role | 选 slash + 提示词 + catalog。 |
| P2 进入方式 | 用户 vs LLM 决定 | 用户控。 |
| P3 catalog 是否动态 | 静态白名单 vs 按 mode 过滤 | 必须动态。 |
| P4 退出方式 | 是否区分 exit / build / 完成 / pending | 多出口对应「不要了 / 进执行 / 自动完成 / 被打断」。 |
| P5 系统提示词 | 全局 system vs 局部 system_reminder | 局部 reminder，**注入 system 区段尾部**。 |
| P6 UI 标识 | 是否显示当前模式 | 显示。 |
| P7 user message 标签 | 仅靠 system 提示 vs 每条贴前缀 | 每条贴 `[mode: ...]`，加强提示。 |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§12） | 说人话 |
|----|------|------------------|--------|
| G1 | PLAN / EXEC 模式由本地 slash 进入，不向 LLM 暴露「Enter/ExitPlanMode」工具 | `plan_enter_is_local_only`、`plan_build_is_local_only` | 只认 slash，不认 LLM tool。 |
| G2 | 进入即注入对应 `<system_reminder>` 到 **system 区段尾部**，且 catalog 切到对应集合 | `plan_enter_injects_planner_reminder_into_system`、`exec_enter_injects_executor_reminder` | 进模式就注提示词、收紧工具。 |
| G3 | `current_mode()` 是单一事实源，UI / catalog / tool_exec / user prefix / reminder 一致引用 | `current_mode_is_single_source_of_truth` | 模式只问 runtime 一处。 |
| G4 | `/plan exit` **仅 PLAN 模式可用**，立即解除 PLAN 模式回 CHAT，下一轮装配不再含 PLANNER reminder/prefix | `plan_exit_restores_chat_only_from_planning` | exit 不当 close 用。 |
| G5 | `/plan build` 是 EXEC 唯一入口；前置 `当前 session 无 active plan && 无 active todos`；目标 PlanFile `mode ∈ {planning, pending}` | `plan_build_gate_checks_no_active_and_plan_mode` | 用户拍板，工具不替。 |
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
| 把 reminder 注入 user message | 所有 `<system_reminder>` 都注入 **system 区段尾部** | reminder 归 system。 |
| 把 `[mode: ...]` 前缀写进 transcript | 仅装配阶段贴，原始消息不动 | 不污染聊天记录。 |
| PLAN 模式独占 transcript | 仍写主 session，仅 `plan.enter` / `plan.exit` 标边界 | 不另开一份会话文件。 |
| PLAN 模式复制一整份 tool catalog | 同一份 catalog，按 mode 过滤可见集 | 不维护两套工具表。 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表（摘要）

| 维度 | 关切 | 说人话 |
|------|------|--------|
| P1 形态 | slash + reminder + catalog + user prefix，非 LLM tool / 非 subagent | 会话开关四件套。 |
| P2 进入 | `/plan "<objective>"` / `/plan build <plan_id\|path>` 本地命令 | 用户控进入。 |
| P3 catalog | `current_mode()` 驱动白名单 | 按 mode 真裁工具。 |
| P4 退出 | `/plan exit`（仅 PLAN）、`/plan build`（PLAN→EXEC）、自动完成 / 自动 pending | 不要 close 命令。 |

完整 R 维度矩阵见 [`plan-runtime.md`](../plan-runtime.md) §4.1。

### 4.2 实施点（拟定）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PM-A** | `/plan` 本地命令族 + `cmd_plan.rs`；`parse.rs` 识别；**交付**：`ChatCommand::Plan` | `src/api/chat/commands/{parse.rs,cmd_plan.rs}`、`cmd_help.rs` | 见 §12：`parse_plan_commands`、`plan_enter_rejects_while_active` | slash 不进 LLM，本地先吃掉。 |
| **PM-B** | `PlanRuntime` API + per-session 实例（详见 [`plan-runtime.md`](../plan-runtime.md) §6）；`enter/exit/build` API；**交付**：`PlanMode` / `PlanRuntime` | `src/api/chat/plan_runtime/{mod.rs,mode.rs}` | 见 §12：`current_mode_is_single_source_of_truth`、`plan_build_gate_checks_no_active_and_plan_mode` | 模式状态一处存。 |
| **PM-C** | `PLANNER_SYSTEM_REMINDER` / `EXECUTOR_SYSTEM_REMINDER` 注入 system 区段尾部 + 退出时移除；`plan.enter` / `plan.exit` / `plan.build` / `plan.complete` / `plan.pending` 事件；**交付**：常量 + transcript | `src/api/chat/plan_runtime/catalog.rs`（拟定）、`src/infra/transcript/...` | 见 §12：`plan_enter_injects_planner_reminder_into_system`、`exec_enter_injects_executor_reminder` | reminder 上线/下线挂在 mode。 |
| **PM-D** | `visible_tools_for_mode` + `tool_exec` 二次校验；PLAN 模式 `write/edit` 路径白名单；frontmatter raw 改硬拦截（详见 [`create-plan.md`](./create-plan.md) §8）；**交付**：白名单常量 | `src/api/chat/plan_runtime/catalog.rs`、`src/core/agent_loop/tool_exec.rs` | 见 §12：`catalog_visible_set_by_current_mode`、`plan_mode_raw_edit_body_allowed_frontmatter_rejected` | catalog 裁一遍，调工具再拦一遍，写盘再拦一遍。 |
| **PM-E** | UI 模式指示器 + `current_mode()` 公开；user message prefix 装配；EXEC 首轮 plan body user meta 注入；**交付**：状态行文案 + prefix 注入 | UI 层 + `plan_runtime/{mod.rs,session_prefix.rs}` | 见 §12：`ui_shows_correct_mode_indicator`、`user_message_prefix_only_in_assembly`、`exec_first_turn_injects_plan_body_user_meta` | 状态行 / prefix / meta 注入合一。 |

下文按实施点展开**技术要点与示意图**；**命令语义细节见 [§5](#5-mode-激活--退出与命令族)**。

#### 4.2.1 PM-A：slash 命令面

- **交付**：`parse_command` 匹配 `/plan` 子命令（`"<obj>"` / `exit` / `build <plan_id|path>`）；`dispatch_chat_command` **优先**消费，不进入 LLM user 文本（对齐 `/ckpt`）。
- **重入**：`enter_plan_mode` 前检查 `mode ∈ {Planning, Executing}` 且 `active_plan_id != None` → `UsageError`；`build_plan` 前检查 `当前 session 无 active plan / active todos` → `UsageError`。

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

- **交付**：`enter_plan_mode` 后在下一轮上下文构造前插入 `PLANNER_SYSTEM_REMINDER`（`<system_reminder kind="planner">`）到 **system 区段尾部**；`build_plan` 后切换为 `EXECUTOR_SYSTEM_REMINDER`；`exit_plan_mode` / cancel_token / 完成派生后下轮不再注入。
- **事件**：`plan.enter` / `plan.exit` / `plan.build` / `plan.complete` / `plan.pending` 标记 mode 边界（主 transcript 不断开）。

**说人话**：进 PLAN/EXEC 各注一段 reminder 到 system 尾部，出了就自动拿掉；transcript 打事件记号方便回放。

#### 4.2.4 PM-D：catalog 动态过滤 + 写盘双保险

- **交付**：`visible_tools_for_mode`（见 [§6.2](#62-catalog-动态过滤实现)）；PLAN 模式 `write/edit` 路径必须在 `~/.tomcat/plans/*.plan.md` 否则 tool error；frontmatter raw 改硬拒（详见 [`create-plan.md`](./create-plan.md) §8）。
- **双保险**：`tool_exec` dispatch 前再查 `current_mode()`。

```text
  Planning    →  全工具集 + create_plan + ask_question − todos；write/edit 仅 ~/.tomcat/plans/*.plan.md
  Executing   →  全工具集 + todos − create_plan − ask_question；write/edit 任意路径（仅 plan 文件 frontmatter 拦截）
  Chat        →  同 Executing；无 plan body 注入
  Completed   →  同 Chat（只读浏览）
  Pending     →  同 Chat（等待 /plan build 续跑）
```

**说人话**：每轮拼上下文前按 mode 裁工具；模型幻觉工具名也会被 tool_exec 挡；PLAN 期写盘路径白名单 + frontmatter 拦截双管齐下。

#### 4.2.5 PM-E：UI、`current_mode()`、user message prefix、EXEC 首轮 user meta

- **交付**：`PlanRuntime::current_mode() -> PlanMode` 为唯一事实源；UI 状态行渲染规则见 §7.1；user message 装配阶段贴 `[mode: ...]` 前缀（详见 §7.3）；EXEC 首轮注入 `<user_meta kind="plan_body">` 携带 PlanFile 全文（详见 §7.4）。
- **测试**：`cfg(test)` 下 `__test_set_mode` 覆写。

**说人话**：界面、catalog、user prefix、reminder 全都只问 runtime 当前 mode，不各自缓存；进 EXEC 第一轮把整份 plan 喂给模型，之后 todos snapshot 就够。

---

## 5. mode 激活 / 退出与命令族

### 5.1 命令一览

| 命令 | 解析层 | 副作用 | 前置条件 | 说人话 |
|------|--------|--------|----------|--------|
| `/plan "<objective>"` | 本地 chat 命令解析（不入 LLM） | 写 `goal`、`mode = Planning`，注入 PLANNER reminder 到 system 区段尾部，catalog 切到 PLAN 集（含写盘路径白名单 `~/.tomcat/plans/*.plan.md`），user prefix 切 `[mode: PLAN]` | 当前 session 无 active 计划（`mode != Planning && mode != Executing`） | 进 PLAN 模式。 |
| `/plan exit` | 本地 | `mode = Chat`；保留 PlanFile 不动（不写盘、不改 frontmatter）；reminder/catalog/prefix 复位 CHAT；写 `plan.exit` 事件 | **`mode == Planning` 仅可用**；其他状态友好提示 | 不要这次规划了，回到 CHAT。 |
| `/plan build <plan_id\|path>` | 本地 | runtime 5 件事（详见 [`plan-runtime.md`](../plan-runtime.md) §5.1）：① 写 `frontmatter.session_key/session_id`；② `frontmatter.mode = executing`；③ swap reminder PLANNER→EXECUTOR；④ user prefix 切 `[mode: EXEC plan_id=…]`，首轮注入 user meta plan body；⑤ catalog 切 EXEC 集 | `当前 session 无 active plan && 无 active todos`；指定的 PlanFile `mode ∈ {planning, pending}` | 把审过 / 续跑的计划推到执行态。 |

> **历史命令下线**（详见 §14）：
> - `/plan apply` → `/plan build <plan_id\|path>`
> - `/plan close [completed\|cancelled]` → 移除；完成由 runtime 自动派生（全 todos completed），暂停由 cancel_token 自动 pending
> - `/plan show` → 暂缓；用户直接打开 `.plan.md`
> - `/goal` → 暂缓；目标输入合并到 `/plan "<obj>"`

### 5.2 模式可调用工具矩阵

| `current_mode()` | catalog 中的 LLM 可见集 | `write/edit` 路径约束 | 说人话 |
|-------------------|--------------------------|------------------------|--------|
| `Chat` | 全工具集（含 `dispatch_agent` / bash / read 等）+ `todos` + `update_plan` + `ask_question`；**不**含 `create_plan` | 任意路径（写 `~/.tomcat/plans/*.plan.md` 的 frontmatter 仍被 raw 拦截） | 普通 Agent：可创建会话待办、也能改任意 planning/pending 的 plan 内待办；可结构化提问。 |
| `Planning` | 全工具集 + `create_plan` + `ask_question` + `todos` + `update_plan`；保留 `read` / `grep` / `find` / `dispatch_agent` / `bash` 等 | `write/edit/hashline_edit/delete` **仅允许** `~/.tomcat/plans/*.plan.md`；frontmatter 仍被 raw 拦截 | 调研、写计划、问用户、用 `update_plan` 调 todos；写工具只能动 plans/。 |
| `Executing` | 全工具集 + `todos` + `update_plan`；**不**含 `create_plan` / `ask_question` | **拒绝任何对 `~/.tomcat/plans/*` 的写**（含正文与 frontmatter） | 推进 plan 用 `update_plan`（默认指向 active plan）；plan 文件全禁写。 |
| `Completed` | 同 `Chat` | 同 `Chat` | 计划结束，回到普通工具集。 |
| `Pending` | 同 `Chat` | 同 `Chat` | 等待 `/plan build` 续跑。 |

> **关键差异（D 方案 / 2026-05 收紧）**：
> 1. `todos` / `update_plan` / `ask_question` 在 CHAT/Planning/Pending/Completed 全可见；`create_plan` 仅 Planning；
> 2. PLAN 模式写工具硬性限制路径（`~/.tomcat/plans/*.plan.md`），离开此目录的任何写一律拒；
> 3. EXEC 模式 plan 文件全禁写（含正文），推进任务**只能**走 `update_plan`；
> 4. mode=completed 自动派生由 `update_plan` 在 EXEC 触发，与 `todos` 无关。

**说人话**：模式一变，模型能看到的工具名单 + 能写的路径就变；不是靠 prompt 提醒，是 catalog + 路径白名单真过滤。

### 5.3 callable exit function（API 形态）

```rust
impl PlanRuntime {
    pub fn current_mode(&self) -> PlanMode { /* ... */ }

    pub async fn enter_plan_mode(&self, objective: &str) -> Result<()> { /* /plan "<obj>" */ }
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

## 6. 系统提示词与 catalog 动态过滤

### 6.1 reminder 常量（PLANNER + EXECUTOR）

#### 6.1.1 `PLANNER_SYSTEM_REMINDER`

> 设计参考：[`plan-mode-execution-playbook-T2-P0-001.md`](../../reports/plan-mode-execution-playbook-T2-P0-001.md) §「PLAN 模式行为契约」，按本期决策改造：① 每次只问 2-4 个关键问题；② mermaid 图改为 ASCII 图；③ reviewer 仅辅助、不做 gate。

```rust
pub const PLANNER_SYSTEM_REMINDER: &str = r#"
<system_reminder kind="planner">
You are now in PLAN mode. Behavior contract (12 rules; D-plan):

1.  Mode awareness: each subsequent user message will be tagged with the prefix
    `[mode: PLAN] `. Do NOT echo or strip this prefix — it is appended by the
    runtime to keep you grounded.

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
    `/plan build`), and auto-derivation (mode=completed on all-completed,
    mode=pending on cancel_token). Raw-editing YAML keys returns a tool error.

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
    `/plan build <plan_id|path>` (into EXEC). Do NOT attempt to leave via
    tool calls. Once the user issues `/plan build`, the runtime will swap the
    system reminder, prefix, and catalog automatically.
</system_reminder>
"#;
```

- **注入位置**：进入 PLAN 模式时，runtime 在下一轮上下文构造前把该 reminder 作为 `<system_reminder>` 段塞到模型可见的 **system 区段尾部**（**不**注入 user message）。
- **退出**：`/plan exit` / `/plan build` 后下一轮不再注入。
- transcript 同步写 `plan.enter` / `plan.exit` 自定义事件。

#### 6.1.2 `EXECUTOR_SYSTEM_REMINDER`

```rust
pub const EXECUTOR_SYSTEM_REMINDER: &str = r#"
<system_reminder kind="executor">
You are in EXEC mode. Your mission: drive the active plan to completion using ANY available tool. Whenever you make progress on a todo, mark it via update_plan; the runtime handles everything else.

1.  Mission first: each user message tag `[mode: EXEC plan_id=...]` points to the active plan. The runtime injected the full PlanFile body once on turn 1 as `<user_meta kind="plan_body">`. Read it, then advance using whatever tools you need (read / grep / bash / write / edit / search_files / dispatch_agent, etc.).

2.  Update via update_plan only: claim the next todo with `set_status(in_progress)` BEFORE running side-effecting tools; mark `completed` immediately when done; use `cancelled` for steps deliberately skipped. Never more than one `in_progress` in the same PlanFile. In EXEC mode `plan_id` defaults to the active plan, so you can omit it.

3.  Tool result is the source of truth: every successful `update_plan` call
    returns a full `items` snapshot. You do NOT need to re-read the PlanFile
    to know the current state — trust the snapshot.

4.  Plan file is off-limits to raw write/edit/delete (frontmatter AND body). The runtime rejects any direct write to `~/.tomcat/plans/*.plan.md` in EXEC. Use `update_plan` for progress. If the plan needs structural rewrite, ask the user to exit and re-plan; do NOT try to leave EXEC via tool calls.

5.  Completion is automatic: when ALL todos in the PlanFile flip to
    `completed`, the runtime promotes `mode = completed`, swaps the
    reminder/prefix/catalog back to CHAT, and you do NOT need to "close" the
    plan.
</system_reminder>
"#;
```

- **注入位置**：`/plan build` 完成后，runtime 在下一轮上下文构造前注入到 **system 区段尾部**；同一轮还会附 `<user_meta kind="plan_body">`（详见 §7.4）。
- **退出**：自动 `mode = completed` / `mode = pending` 后下轮不再注入。

**说人话**：进 PLAN 多一段 12 条契约提醒；进 EXEC 多一段 6 条精简契约提醒（主旨「推进任务 + 仅 update_plan 改进度 + plan 文件全禁写」）；出了模式 reminder 自动消失。

### 6.2 catalog 动态过滤实现

```rust
// catalog.rs（拟定）
//
// D 方案：todos 与 update_plan 在任何模式都可见；
// create_plan / ask_question 仅 PLAN 模式可见；
// PLAN 模式额外受 write/edit 路径白名单约束。
pub fn visible_tools_for_mode(
    mode: PlanMode,
    full_catalog: &ToolCatalog,
) -> Vec<ToolDefinition> {
    match mode {
        PlanMode::Chat | PlanMode::Completed | PlanMode::Pending => {
            // 全工具集 + todos + update_plan − create_plan − ask_question
            full_catalog
                .iter()
                .filter(|t| !PLAN_ONLY.contains(&t.name.as_str()))
                .cloned()
                .collect()
        }
        PlanMode::Planning => {
            // 全工具集 + create_plan + ask_question + todos + update_plan
            // 写盘路径白名单在 tool_exec::write/edit 中二次校验
            full_catalog.iter().cloned().collect()
        }
        PlanMode::Executing => {
            // 全工具集 + todos + update_plan − create_plan − ask_question
            full_catalog
                .iter()
                .filter(|t| !PLAN_ONLY.contains(&t.name.as_str()))
                .cloned()
                .collect()
        }
    }
}

/// PLAN-only tools: hidden in CHAT / EXEC / Completed / Pending.
pub const PLAN_ONLY: &[&str] = &["create_plan", "ask_question"];

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

**说人话**：CHAT/EXEC/Completed/Pending 看一份（全工具 + `todos` + `update_plan`，少 `create_plan` + `ask_question`）；PLAN 看全集（多 `create_plan` + `ask_question`，写盘路径白名单生效）。不靠模型守纪律，是 catalog + 路径白名单 + frontmatter 拦截三重保险。

### 6.3 双保险

即使模型 hallucinate 一个被过滤的工具名，`tool_exec` 在 dispatch 前也会再用 `current_mode()` 二次校验；不在白名单的 → tool error。

**说人话**：catalog 裁一遍，真调工具时再拦一遍；写盘前 path 校验 + frontmatter diff 再拦两遍。

---

## 7. UI 模式标识、`current_mode()` 与 user message prefix

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
    /// 单一事实源；UI / catalog / tool_exec / user prefix / reminder 注入 全部通过此函数读取。
    pub fn current_mode(&self) -> PlanMode { self.state.read().mode }
}
```

- 该函数 O(1)，可在每轮上下文构造前频繁调用。
- 测试中可通过 `PlanRuntime::__test_set_mode(...)` 覆写（仅 `cfg(test)`）。

### 7.3 user message mode prefix（装配阶段注入）

PLAN/EXEC 模式下，runtime 在 LLM 请求**装配阶段**给每条 user message 前缀加模式标签（详细规则同 [`plan-runtime.md`](../plan-runtime.md) §5.4.1）：

| mode | 前缀格式 | 例子 |
|------|---------|------|
| `Chat` | （无） | `请帮我看看这个 bug` |
| `Planning` | `[mode: PLAN] ` | `[mode: PLAN] 请帮我设计 retry 策略` |
| `Executing` | `[mode: EXEC plan_id=<plan_id>] ` | `[mode: EXEC plan_id=chat_plan_a1b2c3d4] 继续干第三步吧` |
| `Completed` / `Pending` | （无） | （回到 CHAT 行为） |

```rust
// session_prefix.rs（拟定）
impl PlanRuntime {
    pub fn user_message_prefix(&self) -> Option<String> {
        let s = self.state.read();
        match s.mode {
            PlanMode::Planning => Some("[mode: PLAN] ".into()),
            PlanMode::Executing => Some(format!(
                "[mode: EXEC plan_id={}] ",
                s.active_plan_id.as_deref().unwrap_or("")
            )),
            _ => None,
        }
    }
}
```

**注入边界**：
- 前缀**只在 runtime 装配 LLM 请求时贴**，**不**改写 transcript JSONL 中的原始 user message。
- transcript 仍以原始消息为准；resume 时不再加前缀（避免叠加）。
- 装配伪代码：
  ```rust
  let user_msg = match runtime.user_message_prefix() {
      Some(prefix) => format!("{}{}", prefix, raw_user_msg),
      None => raw_user_msg,
  };
  ```

**说人话**：每条 user message 自动贴模式 tag，让模型每轮都知道现在在哪个模式；transcript 不污染。

### 7.4 EXEC 首轮 user_meta 注入 plan body

`/plan build` 进入 EXEC 后**第一轮** LLM 请求装配时，runtime 在 system reminder 之后、user message 之前插入一条 **user meta message**：

```text
<user_meta kind="plan_body">
PlanFile path: ~/.tomcat/plans/<slug>_<hash>.plan.md
plan_id: <plan_id>

<完整 PlanFile 正文（frontmatter + body）>
</user_meta>
```

- 仅注入一次（runtime 维护 `exec_first_turn_injected` 标志，build 后第一轮装配即清空 → 设 true → 注入；后续轮次不再注入）。
- pending 续跑时**也**注入一次（视同 fresh EXEC）。
- transcript 中的 user 消息不修改；user_meta 仅装配阶段插入。

```rust
impl PlanRuntime {
    pub fn first_turn_user_meta(&self) -> Option<UserMeta> {
        if self.current_mode() == PlanMode::Executing
            && self.exec_first_turn.swap(false, Ordering::AcqRel)
        {
            let plan = self.read_active_plan_file().ok()?;
            Some(UserMeta {
                kind: "plan_body".into(),
                content: format!(
                    "PlanFile path: {}\nplan_id: {}\n\n{}",
                    plan.path.display(),
                    plan.plan_id,
                    plan.full_content(),
                ),
            })
        } else {
            None
        }
    }
}
```

**说人话**：进 EXEC 第一轮把整份 plan 塞给模型，让它一次性看清全貌；之后 todos 工具的 snapshot 就够了，不再重复注入。

---

## 8. 调度时序

### 8.1 `/plan "<objective>"` 进入 PLAN

```text
用户 ──/plan "改进 benchmark 覆盖"──▶ 本地命令解析
                                              │
                                              ▼
                                  PlanRuntime::enter_plan_mode("...")
                                              │
                          ┌───────────────────┼───────────────────┐
                          ▼                   ▼                   ▼
                  state.mode = Planning  state.goal = "..."   transcript: plan.enter
                                              │
                                              ▼
                                catalog.visible_tools_for_mode(Planning) → PLAN 集
                                              │
                                              ▼
                              下一轮装配：
                                ① system 区段尾部加 PLANNER_SYSTEM_REMINDER
                                ② user message 前缀加 [mode: PLAN]
                                ③ catalog 切 PLAN 集（写盘路径白名单 ~/.tomcat/plans/*.plan.md）
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
       mode = Chat  reminder 移除  user prefix 移除
                          │
                          ▼
                  catalog 复位 CHAT 集
                          │
                          ▼
                  transcript: plan.exit
```

### 8.3 `/plan build <plan_id|path>` 进入 EXEC（含 pending 续跑）

```text
用户 ──/plan build <id|path>──▶ build_plan()
                                     │
                       gate: 当前 session 无 active plan && 无 active todos?
                                     │ no → 拒绝
                                     │ yes
                                     ▼
                       resolve plan_id_or_path → PlanFile
                                     │
                       gate: PlanFile.mode ∈ {planning, pending}?
                                     │ no → 拒绝
                                     │ yes
                                     ▼
                                runtime 5 件事:
                                  ① write frontmatter.session_key/session_id
                                     （pending 续跑覆盖旧值，warning）
                                  ② write frontmatter.mode = executing
                                  ③ swap reminder (PLANNER if any → EXECUTOR)
                                  ④ user prefix → [mode: EXEC plan_id=...]
                                     + 首轮 user_meta plan body
                                  ⑤ catalog swap (CHAT/PLAN → EXEC)
                                     │
                                     ▼
                              optional record(Manual{plan_build:plan_id})
                                     │
                                     ▼
                              transcript: plan.build { plan_id, session_key, session_id }
                                     │
                                     ▼
                              下一轮 LLM 装配：reminder + user_meta + prefix 一气贴上
```

### 8.4 自动完成 / 自动 pending（runtime 内部）

```text
EXEC 中：
  todos.apply_op(...) 成功 ──▶ all todos completed ?
                                    │ no → 继续 EXEC
                                    │ yes
                                    ▼
                              PlanRuntime.on_all_todos_completed():
                                ① write frontmatter.mode = completed
                                ② swap reminder (EXECUTOR → 无)
                                ③ user prefix → 无
                                ④ catalog swap (EXEC → CHAT)
                                ⑤ transcript: plan.complete

EXEC 中：
  cancel_token / SIGTERM / parent abort ──▶ PlanRuntime.on_cancel_token():
                                              ① write frontmatter.mode = pending
                                              ② swap reminder (EXECUTOR → 无)
                                              ③ user prefix → 无
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
                │ /plan "<obj>"                         │
                ▼                                       │
            ┌──────────┐                                │
            │ Planning │── /plan exit ──────────────────┤
            └────┬─────┘                                │
                 │ /plan build <plan_id|path>           │
                 ▼                                      │
            ┌──────────────┐                             │
            │  Executing   │── all todos completed ────▶│
            └──────┬───────┘                             │
                   │ cancel_token / SIGTERM / parent abort│
                   ▼                                     │
            ┌──────────┐  /plan build <plan_id>          │
            │ Pending  │────────────────────────▶ Executing
            └──────────┘
                ▲
                │ (cancel during EXEC)
                │
          ┌─────┴──────┐  (no slash to leave)
          │ Completed  │（只读浏览；开新 plan 走 /plan "<obj>"）
          └────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `Chat` | `/plan "<objective>"` | `Planning` | 注入 PLANNER reminder（system 区段尾部）、user prefix `[mode: PLAN]`、catalog 切 PLAN 集（含写盘路径白名单）、写 `plan.enter` 事件 | 进入 PLAN 模式。 |
| `Planning` | LLM 调 `create_plan(...)` | `Planning` | tool 内 advisory lock + 写 `PlanFile` + 内部派 reviewer；mode 不变 | 模型写计划。 |
| `Planning` | reviewer 返回 summary | `Planning` | 摘要落 transcript `plan.review`、内存 `last_review_summary` 更新；**不**改 mode、**不**改 frontmatter | 审稿员只挑刺。 |
| `Planning` | `/plan exit` | `Chat` | reminder/catalog/prefix 复位 CHAT；保留 PlanFile 不动；写 `plan.exit` 事件 | 中途取消规划。 |
| `Planning` | `/plan build <plan_id\|path>`（指向当前 session 创建的 plan） | `Executing` | runtime 5 件事；可选 `record(Manual{plan_build:plan_id})` | 现在才算正式开干。 |
| `Chat` | `/plan build <plan_id\|path>`（续跑 pending） | `Executing` | 同上 + warning「旧 session 已覆盖」 | 续跑被打断的 plan。 |
| `Executing` | `todos` 更新但未完结 | `Executing` | 更新 frontmatter `todos[]` + panel；返回 full items snapshot | 干活中。 |
| `Executing` | 所有 todo `= completed` | `Completed` | 自动写 frontmatter `mode=completed`；reminder/catalog/prefix 复位 CHAT；写 `plan.complete` 事件 | 做完了。 |
| `Executing` | cancel_token / SIGTERM / parent abort | `Pending` | 写 frontmatter `mode=pending`；reminder/catalog/prefix 复位 CHAT；写 `plan.pending` 事件 | 被打断转 pending。 |
| `Completed` | 用户开新 plan（`/plan "<obj>"`） | `Planning` | 与 `Chat → Planning` 同 | 开下一盘。 |
| `Pending` | `/plan build <plan_id>` | `Executing` | 续跑流程 | 续跑。 |

完整运行时编排见 [`plan-runtime.md`](../plan-runtime.md) §8。

**说人话**：5 档状态。退出 PLAN 只有 `/plan exit`，进 EXEC 只有 `/plan build`；完成/暂停由 runtime 自动派生，没有 close 命令。

---

## 10. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH` | 未设 | 测试用：从指定文件读取 `PLANNER_SYSTEM_REMINDER` 内容覆写默认常量 | 单测可换提示词文件。 |
| `TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH` | 未设 | 测试用：覆写 `EXECUTOR_SYSTEM_REMINDER` | 同上。 |
| `TOMCAT_PLAN_INDICATOR_DISABLED` | `0` | 测试或非交互场景下隐藏 UI 模式标识 | CI 里可关掉状态行。 |
| `TOMCAT_USER_MESSAGE_PREFIX_DISABLED` | `0` | 测试用：关闭 `[mode: ...]` 前缀注入 | 调试单条 prompt 时用。 |

---

## 11. 错误模型 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| `/plan "<obj>"` 时已存在 active 计划 / EXEC | 本地 UsageError，提示 `/plan exit` 或等待执行结束 | 一份 active 计划不能叠两份。 |
| `/plan exit` 时 `mode != Planning` | 本地友好提示「`/plan exit` 仅在 PLAN 模式可用；如需中止执行请等待 cancel_token 或终止进程」 | exit 不当 close 用。 |
| `/plan build` 当前 session 有 active plan / active todos | 本地 UsageError | 不允许两份计划同时跑。 |
| `/plan build` 目标 PlanFile `mode ∉ {planning, pending}` | 本地 UsageError | 已 executing / completed 不能再 build。 |
| `/plan build` 目标 PlanFile 找不到 / frontmatter 不可解析 | 本地 UsageError | 文件没问题再 build。 |
| LLM 在非 Planning 模式调用 `create_plan` / `ask_question` | catalog 已不可见；`tool_exec` 兜底返回 tool error | 模式不对工具看不见，硬调也拦。 |
| LLM 在非 Chat/Executing 模式调用 `todos` | 同上 | Planning/Completed/Pending 不能改 todos。 |
| LLM 在 PLAN 模式 raw `write/edit` 写 `~/.tomcat/plans/*.plan.md` 外路径 | tool error，usage「PLAN 模式仅允许写计划文件正文；如需改其他文件请先 /plan exit」 | 路径白名单。 |
| LLM 在任意模式 raw 改 `~/.tomcat/plans/*.plan.md` frontmatter | tool error，usage「frontmatter 由 todos / `/plan` 命令更新」 | YAML 锁死。 |
| reminder 注入失败（极端 IO） | warning；mode 切换仍生效 | 提示词写失败也别卡死切模式。 |

---

## 12. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：本地解析 | `parse_plan_commands`（待新增） | PENDING | `/plan` 不丢给 LLM。 |
| 单元：进入 PLAN 注入 | `plan_enter_injects_planner_reminder_into_system`（待新增） | PENDING | reminder 进 system 区段尾部。 |
| 单元：进入 EXEC 注入 | `exec_enter_injects_executor_reminder`（待新增） | PENDING | EXEC reminder 进 system 区段尾部。 |
| 单元：EXEC 首轮 user meta | `exec_first_turn_injects_plan_body_user_meta`（待新增） | PENDING | 第一轮 user meta 带 plan 全文。 |
| 单元：current_mode 单一事实源 | `current_mode_is_single_source_of_truth`（待新增） | PENDING | 不允许多份模式状态。 |
| 单元：catalog 可见集 | `catalog_visible_set_by_current_mode`（待新增） | PENDING | 各模式可见集要锁住。 |
| 单元：PLAN 写盘路径白名单 | `plan_mode_write_path_whitelist`（待新增） | PENDING | 非 plan 文件路径 → tool error。 |
| 单元：frontmatter raw 改硬拒 | `plan_mode_raw_edit_body_allowed_frontmatter_rejected`（待新增） | PENDING | 正文放、YAML 拦。 |
| 单元：exit 仅 PLAN | `plan_exit_restores_chat_only_from_planning`（待新增） | PENDING | EXEC/其他状态 exit 拒。 |
| 单元：build gate | `plan_build_gate_checks_no_active_and_plan_mode`（待新增） | PENDING | 前置检查严格。 |
| 单元：全 completed 派生 | `all_completed_promotes_completed`（待新增） | PENDING | 自动 completed。 |
| 单元：cancel_token 派生 | `cancel_token_demotes_pending`（待新增） | PENDING | 自动 pending。 |
| 单元：UI 指示器 | `ui_shows_correct_mode_indicator`（待新增） | PENDING | 状态行要变。 |
| 单元：user prefix 装配 | `user_message_prefix_only_in_assembly`（待新增） | PENDING | 只装配阶段贴。 |
| 单元：transcript 不污染 | `transcript_unchanged_by_prefix`（待新增） | PENDING | 原始消息不动。 |
| 集成：PLAN→EXEC 全链路 | `plan_enter_create_plan_review_build_into_executing`（待新增） | PENDING | 进 PLAN → create_plan → reviewer 摘要 → /plan build → EXEC。 |
| 集成：EXEC→Pending→续跑 | `exec_cancel_to_pending_then_build_resume`（待新增） | PENDING | Ctrl+C → pending → /plan build 续跑。 |

---

## 13. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 模型脑补「我已经退出 PLAN 模式」 | 中 | catalog + `tool_exec` + user prefix + reminder 四重保险 | 光靠 prompt 不够，工具名单 + 模式 tag 都要给。 |
| reminder 与全局 system 冲突 | 低 | reminder 作为 `<system_reminder>` 段贴在 system 区段**尾部**，不修改全局 system；优先级清晰 | 局部提醒，不动全局 system。 |
| catalog 过滤遗漏新增工具 | 中 | 新增工具时必须在 `PLAN_GATED` 中显式登记；CI 用全工具列表与白名单做交叉校验 | 新工具上线要登记白名单。 |
| UI 指示器与 runtime 状态不同步 | 低 | UI 仅查 `current_mode()`，不缓存 | 状态行只问 runtime。 |
| user prefix 写入 transcript 污染聊天记录 | 中 | prefix **只**在装配阶段贴；transcript 保留原始消息；resume 时不再叠加 | 不污染。 |
| 模型在 EXEC 后续轮重复使用首轮 user_meta | 低 | `exec_first_turn_injected` 标志只置一次；后续轮次不注入；模型应转向 `todos` snapshot | 单次注入。 |
| `/plan build` 时旧 session 仍在工作 | 中 | 前置检查 `当前 session 无 active plan / active todos`；pending 续跑时覆盖旧 session 并 warning | gate 严格。 |
| 测试中频繁手动构造 mode | 低 | 暴露 `cfg(test)` 的 `__test_set_mode` | 单测可直接设 mode。 |

---

## 14. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把 PLAN 模式做成 LLM tool（`EnterPlanMode` / `ExitPlanMode`）~~ | **否**：本地 slash + system reminder + catalog 动态过滤 + user prefix。 | 用户控流程，不靠模型自切模式。 |
| ~~把 planner 做成 subagent~~ | **否**：上下文隔离回路过重；PLAN 阶段需要主 Agent 上下文。 | 规划要主会话上下文。 |
| ~~静态 catalog（不按 mode 过滤）~~ | **否**：靠模型守纪律不可靠；动态过滤是双保险的「外环」。 | 必须按 mode 真裁工具。 |
| ~~把 `planner.md` 与 `create-plan.md` 合并成一篇~~ | **否**：mode 编排（本文）与 PlanFile 写入器（`create-plan.md`）作用域不同；分篇更易维护。 | 模式归模式，写文件归写文件。 |
| ~~PLAN 模式独占 transcript~~ | **否**：复用主 transcript + `plan.enter` / `plan.exit` 自定义事件标记边界。 | 主会话一条线。 |
| ~~`Idle` 模式名~~ | **替代**：改名为 `Chat`，更直观。 | 默认状态叫 CHAT。 |
| ~~`ReadyToApply` 中间态~~ | **下线**：reviewer 仅辅助、不做 gate；从 `Planning` 直接经 `/plan build` 跳 `Executing`。 | 状态机少一档。 |
| ~~`Cancelled` 状态~~ | **下线**：cancel_token / 进程退出统一记为 `Pending`，可被 `/plan build` 续跑；用户不要 → `/plan exit` 退 PLAN 文件留着。 | 留可续跑余地，不强收口。 |
| ~~`/plan apply` 进执行态~~ | **替代**：改名 `/plan build <plan_id\|path>`，承载 5 件事。 | apply 字面不够，build 涵盖更多。 |
| ~~`/plan close [completed\|cancelled]`~~ | **下线**：完成由 runtime 自动派生；不要可以 `/plan exit`；cancel 由 cancel_token 自动 pending。 | 状态自然演化。 |
| ~~`/plan show` 命令~~ | **暂缓**：用户直接打开 `.plan.md` 看。 | 用文件代替命令。 |
| ~~独立 `/goal` 命令~~ | **暂缓**：目标输入合并到 `/plan "<obj>"` 入参。 | 简化命令族。 |
| ~~PLAN 模式 catalog = 只读工具 + create_plan + ask_question~~ | **再迭代**：先一度改为「全工具集 + create_plan + ask_question − todos + 写盘路径白名单」；**最终 D 方案**改为「全工具集 + create_plan + ask_question + todos + update_plan + 写盘路径白名单」（`todos` 任何模式可见、`update_plan` 任何模式可见）。 | PLAN 拿到全套：调研 + 计划创建 + 增量改 todos。 |
| ~~`todos` 仅 CHAT/EXEC/Completed/Pending 可见~~ | **替代（D 方案）**：`todos` **任何模式都可见**；它永远只写 `TodoFile`（session 路径），不动 plan。改 plan 内 todos 走新增的 [`update_plan`](./update-plan.md)。 | todos = 个人 scratchpad；plan 内 todos 单独工具管。 |
| ~~mode=completed 自动派生由 `todos` 触发~~ | **替代（D 方案）**：由 [`update_plan`](./update-plan.md) 在 EXEC + target.mode==executing + 全 completed 时触发。 | 改 plan 的工具负责派生 mode。 |
| ~~PLAN/CHAT 下用户要求改 plan 内 todos 必须 `create_plan` 整盘重写~~ | **修复（D 方案）**：增量改用 [`update_plan`](./update-plan.md)，任何模式可见；`create_plan` 仅当结构大改时用。 | 小修不必整盘重写。 |
| ~~frontmatter 三方协同（`create_plan` + `todos` + runtime）~~ | **替代为四方（D 方案）**：`create_plan`（整盘初稿）+ [`update_plan`](./update-plan.md)（增量）+ runtime（mode/session）+ 自动派生。 | 四方各管一段。 |
| ~~reminder 注入到 user message~~ | **否**：所有 `<system_reminder>` 都注入 **system 区段尾部**。 | reminder 归 system。 |
| ~~只靠 reminder 让模型记住 mode~~ | **补充**：PLAN/EXEC 模式 user message 装配阶段加 `[mode: ...]` 前缀；EXEC 首轮额外注入 `<user_meta kind="plan_body">`。 | 每条贴模式 tag。 |
| ~~reviewer verdict 二态做 gate~~ | **否**：reviewer 仅辅助；进 EXEC 由 `/plan build` 拍板。 | 审稿员只挑刺。 |

---

## 15. 关联文档

- 运行时编排：[plan-runtime.md](../plan-runtime.md)（PlanRuntime / TodoRuntime OOD、状态机、5 件事流程）
- 写计划（整盘）：[create-plan.md](./create-plan.md)
- 写计划（增量）：[update-plan.md](./update-plan.md)
- 结构化提问：[ask-question.md](./ask-question.md)
- 会话级待办：[todos.md](./todos.md)
- 审稿子 Agent 契约：[reviewer.md](./reviewer.md)
- 子 Agent 基础设施：[multi-agent.md](../multi-agent.md)
- 标杆写法：[read.md](./read.md)
- 任务卡：[T2-P1-002.md](../../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)
- PLAN 模式行为契约参考：[plan-mode-execution-playbook-T2-P0-001.md](../../reports/plan-mode-execution-playbook-T2-P0-001.md)

**说人话**：想深挖写计划、审稿、todos，从上面链到对应工具 spec；模式切换以本文 + `plan-runtime.md` 为准。
