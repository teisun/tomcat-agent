# Plan Runtime

## 需求背景

Plan Runtime 让计划模式、计划文件和执行中的 todo 共享一个小而可恢复的状态模型。
它的职责是保存状态和给出下一步提示；它不应承担“替 LLM 验收代码”的职责。

```text
AgentMode                         PlanFile.state
─────────                         ──────────────
Chat ── /plan ──► Plan            planning ── /plan build ──► executing
  ▲                  │                ▲                            │
  └── /plan exit ────┘                └──────── pending ◄──────────┘
                                                               │
                                                               ▼
                                                           completed
```

## 计划文件

一个 PlanFile 是 YAML frontmatter 加自由 Markdown body：

```text
PlanFile
├─ identity: plan_id, goal, created_at, session identifiers
├─ lifecycle: planning | executing | pending | completed
├─ todos[]
│   ├─ id, content, status, evidence
│   └─ kind: work | acceptance | unknown
└─ body: user-visible plan prose and Todos Board projection
```

关键决策清单：

- `work` 是普通任务；`acceptance` 仅表示最终验收。一个计划最多一个 acceptance。
- `unknown` 仅用于反序列化旧 kind。旧计划可显示，但旧行为不会被重新启用。
- 未知 frontmatter 字段经 flatten 保留，避免读取旧计划失败。
- 文件写入走 advisory lock、临时文件、fsync 和 rename；工具写入前校验 todo ID、
  同时进行数量与单一 acceptance。

## Todo 更新和完成

`create_plan` 只保存 LLM 给出的 todos。编码计划通常有一个末尾的 `acceptance` todo，
但这是一条提示词建议，不是自动追加或创建阻塞条件。

`update_plan` 使用原子 todo ops：

```text
upsert(id, content?, status?, todo_kind?)
set_status(id, status, evidence?)
remove(id)
```

操作 discriminator 已占用 `kind`，所以 upsert 的 todo 语义字段命名为 `todo_kind`。
它可以让 plan reviewer 的“缺少最终验收 todo”建议被实际执行。

```text
update_plan
    │
    ├─ apply todo operations
    ├─ reject a second acceptance todo
    ├─ rewrite visible Todos Board
    ├─ Acceptance + in_progress → RunVerify hint
    └─ non-empty + all terminal → completed
```

所有 todo `completed` 或 `cancelled` 且列表非空时完成计划。若已完成计划有 todo 被重新
打开，状态回到 `pending`。`park_executing_plan` 只把 lifecycle 降为 `pending`，不会
篡改任意 todo 状态。

## 最终验收

final acceptance 是提示词驱动的：

1. planner 在正文写人话验收章节，并通常写一个最终 acceptance todo；
2. plan reviewer 检查遗漏、错误 kind 和过度设计，保持 advisory；
3. executor 在需要最终验收时加载 `verify` skill；
4. verify skill 执行“核对 diff 与计划 → 修复确认问题 → 按影响范围验证”。

Runtime 给 Acceptance 进入 `in_progress` 的 todo 返回 verify hint，但不会自动派发
code reviewer、verifier 或全量测试。

## 保留但不在完成路径的能力

- `CodeReviewerDispatcher` 和相关生产实现保留，供独立手动流程使用。
- code-review frontmatter 保留，避免破坏现有计划读取。
- verifier 资产和配置在本轮不删除；`update_plan` 不调用它。

## 验证

- create/update 两个入口都校验 acceptance 数量；
- `todo_kind=acceptance` 可新增或修改 todo；
- acceptance 开始时返回 verify hint；
- 全部 todo 终态后完成；
- 恢复、同步和 park 保持 todo 状态；
- 旧 kind 与旧字段可解析和显示。
