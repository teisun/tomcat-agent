# Dispatcher — 调度指令

本文档是工程师 Agent 启动后的**行动手册**。使用方式：`@Tom.md @Dispatcher.md`（或 Jerry / Spike）。

Agent 读取本文件后，须按以下流程执行。

---
## 背景了解
 agent根据自身角色定义读取主项目 [specs规格文档](../openspec/specs/)下文档，实现TASK_BOARD.md 规划好的任务，完成对应功能后提交代码到各自分支，按要求同步进度到本分支的 [status/{branch}.md]

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

读取完上下文后，**禁止直接编码**，须先制定并输出详细开发计划，**经用户确认后**方可进入开发阶段。

### 计划必须包含的内容

1. **待完成子项清单**  
   对照 TASK_BOARD.md 中的子项清单，列出本任务所有待完成子项及当前状态（已完成/待做）。

2. **目标与验收（含功能/步骤的作用与意义、用户故事/用户场景）**  
   - 用一两句话写明「要做出什么」以及验收标准（可运行、可测、门禁通过等）。  
   - 若任务包含多步流程（如加载流程的若干步骤），对**每一步**简要说明：**用户故事/用户场景**（用户在该步骤下会得到什么体验、解决什么问题）、**作用**（做什么）、**意义**（为什么需要、不做的后果）。便于评审与实现时对齐设计意图与用户价值，避免实现偏离或遗漏。

3. **对每个子项给出**  
   - **涉及的文件与模块**：改动的源码路径、依赖的需求/设计文档。  
   - **实现思路**：关键数据结构、算法、调用链路（谁调谁、在什么时机）。  
   - **依赖的现有接口或需新建的接口**：列出已有 API 与计划新增的 API/回调。  
   - **预期的测试要点**：正常路径、边界与异常（非法输入、失败场景），以及期望的错误表现（信息清晰、不崩溃、可恢复等）。

4. **实施顺序与依赖关系**  
   明确各子项的实施顺序，以及子项之间的依赖（例如先扩展数据结构再实现主流程）。

5. **风险点与可能的阻塞项**  
   标出对外部接口或未决设计的依赖、与现有模块的兼容性顾虑、以及若无法解决时的备选方案或降级思路。

### 好的计划的特征

- **可执行**：读完后能按顺序动手，不需要再猜「先改哪一块」。  
- **可验收**：目标与验收标准清晰，完成后能判断是否算「做完」。  
- **可回溯**：关键步骤有「作用与意义」说明，便于后续维护或需求变更时理解设计。  
- **风险可见**：阻塞与不确定性写清楚，方便用户提前决策或补充信息。

### 计划输出前自检（必做）

Agent 输出计划前，须逐条确认以下项**全部满足，缺一不可**：

- [ ] 列出了全部子项及「已完成/待做」状态
- [ ] 写明了总体目标与验收标准（一两句话）
- [ ] 每步/每个子命令写了「用户故事/用户场景」「作用」「意义（不做的后果）」
- [ ] 每个子项列出了「涉及的文件与模块」（含源码路径与依赖的设计文档）
- [ ] 每个子项写了「实现思路」含调用链路（谁调谁、在什么时机）
- [ ] 每个子项明确列出了「依赖的现有接口」和「需新建的接口」
- [ ] 每个子项写了「预期测试要点」含边界场景与期望的错误表现
- [ ] 写了实施顺序与子项间依赖关系
- [ ] 写了风险点，且每个风险有备选方案或降级思路

有任一项未满足，须补全后再输出计划。

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
   - 更新**当前 Git 分支**对应的 status 文件：文件名为「当前分支名（`/` 替换为 `-`）.md」，位于 `status/` 目录；若在 develop 上开发则更新 `status/develop.md`。填入 Cov% 等元数据。
   - 严格加载 [commit-guard.mdc](../.cursor/rules/commit-guard.mdc) 提交规则
4. **提交策略**：
   - 每个子任务完成提交一次，禁止囤积多个任务一次性提交
   - 提交到本地与远端

## 六、阻塞处理

遇依赖阻塞、技术问题、需求不明确时：

1. 在 TASK_BOARD.md 中将任务状态改为 `BLOCKED`，填写**阻塞点**描述
2. 在**当前 Git 分支**对应的 status 文件（`status/` 下「当前分支名，`/` 换 `-`」.md）中更新阻塞状态（含原因与预计解决时间）
3. 禁止静默阻塞

## 七、完成任务

1. 确认所有子项完成，通过门禁（rustfmt/clippy/单测）
2. 更新**当前 Git 分支**对应的 status 文件（即 `status/` 下文件名为「当前分支名，`/` 替换为 `-`」的 .md 文件），标记任务完成；若在 develop 上开发则更新 `status/develop.md`。
3. 将 TASK_BOARD.md 中该任务状态改为 `DONE`
4. **完成前自检（必做）**：
   - 已确认**当前分支**，并已更新 **status/当前分支对应.md**（分支名中 `/` → `-`）。
   - **覆盖率**：若本次有代码变更，已执行 `cargo tarpaulin --lib --packages pi_awsm`，将结果写入上述 status 文件**第一个元数据表**的 Cov% 列；宪法要求 ≥85%。
   - **技术文档**：若有接口/行为变更，已按 [技术文档规范](../openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md) 更新 `docs/` 下对应文档。
   - **提交**：已按 [commit-guard.mdc](../.cursor/rules/commit-guard.mdc) 提交，含 what+why；若为代码变更且 status 中已填 Cov%，commit message 末尾含 `[cov = xx.x%]`。
   - **推送**：已推送到远端。
5. 可继续领取下一个任务（回到第一步）

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
