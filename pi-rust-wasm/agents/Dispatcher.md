# Dispatcher — 调度指令

本文档是工程师 Agent 启动后的**行动手册**。使用方式：`@Tom.md @Dispatcher.md`（或 Jerry / Spike）。

Agent 读取本文件后，须按以下流程执行。

---

## 一、领取任务

1. 读取 [TASK_BOARD.md](./TASK_BOARD.md)。
2. 查看顶部「当前迭代上下文」区，获取 specs 路径、需求文档路径等。
3. 在任务列表中，找到**状态为 `TODO` 且负责人为空**的最高优先级任务（P0 > P1 > P2 > P3）。
4. 若有多个同优先级可选任务，优先选排在前面的（已按推荐顺序排列）。
5. 有依赖的任务，所有依赖须为 `DONE` 才可认领。
6. **一次只认领一个任务**，完成或标记 `BLOCKED` 后才可领下一个。

## 二、认领任务

1. 将该任务的**负责人**改为自己的名字（Tom / Jerry / Spike）。
2. 将**状态**改为 `DOING`。

## 三、读取上下文

根据 TASK_BOARD.md 顶部的「当前迭代上下文」，读取以下文档：

1. **specs 规格文档**（含 Architecture.md、Constitution.md 及子文档）
2. **需求设计文档**（含 task.md、tasks_details.md、design.md）
3. 重点阅读 tasks_details.md 中对应任务的**原子子任务**与**边界场景**

## 四、制定开发计划

读取完上下文后，**禁止直接编码**，须先制定并输出详细开发计划：

1. 列出本任务所有待完成子项（对照 TASK_BOARD.md 中的子项清单）
2. 对每个子项，给出：
   - 涉及的文件与模块
   - 实现思路（关键数据结构、算法、调用链路）
   - 依赖的现有接口或需新建的接口
   - 预期的测试要点
3. 明确实施顺序与各子项之间的依赖关系
4. 识别风险点与可能的阻塞项
5. 将计划输出给用户，**经用户确认后**方可进入开发阶段

## 五、开发

严格按 [Constitution.md](../openspec/specs/Constitution.md) 研发流程执行：

1. **开发前**：
   - 检查工作区状态，若处于 detached HEAD 则自动 checkout -b 工作分支
   - 从 develop 拉取最新代码
   - 创建任务分支（格式见下方「分支命名规范」）
   - 阅读 [编码规范](../openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
2. **开发流程**：
   - 编码（带注释）→ 测试 → 修 bug → 单测通过 → 写技术[文档](../docs/)
3. **提交前**：
   - 更新本分支的 `status/` 文件（与当前分支对应），填入 Cov% 等元数据
   - 严格加载 [commit-guard.mdc](../.cursor/rules/commit-guard.mdc) 提交规则
4. **提交策略**：
   - 每个子任务完成提交一次，禁止囤积多个任务一次性提交
   - 提交到本地与远端

## 六、阻塞处理

遇依赖阻塞、技术问题、需求不明确时：

1. 在 TASK_BOARD.md 中将任务状态改为 `BLOCKED`，填写**阻塞点**描述
2. 在本分支的 `status/` 文件中更新阻塞状态（含原因与预计解决时间）
3. 禁止静默阻塞

## 七、完成任务

1. 确认所有子项完成，通过门禁（rustfmt/clippy/单测）
2. 更新本分支 `status/` 文件，标记任务完成
3. 将 TASK_BOARD.md 中该任务状态改为 `DONE`
4. 可继续领取下一个任务（回到第一步）

---

## 分支命名规范

- 按任务创建分支，格式：`feature/{任务简写}`
  - 示例：`feature/plugin-lifecycle`、`feature/cli-chat`、`feature/cli-commands`
- 从 develop 拉取最新代码作为分支起点
- 已存在的历史分支（如 `feature/wasm-plugin`、`feature/chat`）可继续使用

## 关键规范引用

- [Constitution.md](../openspec/specs/Constitution.md) — 行为规范（必遵）
- [commit-guard.mdc](../.cursor/rules/commit-guard.mdc) — 提交规则（必遵）
- [编码规范](../openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
- [单元测试规范](../openspec/specs/guides/testing/UNIT_TEST_SPEC.md)
- [Status 规范](../openspec/specs/guides/workflow/STATUS_GUIDE.md)
- [Commit Message 规范](../openspec/specs/guides/workflow/COMMIT_MESSAGE_SPEC.md)
- [技术文档规范](../openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md)
