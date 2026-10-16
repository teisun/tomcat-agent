# `update_plan`：增量推进计划待办

```text
plan_id / path / active plan
        │
        v
原有路径授权：拒绝 / 确认 / 放行（等待确认时可取消）
        │
        v
原有文件锁：读最新计划 → 校验 → 应用 todo ops → 派生文件状态 → 原子提交
        │
        v
刷新本次提交的快照 / 面板 / 事件 → 返回完整 items 与 next_step
        ├─ 还有工作：继续实施
        ├─ 验收项进行中：提示 load_skill(verify)
        └─ 全部终态：完成（不自动启动 Code Reviewer / Verifier）
```

用户无需知道 YAML 的写法：修改计划待办用 `update_plan`，个人会话备忘用 `todos`；整份计划的创建由 `create_plan` 负责。三者不是相互同步的状态副本。

## 1. 职责与代码位置

- 实现：`src/core/tools/plan_tool/update_plan.rs`。
- 操作语义复用 `shared_todo_ops.rs::apply_shared_todo_ops`，没有独立的 PlanTodoStore/SessionTodoStore 框架。
- 调用者只能提交 todo 操作，不能通过本工具传 `goal`、`draft`、会话身份或其他机器字段；未知顶层参数拒绝。
- 运行时按提交结果派生文件 `state`，并重建正文 `## Todos Board` 的自动标记区间；正文其余部分不由此工具重写。
- 工具不改变 `AgentMode`，也不代替 `start_plan`、Build 或 `/plan build` 建立执行资格。文件完成与会话模式是两件事。

## 2. 入参与操作

| 字段 | 当前语义 |
| --- | --- |
| `plan_id` | 优先的目标 ID；匹配当前 active plan 时可解析到它已绑定的实际路径 |
| `path` | 没有 `plan_id` 时使用的显式路径；底层支持已绑定的外部计划，目录授权仍需遵守调用入口的权限规则 |
| `replace` | 默认 false；true 时以本批操作生成的新列表替换 todos，不是重写整个计划文件 |
| `ops` | 按顺序应用 `upsert`、`set_status`、`remove` |

- `upsert`：新 ID 需要 `content`；可设置 `status` 与 `todo_kind`（`work` / `acceptance`）。
- `set_status`：指定现有 ID 的状态。`evidence: string[]` 只可与 `completed` 一起提交，追加去重后持久化；旧 `content` 输入不是改写任务描述的入口。
- `remove`：移除指定 ID。状态取 `pending | in_progress | completed | cancelled`。
- 最多三个互不依赖的 `in_progress`；执行中已有 work todo 的 `content` 冻结，进展和验证写入 evidence。
- 协议校验最多一个 acceptance；“把最终验收项放最后”是工作流要求，不应夸称本工具强制生成验收项或执行验收命令。
- 缺少 evidence 的 completed 操作当前仍允许，不能把状态勾选当测试通过证明。

```json
{
  "plan_id": "plan_example_abcd1234",
  "ops": [{
    "kind": "set_status",
    "id": "focused-check",
    "status": "completed",
    "evidence": ["task:<actual-task-id>: focused regression passed"]
  }]
}
```

完整字段约束以实际工具 Schema、`UpdatePlanArgs` 与 `SharedTodoOpArg` 为准，不在文档复制一份易漂移的 Schema。

## 3. 目标与会话边界

解析顺序为 `plan_id → path → runtime.active_plan()`；三者都没有时报错。不要依赖另一个会话的 active plan。执行中计划必须匹配当前 `session_key`，检查位于文件事务内部；非执行计划可由其他会话修订。

LLM 入口在读计划和创建锁文件前，通过原有 PrimitiveExecutor 路径授权流程检查 Write；显式 deny/readonly 不可绕过，未知目录复用已有确认/SessionGrants。默认 plans 目录沿用 AgentPlansDir 的授权，不重复询问。批准后的规范化路径直接进入事务，不在审批等待后重新选取 active plan。内嵌的自定义 executor 若不实现 `authorize_write_path`，该原生更新入口默认拒绝；内部可信 Rust 调用 `execute` 仍共用同一个事务实现。

成功返回的是**本次提交**的完整快照，不是跨会话实时同步承诺。工具可更新已完成计划，但不能跳过 build 把它直接重新开工。
执行中修改另一份计划只返回那份计划的提交结果，不替换当前执行绑定；同一保护也覆盖 PLAN 中新建计划和评审回读。

## 4. 状态规则

### 4.3 op × 目标文件状态

| 目标状态 | 普通待办修改 | `upsert` / `set_status` 的 `in_progress` |
| --- | --- | --- |
| planning | 允许 | 拒绝 |
| pending | 允许 | 拒绝；先 build |
| executing | 仅 owning session_key | 允许，仍受内容冻结/并行数量限制 |
| completed | 允许；出现非终态工作时 reopen 为 pending | 拒绝；reopen 后仍需 build |

至少一个 todo 且全部为 completed/cancelled 时，文件可派生为 completed；空列表不视为完成。这是当前文件状态规则，不限定为旧的 `AgentMode::Executing`。已完成文件新增 pending 工作会回到 pending，并沿用当前会话模式。

## 5. 返回值与消费者

每次成功返回 `plan_id`、`path`、`applied`、`replace`、`plan_state_before/after`、`panel_snapshot_id`、`warnings`、`active_in_progress`、完整 `items` 和 `next_step { phase, hint }`。`active_in_progress` 只取第一个进行中 ID，不意味着只能有一个并行 todo；应通过 items 查看全貌。

面板使用提交后的 TodosPanelSnapshot。非终态发送 `plan.update` 和 `plan.todos`；完成发送 `plan.complete`，reopen 发送 `plan.pending`。这些文件事件不能被解释为会话模式切换，模式仍由专用事件表示。

## 6. 持久化顺序

`update_plan_locked` 在同一把既有 advisory 文件锁内读最新文件、验证 owning session、状态和冻结内容，再应用整批 ops、校验 acceptance 数量、派生 completed/reopen、重建 Todos Board 并提交。

完成事件不能在释锁后再次把旧 PlanFile 整体写回，否则会覆盖中间已经成功的另一条更新。提交后的缓存/面板/转录通知沿用原流程，不引入第二个状态账本、额外锁或自动重试。此处只保证协作写者的文件更新，不宣称跨会话事件全序或持久投递。

### 6.2 失败边界

状态/权限/内容/操作校验失败时整批拒绝，不能先写入其中的合法项。文件锁超时与真实 IO/解析失败继续返回原错误；不借文档恢复旧的自动 Code Review/Verifier 门禁，也不新增未实现的 `TOMCAT_UPDATE_PLAN_*` 配置项。文件锁等待上限目前固定为 2000ms（`file_store::DEFAULT_LOCK_TIMEOUT_MS`）；生产装配不读取 `[plan].lock_timeout_ms`，改这个配置不会生效。

## 7. 现有回归与验收

- `tests/update_plan_test.rs`：非 executing × 两种开始操作的拒绝及文件字节不变、工作描述冻结、evidence 持久化、完整快照、跨 session、外部已绑定路径、完成与 reopen。
- `tests/update_plan_completion_test.rs`：真实锁内旧快照 → 真实工具接受新工作 → 迟到完成通知，后一个提交不得被覆盖；无生产测试钩子。
- `tests/delivery_flow_test.rs` / `tests/lifecycle_test.rs` / `tests/verify_test.rs`：已有运行时交付/生命周期/验收提示链。
- `tests/reviewer_test.rs`：`create_plan` 独立评审须用户明确同意；不是 update_plan 完成时自动评审。

以上路径均相对 `src/core/tools/plan_tool/`。实际通过状态以验收命令日志为准，文档不复制一套 PENDING/PASS 看板。
另外，`src/core/agent_loop/tests/tool_exec_batch_test.rs` 经真实 Native Tool 入口覆盖 deny/readonly、允许的外部路径、未知目录确认/拒绝与拒绝前无文件副作用。

## 8. 退役机制与关联文档

旧 `Pending { id }` / `Completed { id }` 会话状态、`finalize_completed_to_chat`、最后一个 todo 触发同步 Code Review/Verifier、强制机器验收回执，以及旧草案 TodoStore 抽象均不能作为现行调用说明。查历史请用 Git；不要为符合旧文档重新实现它们。

- [create_plan：先保存，再选择评审](./create-plan.md)
- [会话级 todos](./todos.md)
- [Plan Runtime](../plan-runtime.md)
- [Reviewer](./reviewer.md)
- [会话存储](../session-storage.md)
