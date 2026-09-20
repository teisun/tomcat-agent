# Plan 执行与最终验收

## 需求背景

计划执行需要两件彼此独立的事：

1. `update_plan` 可靠地保存 todo 进度；
2. LLM 在最终验收时检查代码和运行合适的验证。

运行时不能替 LLM 规定一套自动评审、固定命令或后台任务收据流程。这些机制会增加
状态、等待和失败分支，却不能判断改动是否合理。

```text
planner ── creates todos ──► executor ── updates todo status ──► completed
                                  │
                                  └─ final Acceptance in_progress
                                              │
                                              ▼
                              next_step hint: load_skill(verify)
                                              │
                                              ▼
                           review diff → fix → impact-scoped checks
```

## 运行时语义

- `create_plan` 只保存调用者给出的 todos，绝不追加运行时 todo。
- `TodoKind` 是 `work`、`acceptance` 或仅用于读取旧文件的 `unknown`。
- `acceptance` 只表示一个 LLM 编写的最终验收 todo；一个计划最多一个。
- `create_plan` 和 `update_plan` 都在写盘前校验这个数量限制。
- todo 变为 `in_progress` 时仍受现有并发上限约束。
- `acceptance` 变为 `in_progress` 时，`next_step.hint` 返回：
  `load_skill(verify)；按影响范围复核 diff 并验证；完成后勾掉本验收 todo`。
- 至少有一个 todo 且所有 todo 都是 `completed` 或 `cancelled` 时，计划变为
  `completed`。任何 todo 被重新打开时，已完成计划回到 `pending`。
- 恢复会话和 park 只更新计划生命周期，不会擅自把 `in_progress` todo 改回
  `pending`。

## 最终验收

`verify` skill 是最终验收的工作说明，而不是运行时门禁。它要求：

```text
核对 diff 与计划
    → 修复确认的问题（偏差、遗漏、不合理设计、过度设计、P0/P1）
    → 按影响范围选择验证
```

验证默认从改动附近开始。只有改动触及核心/共享/基础设施、配置、构建或依赖文件、
公共协议、多个包，或用户明确要求时，才升级到 L3 全量检查。

## 关键决策

- 不自动派发 code reviewer；相关模块可供手动或后续流程使用，但不影响完成。
- 不记录机器强制的验收命令或绿构建证据字段；测试命令与结果属于 todo evidence 和
  LLM 的最终报告。
- 旧计划中的未知 todo kind 解析为 `unknown`，旧 frontmatter 字段保存在 `unknown`
  map，确保计划仍可展示，不为旧 gate 保留行为。
- dormant verifier 保持现状，不属于本轮完成路径。

## 验证

- 创建计划不增加 todo。
- 两个入口都拒绝第二个 `acceptance` todo。
- `update_plan` 可以通过 `todo_kind=acceptance` 补建最终验收 todo。
- `acceptance` 进入执行中会得到 verify hint。
- 全部 todo 终态后完成；park/resume 保留进行中的验收 todo。
