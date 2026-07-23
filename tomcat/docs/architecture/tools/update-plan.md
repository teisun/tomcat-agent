# `update_plan` 工具：PlanFile frontmatter 增量编辑

> **位置**：B 类 `docs/architecture/tools/`。本工具与 [`todos`](./todos.md) **代码复用、提示词分裂**，与 [`create_plan`](./create-plan.md) 互补：`create_plan` 整盘重写、仅 PLAN 模式可见；`update_plan` **增量编辑**、**任何模式可见**。运行时编排见 [`plan-runtime.md`](../plan-runtime.md) §5.3。

本文档定义 `update_plan` 的入参 / 出参、跨模式门控矩阵、自动派生触发点、跨 session 语义。

## 2026-07 Code review 时间线补充

最后一个 todo 完成时，`update_plan` 保持 running，并按 `plan.update + plan.todos -> plan.code_review.started -> plan.code_review -> tool_execution_end` 同步收口。started/result 都以父 session 发出，共享 `<planId>:<round>` attempt 身份和触发工具的 `toolCallId`；child-scoped `sub_agent_start/end` 只作审计，不驱动 ReviewRow。


## 2026-05 Active Binding 补充

当前实现对 `update_plan` 的运行时契约已更新为：

- completed plan **不再**全拒；允许继续编辑并在需要时 reopen；
- reopen completed 时，盘 state 从 `completed -> pending`，runtime 同步切到 `Pending { id }`；
- `update_plan` 始终**不**写 active binding：不设置 `active_plan_path`，也不负责建立 `/plan build` 意义上的绑定；
- all-completed 路径会先把盘写成 `completed`，再经过瞬时 `Completed { id }`，立即 `finalize_completed_to_chat()` 回到 `Chat(retain)`；
- transcript 事件固定写为 `event = "plan.update"`，payload 为 `{ plan_id, path, state }`，并继续遵守“先盘、再释锁、后事件”。

以下正文若仍出现 `active_plan_id`、`plan.complete`、`completed plan 不可编辑` 等旧草稿表述，均以上述稳定契约与仓库代码为准。

**说人话**：`update_plan` 是 LLM 推进 `~/.tomcat/plans/<*>.plan.md` 内 `todos[]` 状态的**主力工具**。任何模式都能调；`/plan build` 之后改的是「正在执行的 plan」，CHAT/PLAN 下改的是「指定 plan_id 的待办」。**整盘重写**仍是 `create_plan` 的事（且只 PLAN 模式能用）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 与 `todos` / `create_plan` 的关系](#2-与-todos--create_plan-的关系)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 门控矩阵（mode × op × plan.state）](#4-门控矩阵mode--op--planstate)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. 调度时序](#6-调度时序)
- [7. 跨 session 与并发](#7-跨-session-与并发)
- [8. 自动派生与 EXEC 收口](#8-自动派生与-exec-收口)
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
| **`update_plan`** | 对 PlanFile `todos[]` 的**增量**编辑工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "update_plan"` | **任何模式可见**；按 `plan_id` 或显式 `path` 路由，EXEC/Pending 缺省跟随当前 active plan path；frontmatter 中**只**能动 `todos[]`；不能动 mode / session_* / plan_id / goal / created_at 等机器字段；不能动 markdown 正文 | 改 plan 的待办进度用这个。 |
| **target PlanFile** | 本次操作的目标计划文件 | 由 `plan_id` 或显式 `path` 解析；若当前 session 已在 EXEC/Pending，缺省目标跟随 active plan path | EXEC/Pending 可缺省；其它模式需显式传 `plan_id` 或 `path` | 改哪份 plan 要说清楚。 |
| **同 op 模型** | 复用 `todos` 的 op 数据结构 | `kind ∈ {upsert, set_status, remove}` | `id` 在目标 PlanFile 内唯一；同一文件最多一个 `in_progress` | 操作语义与 `todos` 一致。 |
| **跨 session 编辑** | 任何 session 都能改任意 plan 的 todos | `update_plan` 不读 / 不写 session_* frontmatter | 跨 session 改 todos 允许；但同时只一个 session 能「执行」（active_plan_id 受 build gate 约束） | 任意聊天窗口都能勾 plan 待办。 |
| **active plan path（per-session runtime）** | 当前 session 正在 EXEC/Pending 的真实 plan 路径 | `PlanRuntime.active_plan_path: Option<PathBuf>`；不写 frontmatter | EXEC/Pending 下 update_plan 默认指向它；`plan_id` 仍来自 frontmatter | 执行态默认改自己手上的那份。 |

---

## 2. 与 `todos` / `create_plan` 的关系

| 工具 | 写什么文件 | 模式可见性 | 语义 | 代码层 | 说人话 |
|------|-----------|-----------|------|--------|--------|
| **`todos`** | `~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md`（session 路径） | **任何模式**（PLAN 期也可调，做 LLM 个人 scratchpad） | 个人 / 会话级待办；**不**写 plan.md | `apply_todos_op(TodoStore)` | 聊天里随手记的清单。 |
| **`update_plan`** | 目标 `PlanFile` 的 frontmatter `todos[]` | **任何模式** | plan 级待办的**增量**修订 | `apply_todos_op(PlanStore, target)` —— 复用 `todos` 的 op 引擎 + 不同 store + 不同 schema | 改 plan 文件的待办用这个。 |
| **`create_plan`** | `~/.tomcat/plans/<*>.plan.md` 的**整盘**（frontmatter 初稿 + 正文 `## Goal` / `## Draft` / `## Todos`） | **仅 PLAN 模式** | 重写：把 LLM 提供的 `goal / draft / todos` 全量落盘，并同步派 reviewer | 独立实现 | 计划结构推倒重来时用。 |

```text
                       ┌──────────────────────────────────────┐
                       │  frontmatter 四方协同 (PlanFile)       │
                       ├──────────────┬───────────────────────┤
                       │ create_plan  │ 初稿写盘（PLAN only）  │
                       │ update_plan  │ todos[] 增量编辑（any mode）              │
                       │ runtime      │ /plan build 时写 session_key/id / mode │
                       │ 自动派生     │ state=completed (all todos done) /        │
                       │              │ state=pending (cancel_token)              │
                       └──────────────┴───────────────────────┘
```

**说人话**：四个写入方各管一摊，LLM **永远不直接写 YAML frontmatter**。

### 2.1 选用决策树

```text
要改 PlanFile 的什么？
   │
   ├─ 整盘重新规划（goal / draft 全换） ──▶ create_plan（PLAN 模式）
   │
   ├─ 仅 todos[] 状态 / 增删 ──▶ update_plan
   │
   ├─ 改 plan 正文任意段 ──▶ raw write/edit（PLAN 模式路径白名单生效；reviewer 也走这里）
   │
   └─ 改 frontmatter 其它机器字段（mode / session_* / plan_id 等） ──▶ 任何工具都不能，由 runtime 写
```

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§11） | 说人话 |
|----|------|------------------|--------|
| G1 | 任何模式都可见、单一入口；不再分裂 `todos.active_scope=plan` 分支 | `update_plan_visible_in_all_modes` | 改 plan 待办随时能改。 |
| G2 | 复用 `todos` 的 op 引擎与文件锁；不重新实现 op 模型 | `update_plan_reuses_todos_op_engine` | 共享代码，减少漂移。 |
| G3 | 默认目标解析：`plan_id -> path -> active_plan_path -> (Executing/Pending 的内嵌 plan_id)` | `update_plan_requires_plan_id_outside_exec` | 改哪份 plan 必须明确，但 retain 场景允许继续沿用当前 active path。 |
| G4 | 只能动 `todos[]`；其它 frontmatter 字段 / markdown 正文均拒 | `update_plan_rejects_non_todo_frontmatter_writes`、`update_plan_rejects_body_writes` | 别越界。 |
| G5 | EXEC 模式下提交后触发 `state=completed` 自动派生；其它模式只改 frontmatter，不改 mode | `update_plan_in_exec_promotes_completed`、`update_plan_outside_exec_does_not_change_mode` | EXEC 才会自动收口。 |
| G6 | 跨 session 编辑允许；同 session 不能同时 EXEC 两份 plan（build gate 不变） | `update_plan_cross_session_allowed`、`active_plan_id_unique_per_session` | 任意聊天窗口可改，开干仍受 build gate。 |
| G7 | ToolResult 自带完整 `items` snapshot；LLM 不必再 `read` plan.md | `update_plan_result_carries_full_snapshot` | 工具结果自带全貌。 |
| G8 | reviewer subagent 默认可用 `update_plan`（只读 reviewer 也可用 todos；改稿 reviewer 才可用 update_plan） | `reviewer_can_use_update_plan_when_allowed` | 让 reviewer 也能落地修订建议。 |

---

## 4. 门控矩阵（mode × op × plan.state）

### 4.1 调用模式 × 必填字段

| 当前 mode | `plan_id` 入参 | 默认目标 | 说人话 |
|-----------|---------------|----------|--------|
| 任意模式 | 可省 | 先看 `active_plan_path` retain；若当前 mode 为 `Executing / Pending`，再回退到内嵌 `plan_id` | retain / 执行态可直接沿用当前 plan。 |
| `Chat` / `Planning` / `Completed` 且无 retain | 需显式 `plan_id` 或 `path` | — | 没有 retain 时还是要说清楚改哪份。 |

### 4.2 目标 plan.state 准入

| target `PlanFile.state` | 是否允许 | 行为 |
|------------------------|---------|------|
| `planning` | ✅ | 改 todos[]；不触发 mode 自动派生 |
| `executing` | ✅（仅当 `target.session_key == 当前 session.session_key`） | 改 todos[]；EXEC 模式下可触发 state=completed 自动派生 |
| `pending` | ✅ | 改 todos[]；不触发 mode 自动派生（要续跑请用 `/plan build`） |
| `completed` | ✅ | 先改 todos[]；若改后不再 all-completed，则盘 `completed -> pending`，runtime 同步切 `Pending { id }` |

> 「`executing` + 跨 session」拒绝原因：避免另一个 session 在你执行中改你手上 todo 造成竞争。CHAT/PLAN 期的 plan（`session_key == null`）跨 session 改无问题。

### 4.3 op × 目标 plan.state

| op | `planning` | `executing` | `pending` | `completed` |
|----|-----------|-------------|-----------|-------------|
| `upsert`（pending） | ✓ | ✓（仅本 session） | ✓ | ✓（reopen 后转 pending） |
| `upsert`（in_progress） | ✗（plan 还没 build，标 in_progress 没意义） | ✓（仅本 session） | ✗（先 `/plan build` 续跑） | ✗（reopen 仍先回 pending） |
| `set_status(in_progress)` | ✗ | ✓ | ✗ | ✗ |
| `set_status(completed/cancelled)` | ✓ | ✓（触发自动派生检查） | ✓ | ✓（若仍全 completed，则盘继续保持 completed） |
| `remove` | ✓ | ✓ | ✓ | ✓（reopen 后转 pending） |

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 工具 JSON Schema

```json
{
  "name": "update_plan",
  "description": "Incrementally update a PlanFile's todos[].\n\nUse this whenever you want to:\n- mark a todo in_progress / completed / cancelled\n- add a new todo\n- rewrite or prune an existing todo list\n\nCallable in ANY mode. Target resolution order is: explicit `plan_id` -> explicit `path` -> retained `active_plan_path` -> current Executing/Pending state. Cross-session editing is allowed for plans whose state is planning, pending, or completed; an executing plan can only be edited by the session that owns it. Completed plans may be reopened: if your edit makes the plan no longer all-completed, disk state changes from completed -> pending.\n\nReturn value: every successful call returns full items snapshot (plus path / warnings / active_in_progress); you do not need to re-read the plan file.\n\nRules: stable id per item; status in pending|in_progress|completed|cancelled; at most one in_progress per PlanFile; in_progress is only allowed when plan.state == executing; replace=true replaces the entire todos[] list with the provided upsert results.",
  "parameters": {
    "type": "object",
    "properties": {
      "plan_id": {
        "type": "string",
        "description": "Target plan id (e.g. plan_my_goal_a1b2c3d4). Optional when runtime already retains an active plan path; otherwise required unless `path` is provided."
      },
      "path": {
        "type": "string",
        "description": "Alternative to plan_id: absolute path under ~/.tomcat/plans/. If both given, plan_id wins."
      },
      "replace": {
        "type": "boolean",
        "description": "If true, todos[] is replaced by the upsert results in ops. Default false."
      },
      "ops": {
        "type": "array",
        "description": "Sequence of mutations applied in order under a single file lock.",
        "items": {
          "type": "object",
          "properties": {
            "kind": {
              "type": "string",
              "enum": ["upsert", "set_status", "remove"]
            },
            "id":           { "type": "string" },
            "content":      { "type": "string", "description": "For upsert: todo content. Required when creating a brand-new todo." },
            "status":       { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] }
          },
          "required": ["kind", "id"]
        }
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
    "applied":           { "type": "integer" },
    "plan_id":           { "type": "string" },
    "plan_path":         { "type": "string" },
    "plan_state_before": { "type": "string", "enum": ["planning", "executing", "completed", "pending"] },
    "plan_state_after":  { "type": "string", "enum": ["planning", "executing", "completed", "pending"] },  // 仅 EXEC 全 completed 时与 before 不同
    "active_in_progress":{ "type": "string", "nullable": true },
    "items": {                              // ★ 完整 todos 快照
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id":           { "type": "string" },
          "content":      { "type": "string" },
          "status":       { "type": "string", "enum": ["pending","in_progress","completed","cancelled"] }
        },
        "required": ["id", "content", "status"]
      }
    },
    "panel_snapshot_id": { "type": "string" },
    "warnings":          { "type": "array", "items": {"type":"string"} }
  },
  "required": ["applied", "plan_id", "plan_state_before", "plan_state_after", "items", "panel_snapshot_id"]
}
```

**说人话**：返回当前 plan 完整 items、本次改了几条、操作前后的 state（用以观察 runtime 是否自动派生 completed）、面板快照编号。

### 5.3 代码复用细节

```rust
// 拟定
pub fn apply_todos_op(store: &dyn TodoStore, ops: &[TodoOp]) -> Result<TodoSnapshot> { ... }

// todos 工具走 SessionTodoStore；update_plan 走 PlanTodoStore
pub struct SessionTodoStore { todos_id: TodosId, file: PathBuf, ... }
pub struct PlanTodoStore    { plan_id: PlanId,  file: PathBuf, frontmatter_lock: ... }

impl TodoStore for SessionTodoStore { ... }   // 写 ~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md
impl TodoStore for PlanTodoStore    { ... }   // 写 ~/.tomcat/plans/<*>.plan.md 的 frontmatter todos[]
```

**说人话**：共享的是 todo-op 语义层；`update_plan` 只比 `todos` 多 plan 定位（`plan_id/path`）与 `replace` 的顶层协议。

---

## 6. 调度时序

```text
LLM ──tool_call("update_plan", { plan_id, ops, ... })──▶ tool_exec::update_plan
                                                                │
                                                                ▼
                            ┌──────────────────────────────────────────────────────┐
                            │ 1. resolve plan_id / path → PlanFile path             │
                            │ 2. state gate（§4）                                   │
                            │ 3. acquire advisory file lock                         │
                            │ 4. apply_todos_op(PlanTodoStore, ops)                 │
                            │    - frontmatter.todos[] in-place                     │
                            │    - 同 op 引擎（与 todos 工具一致）                  │
                            │ 5. write frontmatter back (round-trip via serde_yaml) │
                            │ 6. release lock                                       │
                            └──────────────────────────────────────────────────────┘
                                                                │
                                                                ▼
                            ┌──────────────────────────────────────────────────────┐
                            │ runtime hook (EXEC 模式 + target.state==executing 时): │
                            │    all todos completed? ──▶ code review / verifier    │
                            │       ① write frontmatter.state = completed           │
                            │       ② 瞬时 set_mode_completed()                     │
                            │       ③ finalize_completed_to_chat() -> Chat(retain)  │
                            │       ④ transcript 仍写 plan.update                   │
                            └──────────────────────────────────────────────────────┘
                                                                │
                                                                ▼
                            ┌──────────────────────────────────────────────────────┐
                            │ panel: throttled refresh; transcript: plan.todos      │
                            └──────────────────────────────────────────────────────┘
                                                                │
                                                                ▼
                            ToolResult { applied, items, plan_state_after, ... }
```

**说人话**：解析目标 → 校验门控 → 加锁 → 复用 todos op 引擎写 frontmatter → 解锁 → 看是否触发完成派生 → 刷面板 → 返回快照。

### 6.1 Advisory file lock 实现说明（fs2 / 侧车 / 2s 超时）

- **crate**：`fs2 = "0.4"`（`tomcat/Cargo.toml` 直接依赖）。
- **锁文件**：侧车 `<plan_path>.plan.md.lock`（**每 plan 一把**，位于 `~/.tomcat/plans/`）。`read_plan` 不上锁；只读不参与并发争用。
- **API**：`fs2::FileExt::try_lock_exclusive()`（OS 级 advisory：协作语义 + 内核锁），闭包返回后显式 `FileExt::unlock`。
- **写序列**：`with_advisory_lock(...)` 内部执行 `tmp 写 → fsync → atomic rename(tmp, plan)`；任意一步失败磁盘保持上一稳定态。
- **超时**：默认 `2000ms`（可调 `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS`）；5ms 起指数退避（上限 80ms），溢出返回 `PlanError::LockBusy { waited_ms, holder_pid }`。
- **Cancel/中止路径**：`PlanRuntime::demote_to_pending_on_cancel()` 走同一把锁的 `write_plan`；锁随闭包结束释放，**没有**独立 `release_lock()` API。

### 6.2 拒写（state 矩阵闸门）

`update_plan` 在写盘前做模式矩阵检查，**任何**违反即返回 tool error（不写盘、不刷面板）：

| 目标 `plan.state` | 操作 | 结果 |
|------------------|------|------|
| `completed` | 任意 op | ✅ 允许；若改后不再 all-completed，则 reopen 为 `pending` |
| `planning` / `pending` | `set_status: in_progress` | ❌ `BadOp("in_progress only allowed in executing")` |

---

## 7. 跨 session 与并发

| 场景 | 处理 | 说人话 |
|------|------|--------|
| 同一 session 同时改两份 plan（CHAT/PLAN 期，target.state=planning） | ✓ 允许（这两份 plan 与 active binding 无关） | 改谁都行，反正没在跑。 |
| 同一 session 在 EXEC（active plan=P）调 `update_plan(plan_id=P)` | ✓ 默认 op；可省 plan_id | 改自己手上的 plan。 |
| 同一 session 在 EXEC（active plan=P）调 `update_plan(plan_id=Q)` 改另一份 planning/pending/completed 的 Q | ✓ 允许（只要 Q 不在别的 session executing） | 顺手补另一份的 todos OK，但要注意。 |
| 跨 session 改同一份 plan（target.state=planning/pending） | ✓ 允许 | plan 没绑 session 期，任意 session 改。 |
| 跨 session 改 `target.state=executing` 的 plan | ❌ tool error，提示「该 plan 正由 session X 执行」 | 别越界。 |
| 同进程并发 `update_plan` 同一份 plan | tokio Mutex 串行化 | 同进程排队写。 |
| 跨进程并发 | advisory file lock（`fs2::FileExt`）；获取失败 → tool error | 跨进程靠文件锁挡。 |

---

## 8. 自动派生与 EXEC 收口

仅当**三者同时成立**才触发 completed 收口：

1. 调用方 session.mode == `Executing`；
2. `target.state == executing` 且 `target.session_key == 当前 session.session_key`；
3. 本次提交后所有 `todos[*].status == completed`。

满足时，runtime 会在 verifier/code-review 闭环后：

- 写 `frontmatter.state = completed`；
- 瞬时切到 `Completed { id }`；
- 立即 `finalize_completed_to_chat()` 回 `Chat(retain)`；
- 保留 `active_plan_path` / `active_planning_plan_id`；
- transcript 仍只追加 `plan.update` 自定义事件。

> 其他模式（CHAT/PLAN/Completed/Pending）调 `update_plan` 即使把所有 todo 标 completed，**也只更新 frontmatter，不触发执行态收口**——因为 plan 根本没在执行。要执行请走 `/plan build`。

`cancel_token` 转 `pending` 仍由 runtime 监听处理，与本工具无关。

---

## 9. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_UPDATE_PLAN_LOCK_TIMEOUT_MS` | `2000` | advisory lock busy 超时 | 抢不到锁等多久。 |
| `TOMCAT_UPDATE_PLAN_PANEL_THROTTLE_MS` | `250` | 面板节流间隔 | 界面别刷太勤。 |
| `TOMCAT_UPDATE_PLAN_DISABLE_CROSS_SESSION` | `0` | 设 `1` 后禁止跨 session 改 plan（回归测试 / 强一致场景） | 紧场景下关跨 session。 |

---

## 10. 错误模型 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| `plan_id` / `path` 缺失且 runtime 无 retain | tool error，usage「请提供 plan_id/path，或先通过 /plan build 建立 retain」 | 改哪个说清楚。 |
| `plan_id` 解析不到文件 | tool error | 编错 id。 |
| 跨 session 改 `executing` plan | tool error，usage「该 plan 正由 session X 执行」 | 跨 session 不许动正在跑的。 |
| `set_status(in_progress)` 且 `target.state != executing` | tool error | plan 没在 EXEC 标 in_progress 没意义。 |
| 同 plan 两个 `in_progress` | tool error；整批回滚 | 一个 plan 文件最多一个在干。 |
| 未知 `id` | tool error | 别瞎编 id。 |
| frontmatter round-trip 解析失败 | tool error，附最后 50 字节上下文 | 文件被外部改坏了。 |
| advisory lock 抢不到 | tool error，`LockBusy`，retry hint | 拿不到锁。 |

---

## 11. 测试矩阵（验收）

| 类型 | 测试 | 状态 |
|------|------|------|
| 单元：跨模式 catalog | `update_plan_visible_in_all_modes` | PENDING |
| 单元：op 引擎复用 | `update_plan_reuses_todos_op_engine` | PENDING |
| 单元：plan_id 强制 | `update_plan_requires_plan_id_outside_exec`、`update_plan_defaults_to_active_plan_id_in_exec` | PENDING |
| 单元：frontmatter 写边界 | `update_plan_rejects_non_todo_frontmatter_writes`、`update_plan_rejects_body_writes` | PENDING |
| 单元：自动派生 | `update_plan_in_exec_promotes_completed`、`update_plan_outside_exec_does_not_change_mode` | PENDING |
| 单元：跨 session | `update_plan_cross_session_allowed_for_planning_pending`、`update_plan_cross_session_rejected_for_executing` | PENDING |
| 单元：completed 拒绝 | `update_plan_rejects_completed_target` | PENDING |
| 集成：ToolResult 快照 | `update_plan_result_carries_full_snapshot` | PENDING |
| 集成：reviewer 使用 | `reviewer_can_use_update_plan_when_allowed` | PENDING |

---

## 12. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| LLM 把 `update_plan` 当 `todos` 用（在 CHAT 改个人备忘） | 中 | description 第一段明确「per-session personal todos → use `todos`」；schema 缺省 plan_id 在非 EXEC 报错；测试覆盖 | 描述里讲清楚。 |
| LLM 把 `update_plan` 当 `create_plan` 用（试图整盘重写） | 中 | schema 不支持 `goal` / `draft` 入参；`replace=true` 也仍需逐项 `upsert` | 工具入参就不让你重写。 |
| 跨 session 并发竞争 | 中 | advisory file lock + 跨 session executing 拒绝 | 文件锁挡。 |
| frontmatter 写坏 | 高 | 写前 round-trip + atomic rename；写失败保留原文件；增加 `update_plan_frontmatter_corruption_recovery` 测试 | 写坏要可回滚。 |
| 自动派生在多 op 批次中误触发 | 中 | 在批次提交后**一次性**计算 all completed；批内中间态不触发 | 批量改时只在末尾判一次。 |
| LLM 在 EXEC 同时通过 update_plan 改另一份 plan 让用户困惑 | 低 | warning「正在 EXEC P，建议先稳定本盘」；可由配置升级为 error | 默认警告，可加严。 |

---

## 13. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~`todos` 一个工具 + `active_scope ∈ {session, plan}` 分支~~ | **拆**：`todos` 只管 session；`update_plan` 管 plan；二者**代码复用**op 引擎 + **提示词分裂** | 工具职责单一，LLM 更难混。 |
| ~~PLAN 模式下用户要求改 todos 必须再次调 `create_plan` 整盘重写~~ | **替代**：用 `update_plan` 增量改；`create_plan` 仅当结构大改时用 | 不用每次小修就重写整盘。 |
| ~~CHAT 模式下完全无法改 plan.md 的 todos[]~~ | **修复**：`update_plan` 在 CHAT 可见；按 `plan_id` 或显式 `path` 路由 | 修上一版的缺口。 |
| ~~把 `plan_id` 入参也藏起来由 runtime 推断~~ | **否**：EXEC 缺省取 active，其它模式必填 | 改哪份 plan 必须明说。 |
| ~~把 markdown 正文写权也合到 `update_plan`~~ | **否**：正文走 raw write/edit；本工具只管 frontmatter `todos[]` | 工具职责单一。 |
| ~~允许改 `target.state == completed` 的 plan~~ | **否**：已结案的 plan 拒绝修改；要重新做请用 `create_plan` 开新 plan | 结案的别动。 |
| ~~跨 session 改 `executing` plan~~ | **否**：拒绝；只有 owning session 能改正在跑的那份 | 别越界。 |
| ~~自动派生在任意模式都触发~~ | **否**：仅 EXEC + target.state==executing + 同 session 时；其它模式只改 frontmatter | state 转移由 EXEC 触发。 |
| ~~reviewer subagent 不能用 `update_plan`~~ | **替代**：`allow_review_edit=true` 时附加 `update_plan` + `edit`；reviewer 可落地修订建议而非只挑刺 | 让审稿员能动手。 |

---

## 14. 关联文档

- 整盘重写：[`create-plan.md`](./create-plan.md)
- 个人会话备忘：[`todos.md`](./todos.md)
- 运行时编排：[`plan-runtime.md`](../plan-runtime.md) §5.3 / §6.2 / §7
- PLAN 模式整体规范：[`planner.md`](./planner.md)
- reviewer 子 Agent：[`reviewer.md`](./reviewer.md)
- 文档规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)
- transcript 自定义事件：[`session-storage.md`](../session-storage.md)
