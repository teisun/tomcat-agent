# PlanRuntime：`todos` 工具、PLAN 模式与 `/plan build` 的运行时编排

本文档是 **T2-P1-002 | plan-mode-enhance** 的总览级运行时编排方案，承接 [`checkpoint-resume.md`](./tools/checkpoint-resume.md) 提供的 `CheckpointStore`，并把 **PLAN 模式（Planner / AskQuestion / CreatePlan / UpdatePlan / Reviewer）** 与 **执行态（`update_plan` 推进 + `todos` scratchpad + TodosPanel）** 串成一个闭环。各 LLM 工具与 PLAN 模式内部细节见 `tools/` 子目录下的独立 spec：[`tools/planner.md`](./tools/planner.md)、[`tools/create-plan.md`](./tools/create-plan.md)、[`tools/update-plan.md`](./tools/update-plan.md)、[`tools/ask-question.md`](./tools/ask-question.md)、[`tools/todos.md`](./tools/todos.md)、[`tools/reviewer.md`](./tools/reviewer.md)。仓库当前实现已落地 `/plan`、`create_plan`、`update_plan`、`todos` 与 reviewer / verifier / transcript recover 主链；较早阶段的未实现描述以本文前置的 **v4-g 生效说明** 为准。

本文按 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 主路径编排。

## 2026-05 Active Binding v4-g 生效说明

以下规则已在仓库实现，优先级高于本文较早阶段的草稿描述：

- 运行时字段只有 `active_planning_plan_id` 与 `active_plan_path` 两个；`active_plan_id` 只是 helper，不是第三个持久字段。
- `PlanState` 是当前实现名；稳定态为 `Chat / Planning / Pending / Executing`，`Completed` 仅是 `update_plan` 收口中的瞬时态。
- **只有** `/plan build` 会写 active binding；binding 的有效定义是 `active_plan_path + PlanState 内嵌 plan_id`。`create_plan`、`update_plan`、`/plan exit`、recover 都不直接写 binding。
- transcript 自定义事件统一使用顶层 `event` 字段，计划相关只认 `plan.create` / `plan.build` / `plan.update`；payload 统一为 `{ plan_id, path, state }`。
- 所有计划事件都遵循“先盘、再释锁、后写事件”：先把 `.plan.md` 写成功，再追加 transcript 自定义事件。
- recover 不再扫 `~/.tomcat/plans/` 全目录，也不再做 orphan executing demote；启动时由 `init_context_state()` 单次反向扫描 transcript 尾部，提取 `latest_plan_event`，再交给 `PlanRuntime::attach_from_event(...)` 回盘派生。
- recover 只显式恢复 `pending / executing` 到 `PlanState`；若盘上是 `planning / completed`，则保持 `Chat`，但对 `build/update` 事件保留 `active_plan_path` 作为 retain。
- `/plan build` 现在允许省略参数，默认源顺序固定为：`active_planning_plan_id -> Pending { id } -> active_plan_path`。
- `/plan exit` 只做 state 切换：允许 `Planning / Pending -> Chat`；不写事件、不清字段。
- `update_plan` 已解除 completed 全拒：completed plan 可 reopen；reopen 时盘 `completed -> pending`，runtime 同步切 `Pending { id }`，但依旧**不**写 binding。
- all-completed 收口路径为：盘 `completed` -> 运行时瞬时 `Completed { id }` -> `finalize_completed_to_chat()` 立即回 `Chat(retain)`；`finalize_completed_to_chat()` 不再清 `active_plan_path` / `active_planning_plan_id`。
- CLI prompt 不把 `Completed` 当作稳定用户可见模式；retain 后看到的仍是 `u[Chat]>`。

### 字段责任（现行实现）

| 字段 / helper | 所在处 | 谁会写 | 当前职责 |
|------|------|------|------|
| `active_planning_plan_id` | `PlanRuntime` | `create_plan`、`attach_from_event(plan.create)` | 记住最近一份 planning 草稿，供 `/plan build` 缺省目标优先命中。 |
| `active_plan_path` | `PlanRuntime` | `/plan build`、`attach_from_event(plan.build/update)` | 记住当前绑定/retain 的真实 plan 路径，供 `update_plan` 缺省路由与 build retain 续接。 |
| `active_plan_id()` | `PlanState` helper | 无单独写入 | 从 `Pending / Executing / Completed` 内嵌的 `plan_id` 读值；它不是第三个 runtime 字段。 |

## 2026-05 Current-Tail Collapse Keepalive 补充

阶段二 `current-tail aggregate guard` 在 reasoning loop 的 mid-turn 路径里，如果局部减负仍不够，会把整份 working set 折成单条 `branch_summary`。这一步不会让 LLM 自己“顺手记住当前在做什么”，而是同步读取 `PlanRuntime` 生成一个固定格式的 `Execution Keepalive` 片段，直接拼进 collapse summary。

当前实现的 keepalive 数据来源固定如下：

- `PlanRuntime.mode()`：写入 `mode`。
- `PlanRuntime.active_plan_path()`：写入 `active_plan_path`。
- `PlanState::active_plan_id()`，若取不到则回退 `active_planning_plan_id()`：写入 `active_plan_id`。
- `Planning` 模式：读取 `snapshot_session_todos()`。
- `Executing / Pending` 模式：读取 `active_plan_path` 对应 plan file 的 `frontmatter.todos`。
- `ContextState.latest_plan_event`：写入 `latest_plan_event`。

```text
mid-turn collapse
  -> build_keepalive_snapshot(plan_runtime, latest_plan_event)
  -> "## Structured Summary" + "## Execution Keepalive"
  -> apply_boundary
  -> 下一次 LLM / 后续 reload 都只看这条 summary
```

约束口径：

- keepalive 是 **runtime 生成的结构化真相**，不是模型自由概括。
- collapse 后 transcript 里的 `branch_summary` 插入若失败，只记 `warn`；内存里的 collapse 结果仍继续生效。
- 对应回归测试见 `current_tail_guard_test.rs::collapse_to_branch_summary_keeps_planning_snapshot`、`current_tail_guard_runtime_test.rs::collapse_to_branch_summary_keeps_executing_snapshot`、`current_tail_guard_runtime_test.rs::collapse_to_branch_summary_keeps_pending_snapshot_when_no_in_progress_exists`。

### Transcript 事件 Schema（现行实现）

| 事件 | 写入方 | payload | 备注 |
|------|--------|---------|------|
| `plan.create` | `create_plan` | `{ plan_id, path, state: "planning" }` | 只表明草稿已落盘，不建立 binding。 |
| `plan.build` | `/plan build` | `{ plan_id, path, state }` | 唯一建立 active binding 的动作。 |
| `plan.update` | `update_plan` | `{ plan_id, path, state }` | 反映 plan 盘状态变更；`state` 仅是 fast cache，recover 仍以盘为准。 |

三类事件都遵循同一铁律：**先盘、再释锁、后事件**。

### Recover 流程（现行实现）

1. `init_context_state()` 读取 transcript 尾部，在 `MAX_PLAN_SCAN = 5000` 上限内反向扫描最近一条 `plan.create/build/update`。
2. 识别结果以 `latest_plan_event` 放进 `ContextState`，agent loop 启动时调用 `PlanRuntime::attach_from_event(...)`。
3. `plan.create` 只恢复 `active_planning_plan_id`；`plan.build` / `plan.update` 恢复 `active_plan_path`。
4. 回盘读取 `frontmatter.state` 后，仅 `pending / executing` 显式恢复 `PlanState`；`planning / completed` 统一回到 `Chat(retain)`；目标文件丢失则降为 `Chat(no retain)`。

### 状态图（现行实现）

```text
Chat --/plan--> Planning --/plan build--> Executing
Chat <--/plan exit-- Pending <--cancel/abort-- Executing
Chat <--finalize-- Completed <--all completed + verify/code-review-- Executing
Chat <--/plan exit-- Planning
Pending --/plan build--> Executing
Completed 仅为瞬时态，不作为 recover 稳态，也不作为稳定 prompt 标签。
```

下文若仍出现 `active_plan_id`、`PlanMode`、`plan.enter / plan.exit / plan.complete / plan.pending`、`u[Plan:completed]>` 等历史草稿词汇，均以上述“生效说明 / 字段责任 / event schema / recover 流程”为准。

**说人话**：这篇文档回答四件事。第一，**PLAN 模式不是 LLM 工具**——`/plan` 是本地 slash 命令切换会话状态，进入后系统注入提示词并把 catalog 切到「全工具集 + `create_plan` + `ask_question` + `todos` + `update_plan` + 写盘路径白名单」。第二，`/plan build <plan_id/path>` 是唯一进入执行态的入口；reviewer 仅辅助，不做 verdict gate。第三，**`todos` 和 [`update_plan`](./tools/update-plan.md) 任何模式都可见**——`todos` 写 `TodoFile`（session 本地 scratchpad），`update_plan` 写 `PlanFile.frontmatter.todos[]` / `milestones[]`（按 `plan_id` 或显式 `path` 路由；EXEC/Pending 缺省跟随当前 active plan path）；二者共享 `apply_todos_op` op 引擎，提示词分裂。**TodoRuntime / PlanRuntime 都是 per-session OOD 对象，挂在 `ChatContext` 上**。第四，reviewer 是 internal subagent dispatch（与 codex `codex_delegate.rs` 同构），由 `CreatePlan` 工具内部同步阻塞调用，**不**进 catalog。

---

## 1. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **GoalObjective** | 用户给 Agent 的高层目标 | PLAN 会话中的后续对话；计划文件 frontmatter `goal` | 更新 Goal 不会直接改 `TodoItem`；`/plan` 只负责进入 PLAN，真正落盘的目标来自后续讨论后传给 `create_plan` 的 `goal` | 先定方向，不等于马上开干。 |
| **TodoItem** | 最小可执行步骤 | 运行态 `TodoItem { id, content, status, milestone_id }`；存放载体由工具决定：`todos` → `TodoFile`；[`update_plan`](./tools/update-plan.md) → `PlanFile.frontmatter.todos[]` | `status ∈ {pending, in_progress, completed, cancelled}`；同一文件最多一个 `in_progress`；`id` 单文件唯一 | 真正能推进进度的是 todos。 |
| **Milestone** | 一组 `TodoItem` 的上层分组 | `PlanFile` frontmatter `milestones` | 一条 `TodoItem` 至多属于一个 `Milestone`；状态派生 | todos 管细节，里程碑管大段落。 |
| **PlanFile** | 持久化计划文件 | `~/.tomcat/plans/<slug>_<hash>.plan.md`；详尽 schema 见 [`tools/create-plan.md`](./tools/create-plan.md) §5 | frontmatter（机写）+ 正文（人读）；写入前 advisory file lock；frontmatter 由 `create_plan`（整盘）/ [`update_plan`](./tools/update-plan.md)（增量）/ runtime（mode、session 绑定）/ 自动派生（all completed / cancel_token）**四方协同** | 计划最终得落成一份文件。 |
| **TodoFile** | 任意模式下的会话级轻量待办文件 | `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`；详见 [`tools/todos.md`](./tools/todos.md) §3.4 | 同一 session 磁盘只保留当前 active；`new_todos` 时删旧；任何模式下 `todos` 工具都写它 | 会话本地小白板，旧的就删。 |
| **PLAN 模式（Planner Mode）** | 会话内的"规划模式" | `PlanRuntime.mode == Planning` | `/plan` 进入；catalog = 全工具集 + `create_plan` + `ask_question` + `todos` + `update_plan` + 写盘路径白名单（`~/.tomcat/plans/*.plan.md`）；详见 [`tools/planner.md`](./tools/planner.md) | PLAN 模式是会话开关，不是工具调用。 |
| **EXEC 模式（Executing）** | 推进 `PlanFile` 待办的执行态 | `PlanRuntime.mode == Executing` | `/plan build <plan_id/path>` 进入；catalog = 全工具集 + `todos` + `update_plan` − `create_plan`；first turn 注入 plan body；推进 plan 走 [`update_plan`](./tools/update-plan.md) | 真正开干。 |
| **CHAT 模式** | 默认普通聊天 | `PlanRuntime.mode == Chat` | catalog = 全工具集 + `todos` + `update_plan` − `create_plan`；`todos` 写 TodoFile；`update_plan` 可跨 session 编辑任意 planning/pending 的 PlanFile | 不在规划也不在执行的日常。 |
| **`update_plan` 工具** | 增量编辑 PlanFile frontmatter 的内置 LLM 工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "update_plan"`；详见 [`tools/update-plan.md`](./tools/update-plan.md) | 任何模式可见；只能动 `todos[]` / `milestones[]`；EXEC/Pending 缺省取当前 active plan path，其它模式传 `plan_id` 或 `path`；跨 session 修订有边界 | 推进 plan 待办用这个。 |
| **CreatePlan** | PLAN 模式下创建 / 重写 `PlanFile` 的内置 LLM 工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "create_plan"`；详尽 schema 见 [`tools/create-plan.md`](./tools/create-plan.md) | 仅 `mode == Planning` 时可见；工具名保留不改 | 计划文件主要创建入口。 |
| **AskQuestion** | PLAN 模式下结构化向用户提问的内置 LLM 工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "ask_question"`；schema 见 [`tools/ask-question.md`](./tools/ask-question.md) | 仅 `mode == Planning` 时可见；单次 2-4 题、每题 2-4 选项 | 让模型问问题，而不是自己脑补。 |
| **PlanState（运行态阶段）** | 当前会话在计划闭环里的阶段 | `Chat` / `Planning` / `Executing` / `Pending` + 瞬时 `Completed` | `/plan` 命令族只改变本地 chat 运行态；`PlanState` **不是** LLM 工具名 | 计划阶段是会话状态。 |
| **Reviewer** | 由 `CreatePlan` 工具内部派发的子 Agent | `internal subagent dispatch`；不进 catalog；详见 [`tools/reviewer.md`](./tools/reviewer.md) | 同步阻塞；输出 `summary` 自由文本；**不**做 verdict gate；改稿权由 runtime 内部参数 `[reviewer].default_allow_edit` 控制 | 审稿员是子 Agent，只挑刺，不当法官。 |
| **TodoRuntime** | 单 session 内 `TodoFile` 的内存映射与 IO 入口 | `ChatContext.todo_runtime: TodoRuntime`；详见 §6 | per-session 单实例；管 `active_todos_id` / 内存 items / 节流 panel 推送 / IO | 一个聊天会话一个 todo 大管家。 |
| **PlanRuntime** | 单 session 内 `PlanFile`、PLAN/EXEC mode、reviewer 派发、TodosPanel 投影、checkpoint hook 的编排层 | `ChatContext.plan_runtime: PlanRuntime`；详见 §6 | per-session 单实例；`mode` / `active_planning_plan_id` / `active_plan_path` / build gate 全在里面 | 一个会话一个计划大管家。 |
| **internal subagent dispatch** | 内部 Rust API 形态的子 Agent 派发入口（对标 [codex `run_codex_thread_one_shot`](https://example/codex_delegate)） | `AgentRegistry::spawn_subagent_internal(...)` | 复用 [`multi-agent.md`](./multi-agent.md) §14 基础设施；不进 catalog；`allowed_tools` 由调用方硬编码 | 内部派子 Agent，模型看不到。 |
| **`/plan build`** | 进入执行态的唯一入口 | 本地 slash 命令 `/plan build <plan_id/path>` | runtime 写 `session_key/id`、swap reminder（PLANNER → EXECUTOR）、切换可见 prompt 与 catalog | 用户拍板开干的开关。 |
| **`/plan exit`** | 退出 plan 交互态回 CHAT | 本地 slash 命令；当前实现允许 `Planning / Pending -> Chat` | 不写盘；不清 `active_planning_plan_id` / `active_plan_path`；reminder/catalog 复位为 CHAT | 只是退出当前模式，不是把 plan 弄没。 |
| **cancel_token** | 进程被打断的信号（Ctrl+C / SIGTERM / 父 abort） | tokio cancel token；runtime hook | EXEC 期被截断 → 运行态 mode 设 `pending`，PlanFile frontmatter `state = pending` | 中途打断当暂停处理。 |
| **system_reminder（PLANNER / EXECUTOR）** | 进入 PLAN/EXEC 时注入到 transcript 的 `<system_reminder>` | 进程内常量；注入到 **system 区段尾部**；详见 [`tools/planner.md`](./tools/planner.md) §6 | 仅在当前 mode 期间存在；切走自动消失 | 模式提示词，挂 system。 |
| **CLI prompt helper** | PLAN/EXEC/CHAT 下统一渲染 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` 及对应 agent prompt | `src/api/chat/prompt.rs`；详见 §5.5 | `Completed` 对用户侧折叠显示为 `Chat`；调用侧不得手写 prompt | prompt 显示一处生成。 |

**时间点钉死**：本文中的"进入执行态"专指 **用户显式 `/plan build <plan_id/path>`** 之后；reviewer 给出 accepted 摘要也只是建议，不会自动 build。

---

## 2. 竞品 / 选型对比（调研）

### 2.1 这类能力真正要解决什么

`todo`、`/plan` 常被一起讨论，但它们解决的是两类不同问题：

```text
┌────────────────────┬──────────────────────────────────────────────────────┐
│ 任务拆解            │ 系统把目标拆成可执行步骤与里程碑                     │
│                    │ 例：create_plan / plan file                          │
├────────────────────┼──────────────────────────────────────────────────────┤
│ 执行编排            │ 执行中持续同步状态、bash 输出、checkpoint hook      │
│                    │ 例：todos / panel / file lock / milestone checkpoint │
└────────────────────┴──────────────────────────────────────────────────────┘
```

**说人话**：`/plan` 负责"规划怎么做"，`todos` 负责"现在干啥"。两者通过 `PlanFile` frontmatter 共享真相。

### 2.2 代表性实现横向表（按本期决策摘要）

| 代表 | `todo` 形态 | `plan` 形态 | reviewer | 进执行态触发 | 我们的取舍 | 说人话 |
|------|-------------|-------------|----------|--------------|------------|--------|
| **cc-fork-01** | `TodoWrite` / Task v2 | `EnterPlanMode` / `ExitPlanMode` 工具 | 无 | 用户 `ExitPlanMode` | 借鉴 plan vs exec 解耦 | plan 像闸门，todo 像白板。 |
| **codex** | `update_plan` | `/plan` slash + collaboration mode | `codex_delegate.rs` 内部派 | 用户拍 | 借鉴 internal dispatch + `update_plan` 命名 | 内部派 reviewer，过不过最终用户拍。 |
| **hermes-agent** | `todo_tool` | 无硬 plan gate | role 子 Agent | — | 借鉴 todo schema | todo 自己就是一等公民。 |
| **GenericAgent** | `plan.md` 勾选 | `in_plan_mode` 状态位 | SOP 校验 | 显式状态切换 | 借鉴 markdown 持久化 | 一份 `plan.md` 就能把状态钉住。 |
| **pi-mono `plan-mode`** | 解析步骤 + `[DONE:n]` | 本地 `/plan` 切只读 | 无 | — | 借鉴 widget 投影 + bash 白名单 | 范例很轻，借鉴 UX。 |
| **Tomcat（D 方案）** | 拆分双工具：`todos`（session 路径）+ [`update_plan`](./tools/update-plan.md)（PlanFile frontmatter） | `/plan` + PLAN 模式 + `create_plan`（整盘） | **internal dispatch；仅辅助、不做 gate** | **`/plan build <plan_id/path>`** | 见 §4.1 | 上述能力的有机组合 + 双工具职责单一 + 代码复用。 |

### 2.3 维度词典（R1–R9）

| 维度 | 关切 | 说人话 |
|------|------|--------|
| **R1 命令入口** | `/plan` 走本地 slash、LLM tool，还是纯 prompt/SOP | 本地 slash。 |
| **R2 Todo 模型** | `todos` 是独立工具、内部状态，还是 markdown 勾选 | 独立工具 + 内部状态 + markdown 投影。 |
| **R3 进执行态** | reviewer 通过自动进 / 用户显式触发 | **用户显式** `/plan build`。 |
| **R4 Review 闸门** | reviewer 是否做 verdict gate | **否**（仅辅助，摘要落 transcript）。 |
| **R5 持久化与锁** | 计划状态放哪、谁写、防并发覆盖 | `~/.tomcat/plans/`，三方协同写。 |
| **R6 Catalog 边界** | 哪些能力进 tool catalog | `create_plan` / `ask_question` / `todos` 进；`/plan` 不进。 |
| **R7 Checkpoint 集成** | PlanRuntime 自己做快照 vs 消费 store | 单向消费。 |
| **R8 执行面板** | 面板是否自己管进程 | 复用 `BashTaskRegistry`。 |
| **R9 里程碑粒度** | flat todo 还是 milestone tree | milestone tree（plan 路径）。 |

---

## 3. 目标与设计原则

### 3.1 观察指标表

| 目标 | 观察指标（落地后可核对） | 说人话 |
|------|--------------------------|--------|
| **G1 本地 `/plan` 命令族闭环** | `tomcat chat` 识别 `/plan`、`/plan exit`、`/plan build <plan_id/path>`；这些命令**不**进入 LLM transcript 作为普通 user 文本；进入 PLAN/EXEC 模式后注入对应 `<system_reminder>` 与 catalog 切换 | `/plan` 像 `/restore` 一样先在 chat 层吃掉。 |
| **G2 `todos` 一等状态** | 内置 `todos` 工具可读写完整 todo 列表；同一文件最多一个 `in_progress`；tool、panel、文件三者同步；**工具结果返回完整 items snapshot**，LLM 不需要再读 user message 拿状态 | todos 是结构化状态，且 LLM 拿到工具结果就知道全貌。 |
| **G3 `/plan build` 是 EXEC 唯一入口** | reviewer accepted 不自动 build；用户敲 `/plan build <plan_id/path>` 才进 EXEC；build 前置：当前 session 无 active plan / no active todos | 进执行态由用户拍。 |
| **G4 review 是辅助不是闸门** | `CreatePlan` 写入后内部同步派发 reviewer；reviewer 摘要落 `transcript.plan.review`；**不**写 PlanFile frontmatter、**不**改 mode | 审稿员只挑刺。 |
| **G5 计划文件可恢复** | 每次计划变更刷新 `~/.tomcat/plans/<slug>_<hash>.plan.md`；advisory file lock 冲突时有可见错误；`state == pending` 可被 `/plan build` 续跑；详细协议见 [`tools/create-plan.md`](./tools/create-plan.md) §5 | 重启之后还能接着看，pending 还能续跑。 |
| **G6 checkpoint 单向依赖** | milestone 完成时调用 `CheckpointStore::record`；checkpoint 语义仍由 [`checkpoint-resume.md`](./tools/checkpoint-resume.md) 定义 | 计划会用 checkpoint，但不重新发明 checkpoint。 |
| **G7 TodosPanel 复用现有 bash 状态** | TodosPanel 显示当前 todo 与 bash 任务摘要，来源于 `BashTaskRegistry`，**不**新增第二套进程管理器 | 面板只看现成任务。 |
| **G8 PLAN 模式工具收紧** | `mode == Planning` 时 catalog = **全集（含 `create_plan` / `ask_question` / `todos` / `update_plan`）**；`write/edit/hashline_edit/delete` 仅允许 `~/.tomcat/plans/*.plan.md`；EXEC 期 plan 文件全禁写（含正文，仅 `update_plan` 推进）；frontmatter raw 改硬拦截 | 规划阶段写工具只能动 plans/；执行阶段 plan 文件锁死，进度只能走 update_plan。 |
| **G9 Runtime per-session OOD** | `TodoRuntime` 与 `PlanRuntime` 都是 per-session 单实例，挂在 `ChatContext` 上；多 session 由未来 `ChatContextRegistry` 处理 | 每个会话各自一套大管家，不共享全局 HashMap。 |
| **G10 CLI prompt helper** | PLAN/EXEC/CHAT 的 user/agent prompt 统一由 `src/api/chat/prompt.rs` 渲染为 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` / `u[Plan:completed]>` 及对应 agent prompt | prompt 文案统一生成，调用侧不再手写。 |

### 3.2 非目标

| 非目标 | 推给 / 理由 | 说人话 |
|--------|-------------|--------|
| **重做 `CheckpointStore` / `/restore` 语义** | 已由 [`checkpoint-resume.md`](./tools/checkpoint-resume.md) 定稿 | 快照底座不是这篇文档要重写的。 |
| **把 `/plan` 暴露给 LLM 作为工具** | 本地命令更符合现有 chat 命令体系 | 这是控制面，不该让模型自己瞎按。 |
| **把计划文件放进 `agent_trail_dir`** | `agent_trail_dir` 是运行态只读目录 | 计划文件要能写能看。 |
| **把 reviewer 暴露为 LLM 可见工具** | reviewer 是 PLAN 模式内部审稿环节 | 审稿员不该让模型自己调。 |
| **reviewer accepted 自动进 EXEC** | 进 EXEC 必须由用户显式 `/plan build` | 不偷偷开干。 |
| **`/plan close` 显式收口命令** | 完成由 runtime 派生 `mode = completed`；用户不要可以 `/plan exit` 退 PLAN，文件留着 | 状态自然演化，不要 close 命令。 |
| **LLM 写 frontmatter YAML** | 由 `create_plan` 入参组装，runtime 拼接其余字段；EXEC 期 frontmatter 推进由 `todos` 工具 | LLM 不背 schema。 |

**说人话**：本期不是再造一个 autonomous loop，也不是重做 checkpoint。重点是把 `todos`、`/plan`、review、计划文件接起来；reviewer 仅辅助；EXEC 入口由用户拍板。

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表（精简版）

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| **R1 命令入口** | `/plan` / `/plan exit` / `/plan build <plan_id/path>` 全部为 chat 本地 slash | LLM tool 入口让模型自己进出 PLAN，约束弱 | `/plan` 是用户按的按钮。 |
| **R2 Todos 模型** | 双工具拆分（D 方案）：`todos` 写 `TodoFile`（任何模式可见，session 路径）+ [`update_plan`](./tools/update-plan.md) 写 `PlanFile.frontmatter.todos[]` / `milestones[]`（任何模式可见，按 `plan_id` 或显式 `path` 路由；EXEC/Pending 缺省跟随 active plan path）；二者共享 `apply_todos_op` op 引擎；都返回完整 items snapshot | 单工具 + `active_scope` 双轨容易让 LLM 选错入口 | 工具拆开，代码复用。 |
| **R3 进执行态** | 仅 `/plan build <plan_id/path>` 显式触发；reviewer accepted 不自动 build | 自动 build 绕过用户确认 | 用户拍板。 |
| **R4 Review 闸门** | reviewer 仅辅助：摘要落 transcript；**不**做 verdict gate；改稿权由 runtime 内部参数控制 | gate 模式让 reviewer 卡 build；改稿权下放给模型违背 frontmatter 锁定原则 | 审稿员只挑刺。 |
| **R5 持久化与锁** | `~/.tomcat/plans/<slug>_<hash>.plan.md` + advisory file lock；frontmatter 由 **`create_plan`（整盘初稿）+ [`update_plan`](./tools/update-plan.md)（增量）+ runtime（mode / session 绑定）+ 自动派生（all completed / cancel_token）四方协同写** | 单工具写入会让 mode 切换/todo 推进绕弯 | 四方各管一段。 |
| **R6 Catalog 边界** | CHAT/Pending/Completed: 全集 − `create_plan`（保留 `todos` / `update_plan` / `ask_question`）；PLAN: 全集（含 `create_plan` / `ask_question`）+ 写盘路径仅允许 `~/.tomcat/plans/*.plan.md`；EXEC: 全集 − `create_plan` − `ask_question`（保留 `todos` / `update_plan`）；EXEC 期 plan 文件全禁写 | catalog 不再屏蔽 bash/write；写权限改由路径策略统一拦截 | 三态切 catalog；`todos`/`update_plan`/`ask_question` 默认可见；写权限由路径守卫管。 |
| **R7 Checkpoint 集成** | 单向：`PlanRuntime -> CheckpointStore` | 双向耦合把基础设施绑死 | 单向消费。 |
| **R8 执行面板** | `TodosPanel` 复用 `BashTaskRegistry` 做 bash 摘要；不新增第二套进程管理 | 面板自带 subprocess registry 重复造 | 投影即可。 |
| **R9 里程碑粒度** | `Milestone[]` 结构化字段 + 派生状态 | flat todo 难挂阶段 checkpoint | 大任务分段。 |
| **R10 Runtime OOD** | `TodoRuntime` / `PlanRuntime` per-session，挂 `ChatContext`；多 session 由未来 `ChatContextRegistry` 解决 | 全局 HashMap 会让 mode 切换、面板刷新失去会话隔离 | 一会话一套大管家。 |
| **R11 CLI prompt helper** | 所有可见 prompt 都走 `src/api/chat/prompt.rs`；CHAT agent prompt 保持 `agent.<id>>`，其余模式显示 `agent.<id>[Plan:<state>]>` | 避免调用侧硬编码 prompt | prompt 一处生成。 |

### 4.2 实施点按阶段拆分

> 对齐任务卡 [`T2-P1-002.md`](../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)；当前代码尚未落地，验收锚点大多为 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **PR-PLA 命令面与 PLAN/EXEC 模式骨架** | `/plan` / `/plan exit` / `/plan build <plan_id/path>` 本地命令；`PlanMode` / `PlanRuntime` per-session 对象；进入 PLAN/EXEC 时注入对应 `<system_reminder>` 到 system 区段、CLI prompt 统一走 helper；help 文案；详见 [`tools/planner.md`](./tools/planner.md) | `src/api/chat/commands/{parse.rs,cmd_help.rs,cmd_plan.rs}`、`src/api/chat/plan_runtime/{mod.rs,mode.rs}`、`src/api/chat/prompt.rs`、`src/api/chat/mod.rs` | 见 §11：`parse_plan_commands`、`plan_enter_injects_planner_reminder_into_system`、`prompt_helper_renders_plan_modes` | 先把命令族、PLAN/EXEC 模式与 prompt 渲染搭起来。 |
| **PR-PLB `todos` / `update_plan` / `CreatePlan` / `AskQuestion` 工具与计划文件** | built-in `todos`（任何模式可见，写 TodoFile）；[`update_plan`](./tools/update-plan.md)（任何模式可见，写 PlanFile.frontmatter todos/milestones）；`CreatePlan`（详见 [`tools/create-plan.md`](./tools/create-plan.md)）；`AskQuestion`（详见 [`tools/ask-question.md`](./tools/ask-question.md)）；`TodoRuntime` / `PlanRuntime` 内部状态机 | `src/core/tools/contract/catalog.rs`、`src/core/agent_loop/tool_exec.rs`、`src/api/chat/plan_runtime/{file_store.rs,session_todos_store.rs,plan_todos_store.rs,mod.rs}`、`src/api/chat/plan_runtime/tools/{create_plan.rs,update_plan.rs,ask_question.rs,todos.rs}` | 见 §11：`todos_returns_full_items_snapshot`、`update_plan_visible_in_all_modes`、`plan_file_round_trip_frontmatter`、`create_plan_only_visible_in_planning_mode`（PENDING） | 四个工具落地，状态机串通。 |
| **PR-PLC reviewer 内部派发 + `/plan build` build gate** | `CreatePlan` 写入后内部派发 reviewer；reviewer 摘要落 transcript `plan.review`（**不**写 frontmatter、**不**做 gate）；`/plan build` 闸门：前置 `当前 session 无 active plan && 无 active todos`；详见 [`tools/reviewer.md`](./tools/reviewer.md) | `src/api/chat/plan_runtime/review.rs`、`src/core/agent_loop/dispatch.rs`、`src/api/chat/plan_runtime/tools/create_plan.rs`、`src/api/chat/commands/cmd_plan.rs` | 见 §11：`create_plan_internally_dispatches_reviewer`、`plan_build_requires_no_active_plan_or_todos`、`reviewer_summary_lands_in_transcript_not_frontmatter`（PENDING） | reviewer 仅辅助；build gate 只看运行态。 |
| **PR-PLD TodosPanel 与 bash 输出** | 待办面板、bash `task_id` 摘要投影；TodosPanel 协议详见 [`tools/todos.md`](./tools/todos.md) §7 | `src/api/chat/plan_runtime/panel.rs`、`src/api/chat/mod.rs`、复用 `bash_task_registry` | 见 §11：`todos_panel_reflects_bash_task_status`、`todos_tool_updates_panel_and_file`（PENDING） | TodosPanel 只做投影。 |
| **PR-PLE 里程碑拆分与 checkpoint hook** | `Milestone[]`；milestone 完成自动 `record(Milestone{...})`；mode 派生：全 completed → `mode = completed` | `src/api/chat/plan_runtime/checkpoint.rs`、`src/core/checkpoint/store.rs`（只读） | 见 §11：`milestone_completion_can_record_checkpoint`、`all_todos_completed_promotes_mode_completed`（PENDING） | 里程碑收口才打 checkpoint。 |
| **PR-PLF raw write/edit 拦截 + cancel_token 续跑** | PLAN 期 `write/edit` 路径白名单；frontmatter diff 硬拒；EXEC 期被 cancel_token 截断 → 写 `state = pending`，`/plan build` 续跑 | `src/api/chat/plan_runtime/tool_exec.rs`、`src/api/chat/plan_runtime/mod.rs`（cancel hook） | 见 §11：`plan_mode_raw_edit_body_allowed_frontmatter_rejected`、`cancel_token_demotes_executing_to_pending`、`pending_plan_resumable_via_build`（PENDING） | 写盘硬拦截 + cancel 续跑。 |

#### 4.2.1 PR-PLA：命令面与 PLAN/EXEC 模式骨架

- **交付**：`/plan`、`/plan exit`、`/plan build <plan_id/path>` 全部在 `parse.rs` 内被识别为本地命令并转入 `cmd_plan.rs`；`PlanRuntime` per-session 对象先落地；进入 PLAN/EXEC 时立即注入对应 `<system_reminder>` 到 system 区段尾部；user/agent prompt 统一由 `src/api/chat/prompt.rs` 渲染；`/help` 文案补齐。
- **入口契约**：`parse_command` 识别 `/plan` 并返回 `ChatCommand::Plan { sub, args }`；`dispatch_chat_command` 在主循环**优先**消费，**绝不**落到 user 文本扔给 LLM。
- **状态骨架**：`PlanRuntime { mode: PlanMode, goal: Option<String>, active_plan_id: Option<String>, draft, todos, milestones, build_gate_state, … }`；`PlanMode ∈ {Chat, Planning, Executing, Completed, Pending}`；详细 OOD 见 §6。
- **catalog 动态过滤**：`tool_exec` 在每轮调用前根据 `current_mode()` 计算可见集——
  - `Chat` ⇒ 全工具集 + `todos` + `update_plan` + `ask_question` − `create_plan`
  - `Planning` ⇒ 全工具集 + `create_plan` + `ask_question` + `todos` + `update_plan`（写盘路径仅 `~/.tomcat/plans/*.plan.md`，由 `safety.rs` 路径策略统一拦截）
  - `Executing` ⇒ 全工具集 + `todos` + `update_plan` − `create_plan` − `ask_question`；plan 文件全禁写（仅 `update_plan` 推进）
  - `Completed` / `Pending` ⇒ 同 `Chat`
  - 详见 [`tools/planner.md`](./tools/planner.md) §5。`todos` 和 `update_plan` **任何模式都可见**（D 方案）。
- **active plan 单一约束**：同一 session 同一时刻**仅一个** `active_plan_id`；若 `mode ∈ {Planning, Executing}` 时再次 `/plan`，本地拒绝并提示先 `/plan exit`；若 EXEC 中再次 `/plan build`，拒绝并提示先完成或 cancel。
- **重启恢复**：chat 启动时 `PlanRuntime::recover(plan_dir)` 扫描 `~/.tomcat/plans/`：详见 [`tools/create-plan.md`](./tools/create-plan.md) §5.6。

```text
   user keystroke
        │
        ▼
┌──────────────────┐  匹配 "/plan" 前缀
│  parse.rs        │
└──────┬───────────┘
       ▼  ChatCommand::Plan{sub,args}
┌──────────────────┐
│ dispatch loop    │──── 普通 user 文本 ──▶ runtime 装配 + LLM
└──────┬───────────┘                          ▲
       ▼                                       │ CLI prompt 切到当前 PlanMode
┌──────────────────┐                          │ system 区段尾部加 PLANNER/EXECUTOR reminder
│ cmd_plan.rs      │ 调 PlanRuntime API（enter/exit/build）
└──────┬───────────┘
       │  enter Planning / Executing
       ▼
   swap reminder + filter catalog + (build only) inject user meta plan body
       │
       ▼
┌──────────────────┐  唯一可写入口（cmd_plan / tool_exec::create_plan / tool_exec::todos / runtime 自动转移）
│ PlanRuntime      │
└──────────────────┘
```

**说人话**：先把命令族、PLAN/EXEC 模式与 catalog 动态过滤搭好；user message 自动贴模式标签；普通输入永远不会被偷偷当成 `/plan`。

#### 4.2.2 PR-PLB：`todos` / `CreatePlan` / `AskQuestion` 工具与计划文件

- **交付**：built-in `todos` 工具（schema / desc / exec 三件套，详见 [`tools/todos.md`](./tools/todos.md)）、`CreatePlan` 工具（详见 [`tools/create-plan.md`](./tools/create-plan.md)）、`AskQuestion` 工具（详见 [`tools/ask-question.md`](./tools/ask-question.md)）、`TodoItem` / `Milestone` 结构、`PlanFile` 编解码。
- **catalog 注册（动态可见集）**：`BUILTIN_TOOL_CATALOG` 新增三个条目 `todos` / `create_plan` / `ask_question`；`tool_exec` 在每轮调用前通过 `current_mode()` 过滤可见集。**不**加入 `/plan` / `checkpoint`。
- **`todos` 可调时机（硬约束）**：
  - `mode == Chat` → 写 `TodoFile`（active `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`）；
  - `mode == Executing` 且 `active_plan_id != None` → 写 `PlanFile.todos[]` / `milestones[]`，并派生 `mode`（全 completed → `mode = completed`）；
  - `mode ∈ {Planning, Completed, Pending}` → catalog 不可见；强行调用 tool error；
  - **reviewer 子 Agent 上下文** → 在 internal subagent dispatch 入口硬编码 `allowed_tools`，从源头剔除 `todos`（双保险）。
- **单写入路径（todos 工具）**：
  ```
  validate(todos) → acquire_file_lock(active_file) → mutate(TodoRuntime/PlanRuntime)
                 → write(active file) → release_lock
                 → return ToolResult { items: <full snapshot> }
                 → panel.refresh → emit transcript plan.todos / todos.snapshot
  ```
  任何一步失败整体回滚。`CreatePlan` 工具走同款单写入路径但语义不同（详见 [`tools/create-plan.md`](./tools/create-plan.md)）。
- **文件锁**：基于 `fs2::FileExt::try_lock_exclusive` 在 `<slug>_<hash>.plan.md.lock` 上加 advisory lock；忙等超时（默认 2s）→ `LockBusy` 错误。
- **frontmatter round-trip**：`serde_yaml` 反序列化 → `PlanFileFrontmatter`；正文按 `## Goal` / `## Draft` / `## Notes` / `## Todos Board` 锚点段落级重写，**保留**人类自由备注段。

**说人话**：`todos` 是模型唯一能改 todo 列表的口子；`CreatePlan` 是创建计划文件的唯一口子；两者都走「校验 → 文件锁 → 落盘 → 面板刷新 → 返回 snapshot」单线路径。

#### 4.2.3 PR-PLC：reviewer internal subagent dispatch 与 `/plan build`

- **交付**：reviewer 子 Agent（无主链上下文污染、不进 catalog、走 `internal subagent dispatch`）、`/plan build <plan_id/path>` 闸门；详细派发契约见 [`tools/reviewer.md`](./tools/reviewer.md)。
- **派发入口**：reviewer **不**作为 LLM 工具暴露；由 `CreatePlan` 工具内部调用 `internal subagent dispatch` API（`AgentRegistry::spawn_subagent_internal(...)`）。
- **subagent 隔离**：reviewer 起一个 `AgentLoop` 子任务，注入专属 `SubagentContext { system_prompt, allowed_tools, transcript=fresh, parent_session_id, token_budget }`；`allowed_tools` 由 internal dispatch 路径**硬编码**为 `{read, grep, find, todos}`（默认）或 `{read, grep, find, todos, update_plan, edit}`（当 runtime 配 `allow_review_edit=true`）；**永不**含 `create_plan`（防套娃）；**禁止**调 `bash` / `write` / `dispatch_agent` / `checkpoint`。
- **reviewer 输出契约**：reviewer 最终消息含 `summary:` 自由文本（≤600 字符），可选用 `update_plan` 修订 frontmatter `todos[]`、或用 `edit` 改 plan 正文；**无 verdict 字段**；runtime 把摘要与修改说明写入 `transcript.plan.review` 事件，**不**回写 `.plan.md` 中的审稿块，也**不**改 `mode`。
- **`/plan build` 闸门**：仅当 `当前 session 无 active plan && 无 active todos` 且 `指定的 PlanFile.state ∈ {planning, pending}` 才允许迁移到 `Executing`；按 `[plan] auto_checkpoint_on_build`（默认 false）决定是否 `CheckpointStore::record(Manual{label=format!("plan_build:{plan_id}")})`。
- **build 时 runtime 动作**（5 件事）：
  1. 写 `PlanFile.frontmatter.session_key = 当前 session_key`、`session_id = 当前 session_id`（pending 续跑覆盖旧值，warning）；
  2. 写 `PlanFile.frontmatter.state = executing`；
  3. swap system reminder：移除 PLANNER（若有）/ 注入 EXECUTOR；
  4. CLI prompt 切换到 `u[Plan:executing]>` / `agent.<id>[Plan:executing]>`（详见 §5.5）；
  5. catalog swap：PLAN/CHAT 集 → EXEC 集（与 CHAT 相同：全工具集 + `todos` + `update_plan` − `create_plan`）。

```text
  PLAN 模式 LLM tool_call create_plan(...) ─▶ tool_exec::create_plan
                    │
                    ▼  acquire_file_lock + write PlanFile
                    │  (runtime 装配 frontmatter, LLM 不背 schema)
                    ▼
              internal subagent dispatch (default: read|grep|find|todos;
                                           allow_review_edit=true: +update_plan|edit)
                    │  reviewer 子 Agent 同步阻塞运行
                    ▼
              ReviewSummary { summary, rounds, applied_changes }
                    │
                    ▼
              transcript.plan.review + ToolResult.review_summary
                    │ （mode 不变；reviewer 不做 gate）
                    ▼
            ┌──── 用户读 plan.md / review 摘要后 ────┐
            │                                       │
            ▼                                       ▼
       /plan build <plan_id/path>            /plan exit (回 CHAT)
            │
            ▼ runtime: 5 件事（写 session_key/id, state=executing, swap reminder, swap visible prompt, swap catalog）
            ▼
       Executing
            │
            ▼ todos.set_status(completed) 全 completed
            ▼
       runtime: mode = completed
```

**说人话**：写计划由 LLM 在 PLAN 模式调 `CreatePlan`，工具内部派 reviewer 审稿；reviewer 给的只是摘要，不会自动让计划进 EXEC。用户敲 `/plan build` 那一下才真的开干，runtime 一气把 session 绑定、reminder、可见 prompt、catalog 全切了。

#### 4.2.4 PR-PLD：TodosPanel 与 bash 输出投影

- **交付**：`TodosPanel`（待办 / 当前 / 已完成三段渲染）、`BashTaskRegistry` 摘要投影、当前 `in_progress` todo 与 bash `task_id` 的弱绑定、面板 snapshot 回写 transcript；详尽 UI 协议见 [`tools/todos.md`](./tools/todos.md) §7。
- **面板不持有进程**：`TodosPanel` 内部只缓存 `task_id`；真值始终向 `BashTaskRegistry::snapshot(task_id)` 拉取。
- **todo ↔ bash 弱绑定**：`todos` 把某条 todo 置为 `in_progress` 时，`PlanRuntime` 在内存里记 `current_in_progress_todo`；之后 `tool_dispatcher` 在 `bash` 返回时若读到该字段非空，把 `task_id` 追加到 `panel.attachments[todo_id]`。
- **渲染节流**：panel 刷新走 `RefreshNotifier`，节流 200ms 合批。
- **回放友好**：每次 panel 渲染同步生成纯文本 snapshot 写入 transcript `plan.todos` / `plan.panel` 自定义事件。

**说人话**：TodosPanel 只做投影；bash 状态从 `BashTaskRegistry` 拿，todo 状态从 `PlanRuntime` 内存拿（背后再回写文件）。

#### 4.2.5 PR-PLE：里程碑拆分与 checkpoint hook + mode 自动派生

- **交付**：`Milestone[]` 结构进入 `PlanRuntime` 与 frontmatter；milestone 完成自动 `record(Milestone{milestone_id, plan_id})`；**mode 自动派生**：所有 todo `= completed` → `mode = completed`。
- **触发判定**：在 `apply_todos_op` 完成写入后，重算每个 milestone 状态：若某 milestone 下**全部** todo `= completed` 且**之前**该 milestone 状态非 `completed`，且 `[plan] auto_checkpoint_on_milestone = true` → 调一次 `CheckpointStore::record(Milestone { plan_id, milestone_id, label })`。
- **完成派生**：所有 todo（含所有 milestone 下）= `completed` → `mode = completed`；同时 reminder swap：移除 EXECUTOR、回 CHAT；catalog 复位 CHAT；可见 prompt 回到 `u[Chat]>` / `agent.<id>>`。
- **失败归一化**：checkpoint 失败 → 仅 warning。

```text
  apply_todos_op(...) ──▶ enforce / write
                                  │
                                  ▼
                      recompute milestone status
                                  │
                                  ▼
                      all todos completed?
                                  │
                          yes ──┴── no
                            │         └── 继续 EXEC
                            ▼
                      mode = completed
                            │
                            ▼
                      swap reminder（EXECUTOR → 无）
                      可见 prompt 回 CHAT
                      swap catalog（EXEC → CHAT）
                            │
                            ▼
                      transcript: plan.complete
```

**说人话**：阶段是否完成看下属 todo 是否都勾完；全做完 runtime 自动认为整盘 completed 并复位 catalog/reminder/prompt；打不上 checkpoint 也只是个警告。

#### 4.2.6 PR-PLF：raw write/edit 拦截 + cancel_token 续跑

- **PLAN 期路径白名单**：`tool_exec::write` / `edit` 在 `current_mode() == Planning` 期间：path 必须在 `~/.tomcat/plans/*.plan.md`；其他路径 → tool error。
- **frontmatter 硬拦截**：任意模式下，对 `~/.tomcat/plans/*.plan.md` 的 raw write/edit，把新旧 frontmatter 做语义 diff；非空 → tool error，usage「frontmatter 由 todos / `/plan` 命令更新」。详见 [`tools/create-plan.md`](./tools/create-plan.md) §8。
- **cancel_token 续跑**：runtime 注册 cancel hook，收到 cancel_token / SIGTERM / 父 abort → 当前 EXEC 中的 PlanFile：
  1. 把内存态 `mode = pending`；
  2. 写 `frontmatter.state = pending`；
  3. swap reminder：移除 EXECUTOR；catalog 复位 CHAT；可见 prompt 复位到 CHAT。
  
  下次 `/plan build <plan_id/path>` 时读到 `state == pending` → 允许续跑；写新 `session_key/id`（warning 提示旧 session 已覆盖）。

```text
  EXEC 中
      │
      ▼ cancel_token / SIGTERM / parent abort
      │
      ▼
  runtime hook:
      mode (mem) = pending
      PlanFile.frontmatter.state = pending
      reminder.remove(EXECUTOR)
      catalog → CHAT
      prompt → CHAT
      │
      ▼
  CHAT 状态（state=pending；plan 文件仍可见）
      │
      ▼ 用户：/plan build <plan_id/path>
      │
      ▼
  build gate: PlanFile.state == pending ? yes
      │
      ▼ 同 build 5 件事（warning：旧 session 已覆盖）
      │
      ▼
  Executing（续跑）
```

**说人话**：PLAN 模式只能改 plan 文件；执行中被打断了状态切到 pending，下次 build 续跑 + 旧 session 警告。

---

## 5. 协议（命令面 / 工具 / 文件 / mode 注入）

### 5.1 本地 slash 命令

这些命令由 chat 层本地处理，**不进入** tool catalog，也**不**作为普通 user 文本发给 LLM。

| 命令 | 参数 | 行为 | 失败语义 | 说人话 |
|------|------|------|----------|--------|
| `/plan` | 无 | **进入 PLAN 模式**；注入 PLANNER `<system_reminder>` 到 system 区段尾部；catalog 切到全工具集 + `create_plan` + `ask_question` + `todos` + `update_plan` + 写盘路径白名单；CLI prompt 切为 `u[Plan:planning]>` / `agent.<id>[Plan:planning]>` | 已在 PLAN/EXEC → 本地拒绝并提示先 `/plan exit` | 先切到规划模式，再继续讨论目标。 |
| `/plan exit` | 无 | **仅 PLAN 模式可用**；退出 PLAN 模式回到 `Chat`；保留 `PlanFile` 不动（不写盘、不改 mode）；reminder/catalog/prompt 复位为 CHAT | `mode != Planning` → 友好提示「`/plan exit` 仅在 PLAN 模式可用；如需中止执行请等待 `cancel_token` 或终止进程」 | 中途退出规划，文件留着。 |
| `/plan build <plan_id/path>` | 必填 plan 目标（plan_id 或 path） | **进入 EXEC 模式**（唯一入口）：前置 `当前 session 无 active plan && 无 active todos` 且指定 PlanFile `state ∈ {planning, pending}`；执行 5 件事：① 写 frontmatter `session_key/session_id`（pending 续跑覆盖旧值，warning）；② 写 `state = executing`；③ swap reminder（PLANNER → EXECUTOR，注入 system 区段尾部）；④ CLI prompt 切到 `u[Plan:executing]>` / `agent.<id>[Plan:executing]>`；⑤ catalog swap（PLAN/CHAT → EXEC：全工具集 + `todos` + `update_plan` − `create_plan`）；可选 `record(Manual{plan_build:plan_id})` | 当前 session 已有 active plan / active todos → 拒绝；目标 PlanFile state 不合规 → 拒绝 | 用户拍板开干。 |

> **历史命令下线**：
> - `/plan apply` 改名 `/plan build <plan_id/path>`：apply 字面不够直观，且需要承载更多动作。
> - `/plan close [completed\|cancelled]` 下线：完成由 runtime 派生 `mode = completed`（全 todos completed）；用户不要可以 `/plan exit` 退 PLAN；EXEC 中按 Ctrl+C / 进程退出 → `state = pending`，可被 `/plan build` 续跑；不再单独提供 close。
> - `/plan show` 暂不实现：用户直接打开 `~/.tomcat/plans/*.plan.md` 查看；本期不进 §11 验收。
> - `/goal` 暂不实现：目标在进入 PLAN 后通过自然对话收敛；后续如需显式命令再添加。

### 5.2 built-in `todos` 工具

> **完整 spec 见** [`tools/todos.md`](./tools/todos.md)。本节仅给出概览。

**对外工具名**：`todos`（复数）

**单一事实源**：
- schema / description：`src/core/tools/contract/catalog.rs`
- 执行编排：`src/core/agent_loop/tool_exec.rs`
- 运行态状态：`src/api/chat/plan_runtime/{session_todos_store.rs,mod.rs}`
- TodosPanel UI 协议：[`tools/todos.md`](./tools/todos.md) §7

**返回值约定**：每次 `todos` 工具调用成功后返回 **完整 items snapshot**（含 `id` / `content` / `status` / `milestone_id`）+ `applied` 计数 + `active_in_progress` + `panel_snapshot_id`。**LLM 通过 tool result 感知 todo 全貌**，不依赖 user message 再注入 `.todo.md` 文件内容。

**可调时机（D 方案：所有模式可见）**：
- `todos` 在 **`Chat` / `Planning` / `Executing` / `Completed` / `Pending`** 五种模式都可见；
- 永远写 `TodoFile`（`~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`），**不写** `PlanFile`；
- LLM 用它作「会话内 scratchpad」——任何阶段都能列自己的步骤；
- **改 plan 内 `todos[]` / `milestones[]`** 不走 `todos`，请用 [`update_plan`](./tools/update-plan.md)（§5.3）；
- **reviewer 子 Agent 上下文** → `allowed_tools` 默认含 `todos`（私人记录无副作用）；`update_plan` 仅在 `allow_review_edit=true` 时附加。

### 5.3 built-in `update_plan` 工具（D 方案新增）

> **完整 spec 见** [`tools/update-plan.md`](./tools/update-plan.md)。本节仅给出概览。

**对外工具名**：`update_plan`

**单一事实源**：
- schema / description：`src/core/tools/contract/catalog.rs`
- 执行编排：`src/core/agent_loop/tool_exec.rs`
- 运行态状态：`src/api/chat/plan_runtime/{plan_todos_store.rs,mod.rs}`（与 `todos` 工具共享 `apply_todos_op` op 引擎）

**职责**：对**指定 PlanFile** 的 frontmatter `todos[]` / `milestones[]` 做**增量**编辑（mark in_progress / completed / cancelled、加新 todo、改 milestone 标题 / `todo_ids`）。

**可调时机（任何模式可见）**：
- `Chat` / `Planning` / `Executing` / `Completed` / `Pending` **全部可见**；
- 入参 `plan_id`：`Executing` 模式可省（默认 `session.active_plan_id`），其它模式必填；
- 目标 plan.mode 准入：`planning` / `executing` / `pending` 允许；`completed` 拒绝；
- **跨 session**：允许（target.mode ∈ {planning, pending}）；改 `executing` plan 仅限拥有它的 session；
- **自动派生触发点**：仅当「调用方 mode == Executing + target.mode==executing + target.session 与当前 session 一致 + 本次后全 completed」三者同时成立时，runtime 在同一把锁内派生 `mode = completed`（其它模式提交即使标全 completed 也不改 mode）。

**与 `todos` 的代码复用**：
- `apply_todos_op(store, ops)` 一份实现；`SessionTodoStore`（`todos` 工具用）/ `PlanTodoStore`（`update_plan` 用）各一个实现；
- 文件锁同一份 `advisory file lock` 协议；
- 提示词、schema、catalog 条目独立。

**返回值**：完整 items + milestones snapshot + `plan_state_before` / `plan_state_after`，详见 [`tools/update-plan.md`](./tools/update-plan.md) §5.2。

### 5.4 计划文件（`PlanFile`）

> **完整 frontmatter schema、advisory file lock 协议与 recover 流程见** [`tools/create-plan.md`](./tools/create-plan.md) §5。

**路径**：`~/.tomcat/plans/<slug>_<hash>.plan.md`

**单一事实源原则**：
- **frontmatter**：机器可写、机器可读的 durable source of truth；由 `create_plan`（初稿）/ [`update_plan`](./tools/update-plan.md)（增量）/ runtime（mode / session 绑定）/ 自动派生（mode 转移）**四方协同写**
- **markdown 正文**：人类扫读、review 摘要、自由备注；任何模式允许 raw `write/edit` 修改正文（PLAN 模式受路径白名单约束）
- transcript 中的 `CustomEntry` 只做观测 / resume hint，不覆盖 frontmatter

#### 5.4.1 frontmatter 字段（精简版，对齐 [`tools/create-plan.md`](./tools/create-plan.md) §5.2）

| 字段 | 类型 | 必填 | 写入方 | 说人话 |
|------|------|------|--------|--------|
| `plan_id` | string | 是 | `create_plan` runtime | 计划身份证。 |
| `goal` | string | 是 | `create_plan`（LLM 入参） | 高层目标。 |
| `mode` | enum: `planning \| executing \| completed \| pending` | 是 | `create_plan`（初值 planning）/ runtime（`/plan build`、自动派生） | 当前阶段。 |
| `session_key` | string \| null | 是（null 表示未 build） | runtime（`/plan build` 时写入） | 执行会话的路由键。 |
| `session_id` | string \| null | 是（null 表示未 build） | runtime（`/plan build` 时写入） | 执行会话的 transcript id。 |
| `created_at` | rfc3339 | 是 | `create_plan` runtime | 创建时刻。 |
| `schema_version` | int | 是 | runtime | 版本兼容。 |
| `milestones` | array | 是 | `create_plan`（整盘） / [`update_plan`](./tools/update-plan.md)（增量） | 大阶段。 |
| `todos` | array | 是 | `create_plan`（整盘） / [`update_plan`](./tools/update-plan.md)（增量） | 步骤清单。 |

> **删除字段**：`review_status` / `last_review` / `active` / `last_checkpoint_id` / `updated_at` / `org_session_key` / `org_session_id` 全部下线。详见 §13 历史决策。

#### 5.4.2 文件示例

```yaml
---
plan_id: chat_plan_runtime_a1b2c3d4
goal: "为 chat 模式补齐 todos 工具与 /plan 闭环"
mode: planning
session_key: null
session_id: null
created_at: "2026-05-16T20:58:00+08:00"
schema_version: 1
milestones:
  - id: m1
    title: "生成草案"
    todo_ids: [t1, t2]
  - id: m2
    title: "执行面板与 todo 同步"
    todo_ids: [t3]
todos:
  - id: t1
    content: "planner 提示词、命令族落地"
    status: pending
    milestone_id: m1
  - id: t2
    content: "create_plan / reviewer 同步派发"
    status: pending
    milestone_id: m1
  - id: t3
    content: "实现执行面板状态投影"
    status: pending
    milestone_id: m2
---

## Goal

为 chat 模式补齐 todos 工具与 /plan 闭环

## Draft

- 在 PLAN 模式中生成计划草案
- reviewer 摘要仅作辅助；进入 EXEC 由用户敲 /plan build
- 进入 EXEC 后由 `todos` 推进步骤状态

## Todos Board

- [ ] m1/t1 planner 提示词、命令族落地
- [ ] m1/t2 create_plan / reviewer 同步派发
- [ ] m2/t3 实现 TodosPanel 状态投影
```

### 5.5 user message mode prefix 与首轮 plan body 注入

#### 5.5.1 CLI prompt（统一 helper）

所有对外可见 prompt 都统一走 `src/api/chat/prompt.rs`：

| mode | user prompt | agent prompt |
|------|-------------|--------------|
| `Chat` | `u[Chat]>` | `agent.<id>>` |
| `Planning` | `u[Plan:planning]>` | `agent.<id>[Plan:planning]>` |
| `Executing` | `u[Plan:executing]>` | `agent.<id>[Plan:executing]>` |
| `Pending` | `u[Plan:pending]>` | `agent.<id>[Plan:pending]>` |
| `Completed` | `u[Plan:completed]>` | `agent.<id>[Plan:completed]>` |

- prompt 是显示层文案，不会改写 transcript JSONL 中的原始 user message。
- `current_mode()` 仍是事实源；UI、catalog 与 prompt helper 都只读这一份状态。
- `/plan build` 自动开跑时也复用同一 helper，因此日志与 CLI E2E 断言看到的是 `u[Plan:executing]> start building <path>`。

**说人话**：用户看到的模式提示现在一处生成，不再靠给每条 user message 塞 `[mode: ...]` 标签。

#### 5.5.2 EXEC 首轮上下文

`/plan build` 进入 EXEC 后，不再额外注入 `<plan_meta>`。执行阶段上下文由以下来源共同维持：

- EXEC 对应的 `<system_reminder>`
- 当前 active plan 的磁盘文件
- `update_plan` / `todos` 返回的完整快照
- 统一的可见 prompt（`u[Plan:executing]>`、`agent.<id>[Plan:executing]>`）

**说人话**：进入 EXEC 后不再额外塞一段 plan_meta，计划文件和工具结果本身已经足够支撑后续执行。 

### 5.6 transcript 自定义事件（观测用，非 source of truth）

| `CustomEntry.type` | 载荷 | 作用 | 说人话 |
|--------------------|------|------|--------|
| `plan.enter` | `goal`, `mode=planning` | 记录 `/plan` 进入 PLAN 模式 | PLAN 模式什么时候开的。 |
| `plan.exit` | `reason` | 记录 `/plan exit` | PLAN 模式什么时候退出的。 |
| `plan.create` | `plan_id`, `path`, `revision` | 记录 `CreatePlan` 工具写入 | 计划文件什么时候被工具写过。 |
| `plan.review` | `plan_id`, `summary`, `rounds`, `applied_changes` | 记录 reviewer 摘要（internal subagent dispatch 完成后） | 审核给了什么意见、这是第几轮。 |
| `plan.build` | `plan_id`, `session_key`, `session_id`, `prev_mode` | 记录 `/plan build` 进入 EXEC（含 pending 续跑） | 真正开始执行了。 |
| `plan.todos` | `plan_id`, `todo_ids`, `summary` | 记录 `todos` 工具更新（EXEC 期） | 哪几条状态被改了。 |
| `todos.snapshot` | `todos_id`, `summary` | CHAT 期 `todos` 工具更新 | 哪几条状态被改了（聊天清单）。 |
| `plan.panel` | `plan_id`, `snapshot` | 记录 TodosPanel 渲染 snapshot | 面板长啥样。 |
| `plan.complete` | `plan_id` | 全 todos completed → mode = completed | 计划做完。 |
| `plan.pending` | `plan_id`, `reason` | cancel_token / 进程退出 → mode = pending | 计划被打断转 pending。 |

**说人话**：transcript 里可以记这些事，方便恢复与审计；但真正要信哪条 todo 还在哪步，还是看 `PlanFile.frontmatter`。

---

## 6. TodoRuntime / PlanRuntime per-session OOD 设计

### 6.1 设计原则

| 原则 | 内容 | 说人话 |
|------|------|--------|
| **per-session 单实例** | `ChatContext` 持有 `todo_runtime: TodoRuntime` 与 `plan_runtime: PlanRuntime`，每个 session 各自一份 | 一个会话一套大管家。 |
| **不维护全局 HashMap** | 多 session 由未来 `ChatContextRegistry` 处理；TodoRuntime / PlanRuntime 自身**不**承担"按 session_key 路由"职责 | 不在 runtime 里塞会话表。 |
| **内存映射 + 文件同步** | 每个 `TodoFile` / `PlanFile` 在内存中有一份 `TodoRuntimeState` / `PlanRuntimeState` 副本；工具写入 → 内存改 → 写盘 → 推 panel；任一失败回滚 | 内存与文件双写但走单一通道。 |
| **panel 是只读投影** | 内存状态变化时通过 channel 推 panel；panel 不回写 | 屏幕只播不录。 |

### 6.2 `TodoRuntime`（CHAT 路径）

```rust
pub struct TodoRuntime {
    session_key:      String,                                   // 路由键，构造时绑定
    agent_id:         String,
    todos_dir:        PathBuf,                                  // ~/.tomcat/agents/<agentId>/todos/
    sessions_json:    Arc<SessionsJsonStore>,                   // active_todos_id 主存
    active_todos_id:  Arc<RwLock<Option<String>>>,              // 镜像主存
    state:            Arc<RwLock<TodoState>>,                   // 当前 active TodoFile 的内存映射
    panel_tx:         tokio::sync::watch::Sender<PanelSnapshot>,// 节流推送
}

pub struct TodoState {
    todos_id:    String,
    title:       Option<String>,
    items:       Vec<TodoItem>,
    milestones:  Vec<Milestone>,
    updated_at:  DateTime<Utc>,
}

impl TodoRuntime {
    pub fn hydrate(&self) -> Result<()> { /* 启动时 sessions.json → load TodoFile → state */ }
    pub async fn apply_op(&self, ops: Vec<TodosOp>, replace: bool) -> Result<TodoResult> {
        // 1. acquire <todo_path>.lock
        // 2. validate ops（单 in_progress 等）
        // 3. mutate state (in-memory)
        // 4. write TodoFile（frontmatter + body）
        // 5. emit transcript todos.snapshot
        // 6. panel_tx.send_modify(|s| s.todos = self.state.read().items.clone())
        // 7. return TodoResult { items: <full snapshot>, applied, panel_snapshot_id }
    }
    pub async fn new_todos(&self, title: Option<String>, ops: Vec<TodosOp>) -> Result<TodoResult> {
        // 1. generate new todos_id
        // 2. write new TodoFile
        // 3. purge_inactive_todos(self.session_key, keep=new_id)
        // 4. update sessions.json.activeTodosId
        // 5. apply_op(ops, replace=true) on new file
    }
}
```

**说人话**：`TodoRuntime` 就是当前 session 的「TodoFile 大管家」——构造时绑定 `session_key`，启动 hydrate，每次 `todos` 工具调用进 `apply_op`；新建一盘走 `new_todos`，旧文件由 `purge_inactive_todos` 一并删。多个 session 各自实例化各自的 `TodoRuntime`，互不影响。

### 6.3 `PlanRuntime`（PLAN/EXEC 路径）

```rust
pub struct PlanRuntime {
    session_key:        String,
    session_id:         String,
    plan_dir:           PathBuf,                                // ~/.tomcat/plans/
    state:              Arc<RwLock<PlanRuntimeState>>,
    panel_tx:           tokio::sync::watch::Sender<PanelSnapshot>,
    bash_registry:      Arc<BashTaskRegistry>,
    checkpoint_store:   Arc<dyn CheckpointStore>,
    cancel_token:       tokio_util::sync::CancellationToken,
    exec_first_turn:    Arc<AtomicBool>,                        // 兼容字段；当前仅表示 build 后的首轮执行回合
}

pub struct PlanRuntimeState {
    pub mode:                  PlanMode,                        // Chat | Planning | Executing | Completed | Pending
    pub goal:                  Option<String>,
    pub active_plan_id:        Option<String>,                  // 当前 session 正在编辑 / 执行的 plan
    pub draft:                 Option<String>,                  // PLAN 期 `## Draft` 段镜像
    pub todos:                 Vec<TodoItem>,                   // EXEC 期 PlanFile.todos[] 镜像
    pub milestones:            Vec<Milestone>,
    pub last_review_summary:   Option<String>,                  // 内存暂存，不写 frontmatter
    pub last_checkpoint_id:    Option<String>,
}

impl PlanRuntime {
    pub fn current_mode(&self) -> PlanMode { self.state.read().mode }

    pub async fn enter_plan_mode(&self) -> Result<()> { /* /plan */ }
    pub async fn exit_plan_mode(&self) -> Result<()> { /* /plan exit */ }
    pub async fn build_plan(&self, plan_id_or_path: &str) -> Result<()> { /* /plan build */ }

    pub async fn apply_create_plan(&self, input: CreatePlanInput) -> Result<ToolResult> { /* PR-PLB */ }
    pub async fn apply_todos_op(&self, ops: Vec<TodosOp>, replace: bool) -> Result<TodoResult> {
        // 与 TodoRuntime.apply_op 类似，但写 PlanFile.todos[] + frontmatter
        // 全 completed → self.set_mode(Completed) + reminder/catalog/prefix swap
    }

    pub async fn dispatch_reviewer(&self, plan_id: &PlanId, allow_review_edit: bool) -> Result<ReviewSummary> {
        // PR-PLC: 见 tools/reviewer.md §4.3 / RV-A；write_plan 完成后由 tool_exec::create_plan 调用。
        // 内部走 AgentRegistry::spawn_subagent_internal（multi-agent.md §14.4.2.1 路径 B）；
        // 若 summary.applied_changes，await 返回时 reload_from_disk(plan_id) 刷新内存快照（RV15）。
    }

    pub fn on_cancel(&self) {
        // 注册到 cancel_token；触发时把 mode 切 Pending + 写 frontmatter + swap reminder/catalog/prefix
    }

    pub fn current_prompt(&self, agent_id: Option<&str>) -> String {
        let mode = self.current_mode();
        match agent_id {
            Some(agent_id) => crate::api::chat::prompt::agent_prompt_for_mode(agent_id, &mode),
            None => crate::api::chat::prompt::user_prompt_for_mode(&mode),
        }
    }
}
```

**说人话**：`PlanRuntime` 是当前 session 的「PlanFile 大管家」——管 mode 切换、create_plan 编排、reviewer 派发、todos 写 plan、cancel 续跑和 prompt 状态；同样 per-session 单实例，多会话不共享。

### 6.4 `ChatContext` 持有关系

```text
┌──────────────────────────────────────────────────────────────┐
│ ChatContext (per-session)                                     │
│ ┌────────────────────┐  ┌────────────────────┐               │
│ │ todo_runtime:      │  │ plan_runtime:      │               │
│ │   TodoRuntime      │  │   PlanRuntime      │               │
│ │   (session_key=…)  │  │   (session_key=…)  │               │
│ └─────────┬──────────┘  └─────────┬──────────┘               │
│           │                       │                            │
│ ┌─────────▼──────────┐  ┌─────────▼──────────┐               │
│ │ checkpoint_store:  │  │ bash_registry:     │               │
│ │   Arc<dyn ...>     │  │   Arc<...>          │               │
│ └────────────────────┘  └────────────────────┘               │
└──────────────────────────────────────────────────────────────┘
                  ▲
                  │ 未来 ChatContextRegistry: HashMap<session_key, Arc<ChatContext>>
                  │ 多 session 路由 / 并发 / 复用 在 registry 层做
                  │
   多个 chat session 入口（CLI / API / SDK）
```

| 关系 | 说明 | 说人话 |
|------|------|--------|
| `ChatContext ⊃ TodoRuntime` | 每个会话独占一份 `TodoRuntime` | 每个会话一份 todo 大管家。 |
| `ChatContext ⊃ PlanRuntime` | 每个会话独占一份 `PlanRuntime` | 每个会话一份 plan 大管家。 |
| `TodoRuntime / PlanRuntime ↔ panel_tx` | 内存改完通过 channel 推 panel | 屏幕只读。 |
| `PlanRuntime → CheckpointStore` | 单向引用 | 计划用快照，不影响快照。 |
| `PlanRuntime → BashTaskRegistry` | 单向引用 | 面板要看命令进度时去查。 |
| 未来 `ChatContextRegistry` | 多 session 路由由 registry 持有 `HashMap<session_key, Arc<ChatContext>>`；与 [`multi-agent.md` §14.3.2 `AgentRegistry`](./multi-agent.md#1432-agentregistry进程级) **正交**，详见 §14.3.2.1 对照表 | 多会话路由不在 runtime 内部。 |
| `ChatContext` 持 `root_session_id: String` | 该会话当前父 `AgentLoop` 的 `session_id`（chat_loop 每次启动父 loop 时回写）；用作 [`multi-agent.md` §14.4.2.2](./multi-agent.md#1442-子-agentloop-的所有权与生命周期) 中 `AgentRegistry::get(parent.session_id)` 的锚点，让 `PlanRuntime::dispatch_reviewer` / `dispatch_agent_tool::run` 都能定位到当前父 handle | 聊天室记一根线，子 Agent 派发时按线找当前父。 |

> **双注册表的边界（与 [`multi-agent.md` §14.3.0 / §14.3.2.1](./multi-agent.md#1430-落地选型决策表-ma1ma12)）**：
>
> - `ChatContextRegistry`：key = `session_key`（持久 chat 身份），value = `Arc<ChatContext>`，存 `TodoRuntime` / `PlanRuntime` / 共享 `Arc` 服务 / `root_session_id`，与 chat session 同寿。
> - `AgentRegistry`：key = `session_id`（运行时实例 id，可含 `:sub:<uuid>`），value = `Arc<AgentHandle>`，存 `abort_signal` + `spawn_depth` + `parent_session_id`，**跑时注册结束注销**，**不**持 `AgentLoop`。
> - 父 `AgentLoop` / 子 reviewer `AgentLoop` 都不是 `ChatContext` 的字段；它们由 `chat_loop` / `spawn_subagent_internal` 各自栈帧拥有，跑完 drop。
>
> 详细 OOD 与所有权链见 [`multi-agent.md` §14.4.2.2 子 AgentLoop 的所有权与生命周期](./multi-agent.md#1442-子-agentloop-的所有权与生命周期) 与 [`tools/reviewer.md` §4.5](./tools/reviewer.md#45-ood-与双注册表reviewer-的嵌套但不开新-chatcontext)。

**说人话**：runtime 自己不操心"我是哪个 session"——它就是当前会话独占的一份对象；多会话路由交给上层 `ChatContextRegistry`。这样切到第二个聊天窗口时 panel 状态自然不串。子 Agent（reviewer / `dispatch_agent`）的登记与寿命另走 `AgentRegistry`，两张表互不掺和，只通过 `root_session_id` 一条线连起来。

---

## 7. 调度时序（运行时图）

### 7.0 核心串联流程（ASCII）

> **用途**：TUI 状态说明、commit body、onboarding 一页纸总览。细节拆分见 §7.1–§7.3 与 [`tools/planner.md`](./tools/planner.md) §8。

```text
  CHAT mode  (catalog: full + todos + update_plan − create_plan; prompt = u[Chat]>)
     |
     v
  /plan
     |
     v
  PLAN mode
   - inject PLANNER_SYSTEM_REMINDER (system tail)
   - prompt: u[Plan:planning]> / agent.<id>[Plan:planning]>
   - catalog: full tools + create_plan + ask_question + todos + update_plan
   - write tools (write/edit/delete/str_replace) restricted to ~/.tomcat/plans/*.plan.md
   - read/glob/grep/shell unrestricted
     |
     v
  LLM gathers info via ask_question / todos (scratchpad) / chat
     |
     v
  LLM calls create_plan (goal, draft, todos, milestones)
     |
     v
  PlanFile written: state=planning, session_key=null, session_id=null
     |
     v
  create_plan dispatches reviewer subagent (internal, no catalog visibility)
     |
     v
  reviewer reads repo; default tools: read/grep/find + todos
     allow_review_edit=true → +update_plan + edit (plan body allowed; frontmatter raw still forbidden)
     emits transcript event plan.review (+ ToolResult.review); does NOT gate EXEC
     |
     +-- user iterates: tweak todos via update_plan; rewrite via create_plan --+
     |                                                                          |
     +----------- /plan exit (PlanFile.state unchanged) -----+                  |
     |                                                       |                  |
     v                                                       v                  |
  /plan build <plan_id/path>                              CHAT mode <-----------+
     |                                                   (PlanFile lingers, mode=planning;
     |                                                    update_plan still available cross-session)
     v
  runtime build gate (3 conditions):
     1) session.active_plan_id == None
     2) session has no in_progress todo
    3) target plan.state in {planning, pending}
     |
     v
  runtime writes (5 things, see §5.1):
     ① frontmatter.session_key / session_id = current session
     ② frontmatter.state = executing
     ③ PLANNER_REMINDER off, EXECUTOR_REMINDER on
     ④ prompt = u[Plan:executing]> / agent.<id>[Plan:executing]>
     ⑤ catalog swap: full + todos + update_plan − create_plan − ask_question
     |
     v
  EXEC mode
     - LLM advances the plan with update_plan (mark in_progress / completed; defaults to active_plan_id)
     - each successful update_plan → emits plan.panel event (throttled 200ms) → TodosPanel UI refresh
     - todos remains available as session-local scratchpad (writes only .todo.md, never PlanFile)
     - update_plan ToolResult carries full items + milestones snapshot (no user-message re-inject)
     - subsequent turns: only short EXECUTOR reminder (system tail), no full body re-inject
     - plan file is OFF-LIMITS to raw write/edit/delete (frontmatter AND body); runtime rejects direct writes
     |
     +---- iterate: update_plan(set_status) → plan.panel refresh → next todo ----+
     |                                                                            |
    +-------------- all todos completed (via update_plan in EXEC) ----+
    |                                                                  |
    |                                                                  v
    |                                                runtime auto:
    |                                                   - may run read-only Code Reviewer first
    |                                                   - then runs Verifier in the same update_plan turn
    |                                                   - verify_gate=soft (default): verifier fail is advisory, still promote completed
    |                                                   - verify_gate=gate + verifier fail: stay in EXEC, reopen via update_plan
    |                                                   - on completed: clear runtime.active_plan_id / EXECUTOR_REMINDER / catalog / prompt → CHAT
     |
     +-------------- cancel_token --------------------+
                  (Ctrl+C / process exit /          |
                   runtime kill)                    v
                                          runtime auto:
                                             - frontmatter.state = pending
                                             - clear runtime.active_plan_id
                                             - EXECUTOR_REMINDER off
                                             - catalog / prompt → CHAT
                                                |
                                                v
                                          /plan build <plan_id/path> can resume
                                          (gate condition 3 allows pending)
```

**说人话**：从 CHAT 敲 `/plan` 进 PLAN，模型写 plan、内部审稿只给摘要；用户 `/plan exit` 回 CHAT 或 `/plan build` 开干；干完自动 completed，被打断变 pending 可续跑。下面 §7.1–§7.3 按阶段拆成泳道图与序列图。

### 7.1 `/plan` → `CreatePlan` → reviewer → `/plan build`

```text
User             parse/cmd_plan        PlanRuntime           LLM (PLAN 模式)      tool_exec::create_plan          reviewer (internal dispatch)
 │                    │                    │                        │                       │                              │
 │ /plan              │                    │                        │                       │                              │
 │───────────────────>│ 解析/校验           │                        │                       │                              │
 │                    │───────────────────>│ mode = Planning         │                       │                              │
 │                    │                    │  inject PLANNER reminder → system 区段尾部       │                              │
│                    │                    │  prompt = u[Plan:planning]> / agent.<id>[Plan:planning]> │                              │
 │                    │                    │  filter catalog (PLAN 集 + 写盘路径白名单)       │                              │
 │                    │                    │                        │                       │                              │
 │                    │                    │  (LLM 在 PLAN 模式自由探索 / ask_question / create_plan / raw 改正文)        │
 │                    │                    │                        │ create_plan({...})    │                              │
 │                    │                    │                        │──────────────────────>│ runtime 拼 frontmatter +      │
 │                    │                    │                        │                       │ acquire lock + write PlanFile │
 │                    │                    │                        │                       │ internal dispatch reviewer ──>│ run AgentLoop child
 │                    │                    │                        │                       │<──────────────────────────────│ ReviewSummary { summary, ... }
 │                    │                    │                        │                       │ transcript: plan.review       │
 │                    │                    │                        │<──────────────────────│ ToolResult { review_summary } │
 │                    │                    │  state.last_review_summary 更新（不改 mode）                                    │
 │                    │                    │                        │                       │                              │
 │ /plan build <id>   │───────────────────>│ build_plan(id) gate ─▶ 5 件事:                                                 │
│                    │                    │   ① write session_key/id ② state=executing ③ reminder swap                     │
│                    │                    │   ④ prompt = u[Plan:executing]> / agent.<id>[Plan:executing]>                  │
 │                    │                    │   ⑤ catalog swap                                                                │
 │                    │                    │                        │                       │                              │
 │                    │                    │   transcript: plan.build                                                        │
```

**说人话**：`/plan` 进 PLAN 模式注 prompt 切 catalog；LLM 通过 `CreatePlan` 工具写计划，工具内部派 reviewer，摘要同步返回但不动 mode；用户敲 `/plan build` 才一气切到执行态。

### 7.2 执行态：`update_plan` 推进 + TodosPanel + 里程碑 checkpoint + 完成派生

```mermaid
sequenceDiagram
    autonumber
    participant L as LLM
    participant T as tool_exec(update_plan)
    participant P as PlanRuntime
    participant F as PlanFile
    participant B as BashTaskRegistry
    participant C as CheckpointStore

    L->>T: update_plan({ops:[...]}) (plan_id defaults to active_plan_id in EXEC)
    T->>P: apply_todos_op(PlanTodoStore, ...)
    P->>F: lock + sync frontmatter.todos[]/milestones[]
    P->>B: read bash task summary
    P-->>T: ToolResult { items + milestones full snapshot, applied, plan_state_before/after, panel_snapshot_id }
    alt milestone completed and auto checkpoint enabled
        P->>C: record(Milestone{m_id})
        C-->>P: checkpoint_id
    end
    alt all todos completed (EXEC + target.state == executing + same session)
        P->>P: mode = completed; swap reminder/catalog/prompt → CHAT
        P-->>F: write frontmatter.state = completed
    end
    Note over L,T: todos tool remains available as a session-local scratchpad,<br/>writing only to ~/.tomcat/agents/<id>/todos/*.todo.md (no plan effect).
```

**说人话**：EXEC 期推进 plan 走 `update_plan`（默认指向 `active_plan_id`）；它复用 `todos` 的 op 引擎写 `PlanFile.frontmatter.todos[]` / `milestones[]`。所有 todo 完成后，runtime 会先跑 code reviewer / verifier，再按 `verify_gate` 决定是否真正收工。`todos` 仍可用作个人 scratchpad，写自己的 `.todo.md`，不动 plan。

#### 7.2.1 `verify_gate` 语义（固定默认 `soft`）

- `soft` 是默认值，也是当前兼容语义：verifier 结果仅 advisory。即便 verifier 返回 `fail`，runtime 仍会把 plan 从 `executing` 推进到 `completed`，并自动切回 CHAT。
- `gate` 是显式收紧模式：只有当 verifier 返回 `fail` 时阻塞收工；此时 plan 保持 `executing`，调用方需要用 `update_plan` reopen 现有 todo 或继续新增修复 todo。
- code reviewer 与 verifier 都发生在“最后一个 todo 被标记完成”的那次 `update_plan` 调用内；`verify_gate` 只决定 verifier `fail` 时是否阻塞，不改变 reviewer 的 advisory 定位。
- 默认值不再待定：当前产品语义就是 `soft`；若未来要改默认值，必须视为单独的兼容性决策。

### 7.3 cancel_token 续跑

```text
EXEC 中：
  cancel_token ── triggered (Ctrl+C / SIGTERM / parent abort)
        │
        ▼
  PlanRuntime.on_cancel()
        │
        ├─ state.mode = Pending（内存）
        ├─ write PlanFile.frontmatter.state = pending（带文件锁）
        ├─ reminder.remove(EXECUTOR)
        ├─ catalog → CHAT
        ├─ prompt → CHAT
        └─ transcript: plan.pending { plan_id, reason }
                │
                ▼
        CHAT 状态（pending 计划文件仍在 ~/.tomcat/plans/）

下次启动：
  /plan build <plan_id/path> ──gate──▶
  PlanFile.state == pending ?  yes
        │
        ▼
  build 5 件事（warning：旧 session_key/id 已覆盖）+ exec_first_turn = true
        │
        ▼
  Executing（续跑）
```

**说人话**：执行中被打断了状态切 pending；下次 build 续跑时新 session 接管，旧的 warning 一下。

---

## 8. 状态机

```text
                            (默认进入)
                                │
                                ▼
                          ┌────────┐
                          │  Chat  │◀───────────────────────────────┐
                          └───┬────┘                                │
                              │ /plan                       │
                              ▼                                     │
                          ┌──────────┐                               │
                          │ Planning │── /plan exit ─────────────────┤
                          └────┬─────┘                               │
                               │ /plan build <plan_id/path>          │
                               ▼                                     │
                          ┌────────────┐                              │
                          │ Executing  │── all todos completed ───▶  │
                          └──────┬─────┘                              │
                                 │ cancel_token / SIGTERM             │
                                 │   / parent abort                   │
                                 ▼                                    │
                          ┌────────────┐ /plan build <plan_id/path> ┌──────┴───┐
                          │  Pending   │──────────────────────▶│Executing │
                          └────────────┘                       └──────────┘
                                ▲
        all todos completed     │
                ┌───────────────┘
                │
          ┌─────┴──────┐
          │ Completed  │（只读浏览；mode 不再回 Chat 由用户切换会话或开新 plan）
          └────────────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `Chat` | `/plan` | `Planning` | 注入 PLANNER reminder（system 区段尾部）、prompt 切到 `u[Plan:planning]>` / `agent.<id>[Plan:planning]>`、catalog 切 PLAN 集（含写盘路径白名单）、写 `plan.enter` 事件 | 进入 PLAN 模式。 |
| `Planning` | LLM 调 `create_plan(...)` | `Planning` | tool 内 advisory lock + 写 `PlanFile` + 内部派 reviewer；mode 不变 | 模型写计划。 |
| `Planning` | reviewer 返回 summary | `Planning` | 摘要落 transcript `plan.review`、内存 `last_review_summary` 更新；**不**改 mode、**不**改 frontmatter | 审稿员只挑刺。 |
| `Planning` | `/plan exit` | `Chat` | reminder/catalog/prefix 复位为 CHAT；保留 PlanFile 不动；写 `plan.exit` 事件 | 中途取消规划。 |
| `Planning` | `/plan build <plan_id/path>`（指向当前 session 创建的 plan） | `Executing` | runtime 5 件事：写 `session_key/id` + `state=executing` + reminder swap (EXECUTOR) + prompt 切到 executing + catalog swap；可选 `record(Manual{plan_build:plan_id})` | 现在才算正式开干。 |
| `Chat` | `/plan build <plan_id/path>`（续跑 pending） | `Executing` | 同上 5 件事 + warning「旧 session_key/id 已覆盖」 | 续跑被打断的 plan。 |
| `Executing` | `update_plan` 更新但未完结 | `Executing` | 更新 frontmatter `todos[]` / `milestones[]` + panel；返回 full items + milestones snapshot | 干活中。 |
| `Executing` | `todos` 调用（任意 ops） | `Executing` | 写当前 session 的 `.todo.md`；**不**触动 PlanFile；返回 session items snapshot | LLM 自己记 scratchpad。 |
| `Executing` | `update_plan` 提交后所有 todo `= completed`（target 是本 session 的 active plan） | `Completed` | 同一把锁内自动写 frontmatter `state=completed`；reminder/catalog/prompt 复位 CHAT；写 `plan.complete` 事件；可选 milestone checkpoint | 做完了。 |
| `Executing` | cancel_token / SIGTERM / parent abort | `Pending` | 写 frontmatter `state=pending`；reminder/catalog/prompt 复位 CHAT；写 `plan.pending` 事件 | 被打断转 pending。 |
| `Completed` | 用户开新 plan | `Planning` | 与 `Chat → Planning` 同 | 开下一盘。 |
| `Pending` | `/plan build <plan_id/path>` | `Executing` | 续跑流程 | 续跑。 |

**说人话**：状态机只有 5 档，没有 ready_to_apply、cancelled。`Completed` / `Pending` 都是终态/暂存态，不会自动再变；要继续干，要么 `/plan build` 续跑（pending），要么开新 plan（completed）。

---

## 9. 配置与环境变量

**总则**：`env > config > 默认`。本方案优先少配。

| 配置 / 派生值 | 取值 | 含义 | 说人话 |
|---------------|------|------|--------|
| `plan_dir`（派生） | `~/.tomcat/plans/` | 计划文件目录 | 计划文件单独放。 |
| `[plan] auto_checkpoint_on_milestone` | `true/false`，默认 `true` | 里程碑完成时是否自动 `record(Milestone{...})` | 默认每过一关就打点。 |
| `[plan] auto_checkpoint_on_build` | `true/false`，默认 `false` | `/plan build` 时是否自动 `record(Manual{plan_build:plan_id})` | 开干前打一张快照。 |
| `[plan] auto_milestone_threshold` | 正整数，默认 `5` | LLM 未传 milestones 时，todos 数 ≥ 阈值自动插入 `m-default` | 计划长了就自动分段。 |
| `[reviewer] max_rounds` | 正整数，默认 `1` | reviewer 单 plan 累计派发上限 | reviewer 不能无限挑刺。 |
| `[reviewer] default_allow_edit` | `true/false`，默认 `false` | runtime 是否允许 reviewer 直接 `edit` 计划正文（frontmatter raw 仍拒绝） | 审稿员能不能直接修正文。 |
| `[ask_question] timeout_ms` / `TOMCAT_ASK_QUESTION_TIMEOUT_MS` | 默认 `300000`（5 分钟），`0` = 不超时 | `ask_question` 等待 UI 返回的硬超时 | 别让模型死等用户。 |
| `[todos] purge_inactive_on_new_todos` | `true/false`，默认 `true` | 切换 active todos 时清理同 session 其它非 active 文件 | session 目录别堆垃圾。 |
| `[todos] auto_new_todos_on_replace_after_terminal` | `true/false`，默认 `true` | 全 completed/cancelled 后再 `replace_todos` 自动开新 TodoFile | 旧账翻篇。 |
| `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS` | 默认 2000ms | 计划文件 advisory lock 等待上限 | 等锁最多 2 秒。 |
| `TOMCAT_PLANNER_REMINDER_OVERRIDE_PATH` / `TOMCAT_EXECUTOR_REMINDER_OVERRIDE_PATH` | 可选路径 | 覆写默认 PLAN/EXEC `<system_reminder>` | 自带 prompt 写运维。 |

**说人话**：`auto_checkpoint_on_milestone` 默认开（每个里程碑自动 checkpoint）；`default_allow_edit` / `auto_checkpoint_on_build` 默认关。

---

## 10. 错误模型 / 警告 / 截断

```text
                    /plan or todo or build request
                            │
        ┌───────────────────┼────────────────────┐
        ▼                   ▼                    ▼
   用法错误             文件锁冲突          reviewer 异常退出
   本地 UsageError      本地 Error          tool error（PlanFile 留着）
        │                   │                    │
        └───────────────┬───┴───────────────┬────┘
                        ▼                   ▼
                PlanFile 写入失败      build gate 拒绝
                本地 Error / Tool Err  active plan/todos 占用
                        │
                        ▼
                 todo schema 非法 / >1 in_progress
                 Tool Err（拒绝写入）
```

| 场景 | 归一化结局 | 说人话 |
|------|------------|--------|
| `/plan` 参数不合法 | 本地 `UsageError`，不进入 LLM | 命令写错就当场报。 |
| `/plan exit` 在 `mode != Planning` 触发 | 本地友好提示「`/plan exit` 仅在 PLAN 模式可用；如需中止执行请等待 cancel_token 或终止进程」 | exit 不当 close 用。 |
| `/plan build` 当前 session 有 active plan / active todos | 本地 UsageError | 不允许两份计划同时跑。 |
| `/plan build` 目标 PlanFile `mode ∉ {planning, pending}` | 本地 UsageError | 已 executing / completed 不能再 build。 |
| 计划文件锁冲突 | 本地错误；保持旧状态不变 | 有人正在写，就别偷偷覆盖。 |
| reviewer 异常退出 | tool error 携带 reviewer stderr 摘要；`PlanFile` 保留；mode 仍 planning | 审稿员挂了不影响计划文件。 |
| `todos` 非法状态或两个 `in_progress` | tool error；拒绝写入 | todos 规则要硬。 |
| `PlanFile` 落盘失败 | 本地错误 / tool error；此次状态变更不提交 | 文件没写进去，就别假装成功。 |
| milestone checkpoint 失败 | warning；执行流继续 | 快照打点失败应可见，但不卡 todos。 |
| 在非 Planning 调 `create_plan` / `ask_question` | catalog 不可见；强行调用 → tool error | 模式不对工具看不见，硬调也拦。 |
| 在任意模式调 `update_plan` 改 `target.state=completed` 的 plan | tool error | 已结案 plan 不可改；要重新做请用 `create_plan` 开新 plan。 |
| 跨 session 调 `update_plan` 改 `target.state=executing` 的 plan | tool error | 别越界改正在执行中的 plan。 |
| 在 Planning raw `write/edit` 改 frontmatter | tool error，usage「frontmatter 由 todos / `/plan` 命令更新」 | YAML 锁死。 |
| 在 Planning raw `write/edit` 写 `~/.tomcat/plans/` 外路径 | tool error，usage「PLAN 模式仅允许写计划文件正文」 | 路径白名单。 |
| `recover()` 阶段发现 `mode == executing` 残留 | 强制降级 `mode == pending`、warning 提示用户 `/plan build <plan_id/path>` 续跑 | 重启视为被 cancel。 |
| `recover()` 阶段发现 frontmatter 不可解析 | warning + skip | 烂文件不当 plan。 |

---

## 11. 测试矩阵（验收）

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元：slash 解析 | `api::chat::commands::tests::parse_test::parse_plan_commands`（待新增） | PENDING | `/plan` 先被本地命令层吃掉。 |
| 单元：PLAN 模式注入 | `api::chat::plan_runtime::tests::plan_enter_injects_planner_reminder_into_system`（待新增） | PENDING | reminder 进 system 区段尾部。 |
| 单元：prompt helper | `prompt_helper_renders_plan_modes` | DONE | `u[Chat]>` / `u[Plan:*]>` 和对应 agent prompt 都由 helper 统一产出。 |
| 单元：/plan build prompt | `plan_build_emits_executing_prompt` | DONE | 自动开跑时能看到 `u[Plan:executing]> ...`。 |
| 单元：catalog 动态可见集 | `catalog_visible_set_by_current_mode`（待新增） | PENDING | PLAN 看不到 todos；CHAT/EXEC 看不到 create_plan。 |
| 单元：todos 返回 full snapshot | `todos_returns_full_items_snapshot`（待新增） | PENDING | tool result 自带完整 items。 |
| 单元：todos 单进行中约束 | `todos_state_enforces_single_in_progress`（待新增） | PENDING | 两个进行中要被硬拒绝。 |
| 单元：计划文件 round-trip | `plan_file_round_trip_frontmatter`（待新增） | PENDING | 字段不能丢。 |
| 单元：文件锁 | `plan_file_lock_is_exclusive`（待新增） | PENDING | 并发写必须能挡住。 |
| 单元：build gate | `plan_build_requires_no_active_plan_or_todos`（待新增） | PENDING | 当前 session 有占用就拒。 |
| 单元：reviewer 摘要不写 frontmatter | `reviewer_summary_lands_in_transcript_not_frontmatter`（待新增） | PENDING | review 落 transcript 不入 YAML。 |
| 单元：reviewer 不做 verdict gate | `create_plan_does_not_mutate_mode`（待新增） | PENDING | create_plan 不改 mode。 |
| 单元：recover executing 降级 | `recover_executing_demotes_to_pending`（待新增） | PENDING | 重启视为 cancel。 |
| 单元：pending 续跑 | `pending_plan_resumable_via_build`（待新增） | PENDING | pending 能 build 拉起。 |
| 单元：cancel_token 转 pending | `cancel_token_demotes_executing_to_pending`（待新增） | PENDING | Ctrl+C 切 pending。 |
| 单元：全 completed 派生 mode | `all_todos_completed_promotes_mode_completed`（待新增，由 `update_plan` 在 EXEC 触发） | PENDING | runtime 自动写 completed。 |
| 单元：`update_plan` 全模式可见 | `update_plan_visible_in_all_modes`（待新增） | PENDING | 任何模式都给增量编辑入口。 |
| 单元：`update_plan` 跨 session | `update_plan_cross_session_allowed_for_planning_pending`、`update_plan_cross_session_rejected_for_executing`（待新增） | PENDING | 跨 session 修订有边界。 |
| 单元：`update_plan` op 引擎复用 | `update_plan_reuses_todos_op_engine`（待新增） | PENDING | 同一份 op 引擎。 |
| 单元：`todos` 永远写 session | `todos_never_writes_plan_file`（待新增） | PENDING | todos 不沾 plan。 |
| 单元：raw edit 拦截 | `plan_mode_raw_edit_body_allowed_frontmatter_rejected`（待新增） | PENDING | 正文放、YAML 拦。 |
| 集成：CreatePlan 内部派 reviewer | `create_plan_internally_dispatches_reviewer`（待新增） | PENDING | 摘要必有同步返回。 |
| 集成：/plan build 完整 5 件事 | `plan_build_swaps_session_reminder_prefix_meta_catalog`（待新增） | PENDING | 5 件事一次性做完。 |
| 集成：`update_plan` / file / panel 同步 | `update_plan_updates_plan_file_and_panel`（待新增） | PENDING | 改 plan 不能只改一处。 |
| 集成：bash 面板联动 | `todos_panel_reflects_bash_task_status`（待新增） | PENDING | TodosPanel 要能看到 bash 任务摘要。 |
| 集成：里程碑 checkpoint | `milestone_completion_can_record_checkpoint`（待新增） | PENDING | 里程碑完成后能不能自动打点要锁住。 |
| E2E | `E2E-PLAN-001`：创建 3 里程碑计划 + reviewer 摘要 + `/plan build` 进 EXEC；`E2E-PLAN-002`：EXEC 中 Ctrl+C → pending → 再 `/plan build` 续跑 | PENDING | 任务卡 user-visible 流程。 |
| 文档 | 本文 + 5 工具 spec 同步回链 | 部分（本次） | 文档先把边界说清。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| `todos` 工具与计划文件双写导致漂移 | 中 | `PlanFile` frontmatter 为 durable source of truth；`todos` 走 `PlanRuntime -> file_store` 单通道；禁止 tool 直接改文件 | 只能有一条真正落盘的路径。 |
| reviewer 无限来回 | 中 | `max_review_rounds` 默认 1；超过阈值 warning；用户人工介入 | reviewer 不能无限挑刺。 |
| TodosPanel 再造一套任务状态 | 高 | 强制复用 `BashTaskRegistry` | 已有任务底座就别重复造。 |
| 里程碑 checkpoint 失败让执行流异常中断 | 中 | 默认 warning-only；仅更新 `last_checkpoint_id` 成功时写回 | 快照挂了不该把 todo 也弄丢。 |
| 计划文件路径选错到 `agent_trail_dir` | 高 | 固定派生到 `~/.tomcat/plans/`；路径断言 | 写到只读运行轨迹里会处处别扭。 |
| reviewer 子 Agent 越权调用 `bash` / `checkpoint` / `create_plan` / `dispatch_agent` | 高 | internal subagent dispatch 入口硬编码 `allowed_tools = {read, grep, find, todos}`（默认）或 `{read, grep, find, todos, update_plan, edit}`（runtime 配 `allow_review_edit=true`）；任何模式**永不含** `create_plan` | 让子 Agent 看不到危险工具，且不能套娃。 |
| reviewer 子流程中途崩溃或父进程被杀 | 中 | `PlanRuntime::recover()` 把残留 `executing` 降级 `pending`；启动期 warning 列出受影响 `plan_id` | 重启不要默默吞掉。 |
| 多份 `pending` `PlanFile` 并存 | 中 | 没有「active」概念，由用户在 `/plan build` 时指定 `plan_id` / 路径 | 用户拍。 |
| `/plan` 与 active 计划并发触发 | 中 | `cmd_plan.rs` 在调 `enter_plan_mode` 前先读 `active_plan_id`；非空直接本地拒绝 | 计划入口要单线。 |
| LLM 用 raw write 试图改 frontmatter | 高 | §5 `tool_exec` 硬拦截；tool error 附 usage | 写盘前先拦一刀。 |
| LLM 以为 `reviewer accepted` 就该自己进 executing | 中 | description 明示；catalog 在 PLAN 期没有切 EXEC 的入口 | 文档 + catalog 双保险。 |
| prompt 文案在多处手写后漂移 | 低 | 统一走 `src/api/chat/prompt.rs`，测试直接断言 helper 输出 | 不让 prompt 在不同调用点慢慢分叉。 |
| 多 session 串扰 | 中 | `TodoRuntime` / `PlanRuntime` 都是 per-session 单实例；多会话由 `ChatContextRegistry` 路由 | 一会话一份大管家。 |

---

## 13. 历史决策 / 跨文档修订

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~拆成 `todo-list.md` + `plan-mode.md` 两篇~~ | **替代**：以 `plan-runtime.md` 为运行时主 spec，`tools/` 下按工具粒度拆分。 | 主文档管闭环，工具文档管 schema。 |
| ~~让 `/plan` 走 LLM tool，类似 `EnterPlanMode`~~ | **替代**：本地 slash，进入后 catalog 动态收紧。 | plan 是用户控会话模式。 |
| ~~`Drafting` / `Reviewing` 两个独立运行态~~ | **否**：合并为 `Planning`；create_plan 工具内部同步 reviewer。 | 写计划与审计划是同一调用的两面。 |
| ~~`Idle` 模式名~~ | **替代**：改名为 `Chat`，更直观。 | 默认聊天状态。 |
| ~~`ReadyToApply` 中间态~~ | **下线**：reviewer 仅辅助，不做 gate；直接从 `Planning` 经 `/plan build` 跳 `Executing`。 | 状态机少一档。 |
| ~~`Cancelled` 状态~~ | **下线**：用户不要 → `/plan exit` 退 PLAN 文件留着；执行中按 Ctrl+C / 进程退出 → `Pending`；不再区分 cancelled vs pending。 | 留可续跑余地，不强收口。 |
| ~~`/plan apply` 进执行态~~ | **替代**：改名 `/plan build <plan_id/path>`，承载 5 件事（写 session 绑定、state、reminder swap、prompt 切换、catalog swap）。 | apply 字面不够，build 涵盖更多动作。 |
| ~~`/plan close [completed\|cancelled]`~~ | **下线**：completed 由 runtime 派生（全 todos completed 自动写）；用户不要可以 `/plan exit`；cancel 由 cancel_token 自动转 pending。 | 状态自然演化。 |
| ~~`/plan show` 命令~~ | **暂缓**：用户直接打开 `.plan.md` 看；本期不进验收。 | 用文件代替命令。 |
| ~~独立 `/goal` 命令~~ | **暂缓**：目标在进入 PLAN 后通过自然对话收敛；后续如需再加。 | 简化命令族。 |
| ~~把 reviewer 暴露为 LLM 可见 tool 或独立 dispatch_agent 子类~~ | **否**：走 `internal subagent dispatch`，不进 catalog。 | reviewer 是子 Agent，不是 LLM 工具。 |
| ~~reviewer verdict 二态做 gate~~ | **否**：reviewer 仅输出 `summary` 自由文本，**不**改 mode、**不**写 frontmatter；进 EXEC 由用户 `/plan build` 拍板。 | 审稿员只挑刺。 |
| ~~`create_plan` 入参含 `apply_changes`~~ | **否**：删除；reviewer 改稿权改为 runtime 内部参数 `allow_review_edit`，不暴露 LLM。 | 改稿决定权交给代码。 |
| ~~reviewer 改稿调 `create_plan`~~ | **否**：改稿直接走通用 `edit` + `update_plan`；不再递归调 `create_plan`，避免套娃。 | reviewer 改稿不重建整盘。 |
| ~~`PlanFile` 路径放在 `agent_definition_dir/agents/plan/`~~ | **替代**：固定到 `~/.tomcat/plans/<slug>_<hash>.plan.md`。 | 单独目录。 |
| ~~把 checkpoint 暴露给模型，计划里自己 `take_checkpoint`~~ | **否**：沿用 [`checkpoint-resume.md`](./tools/checkpoint-resume.md)。 | 别让模型把快照当 todo 用。 |
| ~~只写 markdown 复选框，不做结构化 frontmatter~~ | **否**：frontmatter 承担 source of truth。 | 机器得有机器能稳读的部分。 |
| ~~`PlanFile.frontmatter` 包含 `review_status` / `last_review` / `active` / `last_checkpoint_id` / `updated_at`~~ | **否**：全部下线。review 走 transcript `plan.review`；active 由 `mode` 派生；checkpoint id 由 `CheckpointStore` 自管；updated_at 用 mtime 推断。 | schema 精简。 |
| ~~`PlanFile.frontmatter` 加 `org_session_key` / `org_session_id`~~ | **否**：去掉 `org_` 前缀；改为 `session_key` / `session_id`，**仅在 `/plan build` 时写入**当前执行会话；创建期为 null。 | 不区分「创建会话」与「执行会话」。 |
| ~~`TodoRuntime` / `PlanRuntime` 用全局 `HashMap<session_key, _>`~~ | **替代**：per-session 单实例，挂 `ChatContext`；多会话由未来 `ChatContextRegistry` 路由。 | 一会话一份大管家。 |
| ~~`todos` 工具走 `active_scope ∈ {session, plan}` 双轨~~ | **下线（D 方案）**：`todos` 永远写 `TodoFile`（session 路径）；plan 内 `todos[]` / `milestones[]` 由新增的 [`update_plan`](./tools/update-plan.md) 工具管理（任何模式可见）；两者代码共享 `apply_todos_op` op 引擎、提示词分裂。 | 工具职责单一，代码继续复用。 |
| ~~`todos` 仅 EXEC / Chat / Completed / Pending 可见，Planning 剔除~~ | **替代（D 方案）**：`todos` **任何模式都可见**——LLM 在 PLAN 期也能用 `todos` 给自己列调研步骤；工具描述不再分模式叙述，仅保留「3+ 步骤启发」核心。 | 全模式都给个本地小白板。 |
| ~~mode=completed 自动派生由 `todos` 触发~~ | **替代（D 方案）**：由 [`update_plan`](./tools/update-plan.md) 在 EXEC + target.state==executing + 全 completed 时触发；`todos` 永远不写 plan，自然不触发 mode 转移。 | 改 plan 的工具负责派生 mode。 |
| ~~CHAT 模式无法修订 plan.md 的 `todos[]` / `milestones[]`（D 方案前的缺口）~~ | **修复（D 方案）**：[`update_plan`](./tools/update-plan.md) 任何模式可见，可按 `plan_id` 或显式 `path` 路由；EXEC/Pending 缺省跟随 active plan path；跨 session 修订亦允许（除非 target.state=executing 由别 session 持有）。 | 修上一版的缺口。 |
| ~~PLAN 模式下用户要求改 todos 必须再次调 `create_plan` 整盘重写~~ | **替代（D 方案）**：增量改用 [`update_plan`](./tools/update-plan.md)；`create_plan` 仅当结构大改时用（整盘重写）。 | 小修不必整盘重写。 |
| ~~frontmatter 三方协同（`create_plan` + `todos` + runtime）~~ | **替代为四方（D 方案）**：`create_plan`（整盘初稿）+ [`update_plan`](./tools/update-plan.md)（增量 todos/milestones）+ runtime（mode/session 绑定 via `/plan build`）+ 自动派生（all completed / cancel_token）。 | 四方各管一段。 |
| ~~reviewer 子 Agent 的 `allowed_tools` = `{read, grep, find}`（默认）+ `{edit_plan_review_section}`（`allow_review_edit=true`）~~ | **演进（D 方案）**：默认 `{read, grep, find, todos}`（todos 是私人记录无副作用）；`allow_review_edit=true` 附加 `{update_plan, edit}`（前者改 frontmatter todos，后者改计划正文任意段，但 frontmatter raw 仍禁止）；**仍**不含 `create_plan`（防递归套娃）。详见 [`reviewer.md`](./tools/reviewer.md) §5.2。 | 让 reviewer 能落地修订建议。 |
| ~~只靠 system reminder 让模型知道当前 mode~~ | **补充**：CLI 统一显示 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` / `u[Plan:completed]>` 及对应 agent prompt。 | 用户和测试都能直接看到当前模式。 |
| ~~所有 `system_reminder` 注入 user message~~ | **否**：所有 reminder 都注入 system 区段尾部；prompt 显示单独由 helper 渲染。 | reminder 归 reminder，prompt 归 prompt。 |

### 13.1 未来何时拆出 `todos-list.md`

出现以下任一条件时，**应**把 todos 独立为补充型架构文档：

1. 新增独立 `/todos` 本地命令，且存在**不依赖 `/plan`** 的主要用户故事。
2. `todos` 状态被除 `PlanRuntime` 之外的其它运行时复用。
3. `todos` 协议、压缩注入、持久化策略占主文档篇幅过大。
4. 看板把 todos 工具拆成独立任务。

**说人话**：现在 [`tools/todos.md`](./tools/todos.md) 已经独立成工具 spec，主文档保持总览。

### 13.2 与相邻文档的边界

- [`tools/planner.md`](./tools/planner.md) / [`tools/create-plan.md`](./tools/create-plan.md) / [`tools/ask-question.md`](./tools/ask-question.md) / [`tools/todos.md`](./tools/todos.md) / [`tools/reviewer.md`](./tools/reviewer.md)：定义各 LLM 工具与 PLAN 模式 / reviewer 子 Agent 的逐条 schema 与契约；本文定义运行时如何把它们串起来。
- [`multi-agent.md`](./multi-agent.md)：定义 `dispatch_agent` LLM 工具与 `AgentRegistry` / `spawn_depth` / `CascadeAbort` 基础设施；本文中 reviewer 通过 `internal subagent dispatch` 复用同一套 §14 基础设施，**不**走 `dispatch_agent` schema。
- [`checkpoint-resume.md`](./tools/checkpoint-resume.md)：定义 checkpoint 语义、命令面和 store；本文只定义 **PlanRuntime 如何消费它**。
- [`PLAN_SPEC.md`](../../agents/plan/PLAN_SPEC.md)：定义工程师写开发计划的规范；本文定义的是**运行时** plan file 与 TodosPanel。
- [`session-storage.md`](./session-storage.md)：transcript 自定义事件与 `sessions.json` 扩展字段（`activeTodosId` 等）。

---

## 14. 关联文档

- 工具 spec（`tools/`）：[planner.md](./tools/planner.md)（PLAN 模式整体规范）、[create-plan.md](./tools/create-plan.md)（LLM 工具 + PlanFile 协议 + 内联 reviewer）、[ask-question.md](./tools/ask-question.md)、[todos.md](./tools/todos.md)（含 TodosPanel UI 投影）、[reviewer.md](./tools/reviewer.md)
- EXEC 完工后代码验证（verifier，与 reviewer 分拆）：[plan-exec-code-verification.md](./plan-exec-code-verification.md)
- 子 Agent 基础设施：[multi-agent.md](./multi-agent.md)
- 底座依赖：[checkpoint-resume.md](./tools/checkpoint-resume.md)
- 标杆写法：[tools/read.md](./tools/read.md)
- 目录与权限边界：[work-dir-and-data-layout.md](./work-dir-and-data-layout.md)、[permission-system.md](./permission-system.md)
- transcript / custom entry 语义：[session-storage.md](./session-storage.md)
- 任务与计划规范：[T2-P1-002.md](../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)、[PLAN_SPEC.md](../../agents/plan/PLAN_SPEC.md)
- 竞品与工具全景：[agent-tools-comparison.md](../reports/agent-tools-comparison.md)、[plan-mode-and-checkpoint-survey.md](../reports/plan-mode-and-checkpoint-survey.md)

**一句话总结**：Tomcat 的 `PlanRuntime` 采用「**本地 `/plan` slash 切 PLAN/EXEC 模式与 catalog**、**`CreatePlan` LLM 工具创建 PlanFile 并内部派 reviewer**（仅辅助）、**`/plan build <plan_id/path>` 是 EXEC 唯一入口**、**built-in `todos` 改状态并返回完整 snapshot**、**计划文件做 durable source**、**cancel_token 转 pending 可续跑**、**TodoRuntime / PlanRuntime per-session OOD 挂 ChatContext**、**CLI prompt 统一显示当前模式**」的组合路线。本文是运行时主 spec，工具粒度细节散落在 [`tools/`](./tools/) 子目录。
