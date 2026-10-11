# `create_plan`：先保存计划，再由用户选择评审

```text
Plan 模式下提交 goal / draft / todos
          │
          ▼
校验 → 带锁保存 PlanFile → 刷新 ActivePlan → plan.create / plan.todos
          │                              （此时计划已可使用）
          ▼
询问是否进行独立评审：收益 + 额外模型用量 + 可能数分钟以上等待
          ├─ 明确同意 → 同步等待 Plan Reviewer → advisory summary
          ├─ 跳过     → user_skipped，保留计划
          └─ 中断/断连/无交互面板 → interrupted，保留计划
          │
          ▼
工具返回 plan_id / path / state / items / review
用户另行 /plan build；评审意见不构成执行门禁
```

## 1. 需求背景与边界

`create_plan` 将规划内容保存为 `~/.tomcat/plans/<plan_id>.plan.md`。保存和是否花费模型用量进行评审是两件事：先保存，再征求同意；不得把“建议评审”变成每次创建必跑的后台任务。

- 工具目录保持稳定；handler 只允许 Plan 会话模式调用。非 Plan 返回可操作的拒绝，不能把模式限制说成工具不可见。
- runtime 从 goal 派生 `plan_id`，调用者不得传 `plan_id`、旧 `body` 或额外顶层字段。
- 每次创建派生计划身份并更新当前 `ActivePlan`；这不是按指定 ID 重写旧计划的接口。既有待办的增量操作使用 [`update_plan`](./update-plan.md)，正文修订遵守原语的计划文件保护规则。
- PlanFile 是生命周期事实源；ActivePlan 是当前文件视图。评审不是第二份状态权威。

事实源：`src/core/tools/plan_tool/create_plan.rs::{CreatePlanArgs,execute,execute_for_tool}`。

## 2. 入参与落盘

| 输入 | 当前契约 |
| --- | --- |
| `goal` | 非空目标，参与派生身份；文件名由 runtime 决定 |
| `draft` | 非空方案正文，规范化后写入 `## Plan`；不需要重复外层标题 |
| `todos` | 至少一项；每项包含唯一 id、content，status 默认 pending，kind 默认 work |
| `todos[].kind` | work 或 acceptance；规范要求 acceptance 用于最终验收，当前校验硬性限制最多一项（不按索引拦截）；runtime 不自动补建 |

待办复用现有 `ops::apply_todos_ops` 和 `validate_single_acceptance` 校验。完整持久字段以 `src/core/plan_runtime/file_store.rs::PlanFileFrontmatter` 为准，不在本文另维护一份容易漂移的完整 JSON Schema。

持久字段的职责：

- `plan_id`、`goal`、`created_at`、`schema_version` 与初始 `todos` 在创建时写入；文件使用 `state`，不是旧文档的 `mode` 字段。
- 初始 `state=planning`，`session_key` / `session_id` 未绑定；执行归属由 `/plan build` 处理。
- 现有 `code_review_*` 兼容字段仍在类型中，不意味着启用了自动 CodeReviewer 门禁，也不能把它们写成“已全部删除”。
- 正文使用 `## Goal`、`## Plan`、`## Todos Board` 模板；待办事实以 frontmatter 为准，不手工维护第二套状态。

`write_plan` 完成并释放 advisory 文件锁后，才刷新缓存、记录事件和进入交互。保存失败时不启动评审。不要拿耗时的用户等待或模型请求占用计划写锁。

## 3. 明确同意后才评审

`execute_for_tool` 复用现有问答面板，不新增确认协议。面板有两个选项：评审、跳过；展示评审收益、模型成本及等待时间。

只有结果同时满足以下条件，才调用 `dispatch_reviewer`：

1. outcome 为 Answered，且没有已经发生的终止信号；
2. 恰好一条回答，question id 为 `plan-review`；
3. 未跳过、无自定义文本，option ids 恰好为 `["review"]`。

中断、宿主断连、未知取消或没有面板时记录 interrupted；其余非同意回答记录 user_skipped。不能根据翻译后的显示文案或含糊回答推断授权。

同意后同步等待评审结果并写入 `review`。即使评审中止，已保存的计划仍保留；评审结果仅是建议，不改变文件状态、不自动 build，也不触发 CodeReviewer / Verifier。跳过无需再等待模型。

`allow_review_edit` 是 runtime 内部参数，不是模型可传的工具字段。若允许的 reviewer 实际修改了计划，沿用原有重新读盘刷新 ActivePlan 的路径。具体工具权限与审稿范围见 [`reviewer.md`](./reviewer.md)。

## 4. 返回与安全边界

返回包含派生 `plan_id`、展示路径 `path`、`state`、待办 `items` 和 `review`。路径按现有 `format_home_path` 规则展示，不能一概宣称为未缩写绝对路径；评审字段名是 `review`，不是旧示例中的 `review_summary`。

- 同步底层 `execute` 只保存，提供未派发的评审占位；产品工具入口使用 `execute_for_tool`。
- 创建成功与评审成功分开表达；评审不成功不删除计划，也不伪造评审通过。
- 文件锁、原子写入、路径校验沿用 file_store；不为可选评审新增存储、状态机或审批机制。
- raw write/edit 不得修改计划 frontmatter；执行中的计划文件整体不可 raw 编辑。待办使用 `update_plan`，执行状态由既有 runtime / `/plan` 流程管理。
- 缺少 acceptance todo 可由 planner/reviewer 提出或补充；创建器不自动加待办，也不把缺项变为隐藏 build 门禁。

## 5. 测试用例清单

使用已有测试，不以文档中的拟定名称冒充已运行证据：

| 路径 | 证明的行为 |
| --- | --- |
| `src/core/tools/plan_tool/tests/create_plan_test.rs` | 非 Plan 拒绝、保存文件与 active view、多次创建、正文规范化、输入与验收 kind 校验 |
| `src/core/tools/plan_tool/tests/reviewer_test.rs::plan_review_decisions_require_explicit_consent_and_emit_exactly_one_result` | 明确同意、跳过、坏回答和终止路径；不擅自派发；一次结果 |
| 同文件 `create_plan_succeeds_even_when_reviewer_aborts` | 评审中止不撤销已保存计划 |
| 同文件 reviewer 锁与刷新用例 | 派发前已释放写锁，授权修改后的文件重新读取 |
| `src/core/tools/plan_tool/tests/update_plan_test.rs` | 待办增量更新与状态限制，不恢复自动评审/验收门禁 |

这些是测试位置与职责，不是本次运行结果；实际执行数和日志由交付记录给出。

## 6. 历史与关键决策

旧文档的“写完必审”“由 create_plan 按 ID 整盘重写”“仅 Planning 可见”“review_summary 必返”以及拟定 API/测试列表已被当前实现取代，不能用于新增产品行为。

当前取舍是复用现有文件存储、问答面板和 reviewer 派发，只补保存后的明确同意。更小的替代是完全不提供独立评审，但会失去用户已可选择的能力；更复杂的自动门禁没有当前需求。只有用户明确要求自动评审或改变保存/执行契约时，才重新讨论该边界，而不是从旧文档恢复它。

关联：[`plan-runtime.md`](../plan-runtime.md)、[`planner.md`](./planner.md)、[`reviewer.md`](./reviewer.md)、[`update-plan.md`](./update-plan.md)、[`ask-question.md`](./ask-question.md)。
