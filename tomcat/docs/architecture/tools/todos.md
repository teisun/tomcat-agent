# `todos` 工具：执行态待办状态机与 TodosPanel 投影

本文档是内置工具 **`todos`** 的冻结版技术方案（OpenSpec **B 类**：`docs/architecture/tools/`），承接 [`plan-runtime.md`](../plan-runtime.md) 的运行时编排，是 PLAN 闭环里管理 **会话级待办**（**TodoFile** = `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`）的内置 LLM 工具。**实现以仓库代码为准**；本文只保留**已定稿的行为与契约**。

> **重要**：自 D 方案落地起，`todos` **仅**写 `TodoFile`（session 路径），**不再**写 `PlanFile`。需要改 plan.md frontmatter 里的 `todos[]` / `milestones[]`，请用 [`update_plan`](./update-plan.md) 工具。`todos` 在**所有模式都可见**，作用是「LLM 在自由聊天 / 调研 / 规划 / 执行任意阶段，给自己列一份会话内的执行清单」。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：`todos` 是「会话本地的小白板」——任何模式都能用，写到 `todos/xxx.todo.md`，不动 plan 文件。改 plan 里的待办用 [`update_plan`](./update-plan.md)；整盘重写计划用 [`create_plan`](./create-plan.md)（PLAN 模式专属）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
  - [2.4 创建 / 执行 / 持久化专项对比](#24-创建--执行--持久化专项对比)
- [3. 目标与设计原则](#3-目标与设计原则)
  - [2.5 断点续跑：cc-fork 与 Tomcat 对比](#25-断点续跑cc-fork-与-tomcat-对比)
  - [2.6 工具提示词竞品对比](#26-工具提示词竞品对比)
  - [3.2 创建与执行分离（双轨持久化）](#32-创建与执行分离双轨持久化)
  - [3.2.0 `todos` 与 `update_plan` / `create_plan` 的边界（D 方案）](#320-todos-与-update_plan--create_plan-的边界d-方案)
  - [3.3 Milestone 模型](#33-milestone-模型)
  - [3.3.0 Milestone 何时创建、如何触发](#330-milestone-何时创建如何触发)
  - [3.4 TodoFile：路径、命名与当前清单指针](#34-todofile路径命名与当前清单指针)
  - [3.4.2 `active_todos_id` 放哪里](#342-active_todos_id-放哪里拍板)
  - [3.4.6 旧清单清理](#346-旧清单清理拍板)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. One-Glance Map（文件职责总览）](#6-one-glance-map文件职责总览)
- [7. TodosPanel：UI 投影协议](#7-todospanelui-投影协议)
- [8. 状态机与并发约束](#8-状态机与并发约束)
- [9. 配置与环境变量](#9-配置与环境变量)
- [10. 错误模型 / 截断 / 警告](#10-错误模型--截断--警告)
- [11. 测试矩阵（验收）](#11-测试矩阵验收)
- [12. 风险与应对](#12-风险与应对)
- [13. 历史决策](#13-历史决策)
- [14. 关联文档](#14-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **TodoItem** | 单个最小可执行步骤 | `TodoItem { id, content, status, milestone_id? }` | `id` 在**同一载体文件**内唯一；`status ∈ {pending, in_progress, completed, cancelled}`；同一载体**最多一个** `in_progress` | 一条待办，最多只能有一个在做。 |
| **TodoScope** | 待办写在哪种文件里 | `session` \| `plan` | 普通聊天且没有进行中的 plan → 写 `.todo.md`；`/plan build` 之后 → 写 `plan.md` | 两种文件，别混。 |
| **TodoFile** | 普通聊天用的一份待办清单文件 | `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md` | 见 §3.4；**同一 `session_key` 磁盘上只保留当前 active 那一份**（换新盘时删旧文件，§3.4.6） | 一份清单一个文件；旧盘不堆在目录里。 |
| **session_id** | 聊天记录 jsonl 的文件名 id | `sessions.json` → `SessionEntry.session_id` | 与 [`session-storage.md`](../session-storage.md) 一致 | 标识「这一份聊天流水」，不是待办 id。 |
| **session_key** | 在 sessions.json 里找会话用的键 | `agent:<agentId>:<channelKey>`（MVP：`agent:main:main`） | 指向「当前打开哪场聊天」 | 先靠它找到会话，再读到 session_id 和 activeTodosId。 |
| **active_todos_id** | 这场聊天**正在用**哪一份 `.todo.md` | **主存** `sessions.json` → `activeTodosId`；内存里有一份拷贝（§3.4.2） | 指针指向的那份才能改；换新盘后旧文件**删除**（§3.4.6） | 记在 sessions.json，不写在 todo 文件里。 |
| **PlanFile** | 走 `/plan` 时的计划文件 | `~/.tomcat/plans/<slug>_<hash>.plan.md` | 规划用 `create_plan` 写；执行用 `todos` 改 | 审过、apply 之后，以它为准。 |
| **`todos` 工具** | 模型改待办用的内置工具 | `name = "todos"` | 普通聊天和执行 plan 时可见；纯规划时不可见（改用 `create_plan`） | 该用 `todos` 时用 `todos`，规划写计划用 `create_plan`。 |
| **tool result snapshot** | `todos` 工具返回的完整 items 快照 | `ToolResult.items: Vec<TodoItem>` | 每次成功调用返回当前文件的全量 items；LLM **不需要**再读 `.todo.md` / `.plan.md` 拿状态 | 工具结果自带全貌，模型不用再查文件。 |
| **Draft op / Execute op** | 「只加任务」和「改进度」两类操作 | `upsert` 默认 pending vs `set_status` 改进行中/完成 | 规划阶段只能加任务；聊天和执行才能标「正在做」 | 先列清单，再勾进度。 |
| **Milestone** | 把很多 todo 归到一个大阶段 | `Milestone { id, title, order, todo_ids[] }` | 阶段是否完成，看下面 todo 是否都勾完 | 像「第一阶段」「第二阶段」。 |
| **TodosPanel** | 界面上的待办 + 命令行输出摘要 | `PlanPanelState`；`plan.todos` 事件 | 从文件和 BashTaskRegistry **读出来显示**；自己不存一份「真相」 | 像电视屏幕：只播，不录。 |
| **single_write_path** | 改待办只能走一条正规流程 | `apply_todos_op(scope, ...)` | 校验 → 改内存 → 写文件 → 刷新界面；不许旁路偷偷写 | 避免两个地方各写各的，最后对不上。 |

---

## 2. 竞品 / 选型对比（调研）

### 2.1 Agent 待办工具的典型关切

```text
┌──────────────────────────────────────────────────────────────────────┐
│  本地 todos 类工具通常要同时解决的四类问题                            │
├────────────────────┬─────────────────────────────────────────────┤
│  状态约束          │  多个进行中 / 漂移 / 状态枚举开放？            │
│  可见时机          │  规划态调？审计态调？还是只执行态？            │
│  与计划文件关系    │  是不是双写？谁是 source of truth？           │
│  与执行环境联动    │  bash 任务、checkpoint 是否要联动？           │
└────────────────────┴─────────────────────────────────────────────┘
```

**说人话**：四件事——同时只能有一项「进行中」；该用哪个工具、什么时候用要卡死；计划待办以 plan 文件为准；命令跑没跑完去别处查，侧边栏只展示摘要。

### 2.2 常见实现横向对比

| 来源 / 形态 | 工具名 | 状态枚举 | 同时进行中 | 与计划文件 | 与 bash | 说人话 |
|-------------|--------|----------|------------|-------------|---------|--------|
| **cc-fork-01** | `TodoWrite` | pending/in_progress/completed | 单一 | 无 plan 文件；**resume 扫 transcript**（§2.5） | 无联动 | 运行时内存，续跑靠日志回放。 |
| **claude-code 系** | `TodoWrite` | 同上 | 单一 | 通常无 | 无 | 同上。 |
| **hermes-agent** | `todo_tool` | 同上 + `cancelled` | 单一 | 集成 checkpoint | 无 | 加 cancelled 状态对齐取消语义。 |
| **GenericAgent** | `do_update_working_checkpoint` | 自定义 | 单一 | 强联动 checkpoint | 弱联动 | 把 todo 与 checkpoint 绑死。 |
| **本仓库 `todos`** | `todos` | 4 态 | 单一硬约束 | `PlanFile` 单写通道 | 通过 TodosPanel 联动 `BashTaskRegistry` | 状态清楚；改待办走一条路；shell 进度在侧边栏看摘要。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| T1 状态枚举 | 是否区分 cancelled vs completed | cancelled 不是失败，是「不做了」。 |
| T2 单进行中 | 一个还是多个 in_progress | 硬上限 1，多了就拒。 |
| T3 可见时机 | 规划/执行态都能调 todos？ | 普通聊天和执行 plan 能调；纯规划不能。 |
| T4 计划文件耦合 | 是否只允许一条路写 plan | 只允许正规流程写，不许偷偷改。 |
| T5 bash 联动 | 侧边栏自己记 shell 状态吗 | 不记；只拿任务 id 去问 BashTaskRegistry。 |
| T6 panel 投影 | 改一次待办就记一条聊天备份吗 | 250ms 内多次改只记一条，别刷屏。 |
| T7 创建/执行分离 | 规划时能否标「正在做」 | 规划只列任务；真正开干才能勾进度。 |
| T8 持久化载体 | 待办存哪 | 普通聊天落 `.todo.md`；走 plan 的落 `plan.md`。 |
| T9 双轨并存 | 没 plan 时有没有待办清单 | 有；和 plan 是两套文件。 |
| T10 milestone | 要不要分阶段 | 大任务要分阶段，方便打快照。 |
| T11 plan 态 checklist | 规划时能否用待办工具改清单 | 不行，和 codex 一样规划时禁 checklist。 |
| T12 apply 导入 | apply 时是否合并聊天清单进 plan | 默认不合并，避免没审过的条目进计划。 |

### 2.4 创建 / 执行 / 持久化专项对比

> 源码探查：`cc-fork-01/src/tools/TodoWriteTool/`、`EnterPlanModeTool/`；`hermes-agent/tools/todo_tool.py`；`openclaw/src/agents/tools/update-plan-tool.ts`；`codex/codex-rs/core/src/tools/handlers/plan.rs`；`GenericAgent/ga.py` + `memory/plan_sop.md`；`pi-mono/.../plan-mode/`、`todo.ts`；Cursor 契约见 [`cursor-todo-tool-execution-no-review.md`](../../reports/cursor-todo-tool-execution-no-review.md)。

| 来源 | **创建** todos（列任务） | **执行** todos（`in_progress` / 完成） | **持久化载体** | 与 Plan 模式关系 | 说人话 |
|------|--------------------------|----------------------------------------|----------------|------------------|--------|
| **Cursor** | 普通 **Agent** 与 **Plan** 流程中模型均可 `TodoWrite`（merge） | 主要在 **Agent 执行期**更新；Plan 阶段用 `CreatePlan`，**用户**显式 build 后再执行 | 会话内状态板；**无**强制 `*.plan.md` 级 todo 文件 | Plan=闸门，Todo=白板；执行更新**不回** reviewer | 你的理解大体对：两路都能「建」，真正推进在 Agent 干活时。 |
| **cc-fork-01** | `TodoWrite` 随时可写（`appState.todos[session]`） | 同上；与 `EnterPlanMode` **并行**（plan 改 permission，不删 TodoWrite） | 进程内 `appState`；非独立 `.md` | Plan 模式收紧写盘权限，**不**禁止 TodoWrite | 进 plan 模式也不关待办工具，两套各管各的。 |
| **codex** | **`update_plan`** 在 **非 Plan** collaboration mode | 同上；**Plan mode 内调用 `update_plan` 直接报错**（`plan.rs:77-80`） | TUI 事件 `PlanUpdate`；非 frontmatter 计划文件 | Plan 模式**禁止** checklist 工具 | 规划态不能动 checklist，只能调研。 |
| **hermes-agent** | `todo` 工具 `write`/`merge` | 随时可 `in_progress` / `completed` | **会话内存** `TodoStore`；压缩后 `format_for_injection` 再注入 | 与 checkpoint **正交** | 要持久化得自己扩，默认内存。 |
| **openclaw** | `update_plan` 全量步骤列表 | 单 `in_progress`；无独立 todo id | 工具返回 `details.plan`；**无** milestone | 无硬 plan gate | 步骤表，不是 id+merge 模型。 |
| **GenericAgent** | `plan.md` 勾选项 + SOP | `do_update_working_checkpoint` 等 | 工作区 **`plan.md`** markdown | `/plan` 本地模式 + plan 文件 | 计划即 markdown 文件。 |
| **pi-mono `plan-mode`** | 从回复解析 `Plan:` 步骤 → `todoItems[]` | `executionMode` + `[DONE:n]` 标记 | 扩展 **`appendEntry("plan-mode")`** 会话条目 | 本地 `/plan` 只读；**无** reviewer 文件 | 范例：面板 widget，非 durable plan。 |
| **pi-mono `todo.ts`** | 扩展 `todo` tool `add` | `toggle` / `done` | **tool result details**（分支可恢复） | 与 plan-mode **独立** | 状态在 transcript 分支里。 |
| **本仓库（拍板）** | **Planning**：`create_plan`；**Chat**：`todos` → **active** `todos/<todos_id>.todo.md` | **Chat** / **Executing**：`set_status`；Planning 禁止 execute | `todos/*.todo.md` + `plans/*.plan.md`；续跑 **file 优先**（§2.5） | 一会话磁盘只留当前 active；`new_todos` 后删其余；**tool result 自带完整 items snapshot**，模型不必再读文件 | 聊天待办和 plan 各用各的文件；工具结果带全貌。 |

**对你三个问题的直接回答**：

1. **Chat 模式与 PLAN 都能「创建」**：Chat 用 `todos`（`upsert`）；PLAN 用 `create_plan` 往 `*.plan.md` 写——**不是**同一个工具，但都能列任务。  
2. **只有 Agent 能「执行」**：指 `in_progress` / `completed` / `cancelled` + 配合 bash/edit 等副作用工具；**Planning 不允许**（与 codex 禁止 plan 模式下 `update_plan` 同型）。  
3. **持久化**：**对**——普通聊天用 **`todos/<todos_id>.todo.md`**（同一会话磁盘只留当前 active 一份，换新盘删旧，§3.4.6），走 `/plan` 用 **`plans/*.plan.md`**；两套文件不混写。

4. **状态感知**：模型每次成功调用 `todos` 后，`ToolResult` 内带**完整 items snapshot**（同时含 `id` / `content` / `status` / `milestone_id`），LLM **不需要**额外读 `.todo.md` / `.plan.md` 拿状态；transcript 仍写 `todos.snapshot` / `plan.todos` 自定义事件作为重启 fallback（详见 §2.5）。

### 2.5 断点续跑：cc-fork 与 Tomcat 对比

> 源码：`cc-fork-01/src/utils/sessionRestore.ts`（`extractTodosFromTranscript`）、`TodoWriteTool.ts`（`appState.todos[todoKey]`）。

| 阶段 | **cc-fork-01** | **Tomcat（拍板）** | 说人话 |
|------|----------------|-------------------|--------|
| 运行中 | `TodoWrite` → `appState.todos[sessionId]` **进程内存** | `todos` → 写 **`active` `.todo.md`** + 更新 `sessions.json.active_todos_id` | 我们直接写磁盘，不靠进程没关就丢。 |
| 进程退出 | 内存丢失 | **文件仍在** `todos/` | 关掉终端，待办文件还在。 |
| `--resume` / 重开 chat | 从 transcript **倒序**找**最后一次** `TodoWrite` 的 `input.todos`，灌回 `appState` | ① 读 `sessions.json` 的 `active_todos_id` → 加载对应 `.todo.md`；② 若无/损坏，扫 transcript 最后一条 `todos.snapshot` / `plan.todos`；③ 仍无 → 空板 | 他们靠翻聊天记录；我们先认 sessions.json 里的指针，再认文件，最后才翻聊天备份。 |
| 交互模式分支 | 注释写明：开启 **Todo v2** 时用**文件任务**，`AppState.todos` 在交互模式**不用** | 统一走 TodoFile 文件，不维护第二套内存真相 | 他们有两套实现；我们只认文件这一套。 |
| 与 checkpoint | **无**代码级绑定 | milestone 完成可 `CheckpointStore::record`；todo 板与 ckpt **正交**（同 hermes） | 恢复待办和恢复代码快照是两回事，别混。 |

```text
  Tomcat chat 启动 / hydrate
        │
        ▼
  sessions.json[session_key].activeTodosId 存在且 .todo.md 可读?
        │
   yes ─┴─ no
    │         │
    ▼         ▼
  load       scan transcript 最后 snapshot
  .todo.md       │
    │             └── 仍无 → 等首次 todos 调用时 new_todos
    ▼
  PlanRuntimeState 内存镜像（给界面读，不是第二份真相）
```

**说人话**：cc-fork 的 todo 本质是「内存 + 续聊时从聊天记录里扒最后一次 TodoWrite」；Tomcat 以 **`todos/<todos_id>.todo.md` + sessions.json 里的当前指针** 为准，聊天记录只在文件丢了时当备份线索。

### 2.6 工具提示词竞品对比

#### 2.6.1 Cursor 的 todo 系统提示词能看到吗？

**结论：公开侧看不到完整长提示词；只能看到「工具契约 + 短标签」。**

| 能看到什么 | 看不到什么 | 依据 |
|------------|------------|------|
| 客户端枚举 `TODO_WRITE` → UI 名 `TodoWrite` | 发给模型的完整 `prompt()` 正文 | [`cursor-builtin-tools-reference.md`](../../reports/cursor-builtin-tools-reference.md) §9 |
| 当前会话工具 schema：`merge`、`todos[{id,content,status}]`、**至多一个** `in_progress` | 「何时用 / 何时不用」长文是否仍在服务端单独注入 | 会话内工具定义；[`cursor-todo-tool-execution-no-review.md`](../../reports/cursor-todo-tool-execution-no-review.md) §4 |
| 产品语义：Plan=闸门、Todo=执行白板 | 与 cc-fork 是否共用同一份 `PROMPT.ts` 源码 | 官方未开源 Agent 工具注册表 |

**说人话**：Cursor 安装包里只有工具名叫 `TodoWrite`，**没有**像 cc-fork 那样一整份可读的 `prompt.ts`。你在 IDE 里能看到的参数规则（merge、四态、单进行中）是可靠的；**「复杂任务要主动建 todo」** 那类长指导，多半在云端组 prompt，本地仓库抄不到原文。

#### 2.6.2 各 Agent：提示词写在哪、怎么写

> 探查命令示例：`grep -rn TodoWrite cc-fork-01/src/tools/`、`hermes-agent/tools/todo_tool.py`、`codex/codex-rs/core/src/tools/handlers/plan_spec.rs`、`openclaw/src/agents/tool-description-presets.ts`、`pi-mono/packages/coding-agent/examples/extensions/{todo.ts,plan-mode/}`、`GenericAgent/memory/plan_sop.md`。

| 来源 | 工具名 | 提示词载体 | 篇幅 | 写法要点 | 源码锚点 |
|------|--------|------------|------|----------|----------|
| **Cursor** | `TodoWrite` | 服务端 schema +（推测）独立长 prompt | schema 短；长文不公开 | `merge` 增量；四态；单 `in_progress`；**无** plan 文件绑定 | 见 §2.6.1；对标 cc-fork |
| **cc-fork-01** | `TodoWrite` | **`prompt()` 长文** + `description()` 一句 | PROMPT **~180 行** + DESCRIPTION 1 句 | **When to use / When NOT**（含 XML `<example>`）；单进行中；`content`+`activeForm` 双文案；工具结果里续命句 | `cc-fork-01/src/tools/TodoWriteTool/prompt.ts` |
| **hermes-agent** | `todo` | **仅** OpenAI function `description`（**不改** system prompt） | schema **~15 行** | 行为全塞进 description；`merge`/`replace`；3+ 步；压缩后 `format_for_injection` 另路注入 | `hermes-agent/tools/todo_tool.py` `TODO_SCHEMA` |
| **codex** | `update_plan` | `ToolSpec.description` | **4 行** | `step`+`status` 列表；至多一个 `in_progress`；**Plan mode 内禁用**（handler 硬拒） | `codex-rs/core/src/tools/handlers/plan_spec.rs` |
| **openclaw** | `update_plan` | `describeUpdatePlanTool()` | **4 句** | 非平凡多步才用；步骤要短；至多一个 `in_progress` | `openclaw/src/agents/tool-description-presets.ts` |
| **GenericAgent** | （无独立 todo 工具） | **`plan_sop.md` + `ga.py` 注入** | SOP 级 | Markdown 勾选 `plan.md`；验证门在 SOP/拦截器，不是 tool schema | `GenericAgent/memory/plan_sop.md`、`ga.py` |
| **pi-mono `plan-mode`** | （无 todo 工具） | **`before_agent_start` 注入** | 一段 system 消息 | `Plan:` 编号列表；`[DONE:n]` 标记；只读工具集 | `pi-mono/.../plan-mode/index.ts` |
| **pi-mono `todo.ts`** | `todo` | `registerTool.description` | **1 行** | `list/add/toggle/clear` 动作式，**非**执行态四态机 | `pi-mono/.../extensions/todo.ts` |
| **pi_agent_rust** | — | — | — | 无 Agent 级 todo/plan 工具（仅测试 API `test.todo`） | `pi_agent_rust/src/extensions_js.rs` |
| **QevosAgent** | — | — | — | 当前树内 **未检出** 独立 todo/checklist 工具 | 探查 `agent/`、`docs/` 无匹配 |

**横向归纳（提示词策略）**：

| 策略 | 代表 | 优点 | 缺点 |
|------|------|------|------|
| **A. 超长 tool `prompt()`** | cc-fork | 约束细、例子多、模型少犯懒 | token 贵；难缓存；与 Tomcat 多 mode 难维护 |
| **B. 行为全在 schema `description`** | hermes | 静态可缓存；实现简单 | 过长则挤占参数区；难塞例子 |
| **C. 极简 description** | codex、openclaw | 干净；模式门控靠代码 | 模型常忘记何时该建清单 |
| **D. 不用 tool，改 system/SOP 注入** | GenericAgent、pi plan-mode | 规划/执行语义可分离 | 与 tool 调用解耦，难审计单次写入 |
| **E. Tomcat 拍板：B + 浓缩 A** | 本仓库 `todos` | 短 description 写清 mode/ops；**When to use** 保留 5～7 条浓缩规则；长例子不放 tool | 需 §5.1 与 user rules 分工 |

#### 2.6.3 择优结论（写入 §4.1 T19–T22、§5.1）

**拍板**：Tomcat `todos` 工具提示词采用 **hermes 式「行为进 schema description」+ cc-fork 式「何时用/何时不用」浓缩条**，**不**引入 cc-fork 180 行 `PROMPT` 或 pi 式纯 system 注入替代 tool。

| 借鉴来源 | 采纳 | 不采纳 |
|----------|------|--------|
| **hermes** | 全部行为约束写在 `description`；`merge` 语义用 `replace`/`ops` 表达；返回全量列表 | 纯内存、无 mode 门控 |
| **cc-fork** | 3+ 步/用户显式要求时才建清单；**恰好一个** `in_progress`；做完立刻 `completed`；工具结果一句确认 | `content`+`activeForm` 双字段（Tomcat 仅 `content`）；180 行 XML 例子 |
| **codex / openclaw** | description 首段写 **mode**（Chat/Executing/Planning→`create_plan`）；步骤要短 | 无 `id` 的 `step` 列表（Tomcat 用稳定 `id`） |
| **Cursor** | Plan 与 Todo 分工；执行期更新不回 reviewer | 假设能抄到 Cursor 原文 |
| **GenericAgent / pi** | plan 复杂任务走 `create_plan` / plan 文件 | 用 SOP 勾选代替 `todos` tool |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§11） | 说人话 |
|----|------|------------------|--------|
| G1 | `todos` 在**所有模式可见**（Chat / Planning / Executing / Completed / Pending）；只写 TodoFile，不写 PlanFile | `todos_visible_in_all_modes`、`todos_never_writes_plan_file` | 任何模式都能用，但只写自己的 .todo.md。 |
| G2 | 同时刻最多一个 `in_progress`（**按 scope 文件**各自计数） | `todos_state_enforces_single_in_progress` | 同一份清单或同一个 plan 里，不能两项同时「进行中」。 |
| G3 | 写动作走 `apply_todos_op(scope)` 单通道 | `todos_tool_updates_durable_file_and_panel` | 改聊天清单写 `.todo.md`，改 plan 写 `plan.md`，各走各的流程。 |
| G4 | `Planning` 调 `todos` → tool error；创建走 `create_plan` | `todos_tool_rejected_in_planning` | 还在规划时别调 `todos`，用 `create_plan` 写计划里的任务列表。 |
| G8 | `Chat` 写入 **active** `todos/<todos_id>.todo.md`；**同一 `session_key` 只保留当前 active 文件** | `todos_file_round_trip`、`multi_todos_active_anchor`、`new_todos_purges_inactive_files` | 普通聊天待办落盘；换新盘并激活后，同会话其它 `.todo.md` **物理删除**（§3.4.6）。 |
| G10 | 启动恢复：**sessions.json 指针 → .todo.md** > `todos.snapshot` | `resume_prefers_sessions_pointer_over_transcript` | 重开聊天：先看 sessions.json 指向哪份文件，再翻聊天备份。 |
| G9 | 改 plan 内 `todos[]` / `milestones[]` 走 [`update_plan`](./update-plan.md)（任何模式可见）；runtime 在全 completed 时由 `update_plan` 触发 `mode = completed` 派生 | 见 [`update-plan.md` §11](./update-plan.md#11-测试矩阵验收) | plan 待办归 update_plan 管，todos 不掺合。 |
| G11 | `todos` 工具返回的 ToolResult 自带完整 items snapshot | `todos_returns_full_items_snapshot` | 工具结果自带全貌，模型不必再读文件。 |
| G5 | TodosPanel 摘要 bash 任务但不复制状态 | `todos_panel_reflects_bash_task_status` | 侧边栏能显示命令跑到哪了，但**不自己再记一套**运行状态——每次显示时去 BashTaskRegistry 现查（见下文说明）。 |
| G6 | 里程碑全 completed 后允许（warning-only）触发 milestone checkpoint | `milestone_completion_can_record_checkpoint` | 某个阶段下所有 todo 都勾完，可以自动打一个代码快照；打失败只警告，不把 todo 改回去。 |

**说人话（§3 总览）**：两套文件——普通聊天用 `todos/xxx.todo.md`，走 plan 用 `plan.md`。规划时只许列任务（`create_plan`），开干后才许勾进度（`todos`）。分阶段（milestone）主要给 plan 用，聊天清单可以不要。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| 独立 `/todos` 本地 slash | 除非 §13 触发条件出现 | 本期没有单独 slash。 |
| `todos` 自管 bash 进程 | 生命周期归 `BashTaskRegistry` | 不自己起子进程。 |
| `todos` 直接调 `take_checkpoint` | milestone hook 由 runtime 触发 | 模型别直接拍快照。 |
| Planning 态开放 `todos` | 与 codex「plan 模式禁 checklist」一致 | 规划态只 `create_plan`。 |
| 把 session 板与 plan 文件混为一份 | 双 scope 各写各的文件 | 聊天清单和计划文件别合成一个。 |

### 3.2 与 `update_plan` / `create_plan` 的分工

#### 3.2.0 `todos` 与 `update_plan` / `create_plan` 的边界（D 方案）

**拍板结论**：`todos` **永远只**写 `TodoFile`（session 路径），**任何模式都可见**；要改 `~/.tomcat/plans/*.plan.md` 里的 `todos[]` / `milestones[]`，请用 [`update_plan`](./update-plan.md)。整盘重写计划用 [`create_plan`](./create-plan.md)（仅 PLAN 模式可见）。

| 问题 | 答案 |
|------|------|
| `todos` 在 Planning / Executing 等模式下可见吗？ | **可见**（自 D 方案起所有模式都可见；详见 §3.2.1）。 |
| `todos` 会写 `*.plan.md` 吗？ | **不会**。只写 `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`。 |
| 怎么改 plan.md 的 `todos[]` / `milestones[]`？ | 用 [`update_plan`](./update-plan.md)（增量编辑，任何模式可见）。 |
| 怎么整盘重写 plan？ | 用 [`create_plan`](./create-plan.md)（仅 PLAN 模式可见）。 |
| Executing 模式下 `todos` 还能用吗？ | **能**——但只写本 session 的 `.todo.md`；不会动正在执行的 plan。推进 plan 待办仍走 `update_plan`。 |

**设计理由**：

1. **工具职责单一**——`todos` 是「会话本地小白板」，`update_plan` 是「plan 文件编辑器」，`create_plan` 是「plan 文件创世」。三者数据载体、写入边界、可见模式各不相同，混在一起 LLM 容易选错。
2. **代码共用，提示词分裂**——`update_plan` 内部**复用** `todos` 的 op 引擎（同样的 `upsert / set_status / remove`、同样的文件锁），但 schema、提示词、目标 store 完全独立（详见 [`update-plan.md` §5.3](./update-plan.md#53-代码复用细节)）。这让 LLM 在工具描述层面看到两个边界清晰的入口，而代码层共享单一 op 实现。
3. **PLAN 模式下 todos 也可用**——LLM 在 PLAN 期可以用 `todos` 给自己列「调研步骤」，工具描述上**不**特别强调模式区分，保持 description 简洁；模型按 description 的「3+ 步骤启发」自行判断何时使用。
4. **mode 自动派生由 `update_plan` 触发**——`todos` 永远不会改 `plan.md`，自然也不会让 plan 进入 `completed`；mode 转移由 EXEC 模式下 `update_plan` 提交触发（详见 [`update-plan.md` §8](./update-plan.md#8-自动派生与-exec-收口)）。

#### 3.2.1 三个工具的分工对照

```text
                    用户 / 模型
                         │
   ┌─────────────────────┼─────────────────────┐
   ▼                     ▼                     ▼
 todos                update_plan          create_plan
 (any mode)           (any mode)           (PLAN mode only)
   │                     │                     │
   ▼                     ▼                     ▼
 TodoFile             PlanFile              PlanFile
 (session)            frontmatter           整盘
 .todo.md             todos[]/milestones[]  goal/draft/todos/milestones
   │                     │                     │
   ▼                     ▼                     ▼
 个人 / 会话           推进 plan 进度       创建 / 重写计划
 scratchpad           （增量）              （整盘）
```

| 工具 | 模式可见性 | 写什么 | 自动派生触发 mode 转移 |
|------|-----------|--------|------------------------|
| **`todos`** | 任何模式 | `TodoFile`（session 路径） | ✗ 永远不触发（不写 plan） |
| **`update_plan`** | 任何模式 | `PlanFile.frontmatter.todos[]` / `milestones[]`（按 `plan_id` 路由） | ✓ 仅 EXEC + target.mode==executing + 全 completed 时 |
| **`create_plan`** | 仅 Planning | `PlanFile` 整盘（含正文 + 初始 frontmatter） | ✗ 写 `mode = planning` 初值，不做转移 |

**说人话**：要给「会话」记清单 → `todos`；要改「plan 文件里的待办」→ `update_plan`；要「重写整个 plan」→ `create_plan`。

#### 3.2.2 双轨持久化（`TodoFile` 仍是 `todos` 的主战场）

| 载体 | 路径 | 谁写入 | source of truth |
|------|------|--------|-----------------|
| **TodoFile** | `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md` | **`todos` 工具**（任何模式） | 一会话磁盘只留当前 active；`new_todos` 激活新盘删旧，见 §3.4.6 |
| **PlanFile** | `~/.tomcat/plans/<slug>_<hash>.plan.md` | **`create_plan`**（初稿）、**`update_plan`**（增量）、runtime（mode / session 绑定 / 自动派生） | PLAN + milestone + checkpoint；详见 [`create-plan.md`](./create-plan.md) §5.2 |

**`/plan build` 与 `todos` 的关系**（精简）：

- `/plan build` **不**切换 `activeTodosId`、**不**修改 `TodoFile`；它只影响 `PlanFile`（由 runtime 写）与 catalog/reminder/prefix（由 runtime swap）。
- build 之后 LLM 推进 plan 进度用 [`update_plan`](./update-plan.md)；同时 `todos` 仍可调用，作 LLM 个人 scratchpad（如「先确认 X，再 Y」），不影响 plan。
- runtime 5 件事详见 [`plan-runtime.md`](../plan-runtime.md) §5.1。

#### 3.2.3 与 Cursor 的差异（避免抄歪）

| 点 | Cursor | Tomcat 拍板 |
|----|--------|-------------|
| 轻路持久化（TodoFile） | 偏会话内存/产品内部 | **显式** `*.todo.md`，便于 recover / 人工查看；**任何模式都能写**（个人 scratchpad） |
| 重路（PlanFile）创建 | `CreatePlan` + 用户 build | `create_plan` 整盘 + `update_plan` 增量；进 EXEC 仍由 `/plan build` 用户拍 |
| plan 内 todos 推进 | `TodoWrite` 直写计划 | **`update_plan`**（任何模式可见）；`todos` 永远只写 `.todo.md` |
| reviewer 闸门 | 无 | **无**（reviewer 摘要落 `transcript.plan.review`，不 gate）；进 EXEC 由 `/plan build` 用户拍 |
| 执行是否回 reviewer | **否** | **否**（与 [`cursor-todo-tool-execution-no-review.md`](../../reports/cursor-todo-tool-execution-no-review.md) 一致） |

### 3.4 TodoFile：路径、命名与当前清单指针

> 弃用草案路径 `sessions/<session_id>.todos.md`：`session_id` 是 **transcript 文件 id**（见 [`session-storage.md`](../session-storage.md)），一会话会多次「换任务」，不宜 1:1 绑一个 todos 文件。

#### 3.4.1 命名与目录

| 符号 | 含义 | 示例 |
|------|------|------|
| **session_key** | `sessions.json` 路由键 | `agent:main:main` |
| **session_id** | 当前 JSONL transcript 的 id（`SessionEntry.session_id`） | `20260517_a1b2c3d4` |
| **todos_id** | 一份 TodoFile 的全局 id（= 文件名主干） | `td_8f3a1c2b`（`td_` + `xxh32(session_key + created_at_ms)` 截 8 位） |
| **active_todos_id** | 本 **session_key** 当前执行哪一份 | **只**在 `sessions.json` + 运行态；见 §3.4.2 |

**落盘路径（拍板）**：

```text
~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md
```

与 transcript **分目录**：transcript 在 `agents/<agentId>/sessions/<session_id>.jsonl`；TodoFile 在 **`agents/<agentId>/todos/`**，避免「一个 session_id 只能绑一份 todos」。

#### 3.4.2 `active_todos_id` 放哪里（拍板）

**结论**：`active_todos_id` 是 **会话级指针**，放在 **`sessions.json` 的 `SessionEntry` 上**；进程内由 `PlanRuntime` 缓存；**不**靠 `.todo.md` 自身声明「我是不是当前 active」。

| 存储层 | 路径 / 类型 | 字段 | 读写时机 | 说人话 |
|--------|-------------|------|----------|--------|
| **① 持久真相（主存）** | `~/.tomcat/agents/<agentId>/sessions/sessions.json` | `SessionEntry.activeTodosId` | 首次 `todos` / `new_todos` / 切换 active 时 **原子**写回；`tomcat chat` 启动、切换会话时读取 | 「当前用哪份清单」写在这里，关终端也不丢。 |
| **② 运行态镜像** | `ChatContext` → `PlanRuntimeState` | `active_todos_id: Option<String>` | hydrate 时从 ① 灌入；每次 `todos` 成功改盘后与 ① 保持一致 | 程序运行中放内存里，省得每次都读 json。 |
| **③ 清单（可选）** | 同上 `SessionEntry` | `todosIds: string[]` | **拍板**：与 `activeTodosId` 同步为 **仅含当前 id**（`[activeTodosId]`）；不保留历史 id 列表 | 不堆历史指针；目录里也只留当前文件。 |
| **④ 文件内（非指针）** | `todos/<todos_id>.todo.md` | `doc_status: active \| suspended` | 描述本文件是否暂停（如 `/plan build`）；**不能**代替 ① 决定「会话当前写哪份」；旧 `archived` 仅兼容读，新盘用删除代替归档（§3.4.6） | 指针在 sessions.json；文件里最多标 active/suspended。 |

```text
  tomcat chat 启动 / 切换 session_key
        │
        ▼
  sessions.json[session_key].activeTodosId
        │
        ▼
  PlanRuntimeState.active_todos_id  （镜像）
        │
        ▼
  todos/<activeTodosId>.todo.md     （TodoItem[] 真相）
        │
        ▼
  todos 工具写 ops ──成功──▶ 写 .todo.md + emit todos.snapshot
                        └──▶ 若 new_todos：写新文件 → 删同 session 其它 `.todo.md` → 更新 ① 指针 + ③（§3.4.6）
```

**不放哪里**：

| 位置 | 为何不放 |
|------|----------|
| 仅 `.todo.md` frontmatter | 一会话多文件时无法表达「当前选哪份」；且指针切换不必改旧文件。 |
| `PlanFile` / `*.plan.md` | active 指针是 **session 轻路** 语义；Executing 改 plan 时用 `active_plan_id`，与 `active_todos_id` 正交。 |
| transcript JSONL 首行 | 可做 **恢复线索**（`todos.snapshot`），但不是 routing 主存（与 [`session-storage.md`](../session-storage.md)「sessions.json 路由」一致）。 |

**与 `session_id` / `session_key` 的关系**：

- **索引键**：`sessions.json` 用 **`session_key`**（如 `agent:main:main`）定位 `SessionEntry`，其内才有 `sessionId`（transcript 文件 id）与 `activeTodosId`。
- **新 todos 文件**：`todos_id` 生成可带入 `session_key + created_at_ms`；`session_id` 写入 frontmatter 仅作溯源，**不参与**文件名。

详见 [`session-storage.md`](../session-storage.md) §「元数据 store」扩展字段（拟定补丁）。

#### 3.4.3 一会话只改「当前这一份」清单

```text
  session_key (agent:main:main)
        │
        ├── session_id ──▶ .../sessions/<session_id>.jsonl   （对话）
        │
        ├── active_todos_id ──▶ todos/td_xxx.todo.md  （当前唯一清单文件）
        │
        └── SessionEntry.todosIds == [ activeTodosId ]   （与指针同步，不保留历史 id）
```

| 规则 | 说明 | 说人话 |
|------|------|--------|
| 写入目标 | `scope=session` 时**只**写 `sessions.json.activeTodosId` 指向的 `.todo.md` | 工具永远改「当前正在用的」那一份文件。 |
| 单进行中 | **每个 `.todo.md` 内**最多一个 `in_progress` | 同一份清单里不能两项同时在干。 |
| 磁盘只留一份 | 换新盘并激活后，同 `session_key` 下其它 `.todo.md` **删除**（§3.4.6） | 目录里不堆旧清单；要历史看 transcript `todos.snapshot`。 |

#### 3.4.4 何时创建新 `todos_id`（触发）

| # | 触发 | 行为 | 说人话 |
|---|------|------|--------|
| T1 | 本会话 **首次** `todos` 且 `active_todos_id` 为空 | `new_todos()` → 写 `td_*`、设 active（无旧文件可删） | 第一次列待办就新建一个文件。 |
| T2 | `todos` 入参 **`new_todos: true`**（与 ops 同调） | `new_todos()` → 写新文件、设 active → **删除**同 `session_key` 下其余 `.todo.md`（§3.4.6） | 明确「新开一盘」：新文件生效后，旧的直接删。 |
| T3 | `replace=true` 且当前 active 板已 **终态**（全部 `completed`/`cancelled`） | 自动 `new_todos()`（可配置 `auto_new_todos_on_replace_after_terminal`，默认 **true**）→ 同上清理 | 上一盘都做完了再整表替换 = 新任务盘，旧文件删掉。 |
| T4 | `replace=true` 且当前板 **非**终态 | 在**当前 active 板**上整表替换 | 同一任务重写整张清单。 |
| T5 | 用户 `/plan build <plan_id\|path>` | 不切换 `activeTodosId`；当前 `.todo.md` 可标 `doc_status=suspended`；EXEC 模式下推进 plan 改用 [`update_plan`](./update-plan.md)；`todos` 仍可继续用作 session 内 scratchpad | 开干后 plan 内待办归 update_plan；todos 仍能记自己的步骤。 |

**`sessions.json` 扩展字段（拟定）**：

```jsonc
{
  "sessionId": "20260517_a1b2c3d4",
  "activeTodosId": "td_8f3a1c2b",
  "todosIds": ["td_8f3a1c2b"]
}
```

#### 3.4.6 旧清单清理（拍板）

**结论**：每个 **`session_key`** 在 `agents/<agentId>/todos/` 下**只保留当前 `activeTodosId` 对应的一个** `.todo.md`；**新盘创建并成功激活后**，删除该会话下所有**非当前 active** 的 TodoFile（**物理删除**，不做 `archived` 堆积）。

| 项 | 规则 |
|----|------|
| **触发** | `new_todos()` 成功路径：T1 首次创建（无旧文件可删）、T2 `new_todos: true`、T3 终态后自动 `new_todos()` |
| **作用域** | 仅删 `frontmatter.session_key == 当前 session_key` 的 `.todo.md`；**不**删其它会话、**不**删 `plans/*.plan.md` |
| **保留** | 刚创建并已设为 `activeTodosId` 的新文件 |
| **删除对象** | `todos_id != activeTodosId` 且归属当前 `session_key` 的文件（含曾为 `suspended` / 误留的 `archived`） |
| **顺序（同事务）** | ① 创建并写完新 `.todo.md` → ② `purge_inactive_todos(session_key, keep=todos_id)` → ③ 写 `sessions.json`（`activeTodosId` + `todosIds=[id]`）→ 任一步失败则**整批回滚**（新文件也删） |
| **删除失败** | 禁止单独提交「指针已切、旧文件仍在」；重试或 tool error |
| **历史从哪看** | 旧清单内容以 transcript **`todos.snapshot`** 为线索；**不**在 `todos/` 目录留档 |

```text
  new_todos() 成功
        │
        ▼
  write todos/td_NEW.todo.md
        │
        ▼
  for each *.todo.md with same session_key:
        todos_id != td_NEW  ──▶  unlink (delete)
        │
        ▼
  sessions.json.activeTodosId = td_NEW
  sessions.json.todosIds      = [ td_NEW ]
```

| 配置 | 默认 | 说人话 |
|------|------|--------|
| `[todos] purge_inactive_on_new_todos` | `true` | 换新盘就删旧文件；本期不提供「只归档不删」开关。 |

#### 3.4.5 `.todo.md` frontmatter

```yaml
---
schema_version: 1
todos_id:       td_8f3a1c2b
session_key:    agent:main:main
session_id:     "20260517_a1b2c3d4"    # 创建时 transcript id（溯源）；续聊换新 jsonl 仍可沿用同一 todos_id
doc_status:   active | suspended   # archived 仅兼容读；新写入不再产生 archived（旧文件在 new_todos 时删除）
title:          "<可选，首条 todo 或模型 summary>"
todos:
  - { id: t-1, content: "...", status: pending }
milestones: []   # session 轻路可省略；无 checkpoint hook
created_at:     "<rfc3339>"
---
```

> 与 `PlanFile` 一致，`TodoFile` 也不再保留 `updated_at` —— transcript `todos.snapshot` / `plan.todos` 自带时序，文件级时间戳让 git diff 噪声爆炸。

- 每次 `apply_todos_op` 成功写盘后，追加 transcript 自定义事件 **`todos.snapshot`**（含 `todos_id` + todos 摘要），供 §2.5 二级恢复（**不**替代 `sessions.json.activeTodosId`）。

### 3.3 Milestone 模型

> 完整 frontmatter 字段与 `create_plan` 入参见 [`create-plan.md`](./create-plan.md) §5.2；checkpoint hook 见 [`plan-runtime.md`](../plan-runtime.md) §4.2.5。

#### 3.3.0 Milestone 何时创建、如何触发

**结论**：Milestone **只在 plan 路径上「定义/修订」**；**从不**在执行期 `todos` 里新建；**完成**时由 runtime **派生**并可选打 checkpoint——「触发」要分三种语义。

| 语义 | 何时 | 谁触发 | 做什么 | 说人话 |
|------|------|--------|--------|--------|
| **定义（Create）** | 第一次 `create_plan` 成稿 | LLM 传入 `milestones[]` + `todos[].milestone_id` | 写入 `PlanFile` frontmatter | 规划阶段把阶段和任务一次性定好。 |
| **修订（Revise）** | 用户在任意模式下要求改结构、或读到 reviewer 摘要后觉得不满意 | 增量改用 [`update_plan`](./update-plan.md)（`milestone_upsert`）；整盘重写仍用 `create_plan`（PLAN 模式） | 可增删改 milestone **id/title/order/todo_ids**（仍禁止 Executing 期新增 milestone id） | 改阶段分组用 update_plan；重写整盘进 PLAN 用 create_plan。 |
| **默认分组（Auto）** | `create_plan` **未**传 `milestones` 但 `todos.len() >= N`（`N` 默认 5，配置 `[plan] auto_milestone_threshold`） | **runtime** 自动插入 `m-default`「未分组」单 milestone | 避免完全平铺；**不**自动拆多阶段 | 模型偷懒没写阶段时给兜底。 |
| **完成（Complete）** | Executing 下某 milestone 下属 todo **首次**全 `completed` | **runtime** `recompute_milestones()` → `on_milestone_completed` | 可选 `CheckpointStore::record(Milestone{...})` | 不是「创建」，是「阶段做完」触发快照。 |

```text
  Planning                                  Executing
      │                                          │
      ▼                                          ▼
 create_plan ──定义/修订──▶              update_plan 改 todo / milestone
 milestones[]                              （执行期禁止新 milestone_id）
      │     ↻ reviewer 摘要落 transcript          │
      └──────── /plan build <plan_id|path> ──────┘
                                                  │
                                                  ▼
                                       全 completed → checkpoint hook
                                       全 completed → runtime: mode = completed
                                                       （由 update_plan 触发）
                                       cancel_token  → runtime: mode = pending

  注：todos 工具任何模式都可用；它写自己的 .todo.md，不动 plan。
```

| 不做什么 | 原因 |
|----------|------|
| Chat `todos` 创建 plan 级 milestone | session TodoFile 与 plan milestone 语义不同；session 可无 milestone |
| Executing 中 `upsert` 新 `milestone_id` | 阶段边界应在审计划时定稿 |
| 模型直接调 checkpoint | 与 G6 单向依赖一致 |

#### 3.3.1 数据结构

```rust
pub struct Milestone {
    pub id:           String,   // m-001
    pub title:        String,
    pub order:        u32,      // UI 排序；小在前
    pub todo_ids:     Vec<String>,  // 显式归属；与 TodoItem.milestone_id 双向校验
    pub checkpoint_label: Option<String>, // 写入 CheckpointStore 的人类标签
}

// 派生状态（不入库为独立字段，写入时由 runtime 计算）
pub enum MilestoneStatus {
    Pending,      // 下属 todo 全 pending
    Active,       // 至少一个 in_progress 或 completed 但未全完成
    Completed,    // 下属 todo 全 completed
    Empty,        // todo_ids 为空
}
```

| 规则 | 说明 | 说人话 |
|------|------|--------|
| 归属 | 每个 `TodoItem` 至多一个 `milestone_id`；缺省 → `m-default`（「未分组」） | 每条 todo 只属于一个阶段。 |
| 一致性 | `milestones[].todo_ids` 与 `todos[].milestone_id` 在 `apply_todos_op` / `create_plan` 提交时交叉校验；不一致 → tool error | 别里程碑列表和 todo 各写各的。 |
| 排序 | UI / TodosPanel 按 `milestone.order` 再按 todo 插入序 | 先按阶段排，再按条排。 |
| 状态 | **只派生**，禁止 LLM 直接写 `milestone.status` | 阶段完成看下属 todo 是否全勾完。 |

#### 3.3.2 生命周期与 checkpoint

```text
  create_plan 写入 milestones[] + todos[]
            │
            ▼
  Executing：todos.set_status(completed) ...
            │
            ▼
  recompute MilestoneStatus
            │
            ▼
  某 milestone 首次 → Completed ?
            │
      yes ──┴── no
       │         └── 不变
       ▼
  [plan] auto_checkpoint_on_milestone == true ?
       │
  yes ─┴─ no → 仅 UI 展示阶段完成
       ▼
  CheckpointStore::record(Milestone { plan_id, milestone_id, label })
       │
  fail └── warning-only（不回滚 todo）
```

| 配置 | 默认 | 说人话 |
|------|------|--------|
| `[plan] auto_checkpoint_on_milestone` | `true` | 阶段全做完可自动打点。 |
| `[plan] milestone_checkpoint_requires_plan_scope` | `true` | 只有 plan 文件上的 milestone 触发；session 板不触发。 |

**Session 板**：默认**无** milestone 章节或仅 `m-default`；若用户在中等任务里也要分段，允许 `milestones[]` 但 **不** 挂 checkpoint（避免与 plan 闭环混淆）。

#### 3.3.3 与 `create_plan` / `todos` 的分工

| 阶段 | 谁写 milestone | 谁写 todo 归属 |
|------|----------------|----------------|
| Planning | `create_plan.milestones[]` + `create_plan.todos[].milestone_id` | 一次性草案 |
| Executing | `todos` **不得** `upsert` 新 `milestone`（防执行期改阶段边界）；可 `upsert` todo 但 `milestone_id` 仅允许已有 id | 执行期可加细项 todo，不能偷偷加阶段 |
| Chat 聊天清单 | 可选；无 reviewer | 普通聊天清单可不分阶段，结构从简 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| T1 状态枚举 | 4 态 `{pending, in_progress, completed, cancelled}` | 3 态无法表达「主动放弃」。 | 取消和完成要分开。 |
| T2 单进行中 | **按 scope 文件**硬上限 1 | 软上限会被模型绕开。 | 每个 md 文件各最多一个在干。 |
| T3 可见时机 | **所有模式**可见（Chat / Planning / Executing / Completed / Pending）；只写 `TodoFile`（session 路径） | 规划态可用 todos 给自己列调研步骤；改 plan 内 todos 走 `update_plan`。 | 任何模式都能用，写自己的 .todo.md。 |
| T4 持久化 | `todos/*.todo.md` + `plans/*.plan.md`；`sessions.json` 只存指针 | 1:1 `session_id.todos` 无法多任务；纯内存无法 recover。 | 聊天清单和 plan 各用各的文件。 |
| T5 创建/执行 | Planning 禁 `set_status`/`in_progress`；Chat/Executing 全开 | 规划态标 in_progress 会破坏「先列计划再开干」（reviewer 仅辅助、不做 gate；开干由 `/plan build` 拍）。 | 规划只列任务，开干才能勾「进行中」。 |
| T6 单写通道 | `apply_todos_op(scope)` + 按路径 file lock | 双写漂移。 | 改待办只能走一条正规写入流程。 |
| T7 bash 联动 | TodosPanel 只持 `task_id` 引用 | 复制 bash 两套真相。 | 侧边栏不自己记 shell 是否在跑，只拿 id 去查。 |
| T8 panel | 节流 250ms → 一条 `plan.todos` | 每次写都 snapshot 塞爆 transcript。 | 界面别刷太勤，聊天记录也别记太密。 |
| T9 milestone | 仅 **plan scope** 结构化 `Milestone[]`；状态派生 | 平铺 todo 无法挂阶段 checkpoint。 | 大 plan 要分阶段，方便阶段性打快照。 |
| T10 build 导入 | 默认 **不** merge session→plan | 未经 reviewer 的 session todo 污染计划。 | `/plan build` 时不自动把聊天清单并进 plan。 |
| T11 执行期改 milestone | Executing 禁止新增 milestone id | 执行期改阶段边界难审计。 | 阶段划分在规划时定好，执行期别改。 |
| T12 Cursor 对齐 | reviewer 只绑 `/plan`（仅辅助）；todo 执行不回审 | 把 todo 当审批流会卡执行。 | reviewer 只挑刺，勾待办时不用再审一遍。 |
| T23 自动完成 | 全 todos completed → runtime 自动写 `mode=completed`、复位 catalog/reminder/prefix | 不要 close 命令 | 做完整盘自动收口。 |
| T24 自动 pending | cancel_token / SIGTERM / parent abort → runtime 自动写 `mode=pending`、复位 catalog/reminder/prefix | 区分 cancelled vs pending 太累 | 被打断转 pending，可续跑。 |
| T25 tool result snapshot | 每次成功 `todos` 返回完整 items snapshot | 让 LLM 重新读文件成本太高 | 工具结果自带全貌。 |
| T13 milestone 创建 | 仅 `create_plan`（Planning）；Executing 禁新建 | 执行期改阶段难审计。 | 阶段在写 plan 时定。 |
| T14 milestone 完成 | runtime 派生 Completed → 可选 ckpt | 模型不「创建完成事件」。 | 阶段下 todo 全勾完，系统自动认为阶段完成。 |
| T15 TodoFile 路径 | `todos/<todos_id>.todo.md`；非 `sessions/<sid>.todos.md` | 1 session 多任务、sid 歧义。 | 一份清单一个文件，文件名用 todos_id。 |
| T16 当前清单指针 | `sessions.json.activeTodosId`；只改指针指向的那份文件 | 多份清单并存会搞混。 | 换任务就换新文件，sessions.json 只记当前 id。 |
| T17 续跑 | `sessions.json.activeTodosId` → `.todo.md` > `todos.snapshot` | 抄 cc 只靠 transcript 不稳。 | 重开聊天：先认指针，再认文件，最后翻聊天备份。 |
| T18 旧清单清理 | `new_todos` 激活后 **`purge_inactive_todos`**：同 `session_key` 只留当前 `.todo.md`；`todosIds` 同步为 `[activeTodosId]` | 多文件 archived 堆目录、指针与磁盘不一致。 | 换新任务盘：新文件生效后，同会话其它待办文件全删；目录里只留最新一份。 |
| T19 提示词载体 | **仅** `todos` 的 JSON **`description`**（+ 参数 field `description`）；**不**单独挂 100+ 行 `prompt()` | cc-fork 超长 PROMPT 难缓存、难随 mode 演进 | 行为写进 tool schema，方便 catalog 按 mode 下发。 |
| T20 何时用清单 | 浓缩 cc-fork：**≥3 步**、用户给多任务、用户点名要 todo、开干前标 `in_progress` | 无触发规则则模型从不建清单 | 该建清单时有明确条件，别啥都建。 |
| T21 何时不用 | 单步 trivial、纯问答、Planning（改 `create_plan`） | 一律强制 todo 拖慢简单任务 | 简单活直接干，规划别调 `todos`。 |
| T22 工具结果反馈 | 成功返回后附 **一句** 执行提醒（可选，对齐 cc-fork tool result） | 过长 tool result 占上下文 | 改完待办提醒继续用清单，别丢进度。 |

### 4.2 实施点（拟定）

> 与 [`plan-runtime.md`](../plan-runtime.md) **PR-PLD** / **PR-PLE** 对齐；当前代码 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **TD-A** | `todos` catalog：**所有模式可见**；`tool_exec` 仅校验 `apply_todos_op` 通用约束（store 永远是 `SessionTodoStore`）；**交付**：catalog 条目 + §5.1 精简 description | `catalog.rs`、`tool_exec.rs` | §11：`todos_visible_in_all_modes`、`todos_never_writes_plan_file`（PENDING） | 任何模式都给模型 `todos`；只写 session 路径。 |
| **TD-B** | `apply_todos_op(scope)` + `session_store` / `file_store` 双 store；**交付**：`TodoScope`、`TodosOp` | `plan_runtime/{mod.rs,session_store.rs,file_store.rs}`（拟定） | §11：`session_todos_file_round_trip`、`file_store_single_write_path`（PENDING） | 改聊天清单和改 plan 各走各的写入流程。 |
| **TD-F** | `todos/<todos_id>.todo.md` + `SessionEntry.activeTodosId`（§3.4.2）；`new_todos` + `purge_inactive_todos`（§3.4.6）；`todos.snapshot` | `todos_store.rs`、`sessions.json` 扩展 | §11：`todos_file_round_trip`、`multi_todos_active_anchor`、`new_todos_purges_inactive_files`、`resume_prefers_sessions_pointer`（PENDING） | 换新盘：写新文件、删同会话旧文件、更新指针，一步做完。 |
| **TD-C** | TodosPanel 投影 + `plan.todos` transcript snapshot（250ms 节流）；**交付**：`PlanPanelState` / `TodoItemView` | `src/api/chat/plan_runtime/panel.rs`（拟定） | 见 §11：`panel_snapshot_is_throttled`、`todos_tool_updates_plan_file_and_panel`（PENDING） | 界面跟着文件变；聊天记录别记太密。 |
| **TD-D** | panel 通过 `task_id` 弱绑定 `BashTaskRegistry::get_summary`；**交付**：`BashSummaryRef` 摘要结构 | `panel.rs` + 既有 `src/core/bash_task_registry.rs` | 见 §11：`todos_panel_reflects_bash_task_status`（PENDING） | 侧边栏显示命令输出时，只记任务 id，去 Registry 现查，不自己存状态。 |
| **TD-E** | milestone 全 `completed` 时 `CheckpointStore::record(Milestone{...})`（warning-only 失败）；**交付**：`on_milestone_completed` hook | `src/api/chat/plan_runtime/checkpoint.rs`（拟定）、只读 `src/core/checkpoint/store.rs` | 见 §11：`milestone_completion_can_record_checkpoint`（PENDING） | 一个阶段全做完，可自动打代码快照。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 4.2.1 TD-A：`todos` 注册与 catalog（D 方案：全模式可见）

- **交付**：`visible_tools_for_mode`：**所有模式**注入 `todos`（Chat / Planning / Executing / Completed / Pending）。
- **scope**：`todos` 永远走 `SessionTodoStore` → 写 `~/.tomcat/agents/<agentId>/todos/<todos_id>.todo.md`；**不再**有 `active_scope=plan` 分支。
- **op 约束**：通用 `apply_todos_op` 校验（同一文件最多一个 `in_progress`、id 唯一、未知 id 拒绝等）；与 mode 无关。
- **返回值**：`ToolResult` 始终包含完整 items snapshot；LLM 不必读文件。

```text
  current_mode() ──▶ todos 任何模式可见
        │
        ▼
  tool_exec.todos：
        │
        ▼
  apply_todos_op(SessionTodoStore, ops)
        │
        ▼
  ToolResult { items: <full snapshot>, applied, panel_snapshot_id, scope: "session" }
```

**说人话**：所有模式都能用 `todos`；它永远只写自己的 `.todo.md`，不动 plan。改 plan 内待办用 [`update_plan`](./update-plan.md)；整盘重写用 [`create_plan`](./create-plan.md)。

#### 4.2.2 TD-B：`apply_todos_op` 单写通道

- **交付**：`apply_todos_op(ops, replace)` 在 **同一把** advisory file lock 下顺序执行：校验（单 `in_progress`、id 存在性）→ `PlanRuntimeState` patch → `file_store::write_plan`（frontmatter `todos` + 正文 `## Todos Board` 重写）→ 失败则整批回滚。
- **并发**：同进程 `tokio::Mutex` 串行；跨进程靠 `<plan>.lock` + `TOMCAT_TODOS_FILE_LOCK_TIMEOUT_MS`。

```text
  tool_exec::todos
        │
        ▼
  validate(ops) ──fail──▶ ToolError（整批回滚）
        │
        ▼
  acquire_lock(plan_path)
        │
        ▼
  patch(state) → write(PlanFile) → release_lock
        │
        ▼
  panel.schedule_snapshot()   // 节流，见 TD-C
```

**说人话**：校验、改内存、写文件在同一把锁里做完，半路失败就整批撤销。

#### 4.2.3 TD-C：TodosPanel 与 transcript snapshot

- **交付**：`panel.rs` 从 `PlanRuntimeState` 投影 `TodoItemView[]`；节流窗口内多次 `apply_todos_op` 合并为一条 `plan.todos` 自定义事件（含 `snapshot_id` / `todos` / `bash_summary`）。
- **非目标**：panel **不**维护独立 todos 数组副本为 source of truth。

```text
  apply_todos_op 成功
        │
        ▼
  panel.throttle(250ms)
        │
        ▼
  emit transcript plan.todos  ──fail──▶ warning-only（不挡 tool 返回）
```

**说人话**：改待办成功后界面排队刷新，250ms 内多次修改在聊天记录里也只记一条备份。

#### 4.2.4 TD-Bash：TodosPanel 与 `BashTaskRegistry` 弱绑定

- **交付**：`PlanPanelState.bash_summary` 仅持 `task_id`；渲染时 `BashTaskRegistry::get_summary(task_id)` 取尾行（默认 3 行）；registry 无此 id → warning + 摘要 `unknown`。
- **节奏**：bash 状态变化**不** push panel；以 todos 写动作为重绘节拍。

**说人话**：侧边栏要显示「这条 todo 关联的命令跑得怎样」时，只保存一个 **bash 任务编号**；每次刷新界面时，拿这个编号去问 **BashTaskRegistry**「现在是在跑、跑完了还是失败了、最后几行输出是什么」。**侧边栏自己不维护**「命令是否在跑」——避免和 Registry 各记一套、以后对不上。没有对应任务就显示 unknown，不挡改待办。

#### 4.2.5 TD-E：Milestone 派生态与 checkpoint hook

- **交付**：`recompute_milestones()` 在每次 `apply_todos_op` 后运行；`on_milestone_completed` 仅 `scope==plan`；见 §3.3.2。
- **约束**：Executing 禁止 `upsert` 新 `milestone_id`；`create_plan` 负责里程碑草案。
- **边界**：`todos` **不**暴露 `take_checkpoint`（对齐 [`checkpoint-resume.md`](./checkpoint-resume.md)）。

**说人话**：阶段是否完成看下属 todo 是否都勾完；全完且开着自动打快照才写 checkpoint；**只有 plan 里的阶段**会触发，普通聊天清单不会。

#### 4.2.6 TD-F：TodoFile、清理与续跑

- **交付**：`todos_store::{create,read,write,delete}`；`purge_inactive_todos(session_key, keep_todos_id)`（§3.4.6）；路径 `todos/<todos_id>.todo.md`；`new_todos` 与 `sessions.json` 更新 **同事务**（失败整批回滚）。
- **new_todos**：`todos { new_todos: true }` 或 T3 自动规则（§3.4.4）；成功后目录内该 `session_key` **仅**剩当前文件。
- **recover**（§2.5）：`active_todos_id` → 读文件 → fallback 扫 `todos.snapshot` → 空板。
- **transcript**：每次成功写盘 emit `todos.snapshot`（`todos_id`, `todos[]` 摘要），供删盘后的历史线索。

**说人话**：一份待办一个 md；换新盘时写新的、删掉同会话旧的、更新指针，三件事绑在一起；重启先认指针和文件，没有再看聊天记录备份。

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 工具 JSON Schema

```json
{
  "name": "todos",
  "description": "Track a session-local todo list (a personal scratchpad you can keep across tool calls).\n\nWhen to use: any multi-step work (3+ distinct steps), multiple user tasks, or whenever you want a checklist to keep yourself organized across turns. Mark one item in_progress before starting it; mark it completed as soon as it's done.\n\nWhen NOT to use: single trivial step or pure Q&A.\n\nReturn value: every successful call returns a full items snapshot under `items` (id/content/status/milestone_id). You do NOT need to re-read the file to know the current state.\n\nRules: stable id per item; status in pending|in_progress|completed|cancelled; at most one in_progress at any time; use ops (upsert/set_status/remove) or replace=true for full list replacement. new_todos=true creates a new file, activates it, and deletes other todo files belonging to this session.",
  "parameters": {
    "type": "object",
    "properties": {
      "new_todos": {
        "type": "boolean",
        "description": "If true, create a new todos_id, activate it, delete other TodoFiles for this session_key, then apply ops. Default false."
      },
      "title": {
        "type": "string",
        "description": "Optional title for new_todos (stored in .todo.md frontmatter)."
      },
      "ops": {
        "type": "array",
        "description": "Sequence of mutations applied in order under a single file lock.",
        "items": {
          "type": "object",
          "properties": {
            "kind": {
              "type": "string",
              "enum": ["upsert", "set_status", "remove"],
              "description": "upsert = create or replace by id; set_status = mutate status only; remove = mark cancelled (soft delete)"
            },
            "id":          { "type": "string" },
            "content":     { "type": "string" },
            "status":      { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] },
            "milestone_id":{ "type": "string" }
          },
          "required": ["kind", "id"]
        }
      },
      "replace": {
        "type": "boolean",
        "description": "If true, the entire todos array is replaced by `ops` upsert results. Default false."
      }
    },
    "required": ["ops"]
  }
}
```

### 5.2 出参

```jsonc
{
  "type": "object",
  "properties": {
    "applied":           { "type": "integer" },           // 实际生效操作数
    "active_in_progress":{ "type": "string", "nullable": true }, // 当前唯一 in_progress 的 id
    "todos_id":          { "type": "string" },            // 当前 TodoFile id
    "scope":             { "type": "string", "enum": ["session"] }, // D 方案：永远 session
    "items": {                                             // ★ 完整 items snapshot（LLM 直接消费，不必再读文件）
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id":           { "type": "string" },
          "content":      { "type": "string" },
          "status":       { "type": "string", "enum": ["pending","in_progress","completed","cancelled"] },
          "milestone_id": { "type": "string", "nullable": true }
        },
        "required": ["id", "content", "status"]
      }
    },
    "panel_snapshot_id": { "type": "string" },            // transcript：plan.todos 或 todos.snapshot
    "warnings":          { "type": "array", "items": {"type":"string"} }
  },
  "required": ["applied", "scope", "items", "panel_snapshot_id"]
}
```

**说人话**：模型传一批操作（增改状态/软删），也可 `replace` 整表替换；返回改了几条、当前哪条是「进行中」、**当前文件的全量 items**（id/content/status/milestone_id 都给到）以及界面刷新用的快照编号。LLM 拿到这个结果就能立刻继续推进，**不必再 `read`**一遍 `.todo.md` 或 `.plan.md`。

### 5.3 调用前置条件与 op 门控矩阵

| 条件 | 处理 | 说人话 |
|------|------|--------|
| 任意 mode | catalog 可见；走 `SessionTodoStore` 写 active `TodoFile` | 全模式都能用，永远写 session 文件。 |
| 同一文件两个 `in_progress` | tool error；整批回滚 | 一个 .todo.md 只能一个在干。 |
| 未知 `id` | tool error | 不能瞎编 id。 |
| `replace=true` 且 ops 含非 `upsert` op | tool error | 整表替换只接收 upsert。 |

> 写 plan 内 todos / milestones？请用 [`update_plan`](./update-plan.md)；整盘重写计划？请用 [`create_plan`](./create-plan.md)（PLAN 模式专属）。`todos` 不参与这两件事。

---

## 6. One-Glance Map

| 路径 | 职责 | 说人话 |
|------|------|--------|
| `src/api/chat/plan_runtime/catalog.rs`（拟定） | 在 `current_mode()` 返回 `Executing` 时把 `todos` 注入 LLM 可见集；其余 mode 剔除 |
| `src/api/chat/plan_runtime/tool_exec.rs`（拟定） | `todos` 入口；校验 `mode == Executing`；调用 `apply_todos_op` |
| `src/api/chat/plan_runtime/{file_store,todos_store}.rs`（拟定） | plan 文件 + `todos/*.todo.md`；advisory lock |
| `src/api/chat/plan_runtime/panel.rs`（拟定） | TodosPanel 投影；节流；snapshot 到 transcript |
| `src/core/bash_task_registry.rs`（既有） | bash 任务真实状态；TodosPanel 通过 `task_id` 引用 |
| `src/core/checkpoint/...`（既有） | milestone 完成回调挂接的 checkpoint hook | 里程碑完成时尝试 checkpoint。 |

**阅读顺序（说人话）**：`catalog` 决定当前模式能不能看见 `todos` → `tool_exec` 校验后调用 `apply_todos_op` → `file_store` 写入磁盘 → `panel` 刷新界面并节流写一条 `plan.todos` 聊天事件 → shell 是否在跑只问 `BashTaskRegistry`，不问面板自己。

---

## 7. TodosPanel：UI 投影协议

### 7.1 责任边界

TodosPanel **只是** `PlanRuntimeState.todos` + `BashTaskRegistry` 摘要的投影；它**不**：

- 维护独立的 todos 数组副本（投影时按需拷贝）；
- 维护 bash 进程生命周期（调用 `BashTaskRegistry::get_summary`）；
- 写 `PlanFile`（写归 `apply_todos_op` 单通道）；
- 决定 `current_mode()`（mode 由 `PlanRuntime` 决定）。

**说人话**：TodosPanel 是**显示屏**：待办勾到哪了，从 `.todo.md` 或 `plan.md` 读出来显示；命令跑得怎样，从 BashTaskRegistry 读出来显示。它**不**自己存待办、**不**自己管 bash 进程、**不**写文件，也**不**决定现在是规划态还是执行态。

### 7.2 渲染数据结构

```rust
pub struct PlanPanelState {
    pub plan_id:            String,
    pub goal:               Option<String>,
    pub todos:              Vec<TodoItemView>,    // 投影自 PlanRuntimeState.todos
    pub bash_summary:       Vec<BashSummaryRef>,  // 通过 BashTaskRegistry 获取
    pub last_milestone:     Option<MilestoneId>,
    pub last_snapshot_id:   String,               // transcript 中对应的 plan.todos snapshot id
}

pub struct TodoItemView {
    pub id:           String,
    pub content:      String,
    pub status:       TodoStatus,
    pub milestone_id: Option<String>,
}

pub struct BashSummaryRef {
    pub task_id:   String,        // 由 BashTaskRegistry 颁发
    pub status:    BashStatus,    // running / done / failed
    pub stdout_tail_lines: u32,   // 摘要行数（默认 3）
}
```

### 7.3 节流与 transcript snapshot

- 节流窗口 `PANEL_SNAPSHOT_THROTTLE_MS = 250`：在窗口内多次 `apply_todos_op` 只产生 1 条 `plan.todos` 事件。
- snapshot 事件 schema：

```jsonc
// transcript 自定义事件 plan.todos
{
  "type": "plan.todos",
  "snapshot_id": "<uuid_v7>",
  "plan_id":     "<plan_id>",
  "todos":       [/* TodoItemView[] */],
  "bash_summary":[/* BashSummaryRef[] */],
  "ts":          "<rfc3339>"
}
```

- snapshot 与 [`session-storage.md`](../session-storage.md) 中的 transcript 自定义事件协议对齐；同步落盘失败 → warning-only，不阻塞 tool 返回。

### 7.4 与 BashTaskRegistry 的弱绑定

> **原先那句「面板只看 bash 摘要，不另存一份状态」是什么意思？**
>
> - **面板**：侧边栏 TodosPanel，用来展示待办和关联命令的进度。
> - **bash 摘要**：若某条 todo 挂着后台 shell，只显示「在跑 / 完了 / 失败了」以及**最后几行输出**，不塞整段日志。
> - **不另存一份状态**：面板和 `.todo.md` **都不**再记 pid、退出码、完整 stdout——那些由 **`BashTaskRegistry`** 统一管。每次刷新界面时，用 todo 上记的 **`task_id` 去问 Registry**；Registry 里没有就显示 unknown，照样能改待办。
>
> 一句话：**命令跑成什么样，只信 Registry；面板只负责「现查现画」，不当第二本账。**

- 引用：仅持 `task_id`，不持任务对象指针。
- 摘要刷新：每次 panel 渲染调用 `BashTaskRegistry::get_summary(task_id)`，registry 内部按 mtime 节流；registry 不存在该 task_id 时记录 warning，不阻塞渲染。
- 反向通知：bash 任务状态变化**不**主动 push panel；panel 重绘以 todos 写动作为节奏。

**说人话**：你连续改好几次待办，界面最多每 250ms 刷新一次；聊天记录里也只记一条备份，免得刷屏。旁边若挂着 shell 任务，显示的是 Registry 里查到的最后几行输出；查不到就提示一下，照样能改待办。

---

## 8. 状态机与并发约束

### 8.1 单进行中状态机

```
                ┌─────────────┐
                │   pending   │
                └──────┬──────┘
                       │ set_status: in_progress
                       │ (need: no other in_progress)
                       ▼
                ┌─────────────┐
                │ in_progress │
                └──┬───────┬──┘
   set_status:    │       │   set_status:
   completed      │       │   cancelled
                  ▼       ▼
          ┌───────────┐ ┌───────────┐
          │ completed │ │ cancelled │
          └───────────┘ └───────────┘
```

**说人话**：pending 只能有一个变 in_progress；做完变 completed，放弃变 cancelled。

### 8.2 并发写入约束

| 场景 | 处理 | 说人话 |
|------|------|--------|
| 同一进程并发 `apply_todos_op` | tokio Mutex 串行化；批次内顺序应用 ops | 同进程排队写。 |
| 跨进程并发 | advisory file lock（`fs2::FileExt`）；获取失败 → tool error | 跨进程靠文件锁挡。 |
| 写入磁盘后崩溃 | 启动 `recover()` 重读 frontmatter 校验合法性，详见 [`create-plan.md`](./create-plan.md) §5 | 崩了重启靠 recover 修。 |

---

## 9. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_TODOS_PANEL_THROTTLE_MS` | `250` | TodosPanel snapshot 节流窗口 | 面板快照别刷太勤。 |
| `TOMCAT_TODOS_BASH_TAIL_LINES` | `3` | TodosPanel bash 摘要保留尾行数 | bash 尾行默认 3 行。 |
| `TOMCAT_TODOS_FILE_LOCK_TIMEOUT_MS` | `2000` | 计划文件 advisory lock 等待上限 | 等锁最多 2 秒。 |
| `[todos] purge_inactive_on_new_todos` | `true` | `new_todos` 激活后删除同 `session_key` 其它 `.todo.md`（§3.4.6） | 换新盘就删旧文件。 |

---

## 10. 错误模型 / 截断 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| 任意模式调 `todos` 写 plan 路径 | tool error；提示用 `update_plan` | todos 不写 plan。 |
| 两个 `in_progress` | tool error；整批 ops 回滚 | 不能两个同时在干。 |
| 未知 `id`（`set_status`/`remove`） | tool error，列出未知 id | id 必须已存在。 |
| `replace=true` 且 `ops` 为空 | tool error | 不许空替换清空表。 |
| advisory lock 获取失败 | tool error，附 lock holder pid（如可读取） | 别人占锁就报错。 |
| panel snapshot 写失败 | warning；`applied` 仍为成功 | 快照失败不挡改 todos。 |
| `BashTaskRegistry::get_summary` 找不到 task_id | warning；摘要置 `unknown` | 没 bash 任务也照常显示。 |
| `purge_inactive_todos` 删盘失败 | tool error；`new_todos` 整批回滚 | 旧文件删不干净就不算换新盘成功。 |

---

## 11. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：catalog 可见集 | `api::chat::plan_runtime::tests::catalog_visible_set_by_current_mode`（待新增） | PENDING | 模式不对就别让模型看见 todos。 |
| 单元：单进行中 | `api::chat::plan_runtime::tests::todos_state_enforces_single_in_progress`（待新增） | PENDING | 两个进行中要硬拒。 |
| 单元：规划态拒绝 | `todos_tool_rejected_in_planning`（待新增） | PENDING | 规划态调要直接 error。 |
| 单元：TodoFile round-trip | `todos_file_round_trip`（待新增） | PENDING | `.todo.md` 能读写。 |
| 单元：多份清单指针 | `multi_todos_active_anchor`（待新增） | PENDING | 只改当前 active 文件。 |
| 单元：new_todos 清理 | `new_todos_purges_inactive_files`（待新增） | PENDING | 激活新盘后，同 session 旧 `.todo.md` 必须删光。 |
| 单元：续跑优先级 | `resume_prefers_sessions_pointer_over_transcript`（待新增） | PENDING | 先 sessions.json 再 snapshot。 |
| 单元：milestone 仅 create_plan 创建 | `milestones_only_via_create_plan`（待新增） | PENDING | Executing 禁新 milestone。 |
| 单元：Executing 禁新 milestone | `executing_cannot_add_milestone`（待新增） | PENDING | 执行期不能加阶段。 |
| 单元：file_store 单通道 | `api::chat::plan_runtime::tests::file_store_single_write_path`（待新增） | PENDING | 旁路写必须挡。 |
| 单元：panel 节流 | `api::chat::plan_runtime::tests::panel_snapshot_is_throttled`（待新增） | PENDING | 250ms 内多次写只一条 snapshot。 |
| 集成：todos / file / panel 同步 | `tests/plan_runtime_integration_tests.rs::todos_tool_updates_plan_file_and_panel`（待新增） | PENDING | 改 todos 不能只改一处。 |
| 集成：bash 摘要 | `tests/plan_runtime_integration_tests.rs::todos_panel_reflects_bash_task_status`（待新增） | PENDING | 面板要能看见 bash 任务摘要。 |
| 集成：里程碑 checkpoint | `tests/plan_runtime_integration_tests.rs::milestone_completion_can_record_checkpoint`（待新增） | PENDING | 全部完成可触发 checkpoint hook。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 模型在执行态外强行调 `todos` | 中 | catalog 过滤 + tool_exec 兜底校验 | 双重防线。 |
| 模型一次性 `replace=true` 把 todos 清空 | 高 | `replace=true` 且 `ops` 为空直接 error | 不允许误清空。 |
| `new_todos` 误删上一盘未导出内容 | 中 | 仅 `new_todos` / 终态自动换新盘触发；删前依赖 transcript `todos.snapshot` | 换新盘前重要内容应已在聊天备份里。 |
| advisory lock 卡死 | 中 | `TOMCAT_TODOS_FILE_LOCK_TIMEOUT_MS` 可配 | 拿不到锁就报，不死等。 |
| TodosPanel 节流过紧导致用户延迟看到状态 | 低 | 250ms 默认值；`TOMCAT_TODOS_PANEL_THROTTLE_MS` 可调 | 可调。 |
| bash 摘要泄漏长 stdout | 中 | 默认 3 行尾；`TOMCAT_TODOS_BASH_TAIL_LINES` 可调 | 默认窄。 |

---

## 13. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把 `todos` 改名留作 `todo`（单数）~~ | **替代**：复数 `todos` 与 cc-fork-01 / hermes / pi-mono 默认命名一致。 | 跟生态对齐。 |
| ~~允许多个 `in_progress`~~ | **否**：硬上限 1。 | 多就乱套。 |
| ~~`todos` 直接调 `take_checkpoint`~~ | **否**：checkpoint 不进 catalog；由 `PlanRuntime` 在 milestone 完成回调中触发。 | 模型别管快照。 |
| ~~TodosPanel 自管 bash 进程状态~~ | **否**：复用 `BashTaskRegistry`，只持 `task_id` 与摘要缓存。 | 不重造状态。 |
| ~~PlanFile frontmatter 与 panel 各持一份 todos~~ | **否**：单写通道；frontmatter 是 source of truth，panel 只读盘显示。 | 计划文件和界面各记一套待办，迟早对不上。 |
| ~~`todos` 仅 Executing 可见~~ | **再迭代**：先一度改为 `Chat / Executing / Completed / Pending` 可见、Planning 剔除；**最终 D 方案**改为**所有模式**可见且只写 session（plan 内 todos 拆给 `update_plan`）。 | 任何模式都给个本地小白板。 |
| ~~`todos.active_scope ∈ {session, plan}` 双轨~~ | **D 方案下线**：`todos` 永远写 session；plan 内 todos 由 [`update_plan`](./update-plan.md) 管。op 引擎仍共享代码。 | 工具职责单一；代码继续复用。 |
| ~~`/plan build` 切 `active_scope = plan`，让 `todos` 写 plan.md~~ | **下线**：build 不再切 scope；EXEC 期推进 plan 用 `update_plan`。 | `todos` 不参与 EXEC 写盘。 |
| ~~PLAN 模式下用户要求改 todos 必须再次调 `create_plan` 整盘重写~~ | **替代**：任何模式都用 [`update_plan`](./update-plan.md) 增量改；`create_plan` 仅当结构大改时用。 | 小修不必整盘重写。 |
| ~~mode=completed 自动派生由 `todos` 触发~~ | **替代**：由 [`update_plan`](./update-plan.md) 在 EXEC 模式 + target.mode==executing + 全 completed 时触发。 | 改 plan 的工具负责派生 mode。 |
| ~~`sessions/<session_id>.todos.md` 1:1~~ | **替代**：`todos/<todos_id>.todo.md` + `active_todos_id`（§3.4）。 | 用 todos_id 命名；指针在 sessions.json。 |
| ~~`new_todos` 后旧板 `archived` 留目录~~ | **替代**：`purge_inactive_todos` 物理删除（§3.4.6、T18）。 | 磁盘只留当前一份；历史看 transcript。 |
| ~~session todo 纯内存~~ | **否**：TodoFile 文件 + transcript snapshot 备份（§2.5）。 | 聊天待办也落盘，重启比 cc 只扫聊天记录稳。 |
| ~~Executing 可建 milestone~~ | **否**：仅 `create_plan` 定义/修订（§3.3.0）。 | 阶段在规划定稿。 |
| ~~`/plan apply` 自动 merge session todos 进 plan~~ | **默认否**；`import_session_todos_on_build=false`；命令本身已 rename 为 `/plan build`。 | 未审条目不污染计划；apply→build 后语义不变。 |
| ~~PlanFile mode 保留 `ReadyToApply` / `cancelled`~~ | **否**：state 收敛为 `planning / executing / completed / pending`；reviewer 不当 gate，cancel 走 `pending` 续跑（详见 [plan-runtime.md §13](../plan-runtime.md#13-历史决策) `H1`–`H3`）。 | 状态机简化；中断变 pending，可续跑。 |
| ~~`/plan close`、`/plan show`~~ | **下线**：close 由 runtime 派生 `completed` / `pending` 自动完成；show 暂缓（CLI 可读 `.plan.md`）。 | 少一条命令，少一条状态。 |
| ~~`review_status` / `last_review` / `apply_changes` 暴露到 frontmatter / 工具入参~~ | **否**：reviewer 摘要仅落 `transcript.plan.review`；reviewer 改稿权改为 runtime 内部参数（`allow_review_edit`，作用域仅限 `## Review` 段）。 | 用户不感知，程序内部传参。 |
| ~~`org_session_key` / `org_session_id`~~ | **改为**：`session_key` / `session_id`，**`/plan build` 时**才写入；规划期不绑 session。 | 计划在未开干前不绑某个 session，开干时再溯源。 |
| ~~PlanFile `last_checkpoint_id` / `updated_at` / `active`~~ | **下线**：checkpoint 由 LLM 自行控制，时间戳与活跃指针只制造 git diff 噪声；TodoFile 同步去掉 `updated_at`。 | frontmatter 越精简越好维护。 |
| ~~`todos` 返回值不带 items snapshot，LLM 自行 `read` 拿状态~~ | **否**：`ToolResult` 内带完整 items snapshot（§5.2、§7）。 | 工具结果一次给全貌，少一次 `read`。 |
| ~~cc-fork 180 行 TodoWrite PROMPT 原样搬进 catalog~~ | **否**：§2.6 择优 **T19–T22**；§5.1 浓缩 description + mode 门控 | 太长难维护，Planning 语义也对不上。 |
| ~~行为只靠 system prompt 不写 tool description~~ | **否**：学 hermes，行为进 schema（Planning 仍靠 `create_plan` description） | 纯 system 注入难审计、难按 mode 切换。 |

---

## 14. 关联文档

- 运行时编排：[plan-runtime.md](../plan-runtime.md)
- 写入计划文件协议（整盘重写）：[create-plan.md](./create-plan.md)
- 写入计划文件协议（增量编辑）：[update-plan.md](./update-plan.md)
- PLAN 模式整体规范：[planner.md](./planner.md)
- 子 Agent 基础设施：[multi-agent.md](../multi-agent.md)
- 标杆写法：[read.md](./read.md)
- 任务卡：[T2-P1-002.md](../../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)
- checkpoint 底座：[checkpoint-resume.md](./checkpoint-resume.md)
- Cursor 轻路参考：[cursor-todo-tool-execution-no-review.md](../../reports/cursor-todo-tool-execution-no-review.md)
- Cursor 内置工具枚举（无长 description）：[cursor-builtin-tools-reference.md](../../reports/cursor-builtin-tools-reference.md) §9
- cc-fork TodoWrite 提示词原文：`cc-fork-01/src/tools/TodoWriteTool/prompt.ts`
- 会话目录布局：[session-storage.md](../session-storage.md)、[work-dir-and-data-layout.md](../work-dir-and-data-layout.md)

**说人话**：会话本地待办看 §3.2 的 `todos/xxx.todo.md`；改 plan 内待办与阶段看 [`update-plan.md`](./update-plan.md)；整盘重写计划看 [`create-plan.md`](./create-plan.md)；整条流程看 [`plan-runtime.md`](../plan-runtime.md)。
