# `todos` 工具：会话级 TodoFile 与 TodosPanel

本文档是内置工具 **`todos`** 的冻结版技术方案（OpenSpec **B 类**：`docs/architecture/tools/`）。承接 [`plan-runtime.md`](../plan-runtime.md) 的运行时编排，描述 `todos` 在 D 方案下的最终职责：**它只管理 session-local 的 TodoFile scratchpad，不再写 PlanFile**。**实现以仓库代码为准**；本文只保留当前代码已经采用的行为与契约。

> **重要**：`todos` **仅**写 `~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md`。要改 plan 文件 frontmatter 里的 `todos[]`，请用 [`update_plan`](./update-plan.md)；要整盘重写计划，请用 [`create_plan`](./create-plan.md)。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：`todos` 是给当前会话用的本地小白板。任何模式都能用，适合模型把多步工作拆成一份 scratchpad 清单；它不会碰 `plan.md`，也不会影响 `/plan build` 的状态机。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 与 `update_plan` / `create_plan` 的分工](#2-与-update_plan--create_plan-的分工)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 运行时行为](#4-运行时行为)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. 状态机与并发约束](#6-状态机与并发约束)
- [7. TodosPanel：UI 投影协议](#7-todospanelui-投影协议)
- [8. 配置与环境变量](#8-配置与环境变量)
- [9. 测试矩阵（验收）](#9-测试矩阵验收)
- [10. 历史决策](#10-历史决策)
- [11. 关联文档](#11-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **TodoItem** | 单个最小执行步骤 | `TodoItem { id, content, status }` | `id` 在同一份 TodoFile 内唯一；`status ∈ {pending, in_progress, completed, cancelled}`；同一文件最多一个 `in_progress` | 一条待办，且一次只能真正在做一条。 |
| **TodoFile** | 一份 session-local scratchpad 文件 | `~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md` | frontmatter 记录 `todos_id` / `session_id` / `title?` / `created_at` / `schema_version`；正文固定 `## Todos` 列表 | 一份清单一个文件。 |
| **`active_todos_id`** | 当前 session 正在使用的 scratchpad 逻辑 id | `PlanRuntime.active_todos_id`（仅内存） | 首次调用时自动生成；`new_todos=true` 时显式切换到新 id；**不参与文件命名** | 当前会话“正在用哪块白板”的内存指针。 |
| **`new_todos`** | 清空当前 scratchpad 并重新开始 | `todos` 顶层布尔入参 | 以空列表作为初始状态，再应用本次 `ops`；同一 `session_id` 文件被覆盖写 | 换一块新白板，但文件路径不变。 |
| **`replace`** | 用一批 `upsert` 重写整表 | `todos` 顶层布尔入参 | 仅接受 `upsert`；`ops` 为空或含 `set_status` / `remove` 直接报错 | 整表重写模式。 |
| **`title`** | TodoFile 的可选标题 | `.todo.md` frontmatter `title` | 仅在持久化路径启用时写盘；通常与 `new_todos` 一起用 | 给这块白板起个名字。 |
| **TodosPanel** | UI 上的 todo 投影 | `TodosPanelSnapshot { panel_snapshot_id, scope, items, warnings }` | 只读快照，不是 source of truth；`scope` 对 `todos` 固定为 `session` | 侧边栏 / CLI 面板看到的是投影，不是另一份状态。 |

---

## 2. 与 `update_plan` / `create_plan` 的分工

| 工具 | 写什么 | 模式可见性 | 用来做什么 | 说人话 |
|------|--------|-----------|-----------|--------|
| **`todos`** | `TodoFile`（session 路径） | **任何模式** | 会话级 scratchpad、多步执行清单、调研步骤记录 | 给当前会话自己记事。 |
| **`update_plan`** | `PlanFile.frontmatter.todos[]` | **任何模式** | 推进或修订某个 plan 的待办 | 改计划文件里的待办。 |
| **`create_plan`** | `PlanFile` 整盘（frontmatter 初稿 + 正文） | **仅 PLAN 模式** | 起草或重写一整份计划 | 重写整盘计划。 |

```text
要改什么？
  │
  ├─ 会话里的个人 scratchpad / 调研步骤 / 多步任务清单 ──▶ todos
  │
  ├─ plan.md frontmatter 里的 todos[] ──▶ update_plan
  │
  └─ 整个 plan 的 goal / draft / todos 结构大改 ──▶ create_plan
```

**说人话**：同样都是“待办”，但载体不同。`todos` 写自己的 `.todo.md`；`update_plan` 写计划文件；`create_plan` 则是整盘重写。

---

## 3. 目标与设计原则

| ID | 目标 | 说人话 |
|----|------|--------|
| G1 | `todos` 在所有模式都可见 | 任何时候都可以先给自己列个执行清单。 |
| G2 | `todos` 永远只写 session TodoFile，不写 PlanFile | 工具职责单一，不和 `update_plan` 重叠。 |
| G3 | 与 `update_plan` 共享同一套 todo-op 语义（`upsert` / `set_status` / `remove`） | LLM 在两个工具间切换时不用重新学习一套 op。 |
| G4 | `new_todos` / `title` / `replace` 只影响 session scratchpad | 新建白板、命名白板、整表替换都只发生在本地清单。 |
| G5 | 同一份 TodoFile 最多一个 `in_progress` | 避免清单失真。 |
| G6 | 成功调用总是返回完整 `items` 快照 | LLM 无需再 `read` `.todo.md` 才知道当前状态。 |
| G7 | 持久化是增强项，不是主流程前提 | 即使磁盘异常，内存态也能继续推进。 |
| G8 | TodosPanel 只消费 `panel_snapshot_id + items` 快照 | UI 不额外保留第二份“真相”。 |

**说人话（§3 总览）**：D 方案把 `todos` 收敛成一个轻量、稳定、全模式可见的 session 工具。它既能帮模型做多步工作管理，又不会和 plan 状态机互相污染。

---

## 4. 运行时行为

### 4.1 持久化路径与文件格式

当 `ChatContext` 注入 `TodosRuntime` 时，`todos` 会把当前 session 清单持久化到：

```text
~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md
```

磁盘格式由 [`todo_runtime.rs`](../../../src/api/chat/plan_runtime/todo_runtime.rs) 定义：

```yaml
---
todos_id: td_12345678
session_id: 1747656000000_abcd1234
title: research-notes        # 可选
created_at: 2026-05-19T20:00:00+08:00
schema_version: 1
---

## Todos

- [ ] t1: inspect API surface
- [~] t2: run focused regression
- [x] t3: update docs
- [-] t4: drop abandoned idea
```

checkbox 与状态的映射：

- `[ ]` = `pending`
- `[~]` = `in_progress`
- `[x]` = `completed`
- `[-]` = `cancelled`

### 4.2 `new_todos` 与 `active_todos_id`

`todos` 每次执行都走 session 路径：

1. `new_todos=false`：读取当前 in-memory session todos，再应用 `ops`。
2. `new_todos=true`：从空列表开始应用 `ops`，并调用 `rotate_active_todos_id()` 切到一块新的内存白板。
3. 若当前 session 尚无 active id，则调用 `ensure_active_todos_id()` 生成 stable id。

换句话说，`new_todos` 的语义不是“新建另一个磁盘文件”，而是“清空当前 scratchpad 并覆盖写同一个 `<session_id>.todo.md`”。

### 4.3 `replace=true`

`replace=true` 表示整表替换，但它仍然走共享的 todo-op 引擎，约束如下：

- `ops` 必须全部是 `kind = "upsert"`；
- `ops` 不能为空；
- 替换后的整表仍要满足“最多一个 `in_progress`”。

这保证 `todos` 和 `update_plan` 在“重写一组待办”这件事上的协议一致。

### 4.4 单文件覆盖语义

`todos` 已经改成“**每个 session 只有一个磁盘文件**”模型，因此不存在旧文件清理：

- `new_todos=false`：覆盖写 `todos/<session_id>.todo.md` 的最新内容；
- `new_todos=true`：仍然覆盖写**同一个** `todos/<session_id>.todo.md`，只是内存 scratchpad 先清空再应用本次 `ops`；
- 不再有 `purge_inactive()` / 多文件枚举 / 按 `session_key` 过滤删除。

**说人话**：现在磁盘上天然只有这一份文件，不需要再“切新文件再删旧文件”。

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 工具 JSON Schema

```json
{
  "name": "todos",
  "description": "Track a session-local todo list (a personal scratchpad you can keep across tool calls).\n\nWhen to use: any multi-step work (3+ distinct steps), multiple user tasks, or whenever you want a checklist to keep yourself organized across turns. Mark one item in_progress before starting it; mark it completed as soon as it's done.\n\nWhen NOT to use: single trivial step or pure Q&A.\n\nReturn value: every successful call returns a full items snapshot under `items` (id/content/status). You do NOT need to re-read the file to know the current state.\n\nRules: stable id per item; status in pending|in_progress|completed|cancelled; at most one in_progress at any time; use ops (upsert/set_status/remove) or replace=true for full list replacement. new_todos=true clears the current scratchpad and overwrites the same `todos/<session_id>.todo.md` file.",
  "parameters": {
    "type": "object",
    "properties": {
      "new_todos": {
        "type": "boolean",
        "description": "If true, clear the current scratchpad before applying ops. The same `todos/<session_id>.todo.md` file is overwritten; no extra file is created. Default false."
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
              "description": "upsert = create or update by id; set_status = mutate status only; remove = delete by id"
            },
            "id":      { "type": "string" },
            "content": { "type": "string" },
            "status":  { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] }
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

实现层当前返回以下字段（其中部分为可选）：

```jsonc
{
  "scope": "session",
  "mode": "chat|planning|executing|completed|pending",
  "applied": 2,
  "replace": false,
  "new_todos": false,
  "active_in_progress": "t2",
  "items": [
    { "id": "t1", "content": "inspect API surface", "status": "completed" },
    { "id": "t2", "content": "run focused regression", "status": "in_progress" }
  ],
  "title": "research-notes",                   // 可选
  "active_todos_id": "td_12345678",            // 始终是当前内存 scratchpad id
  "persisted_path": ".../sid-a.todo.md",       // 持久化成功时出现
  "panel_snapshot_id": 1747656000000000
}
```

返回约束：

- `items` 始终是**完整快照**；
- `active_in_progress` 为当前唯一 `in_progress` 的 `id`，否则为 `null`；
- `panel_snapshot_id` 来自 `TodosPanelSnapshot`，UI 用它做去重 / 防回退；
- `scope` 对 `todos` 固定为 `"session"`。

**说人话**：模型拿到这一条 ToolResult 就知道“当前白板长什么样”，不需要再读文件。

---

## 6. 状态机与并发约束

### 6.1 单文件状态约束

| 约束 | 处理 | 说人话 |
|------|------|--------|
| 两条 todo 同时 `in_progress` | tool error，整批回滚 | 一次只准真正在做一条。 |
| `set_status` / `remove` 指向未知 `id` | tool error | 不能瞎编 id。 |
| `replace=true` 但 `ops` 含非 `upsert` | tool error | 整表替换只能给最终态。 |
| `replace=true` 且 `ops` 为空 | tool error | 不允许靠空替换清空表。 |

### 6.2 并发与锁

- in-memory：先对 session todos 做替换，再决定是否持久化；
- 持久化：`TodosRuntime::persist()` 采用 `tmp -> rename` 原子写；
- 当前实现里 `todos` 的持久化失败只记 warning，不阻塞主流程；
- `new_todos` 不再触发任何 purge，因为磁盘上天然只有 `todos/<session_id>.todo.md` 这一份文件。

**说人话**：这块白板首先保证当前回合可用，其次才尽量把磁盘状态追上来。

---

## 7. TodosPanel：UI 投影协议

`todos` 成功后会立刻构造：

```rust
TodosPanelSnapshot::new_session(items)
```

并通过 `RefreshNotifier` fanout 给注册的 panel。快照字段：

| 字段 | 含义 | 说人话 |
|------|------|--------|
| `panel_snapshot_id` | 单调递增快照 id | UI 用它防止旧数据回刷。 |
| `scope` | 固定为 `session` | 这是会话白板，不是 plan 白板。 |
| `items` | 当前完整 todo 列表 | 面板拿来直接渲染。 |
| `warnings` | 额外警告 | 当前 `todos` 路径通常为空。 |

CLI 默认渲染形态：

```text
[panel#1747656000000000] session 1 of 2 Done in_progress=t2
  [x] t1 ▸ inspect API surface
  [~] t2 ▸ run focused regression
```

**说人话**：面板只是“播报器”。真正状态仍然来自 in-memory todos / TodoFile。

---

## 8. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_TODOS_PANEL_THROTTLE_MS` | `250` | TodosPanel snapshot 节流窗口 | 面板别刷太勤。 |
| `TOMCAT_TODOS_BASH_TAIL_LINES` | `3` | panel bash 摘要保留尾行数 | 如果面板关联网 shell 任务，默认只看最后 3 行。 |
| `TOMCAT_TODOS_FILE_LOCK_TIMEOUT_MS` | `2000` | `.todo.md` 原子写入等待上限 | 等锁最多 2 秒。 |

---

## 9. 测试矩阵（验收）

| 类型 | 测试 | 说人话 |
|------|------|--------|
| 单元：全模式可见 | `todos_visible_in_all_modes` | 任何模式都要能调 `todos`。 |
| 单元：单进行中约束 | `todos_state_enforces_single_in_progress` | 两个进行中必须拒绝。 |
| 单元：TodoFile round-trip | `todo_file_roundtrips_markdown_with_status_checkboxes` | `.todo.md` 要能稳定序列化。 |
| 单元：new_todos 覆盖 | `todos_new_todos_overwrites_same_session_file` | 换新白板会覆盖同一个 session 文件。 |
| 单元：多 session 隔离 | `todos_runtime_isolates_multiple_sessions_without_purge` | 不同 session_id 各写各的文件，互不影响。 |
| 集成：面板刷新 | `todos_tool_updates_session_panel_snapshot` | 工具成功后 UI 快照要同步。 |
| 集成：schema 漂移保护 | `tool_catalog_doc` | 文档里的 schema 不能和 catalog 漂移。 |

---

## 10. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~`todos.active_scope ∈ {session, plan}`~~ | **下线**：`todos` 永远只写 session；plan 内待办由 [`update_plan`](./update-plan.md) 管。 | 工具职责单一。 |
| ~~Planning 模式剔除 `todos`~~ | **下线**：D 方案改为所有模式都可见。 | 规划时也允许先给自己列调研步骤。 |
| ~~`todos` 推进 `plan.md` 并触发 mode 派生~~ | **下线**：mode 派生只由 [`update_plan`](./update-plan.md) 触发。 | 改 plan 的工具负责 plan 状态机。 |
| ~~旧白板长期归档堆目录~~ | **替代**：每个 session 固定一个 `todos/<session_id>.todo.md`，`new_todos` 直接覆盖写。 | 目录里天然只有当前这一份。 |
| ~~ToolResult 不带全量 items，靠再次 `read` 文件补状态~~ | **否**：`todos` 总是返回完整 `items`。 | 一次调用就给足上下文。 |

---

## 11. 关联文档

- plan 增量待办编辑：[update-plan.md](./update-plan.md)
- plan 整盘重写：[create-plan.md](./create-plan.md)
- PLAN / EXEC 运行时编排：[plan-runtime.md](../plan-runtime.md)
- reviewer 子 Agent：[reviewer.md](./reviewer.md)
- transcript / session 存储：[session-storage.md](../session-storage.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)

**说人话**：会话本地 scratchpad 看本文；改 `plan.md` 里的待办看 [`update-plan.md`](./update-plan.md)；整盘重写计划看 [`create-plan.md`](./create-plan.md)。
