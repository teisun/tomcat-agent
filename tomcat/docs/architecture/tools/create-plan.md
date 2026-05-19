# `create_plan` 工具：PlanFile 写入器与内联 reviewer 派发

本文档是内置工具 **`create_plan`** 的冻结版技术方案（OpenSpec **B 类**：`docs/architecture/tools/`）。承接 [`plan-runtime.md`](../plan-runtime.md) 与 [`planner.md`](./planner.md)：**仅在 PLAN 模式可见**，是唯一**创建/重写** `PlanFile` 文件的内置 LLM 工具，并在写入完成后**同步**通过 `internal subagent dispatch`（见 [`reviewer.md`](./reviewer.md)）派发 reviewer 子 Agent，把审稿结果一并塞回工具结果。**实现以仓库代码为准**；本文只保留**已定稿的行为与契约**。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：这是 PLAN 模式里唯一会主动**创建**计划文件的工具——把 LLM 写的草案落到 `~/.tomcat/plans/<slug>_<hash>.plan.md`，加文件锁，然后立刻派一个 reviewer 子 Agent 来挑刺，审稿摘要一起返回。reviewer 只是辅助：是否进入执行态由用户敲 `/plan build <plan_id|path>` 决定。**`create_plan` 名字保留**，职责也不变；执行态推进 `todos[]` / `mode` 字段走 [`update_plan`](./update-plan.md) 工具，整盘 mode 切换走 [`/plan`](../plan-runtime.md#51-本地-slash-命令) 命令族与 runtime 自动转移，**三者协同**改动 `PlanFile` frontmatter，互不重叠。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. PlanFile 文件协议](#5-planfile-文件协议)
  - [5.7 修订路径（`update_plan` 增量 vs `create_plan` 整盘重写）](#57-修订路径update_plan-增量-vs-create_plan-整盘重写)
- [6. 协议（入参 / 出参 / Schema）](#6-协议入参--出参--schema)
- [7. 内联 reviewer 派发](#7-内联-reviewer-派发internal-subagent-dispatch)
- [8. 写盘路径白名单与 frontmatter 硬拦截](#8-写盘路径白名单与-frontmatter-硬拦截)
- [9. One-Glance Map](#9-one-glance-map)
- [10. 调度时序](#10-调度时序)
- [11. 状态机](#11-状态机)
- [12. 错误模型 / 截断 / 警告](#12-错误模型--截断--警告)
- [13. 测试矩阵（验收）](#13-测试矩阵验收)
- [14. 风险与应对](#14-风险与应对)
- [15. 历史决策](#15-历史决策)
- [16. 关联文档](#16-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **`create_plan` 工具** | PLAN 模式下创建 / 重写 `PlanFile` 的内置 LLM 工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "create_plan"` | 仅 `mode == Planning` 时可见；写入前 advisory file lock；写入后同步内联 reviewer | 计划文件主要创建入口。名字保留不改。 |
| **PlanFile** | 持久化计划文件 | `~/.tomcat/plans/<slug>_<hash>.plan.md` | frontmatter（机写）+ markdown 正文（人读）；schema 见 §5 | 这份文件就是计划本身。 |
| **slug** | `goal` 派生的 URL-safe 短标识 | `slug(goal, max_len=40)` | ASCII / `-` / 数字；空格转 `-`；超长截断；保留前缀 | 把目标做个文件名。 |
| **plan_id hash** | 8 字符 16 进制；basis = `goal + ts_ms` | xxh32 | 同一时刻同一目标的 collision 概率极低；冲突时本地 retry `goal + new_ts_ms` | 给文件名加个唯一后缀。 |
| **internal subagent dispatch** | 内部 Rust API 形式的子 Agent 派发，不进 catalog | `AgentRegistry::spawn_subagent_internal(...)`（见 [`multi-agent.md`](../multi-agent.md) §14.6.1） | 由 `create_plan` 在落盘成功后同步 await | LLM 看不到，工具内部自己开。 |
| **reviewer 子 Agent** | 同步派发的审稿子 Agent；输出落 transcript 摘要 + 可选直接写正文 | `SubagentType::Reviewer`；不进 catalog | 写盘成功后立即派发；reviewer 改稿权由 **runtime 内部参数**控制，**不**作为 `create_plan` 工具入参暴露 | 内部审稿员，是不是改稿由 runtime 决定。 |
| **frontmatter 四方协同** | `PlanFile` frontmatter 谁来写 | `create_plan` / `update_plan` / runtime / 自动派生 | `create_plan` 写初稿（schema + `goal/draft/todos`）；[`update_plan`](./update-plan.md) **增量**改 `todos[]`（任何模式）；runtime 在 `/plan build` 时写 `mode=executing` 与 `session_key/id`；**自动派生**在 EXEC + 全 completed → `mode=completed`、cancel_token → `mode=pending` | 四方各管一段，**LLM 永远不直接动 frontmatter** YAML。 |

---

## 2. 竞品 / 选型对比（调研）

### 2.1 Agent 写计划工具的典型关切

```text
┌──────────────────────────────────────────────────────────────────────┐
│  本地 create_plan 类工具通常要解决的四类问题                          │
├────────────────────┬─────────────────────────────────────────────┤
│  落盘安全          │  并发写、advisory lock、原子 rename          │
│  schema 稳定       │  frontmatter 机写、正文人读                   │
│  审稿介入          │  写完是否立刻审？同步 vs 异步？是否做闸门     │
│  与执行态切换      │  审完是否自动进执行态？谁决定？               │
└────────────────────┴─────────────────────────────────────────────┘
```

**说人话**：写计划工具要解决落盘安全、schema 稳定、写完要不要立刻审、审完怎么进执行态四件事。Tomcat 选择「写完同步派 reviewer 但不做 verdict gate；进执行态由用户 `/plan build` 决定」。

### 2.2 常见实现横向对比

| 来源 / 形态 | 工具名 | 落盘路径 | 审稿时机 | 审稿者形态 | 是否做 verdict gate | 说人话 |
|-------------|--------|----------|----------|------------|---------------------|--------|
| **cc-fork-01** | `EnterPlanMode` + `update_plan` | session 内存 | 仅由用户 `ExitPlanMode` 决定 | 无独立 reviewer | 否 | 没审稿，靠人。 |
| **codex** | `update_plan` + `codex_delegate.rs` | conversation state | 内部 dispatch 调 reviewer | **内部 Rust 函数**（不进 catalog） | 否 | 内部派 reviewer，模型不见，过不过最终用户拍。 |
| **hermes-agent** | `delegate_task` 通用派发 | 自定义 store | 角色内置 reviewer 模板 | 通用 dispatch + role | 否 | 复用 dispatch 工具。 |
| **openclaw** | `update_plan` + `sessions_spawn` | per-session | 由用户决定派 reviewer | LLM-facing dispatch | 否 | LLM 自己叫 reviewer。 |
| **本仓库 `create_plan`** | `create_plan` | `~/.tomcat/plans/...` | **写完即同步派 reviewer** | **internal subagent dispatch** | **否（reviewer 仅辅助）** | 落盘 + 自动审稿；过不过最终由用户 `/plan build` 拍。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| C1 落盘路径 | 跟运行轨迹 / 工程开发计划是否混 | 单独一个根 `~/.tomcat/plans/` |
| C2 文件锁 | advisory vs mandatory | advisory + 超时 |
| C3 schema | frontmatter 必填字段 | plan_id/goal/mode/todos/created_at（精简版） |
| C4 审稿时机 | 写完立刻 / 仅显式触发 | 写完即同步派 reviewer |
| C5 审稿者形态 | LLM-facing tool / internal dispatch | internal dispatch（与 codex 同构） |
| C6 reviewer 改稿权 | LLM 入参 / runtime 内部参数 | **runtime 内部参数**，**不**进工具 schema |
| C7 进执行态 | reviewer accepted 自动 / 用户显式 | **用户显式** `/plan build <plan_id\|path>` |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§13） | 说人话 |
|----|------|------------------|--------|
| G1 | `create_plan` 仅 `mode == Planning` 进 catalog | `create_plan_visible_only_in_planning` | 只有规划态能创建计划文件。 |
| G2 | 写入前拿 advisory file lock | `plan_file_lock_is_exclusive` | 写之前先抢锁。 |
| G3 | 写入后同步内联 reviewer，**摘要**同条 ToolResult 返回；**verdict 不做 gate** | `create_plan_internally_dispatches_reviewer` | 写完立刻审，结果同条返回；过不过由用户后续 `/plan build` 拍。 |
| G4 | reviewer `allowed_tools` 与改稿权由 **runtime 内部硬编码**，不暴露 LLM | `reviewer_subagent_blocks_bash_todos_checkpoint` | 审稿员工具名单代码写死。 |
| G5 | LLM 永远不直接写 frontmatter；frontmatter 由 `create_plan` / `update_plan` / runtime / 自动派生四方协同 | `frontmatter_never_written_by_llm_raw_edit` | YAML schema 不用让模型背。 |
| G6 | frontmatter round-trip 不丢字段 | `plan_file_round_trip_frontmatter` | 读写 frontmatter 不丢字段。 |
| G7 | 路径固定 `~/.tomcat/plans/<slug>_<hash>.plan.md` | `plan_file_path_fixed_under_dot_tomcat` | 计划文件固定在家目录下。 |
| G8 | PLAN 模式期间允许 LLM `write/edit` 计划文件**正文**；frontmatter diff 硬拦截 | `plan_mode_raw_edit_body_allowed_frontmatter_rejected` | 正文可改，YAML 锁死。 |

**说人话（§3 总览）**：规划态唯一创建计划入口——落盘带锁、写完内部派 reviewer、摘要跟在同一次 tool 结果里回来；reviewer 是辅助不是闸门。正文允许 raw `write/edit`，frontmatter 一律走工具。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| reviewer 作为 LLM 工具 | 见 [`reviewer.md`](./reviewer.md) §4 | 模型看不见 reviewer。 |
| 通用任意路径 write | 写域固定 `~/.tomcat/plans/` | 不能当万能写文件工具。 |
| 执行态调用 | catalog 已不可见 | build 之后不能再 `create_plan` 重写。 |
| LLM 写 frontmatter YAML | 由 `create_plan` 入参组装，runtime 拼接其余字段 | LLM 不背 schema。 |
| `/plan build` 之外自动进执行态 | 即使 reviewer accepted 也只是建议，用户拍板 | 不偷偷开干。 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| C1 落盘路径 | 固定 `~/.tomcat/plans/<slug>_<hash>.plan.md` | 写到 `agent_trail_dir` 与运行轨迹混；写到 `agents/plan/` 与工程模板混。 | 计划单独放 `~/.tomcat/plans/`。 |
| C2 文件锁 | advisory lock + `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS`（默认 2000ms） | mandatory lock 跨 OS 不一致；无锁会被并发写漂移。 | 写前抢 advisory 锁。 |
| C3 schema | frontmatter 必填 `plan_id` / `goal` / `mode` / `todos` / `created_at` / `schema_version`；`session_key` / `session_id` 在 `/plan build` 时写入；删除 `review_status` / `last_review` / `active` / `last_checkpoint_id` / `updated_at` | 字段越多机器越累，且 review 结果走 transcript 不需要 frontmatter 保存 | 字段精简，机读必备项齐就行。 |
| C4 审稿时机 | 写完立刻同步 await reviewer，摘要同条 ToolResult 返回 | 异步派会让 LLM 在没有 verdict 的情况下盲改；显式触发会要求 LLM 多一个 tool_call。 | 写完马上审，别分两轮。 |
| C5 审稿者形态 | internal subagent dispatch；不进 catalog | LLM-facing tool 把 reviewer 当通用工具会被乱调；通用 `dispatch_agent` 暴露过多 schema。 | 内部派子 Agent，不进工具表。 |
| C6 reviewer 改稿权 | **runtime 内部硬编码 `allow_review_edit: bool`**，**不**作为 `create_plan` 工具入参；允许用 `edit + update_plan` 修改计划内容，但 frontmatter raw 仍不允许 | LLM 入参形态会把改稿权下放给模型，违背「frontmatter 不让 LLM 碰」原则 | 改不改稿代码决定，不是模型决定。 |
| C7 进执行态 | 仅 `/plan build <plan_id\|path>` 显式触发 | reviewer accepted 自动 build 会绕过用户确认；多 plan 共存时无法二选一 | 用户拍板，工具不替。 |

### 4.2 实施点（拟定）

> 与 [`plan-runtime.md`](../plan-runtime.md) **PR-PLB** / **PR-PLC** 对齐；当前代码 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **CP-A** | built-in `create_plan` + 仅 `Planning` 可见；**交付**：catalog 条目 | `src/core/tools/contract/catalog.rs`、`src/api/chat/plan_runtime/catalog.rs`（拟定） | 见 §13：`create_plan_visible_only_in_planning`（PENDING） | 规划态才能创建计划文件。 |
| **CP-B** | `tool_exec::create_plan`：校验 mode、组装 `PlanFile` frontmatter、调 file_store + review；**交付**：工具出参含 `review_summary` | `src/api/chat/plan_runtime/tool_exec.rs`、`tools/create_plan.rs`（拟定） | 见 §13：`create_plan_internally_dispatches_reviewer`（PENDING） | 一次 tool 调用写完并审完。 |
| **CP-C** | 路径 `~/.tomcat/plans/<slug>_<hash>.plan.md`；advisory lock；tmp + atomic rename；frontmatter round-trip；**交付**：§5 schema | `src/api/chat/plan_runtime/file_store.rs`（拟定） | 见 §13：`plan_file_lock_is_exclusive`、`plan_file_round_trip_frontmatter`、`plan_file_path_fixed_under_dot_tomcat`（PENDING） | 落盘带锁、原子写、字段不丢。 |
| **CP-D** | `PlanRuntime::dispatch_reviewer`（方法 API，详见 [`reviewer.md` §4.3 / RV13](./reviewer.md#43-派发入口api-形态)）+ `AgentRegistry::spawn_subagent_internal`（[`multi-agent.md` §14.4.2.1 路径 B](../multi-agent.md#14421-调用栈与代码落点)）；`allowed_tools` 与 `allow_review_edit` 硬编码；reviewer 改稿时跳过递归内联；**交付**：`ReviewSummary`（不含 verdict gate 字段） | `src/api/chat/plan_runtime/mod.rs`（`PlanRuntime::dispatch_reviewer` 方法）+ `src/api/chat/plan_runtime/review.rs`（prompt/parser helpers）+ `src/core/agent_registry.rs` | 见 §13：`reviewer_subagent_blocks_bash_todos_checkpoint`、`reviewer_create_plan_does_not_redispatch_reviewer`（PENDING） | 内部派审稿员，摘要同条返回。 |
| **CP-E** | 启动 `PlanRuntime::recover()`：坏文件跳过、`planning` 残留保留为草案、`pending` 残留可被 `/plan build` 续跑、多 active 归一；**交付**：recover warning 文案 | `file_store.rs::recover()`（拟定） | 见 §13：`recover_planning_keeps_draft_with_warning`、`recover_pending_resumable_via_build`（PENDING） | 重启把 plan 状态收拾干净；pending 计划可被 build 续跑。 |
| **CP-F** | transcript `plan.create` / `plan.review` 事件；**交付**：事件 schema | `src/infra/transcript/...`（既有） | 集成测试挂接（PENDING） | 写计划记一笔到 transcript；review 摘要也单独落一条。 |
| **CP-G** | PLAN 模式期 `tool_exec::write` / `tool_exec::edit` 拦截：path 必须在 `~/.tomcat/plans/`；frontmatter diff 硬拒；**交付**：拦截器规则 | `src/api/chat/plan_runtime/tool_exec.rs`（拟定） | 见 §13：`plan_mode_raw_edit_body_allowed_frontmatter_rejected`（PENDING） | 写盘路径白名单 + frontmatter 写不动。 |

下文按实施点展开**技术要点与示意图**；**PlanFile 字段级协议仍以 [§5](#5-planfile-文件协议) 为准**。

#### 4.2.1 CP-A：`create_plan` 注册与可见过滤

- **交付**：`BUILTIN_TOOL_CATALOG` 注册 `create_plan`；**仅** `Planning` 可见；`Chat` / `Executing` / `Completed` / `Pending` 均剔除（PLAN 之外要修计划只能 `/plan exit` 回 CHAT，或者 raw `write/edit` 正文）。

**说人话**：create_plan 只在 PLAN 模式出现。

#### 4.2.2 CP-B：`tool_exec::create_plan` 编排

- **交付**：单入口串行：`validate(mode==Planning)` → `derive_path(goal)` → `runtime.build_frontmatter(input)`（仅 LLM 填 `goal`/`draft`/`todos`；其余由 runtime 拼接） → `file_store::write_plan`（**释放 advisory lock**，[`reviewer.md` RV14](./reviewer.md#41-落地选型决策表)） → `PlanRuntime::dispatch_reviewer(plan_id, allow_review_edit=runtime_default)` → 据摘要写 transcript → 构造 `ToolResult { plan_id, path, review_summary }`。
- **mutex**：与 reviewer 派发共用 `PlanRuntime` 锁，避免并发双写 + 双审。

```text
  LLM tool_call create_plan ({ goal, draft, todos })
        │
        ▼
  runtime.build_frontmatter (LLM 入参 + runtime 默认值)
        │
        ▼
  write_plan (lock → tmp → rename)
        │
        ▼
  PlanRuntime::dispatch_reviewer (sync await; allow_review_edit 由 runtime 决定)
        │
        ▼
  transcript: plan.create + plan.review
        │
        ▼
  ToolResult { plan_id, path, review_summary }
```

**说人话**：写盘和审稿在同一次工具调用里串起来；LLM 只交「人写的部分」，其余由 runtime 兜底。

#### 4.2.3 CP-C：`file_store` 落盘

- **交付**：`slug(goal)` + `hash(goal+ts)` 派生路径；`serde_yaml` frontmatter；正文锚点 `## Goal` / `## Draft` / `## Notes` / `## Todos Board` 段落级重写。
- **锁**：`<path>.lock` + `fs2::try_lock_exclusive`；超时 `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS`。

```text
  acquire_lock
        │
        ▼
  write(.tmp) ──rename──▶ .plan.md
        │
        ▼
  release_lock
```

**说人话**：先写临时文件再 rename，别人占锁就报错。

#### 4.2.4 CP-D：内联 reviewer（见 [`reviewer.md`](./reviewer.md)）

- **交付**：`PlanRuntime::dispatch_reviewer(plan_id, allow_review_edit)` 调 `AgentRegistry::spawn_subagent_internal`（[`multi-agent.md` §14.4.2.1 路径 B / §14.4.2.2](../multi-agent.md#14421-调用栈与代码落点)）；`SubagentType::Reviewer`、`Role::Leaf`；`parent.subagent_type == Reviewer` 时 `create_plan` **不再**二次派 reviewer。
- **改稿权 runtime 参数**：`PlanRuntime::dispatch_reviewer` 的内部参数 `allow_review_edit: bool`（默认 `false`），**不**作为 `create_plan` 工具入参；当为 `true` 时给 reviewer 加只读 + 可改 `~/.tomcat/plans/*.plan.md` 正文任意段的能力（frontmatter raw 仍不允许）；reviewer 通过 `update_plan` 改 frontmatter `todos[]` 后，`PlanRuntime` 在 await 返回时**重读 PlanFile** 刷新内存快照（[`reviewer.md` RV15](./reviewer.md#41-落地选型决策表)）。
- **输出契约**：reviewer 最终消息含 `summary:` 自由文本（≤600 字符）；修改说明与审稿摘要只落 `transcript.plan.review` / `ToolResult.review`，**不**写入 `PlanFile.body` 或 frontmatter。

**说人话**：审稿细节在 reviewer spec；这里只保证 create_plan 写完必审、审完必把摘要回填到 ToolResult 与 transcript。

#### 4.2.5 CP-E：`recover()`

- **交付**：扫描 `~/.tomcat/plans/*.plan.md`；不可解析 → warning skip；`mode==planning` 残留 → 保留为草案（用户回 PLAN 模式重新编辑或 `/plan build` 后续 pending）；`mode==pending` → 等待 `/plan build` 续跑；多 plan 共存 → 没有「active」概念，由 `/plan build <plan_id>` 二选一。

**说人话**：启动时只看 frontmatter，pending 的等用户敲 build 续跑。

#### 4.2.6 CP-F：transcript `plan.create` / `plan.review`

- **交付**：每次成功 `create_plan` 写 `plan.create`（plan_id / path）；reviewer 派发完成后写 `plan.review`（plan_id / summary / round / applied_changes）；失败写盘则不写事件。

**说人话**：成功写计划才在 transcript 留痕；review 单独记一条。

#### 4.2.7 CP-G：PLAN 模式 raw write/edit 拦截

- **交付**：`tool_exec::write` 与 `tool_exec::edit` 在 `current_mode() == Planning` 期间：
  1. **路径白名单**：path 必须在 `~/.tomcat/plans/*.plan.md`，其它路径拒绝；
  2. **frontmatter 拦截**：把新旧 frontmatter 反序列化为 `serde_yaml::Value` 后做 diff，**任何 frontmatter 字段变化**（包括新增、删除、修改）→ tool error，提示「frontmatter 由 todos / `/plan` 命令更新，正文可自由 raw edit」；
  3. **正文**自由放行。
- **其它模式**（CHAT / EXEC / COMPLETED / PENDING）：`write` / `edit` 对 `~/.tomcat/plans/*.plan.md` 同样套用 ②③，**但不**做路径白名单（其它文件正常放行）。

```text
  tool_exec::write / edit
        │
        ▼
  path 在 ~/.tomcat/plans/*.plan.md ?
        │
   no ──┴── yes
   │           │
   ▼           ▼
  PLAN 模式? ──▶ frontmatter diff ≠ 空 ?
   yes/no            │
   │                no ──┴── yes
   │                 │         │
   │                 ▼         ▼
   │              放行        tool_error
   │
   PLAN ──▶ tool_error（路径不在白名单）
   else ──▶ 放行
```

**说人话**：PLAN 模式只能改 plan 文件；任何模式下都别让 LLM 用 raw write 修计划文件的 YAML 字段。

---

## 5. PlanFile 文件协议

### 5.1 路径与文件名

```
~/.tomcat/plans/<slug>_<hash>.plan.md
```

| 段 | 取值 | 备注 | 说人话 |
|----|------|------|--------|
| `~` | `dirs::home_dir()` 或测试覆写 `TOMCAT_HOME` | 与 [`work-dir-and-data-layout.md`](../work-dir-and-data-layout.md) 一致 | 用户家目录下。 |
| `<slug>` | `slug(goal, 40)`：ASCII，空格 / 非字母数字转 `-`，超长截断 | 命中名 collision 不致命，靠 hash 分辨 | 目标变短文件名。 |
| `<hash>` | `xxh32(goal + ts_ms)` 十六进制截 8 字符 | collision 时 `goal + new_ts_ms` retry | 后缀防重名。 |

**说人话**：计划文件路径 = 家目录 + `plans/` + 目标 slug + 时间 hash，一眼能认是哪次规划。

### 5.2 frontmatter schema（精简版）

```yaml
---
plan_id:        plan_<slug>_<hash>      # = "plan_" + slug + "_" + hash
goal:           "<原始 goal 一行>"
mode:           planning | executing | completed | pending
session_key:    "<执行会话的路由键，build 时写入>"   # planning 期为 null
session_id:     "<执行会话的 transcript id，build 时写入>"  # planning 期为 null
created_at:     "<rfc3339>"
schema_version: 1
todos:
  - id:           t-001
    content:      "<step content>"
    status:       pending | in_progress | completed | cancelled
---
```

| 字段 | 何时写 | 何时改 | 写入方 | 说人话 |
|------|--------|--------|--------|--------|
| `plan_id` | `create_plan` 落盘 | 不可变 | `create_plan` runtime | 计划身份证。 |
| `goal` | `create_plan` 入参 | LLM 在 PLAN 期再次 `create_plan` 可改 | `create_plan` runtime | 目标原文。 |
| `mode` | `create_plan` 默认写 `planning` | `/plan build` → `executing`；全 todo completed → `completed`；cancel_token / process exit → `pending` | `create_plan` / runtime（**不**由 LLM 直接写） | 整盘阶段。 |
| `session_key` / `session_id` | `/plan build` 时由 runtime 写入 | 不可变（provenance） | runtime | 哪个会话开干的。 |
| `created_at` | `create_plan` 落盘 | 不可变 | `create_plan` runtime | 创建时刻。 |
| `schema_version` | `create_plan` 落盘 | runtime 升级时迁移 | runtime | 兼容性版本。 |
| `todos` | `create_plan` 入参 | `update_plan` 工具（任意模式）改 `status` / `content` / 增删；PLAN 期 `create_plan` 重写整列 | `create_plan` / `update_plan` | 步骤清单。 |

> **删除字段（与历史 schema 的差异）**：`review_status` / `last_review` / `active` / `last_checkpoint_id` / `updated_at` / `org_session_key` / `org_session_id` 全部**不再保留**——review 结果走 transcript `plan.review`；active 状态由 `mode` 派生；checkpoint id 由 `CheckpointStore` 自己持有；updated_at 通过文件 mtime 推断；session 信息只在 build 时绑一次（无 org_ 前缀）。详见 §15。

### 5.3 正文骨架

```markdown
# <goal>

## Goal
<original objective verbatim>

## Draft
<plan body markdown，由 LLM 写>

## Notes
<自由备注；LLM/用户随手记>

## Todos Board
<!-- todos-board:auto:begin -->
- [ ] t-001  <content>
- [/] t-002  <content>            # in_progress
- [x] t-003  <content>            # completed
- [-] t-004  <content>            # cancelled
<!-- todos-board:auto:end -->
```

> 正文「Todos Board」每次 `create_plan` / `update_plan` 写入时由 file_store 在 `<!-- todos-board:auto:begin -->` / `<!-- todos-board:auto:end -->` 标记之间**自动重写**，**不**接受人手编辑（手编会在下次写入时被覆盖）。`## Draft` / `## Goal` / `## Notes` 允许 LLM 在 **PLAN 模式**对路径 `~/.tomcat/plans/*.plan.md` 做 raw `write/edit`；EXEC 模式 plan 文件全禁写，只有 `update_plan` 能推进 frontmatter。

**说人话**：frontmatter 是机器真相；正文 Goal/Draft/Notes 给人看；Todos Board 由 runtime 自动生成；reviewer 摘要只进 transcript 和 tool result，不塞回 `.plan.md`；PLAN 期正文允许 raw 改、EXEC 期 plan 文件锁死；frontmatter 任何模式都拒绝 raw 改。

### 5.4 advisory file lock

| 行为 | 实现 | 说人话 |
|------|------|--------|
| 锁文件 | `<plan_path>.lock`（同目录） | 锁文件挨着计划文件。 |
| 实现 | `fs2::FileExt::try_lock_exclusive()` | 独占 advisory 锁。 |
| 等待 | `TOMCAT_PLAN_FILE_LOCK_TIMEOUT_MS`（默认 2000ms）内重试 | 最多等 2 秒。 |
| 失败 | tool error；附 lock holder pid（如可读） | 别人占锁就报错。 |
| 释放 | 整个 `apply` 闭包结束（写入 + 原子 rename + 释放） | 写完自动放锁。 |

### 5.5 原子写入

```rust
// file_store::write_plan(plan: &PlanFile) -> Result<()>
let tmp = path.with_extension("plan.md.tmp.<rand>");
fs::write(&tmp, frontmatter_and_body)?;
fs::rename(&tmp, &path)?; // atomic on POSIX same-fs
```

**说人话**：先写临时文件再 rename，避免写一半崩了留下半截。

### 5.6 recover 流程

启动期 `PlanRuntime::recover()` 从 `~/.tomcat/plans/` 读取所有 `*.plan.md`：

1. 解析 frontmatter；不可解析的写 warning 并跳过。
2. `mode == planning` 残留 → **保留**为草案，提示用户在 PLAN 模式重新打开（无 active 概念，无需强转）。
3. `mode == pending` → 等待用户 `/plan build <plan_id>` 续跑；runtime 把它列入「可恢复列表」。
4. `mode == executing` 残留 → 强制降级为 `pending`（前一进程崩溃的语义等价于被 cancel_token 截断）；warning。
5. `mode == completed` → 只读展示。
6. 多份 `pending` 共存 → 不强制选一份；由用户在 `/plan build` 时指定 `plan_id` / 路径。

**说人话**：重启时扫 `~/.tomcat/plans/`——坏的跳过，planning 留着，executing 残留视为 pending，pending 等用户 build 续跑。

### 5.7 修订路径（`update_plan` 增量 vs `create_plan` 整盘重写）

**拍板结论**：PlanFile 的修订分**两档**——

| 场景 | 用哪个工具 | 模式可见性 | 触发 reviewer？ | 说人话 |
|------|-----------|-----------|-----------------|--------|
| 改单条 / 几条 todo 状态（pending → in_progress → completed / cancelled） | [`update_plan`](./update-plan.md) | 任何模式 | ✗ | 推进 plan 用 update_plan。 |
| 新增一条 todo | [`update_plan`](./update-plan.md)（`upsert` 新 id） | 任何模式 | ✗ | 加一条不重写整盘。 |
| 改 todo 文案 / 重排整列 | [`update_plan`](./update-plan.md)（`upsert` / `replace=true`） | 任何模式 | ✗ | 小中型修订优先增量做。 |
| 整盘结构重写（goal 改写、方案重切、todo 大幅重排） | **`create_plan`** | 仅 PLAN 模式 | ✓ 同步派 reviewer | 推倒重来。 |
| 改 plan 正文任意段 | raw `write` / `edit`（**PLAN** 模式：路径白名单 `~/.tomcat/plans/*.plan.md`；**EXEC** 模式：禁止；CHAT/Pending/Completed：常规权限） | PLAN / CHAT / Pending / Completed | ✗ | EXEC 期 plan 文件锁死，其它模式允许 raw 改。 |
| 改 frontmatter `mode` / `session_*` / `plan_id` / `created_at` / `goal` / `schema_version` | **任何工具都不能**；由 runtime 写 | — | — | 机器字段保留给 runtime。 |

```text
                  改 PlanFile 的什么？
                          │
   ┌──────────────────────┼─────────────────────────────────────┐
   │                      │                                     │
   ▼                      ▼                                     ▼
frontmatter            正文                              整盘重写
todos                  Goal / Draft / Notes / Board      （结构大变）
   │                      │                                     │
   ▼                      ▼                                     ▼
 update_plan            raw write / edit                  create_plan
 (any mode)             (PLAN: 路径白名单)                 (PLAN only, 派 reviewer)
```

**推荐流**：

1. **小修小补**（标 todo 完成、加一条、改个文案） → 任何模式直接 `update_plan`，不进 PLAN 也行；
2. **方向调整但不大改结构** → 进 PLAN，先 `update_plan` 调几下；reviewer 派发由 `create_plan` 触发，所以这步不会派 reviewer，但用户可手动让 LLM 用 `dispatch_agent` 起 explore subagent 自审；
3. **结构推倒重来** → 进 PLAN，调 `create_plan` 整盘重写；自动派 reviewer，摘要落 transcript。

`create_plan` 调用时的语义仍是「**rewrite**」——LLM 要把保留的 todo（含已 completed 状态）原样填回入参；runtime 不做增量 diff。

---

## 6. 协议（入参 / 出参 / Schema）

### 6.1 工具 JSON Schema

```json
{
  "name": "create_plan",
  "description": "Draft a new PlanFile, or REWRITE an existing one wholesale, under ~/.tomcat/plans/. Only callable while in PLAN mode. After persisting, internally dispatches a reviewer subagent and returns its summary in the same tool result. The reviewer is advisory only — entering EXEC mode requires the user to issue `/plan build <plan_id|path>` separately.\n\nWhen to use this vs `update_plan`:\n- create_plan = WHOLESALE rewrite of the plan body (goal / draft / todos). Use it on first draft, or when the plan structure changes substantially.\n- update_plan = INCREMENTAL edit of frontmatter `todos[]`. Use it 99% of the time you want to mark a todo done, add a single todo, or revise one item in place. update_plan is callable in ANY mode; create_plan is PLAN-only.\n\nNotes for the model: you only provide `goal` / `draft` / `todos`. The runtime fills the rest of the frontmatter (plan_id, mode, created_at, session_key/id when /plan build runs). Do NOT touch frontmatter YAML via raw write/edit.",
  "parameters": {
    "type": "object",
    "properties": {
      "goal":          { "type": "string", "description": "High-level objective; reused for slug and frontmatter `goal`." },
      "draft":         { "type": "string", "description": "Plan body markdown for the `## Draft` section. Free-form, human-readable." },
      "todos": {
        "type": "array",
        "description": "Initial todo list. Status defaults to `pending` if omitted; in_progress is allowed but rare in planning.",
        "items": {
          "type": "object",
          "properties": {
            "id":           { "type": "string" },
            "content":      { "type": "string" },
            "status":       { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] }
          },
          "required": ["id", "content"]
        }
      }
    },
    "required": ["goal", "draft", "todos"]
  }
}
```

> **入参精简说明**：LLM 只填三个字段。`plan_id` / `mode` / `created_at` / `session_key` / `session_id` / `schema_version` 全部由 runtime 在 `build_frontmatter` 中拼接，**模型不背 frontmatter schema**。**`apply_changes` 字段已下线**——reviewer 改稿权改为 runtime 内部参数（详见 §4.2.4 与 [`reviewer.md`](./reviewer.md) §4）。

### 6.2 出参

```jsonc
{
  "type": "object",
  "properties": {
    "plan_id":        { "type": "string" },
    "path":           { "type": "string", "description": "Absolute plan file path" },
    "review_summary": {
      "type": "object",
      "description": "Advisory summary from the inline reviewer subagent. Not a gate; the user decides whether to /plan build.",
      "properties": {
        "summary":         { "type": "string", "description": "Reviewer rationale, <= 600 chars" },
        "rounds":          { "type": "integer", "description": "Cumulative reviewer runs for this PlanFile" },
        "applied_changes": { "type": "boolean", "description": "Whether reviewer applied any plan edits during review (controlled by runtime, not LLM)" }
      },
      "required": ["summary", "rounds", "applied_changes"]
    },
    "warnings":       { "type": "array", "items": { "type": "string" } }
  },
  "required": ["plan_id", "path", "review_summary"]
}
```

**说人话**：入参是 goal/draft/todos；出参必带 plan_id、路径和 review 摘要（仅辅助，不做 gate）。

---

## 7. 内联 reviewer 派发（internal subagent dispatch）

### 7.1 派发入口

`dispatch_reviewer` 是 `PlanRuntime` 的方法（见 [`reviewer.md` §4.3](./reviewer.md#43-派发入口api-形态)）。`review.rs` 只保留 prompt / 解析 helpers；本节示意调用方契约：

```rust
// src/api/chat/plan_runtime/mod.rs（拟定）
impl PlanRuntime {
    pub async fn dispatch_reviewer(
        &self,
        plan_id: &PlanId,
        allow_review_edit: bool,        // 默认由 runtime 配置决定，不暴露 LLM
    ) -> Result<ReviewSummary> {
        // RV14：write_plan 已释放 advisory lock 后才能调用本方法
        let allowed_tools = if allow_review_edit {
            // 默认全部 + update_plan（改 frontmatter todos）+ edit（改 plan 正文）
            // 永远不含 create_plan（防套娃）；通用 edit 在 reviewer 路径上有路径白名单 + frontmatter diff guard
            &["read", "grep", "find", "todos", "update_plan", "edit"][..]
        } else {
            // 默认含 todos（个人 scratchpad，写 reviewer 子 Agent 自己的 .todo.md，无副作用）
            &["read", "grep", "find", "todos"][..]
        };

        let parent = self.deps.agent_registry.get(&self.session_id())?;
        let cfg = AgentLoopConfig {
            session_id:        format!("{}:sub:reviewer:{}", parent.session_id, Uuid::new_v4()),
            parent_session_id: Some(parent.session_id.clone()),
            spawn_depth:       parent.spawn_depth + 1,
            subagent_type:     SubagentType::Reviewer,    // internal-only 枚举位
            role:              Role::Leaf,
            tool_definitions:  resolve_internal_tools(&self.deps.parent_catalog, allowed_tools),
            // ... model / max_turns / system_prompt（reviewer 模板，见 reviewer.md §5.1）
            ..Default::default()
        };

        let summary = self.deps.agent_registry
            .spawn_subagent_internal(&self.spawn_deps(), &parent, cfg,
                                     vec![build_review_prompt(&self.state.read_plan(plan_id)?,
                                                              allow_review_edit)])
            .await?
            .try_into_review_summary()?;

        // RV15：reviewer 通过 update_plan 改了 frontmatter → 刷新内存快照
        if summary.applied_changes {
            self.state.reload_from_disk(plan_id)?;
        }
        Ok(summary)
    }
}
```

### 7.2 与 `multi-agent.md` 的关系

- 完整复用 §14 基础设施：`AgentRegistry` / `spawn_depth` / `CascadeAbort` / `SubAgentStart`/`End` 事件。
- **不**走 `dispatch_agent` schema：`subagent_type` 取 internal-only 的 `Reviewer` 枚举位（不出现在 `dispatch_agent` 的 `enum` 列表）；`allowed_tools` 在调用方代码里硬编码，LLM 看不到也改不了。
- 详见 [`multi-agent.md`](../multi-agent.md) §14.6.1。

### 7.3 reviewer 输出与状态转移（**非 gate**）

| 输出 | 反馈给用户 | `PlanRuntime` 转移 | 说人话 |
|------|------------|--------------------|--------|
| `summary`（自由文本） | 同条 ToolResult 返回；同时写 `transcript.plan.review` | **不**修改 `mode` | 摘要写给人看，不动状态机。 |
| reviewer 用 `update_plan` 修订 frontmatter `todos[]`（仅 `allow_review_edit=true`） | 用户看 `.plan.md` 时能直接读到新版 todos | **不**修改 `mode`（仅当 EXEC + target.mode==executing + 同 session 时才派生 completed，reviewer 通常不满足这条件） | 审稿员能直接调待办。 |
| reviewer 用 `edit` 改 plan 正文（仅 `allow_review_edit=true`） | 用户看 `.plan.md` 时能直接读到 | **不**修改 frontmatter | 改稿可直接修正文。 |
| reviewer 异常退出 | tool error；`PlanFile` 保留 | **不**修改 `mode`（停留 `planning`） | 审稿挂了不影响计划文件本身。 |

> **关键改动**：reviewer 不再是 verdict gate。是否进入 `executing` **仅由** `/plan build <plan_id|path>` 命令决定（详见 [`plan-runtime.md`](../plan-runtime.md) §5.1 与 §8）。

详细契约见 [`reviewer.md`](./reviewer.md) §5。

### 7.4 reviewer 改稿权（runtime 控制）

| `allow_review_edit`（runtime 参数） | reviewer 可写范围 | 说人话 |
|-------------------------------------|------------------|--------|
| `false`（默认） | `{read, grep, find, todos}`；只能产出 summary（`todos` 写 reviewer 自己的 `.todo.md`，不影响 plan） | 审稿员只读 + 自己记笔记。 |
| `true` | 上述 + `{update_plan, edit}`：`update_plan` 改 frontmatter `todos[]`；`edit` 可改 `~/.tomcat/plans/*.plan.md` 的正文任意段；**仍不能** raw 改 frontmatter | 审稿员可直接修正文 + 调 todos。 |

> reviewer **永远**不能调用 `create_plan` 工具（任何模式都不在 `allowed_tools` 内 + `tool_exec.create_plan` 在 `subagent_type == Reviewer` 路径直接 tool error，双保险）；改 plan 内 `todos[]` 走 [`update_plan`](./update-plan.md)；改正文走通用 `edit`（路径白名单 + frontmatter diff guard）。

**说人话**：内部派 reviewer 复用 multi-agent 底座但不走 dispatch_agent；摘要不做 gate；reviewer 改稿权由代码决定（默认只读 + `todos` scratchpad），打开改稿就能用 `update_plan` + `edit` 落地建议。

---

## 8. 写盘路径白名单与 frontmatter 硬拦截

### 8.1 LLM 写计划文件的合法路径

| 场景 | 工具 | 路径约束 | 字段约束 | 说人话 |
|------|------|----------|----------|--------|
| PLAN 期建初稿 / 整盘重写 | `create_plan` | `~/.tomcat/plans/*.plan.md`（runtime 派生） | LLM 只填 goal/draft/todos | 主要入口；仅 PLAN 可见。 |
| PLAN 期改正文 | raw `write` / `edit` | **必须**在 `~/.tomcat/plans/*.plan.md` | **frontmatter 整体不可变**，正文可改 | PLAN 模式下其他路径写盘全部拒绝。 |
| 任意模式改 frontmatter `todos[]` | [`update_plan`](./update-plan.md) | 目标 PlanFile（按 `plan_id` 路由） | LLM 仅传 ops，runtime 改 YAML；只能动 todos | 推进 plan 待办用 update_plan。 |
| 任意模式改正文 | raw `write` / `edit` | `~/.tomcat/plans/*.plan.md`（PLAN 模式仅限本盘） | 同 PLAN 期正文约束（frontmatter 不可变） | 笔记可补，YAML 锁死。 |
| 整盘 mode 切换 | `/plan build` + runtime；自动派生 mode=completed（由 `update_plan` 触发）/ mode=pending（cancel_token 触发） | 目标 PlanFile | runtime 写 `mode` / `session_key` / `session_id` | 用户拍板 + runtime 兜底。 |
| 任意模式记会话级 scratchpad | [`todos`](./todos.md) | `~/.tomcat/agents/<agentId>/todos/*.todo.md` | 不写 plan.md | 写自己的 .todo.md。 |

### 8.2 frontmatter diff 硬拦截

```text
  tool_exec::write / edit
        │
        ▼
  parse(target_path) ──in ~/.tomcat/plans/*.plan.md ?
        │           no ──▶ PLAN 模式? ──yes──▶ tool_error (路径白名单)
        │                                 no ──▶ 放行（其它文件）
        │ yes
        ▼
  read old file → parse frontmatter
        │
        ▼
  parse new content → parse frontmatter
        │
        ▼
  YAML diff (semantic, not textual) ──≠ 空 ?
        │              yes ──▶ tool_error "frontmatter must be updated via update_plan / /plan command; raw edit not allowed"
        │              no  ──▶ 放行（仅正文改了）
```

**说人话**：写 plan 文件之前先看 frontmatter 是不是动了，动了就拒绝并告诉模型「`todos[]` 用 [`update_plan`](./update-plan.md) 改；`mode` / `session_*` 由 `/plan` 命令族 + runtime 写；正文可以 raw write/edit」。

---

## 9. One-Glance Map

| 路径 | 职责 | 说人话 |
|------|------|--------|
| `src/api/chat/plan_runtime/catalog.rs`（拟定） | `mode == Planning` 时把 `create_plan` 注入可见集 | 规划态才可见。 |
| `src/api/chat/plan_runtime/tool_exec.rs`（拟定） | `create_plan` 入口校验；调用 `file_store::write_plan`；调用 `PlanRuntime::dispatch_reviewer`（[`reviewer.md` §4.3](./reviewer.md#43-派发入口api-形态)）；写 transcript；PLAN 期 write/edit 拦截 | 工具总入口 + raw 写盘拦截。 |
| `src/api/chat/plan_runtime/file_store.rs`（拟定） | 路径派生、frontmatter round-trip、advisory lock、原子写、recover | 读写计划文件。 |
| `src/api/chat/plan_runtime/review.rs`（拟定） | reviewer 派发入口、`allowed_tools` 与 `allow_review_edit` 硬编码 | 内部派审稿员。 |
| `src/core/agent_registry.rs`（既有/拟扩展） | `spawn_subagent_internal(...)`；复用 `multi-agent.md` §14 | 子 Agent 注册表。 |
| `src/infra/transcript/...`（既有） | `plan.create` / `plan.review` 事件落盘 | 记一笔写计划 + review 摘要。 |

**阅读顺序（说人话）**：catalog 注入 → tool_exec 校验 → file_store 落盘 → review 派 reviewer → transcript → 把含 review 摘要的结果还给 LLM。

---

## 10. 调度时序

```
LLM ──tool_call("create_plan", { goal, draft, todos })──▶ tool_exec
                                                                          │
                                                校验 mode == Planning（若否 → tool_error）
                                                                          │
                                                                          ▼
                                          runtime.build_frontmatter（LLM 入参 + 默认值）
                                                                          │
                                                                          ▼
                                      file_store::write_plan(...)（advisory lock + atomic rename）
                                                                          │
                                                                          ▼
                                      PlanRuntime::dispatch_reviewer(plan_id, allow_review_edit=runtime_cfg)
                                                                          │
                                                          （internal subagent dispatch）
                                                                          │
                                                                          ▼
                                                 ReviewSummary { summary, rounds, applied_changes }
                                                                          │
                                                                          ▼
                                          transcript: plan.create + plan.review
                                                                          │
                                                                          ▼
                                      ToolResult 返回 LLM（含 review_summary；mode 仍 planning）

  ╳ create_plan 不再修改 mode；进入 executing 由 /plan build 触发（见 plan-runtime.md §5.1）
```

**说人话**：一次 create_plan = 校验 → 拼 frontmatter → 加锁写文件 → 同步审稿 → 记 transcript → 同条 tool 结果带回 review 摘要；mode 不动。

---

## 11. 状态机

`create_plan` 调用本身的内部子状态：

```
┌──────────┐  acquire lock  ┌──────────┐  write  ┌──────────┐  dispatch  ┌─────────────┐
│ Pending  │───────────────▶│  Locked  │────────▶│ Persisted│───────────▶│ Reviewing   │
└──────────┘                └─────┬────┘         └──────────┘            └──────┬──────┘
                                  │ timeout/fail                                │
                                  ▼                                             ▼
                            ┌──────────┐                              ┌─────────────────┐
                            │  Error   │                              │ Summary Emitted │
                            └──────────┘                              └─────────────────┘
```

整体 `PlanRuntime` 的 mode 转移由 [`plan-runtime.md`](../plan-runtime.md) §8 定义；create_plan 本身**不**改 mode。

**说人话**：单次调用内：抢锁 → 写盘 → 审稿 → 摘要回填；锁失败或写失败就 Error，审稿挂了文件仍留着。

---

## 12. 错误模型 / 截断 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| `mode != Planning` | catalog 已不可见；强行调用返回 tool error，附 usage `先 /plan "<objective>"` | 非规划态不能创建计划。 |
| advisory lock 获取失败 | tool error（附 holder pid） | 锁被占就拒写。 |
| frontmatter 序列化 / 反序列化失败 | tool error；保留旧文件（原子 rename 未触发） | 序列化挂了不动旧文件。 |
| reviewer 子 Agent 异常退出 | tool error 携带 reviewer stderr 摘要；`PlanFile` 保留，`mode` 仍 `planning` | 审稿员挂了不影响计划文件。 |
| reviewer 超 `max_review_rounds` 仍未给摘要 | 非错误；warning 提示「reviewer 未给出摘要」 | 审太多次就算了。 |
| `allow_review_edit=true` 但 reviewer 没改计划内容 | warning；不阻塞返回 | 开了改稿权但没改只 warning。 |
| transcript 写失败 | warning | 记盘失败不挡返回。 |
| recover 阶段发现 `mode == executing` 残留 | warning + 强制降级 `pending`；保留 frontmatter 其他字段 | 重启视为被 cancel_token 截断。 |
| recover 阶段发现 frontmatter 不可解析 | warning + skip | 烂文件不当 plan。 |
| PLAN 期 raw `write/edit` 改 frontmatter | tool error，usage「frontmatter 由 todos / /plan 命令更新」 | 硬拦截 YAML。 |
| PLAN 期 raw `write/edit` 写入 `~/.tomcat/plans/` 外路径 | tool error，usage「PLAN 模式仅允许写计划文件」 | 路径白名单。 |

---

## 13. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：catalog 可见性 | `create_plan_visible_only_in_planning`（待新增） | PENDING | 模式不对就别让模型看见。 |
| 单元：file lock | `plan_file_lock_is_exclusive`（待新增） | PENDING | 并发写要挡住。 |
| 单元：frontmatter round-trip | `plan_file_round_trip_frontmatter`（待新增） | PENDING | 字段不能丢。 |
| 单元：路径固定 | `plan_file_path_fixed_under_dot_tomcat`（待新增） | PENDING | 不许写到别处。 |
| 单元：reviewer allowed_tools 收紧 | `reviewer_subagent_blocks_bash_todos_checkpoint`（待新增） | PENDING | reviewer 不许调危险工具。 |
| 单元：reviewer 改稿权由 runtime 控制 | `reviewer_allow_edit_controlled_by_runtime`（待新增） | PENDING | LLM 入参没有 `apply_changes`。 |
| 单元：reviewer 防递归内联 | `reviewer_does_not_redispatch_reviewer`（待新增） | PENDING | 改稿时别再叫一个 reviewer。 |
| 单元：recover planning 保留 | `recover_planning_keeps_draft_with_warning`（待新增） | PENDING | 重启留着草案。 |
| 单元：recover executing 降级 pending | `recover_executing_demotes_to_pending`（待新增） | PENDING | 崩溃 == cancel。 |
| 单元：recover pending 可 build 续跑 | `recover_pending_resumable_via_build`（待新增） | PENDING | pending 续跑要能拉起来。 |
| 单元：LLM 入参无 frontmatter 字段 | `create_plan_inputs_have_no_frontmatter_schema`（待新增） | PENDING | 入参里不许有 mode / plan_id 等。 |
| 单元：PLAN 期 frontmatter raw edit 拒绝 | `plan_mode_raw_edit_body_allowed_frontmatter_rejected`（待新增） | PENDING | 正文放、YAML 拦。 |
| 集成：create_plan 内部派 reviewer | `create_plan_internally_dispatches_reviewer`（待新增） | PENDING | 摘要必须同条返回。 |
| 集成：create_plan 不改 mode | `create_plan_does_not_mutate_mode`（待新增） | PENDING | mode 转移在 /plan build。 |

---

## 14. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| reviewer 子 Agent 持久 hang | 高 | 复用 `multi-agent.md` §14 的 `CascadeAbort` 与 `max_turns`；reviewer 模板默认 `max_turns = 8` | 审稿员卡住要能级联 abort。 |
| `allow_review_edit=true` 时 reviewer 反复改稿不收敛 | 中 | reviewer 仅能改计划文件与 todos；`max_review_rounds` 默认 1；超限留 warning | 改稿别无限循环。 |
| frontmatter 字段漂移（schema 升级） | 中 | `schema_version` 字段；read 时不识别版本 → warning + 兼容模式 | 升级 schema 要兼容读。 |
| 多平台路径差异（Windows） | 中 | `dirs::home_dir()` + 路径校验；`<slug>` 限定 ASCII | Windows 路径要 sanitize。 |
| advisory lock 不被遵守 | 中 | 仅本仓库内的 `create_plan` / `update_plan` / runtime 三方获取；外部手编无锁 → recover 时通过 mtime 容错 | 手改文件靠 recover 容错。 |
| LLM 误以为 reviewer accepted 就该自己进 executing | 中 | description 明示「reviewer is advisory」；EXEC 入口仅 `/plan build` | 文档 + catalog 双保险。 |
| LLM 用 raw write 试图改 frontmatter | 高 | §8.2 硬拦截；tool error 附 usage 引导用 `todos` / `/plan` | 写盘前先拦一刀。 |

---

## 15. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把 reviewer 暴露为 LLM 可见 tool（`review_plan`）~~ | **否**：reviewer 走 internal dispatch；详见 [`reviewer.md`](./reviewer.md) §4。 | 审稿不进 catalog。 |
| ~~把 `create_plan` 与 `update_plan` 拆成两个工具（早期否决）~~ | **D 方案重新启用**：`create_plan` 仅 PLAN 模式 + 整盘重写；[`update_plan`](./update-plan.md) 任何模式 + 增量编辑；二者代码共享 op 引擎、提示词分裂。 | 拆成两个工具，职责更清晰。 |
| ~~路径放 `agent_definition_dir/agents/plan/`~~ | **否**：固定 `~/.tomcat/plans/`，与运行轨迹与 PLAN_SPEC 模板分离。 | 别和工程计划模板混。 |
| ~~把 PlanFile schema 单独写一篇 `plan-file.md`~~ | **否**：合并入本文 §5，与工具行为契约同处。 | 协议跟工具一篇写完。 |
| ~~异步派 reviewer，verdict 后续轮次再回填~~ | **否**：同步 await，摘要与文件写入同条工具结果返回。 | 必须同步带回 review 摘要。 |
| ~~`create_plan` 改名为 `plan`~~ | **否**：名字保留 `create_plan`，职责也不变（仅创建/重写 plan.md）；mode/todos 推进由 [`update_plan`](./update-plan.md)（增量）+ `/plan` 命令族 + runtime 自动派生协同。 | 工具名稳定，不折腾下游。 |
| ~~frontmatter 三方协同（`create_plan` + `todos` + runtime）~~ | **替代为四方**：`create_plan`（整盘初稿）+ [`update_plan`](./update-plan.md)（增量 todos）+ runtime（mode/session 绑定）+ 自动派生（all completed / cancel_token）。 | 四方各管一段。 |
| ~~mode=completed 由 `todos` 在 EXEC 全 completed 时触发~~ | **替代**：由 [`update_plan`](./update-plan.md) 在 EXEC + target.mode==executing + 全 completed 时触发；`todos` 永远不写 plan，自然不触发。 | 改 plan 的工具负责派生 mode。 |
| ~~CHAT 模式无法修订 plan.md 的 `todos[]`（D 方案前的缺口）~~ | **修复**：[`update_plan`](./update-plan.md) 任何模式可见，按 `plan_id` 路由。 | 修上一版的缺口。 |
| ~~frontmatter 保留 `review_status` / `last_review` / `active` / `last_checkpoint_id` / `updated_at`~~ | **否**：全部下线。review 走 transcript `plan.review`；active 由 `mode` 派生；checkpoint id 由 `CheckpointStore` 自管；updated_at 用 mtime；机器字段越少越省维护。 | schema 精简，只留必备。 |
| ~~frontmatter 加 `org_session_key` / `org_session_id` 表示创建源~~ | **否**：去掉 `org_` 前缀；改为 `session_key` / `session_id`，**仅在 `/plan build` 时写入**当前执行会话，作为 provenance；创建期不绑 session。 | 不区分「创建会话」与「执行会话」，build 一次性绑定。 |
| ~~`mode` 包含 `ready_to_apply`~~ | **否**：移除；reviewer 仅辅助，是否 build 由用户拍板，无需中间态。 | 状态机少一档。 |
| ~~`mode` 包含 `cancelled`~~ | **否**：cancel_token / 进程退出统一记为 `pending`，可被 `/plan build` 续跑；用户不要的计划由 `/plan exit` 在 PLAN 模式回 CHAT 即可，文件留着不强制收口。 | 别区分「取消」和「暂停」，留可续跑的余地。 |
| ~~工具入参含 `apply_changes`~~ | **否**：删除；reviewer 改稿权改为 **runtime 内部参数** `allow_review_edit`，模型看不到。 | 改稿决定权交给代码。 |
| ~~`/plan apply` 是进执行态入口~~ | **替代**：改名 `/plan build <plan_id\|path>`；同时承载「写 session_key/id」「reminder swap」「user meta 注入 plan 全文」「catalog swap」四件事。 | apply 名字不够直观，改 build。 |
| ~~`/plan close` 显式收口~~ | **下线**：完成由 `mode = completed`（runtime 自动派生）触发；不要可以 `/plan exit` 退 PLAN，文件留着；执行中按 Ctrl+C / 进程退出 → `pending`。 | 不要把「关闭」做成命令，状态自然演化。 |
| ~~reviewer verdict 二态做 gate（accepted → ReadyToApply）~~ | **否**：verdict 字段下线，只留 summary 字段；reviewer 改稿权由 runtime 控制；用户敲 `/plan build` 才进 executing。 | reviewer 是顾问不是法官。 |

---

## 16. 关联文档

- PLAN 模式整体规范：[planner.md](./planner.md)
- 运行时编排：[plan-runtime.md](../plan-runtime.md)
- reviewer 子 Agent 契约：[reviewer.md](./reviewer.md)
- 子 Agent 基础设施：[multi-agent.md](../multi-agent.md)
- PlanFile 增量编辑：[update-plan.md](./update-plan.md)
- 会话级待办：[todos.md](./todos.md)
- 结构化提问：[ask-question.md](./ask-question.md)
- checkpoint 底座：[checkpoint-resume.md](./checkpoint-resume.md)
- 标杆写法：[read.md](./read.md)
- 任务卡：[T2-P1-002.md](../../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)

**说人话**：写计划文件看本文 §5+§6+§8；审稿细节看 `reviewer.md`；整条流程看 `plan-runtime.md`。
