# Plan Reviewer

## 需求背景

计划在进入执行前需要第二双眼睛，但审稿不应成为运行时收口门禁。自动阻塞、反复派发
审稿子 Agent 会把简单交付变成长时间等待，也不能替主 Agent 判断用户是否接受取舍。

## 解决方案

`create_plan` 同步调用 plan reviewer 并返回 advisory summary。审稿意见不改变计划
状态，不阻止 `/plan build`，也不触发 code reviewer 或 verifier。

```text
create_plan
    │
    ├─ persist caller-authored todos
    ├─ dispatch plan reviewer (advisory)
    │       ├─ read plan and relevant project context
    │       └─ return <review> findings
    ▼
planner chooses whether to revise the plan
```

审稿范围：

- 计划是否完整、可理解，是否把问题与解决方案说清楚；
- 验证是否与改动风险相称；
- 编码计划是否有可读的验收章节和一个最终 `acceptance` todo；
- 中间里程碑是否错误地使用了 `acceptance` kind；
- 是否有超过需求和影响范围的结构、依赖、协议、存储或状态机。

缺少最终验收 todo 时，reviewer 可通过 `update_plan` 的 `upsert.todo_kind=acceptance`
补建；第二个验收 todo 会被工具拒绝。

## Code reviewer 与 verifier

`CodeReviewerDispatcher`、生产实现、相关 prompt 和 frontmatter 字段仍保留，供手动或
后续独立流程使用。本轮没有自动调用它们，也没有把它们放入 `update_plan` 的完成条件。

dormant verifier 同样保持现状，不是本轮的删除目标。

## 验证

- reviewer 输出必须是 advisory `<review>` 格式，不产生 approve/reject verdict。
- 创建计划即使 reviewer 无法返回，也能保留计划并由用户决定下一步。
- reviewer 对缺失验收、错误 kind 和过度设计记录 concern，而不是门禁状态。
