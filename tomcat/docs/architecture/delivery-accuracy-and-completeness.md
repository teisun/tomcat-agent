# 交付准确性与完整性

## 需求背景

完成 todo 不等于交付正确。最终阶段仍要确认：实际 diff 是否兑现计划、是否遗漏行为、
是否引入不必要的复杂度，以及验证是否覆盖真正受影响的边界。

## 解决方案

最终验收由 LLM 按 `verify` skill 执行，而不是由运行时自动评审或固定测试命令驱动。

```text
Plan
 ├─ work todos: 实现与局部检查
 └─ optional final acceptance todo
             │ in_progress
             ▼
      review diff against plan
             │
      fix confirmed P0/P1 or over-design
             │
      validate by impact radius
             ▼
       mark the todo completed
```

关键决策清单：

- 计划可有一个、也只能有一个最终 `acceptance` todo；普通里程碑检查是 `work`。
- plan reviewer 只提出建议：缺少验收步骤、错误使用 kind、遗漏验收章节或过度设计都
  是 concern，不阻塞计划。
- “过度设计”有可复核定义：某个结构、依赖、协议、存储或状态分支服务于本次需求和
  影响范围之外的场景，删掉后本次验收仍会全绿。
- 验收按影响范围升级，不默认全量。核心/共享/基础设施、配置、构建与依赖、协议、
  跨包改动或用户要求才需要 L3。
- 运行时只提供一句 verify hint 并保存 todo 状态；它不裁定测试范围，也不伪造证据。

## 验证

交付前必须能回答：

1. 每项计划承诺是否能在 diff 中找到对应实现或明确取消理由？
2. 是否有遗漏、错误范围、不可解释的抽象或未处理的 P0/P1？
3. 检查从改动文件所属测试开始，是否已经覆盖到受影响的 API、协议或依赖方？
4. 如果验收期间改了代码，是否重新检查了受影响边界？

`update_plan` 的持久化不重置进行中的 todo。因此中断、park 和恢复不会丢失最终验收
进度，也不会把旧运行时流程重新带回完成路径。
