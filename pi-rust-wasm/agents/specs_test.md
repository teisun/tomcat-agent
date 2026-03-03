# specs_test_agent：测试specs流程

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 目标

不管有没有从PLAN.md领到任务，都写一个hello world程序



## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 5 节「CLI 交互层」、核心交互设计
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-011

## 验收标准

- 单测覆盖率≥90%。
