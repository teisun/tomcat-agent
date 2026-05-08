# Dispatcher — 调度指令

本文档是工程师 Agent 启动后的**行动手册**。使用方式：`@Tom.md @Dispatcher.md`（或 Jerry / Spike）。

Agent 读取本文件后，须按以下流程执行。

---
## 背景了解
Agent 根据自身角色定义读取主项目 [openspec/specs 规范文档](../openspec/specs/)，并按当前迭代看板（索引与立项 [TASK_BOARD_002/README.md](./TASK_BOARD_002/README.md)；单卡 `TASK_BOARD_002/tasks/T2-*.md`）规划好的任务实现功能，提交到各自分支，并同步本分支 [docs/status/{branch}.md](../docs/status/)

## 1、领取任务

1. 读取 [TASK_BOARD_002/README.md](./TASK_BOARD_002/README.md) 的 **§4 任务索引表**。
2. 打开目标任务的 **[tasks/T2-*.md](./TASK_BOARD_002/tasks/)** 单卡，确认状态、依赖与验收。
3. 在 **§4 索引** 与任务卡中，找到**状态为 `TODO` 且负责人为空**的最高优先级任务（P0 > P1）。仅 `TODO` 可认领；`PENDING_INTEGRATION` 表示已交集成、不可认领。
4. 若有多个同优先级可选任务，优先选索引表中排在前面的（已按推荐顺序排列）。
5. 有依赖的任务，所有依赖须为 `DONE` 才可认领。
6. **一次只认领一个任务**，完成或标记 `BLOCKED` 后才可领下一个。

## 2、认领任务

1. 在该任务对应的 **`TASK_BOARD_002/tasks/T2-*.md`** 中，将**负责人**改为自己的名字（Tom / Jerry / Spike）。
2. 将**状态**改为 `DOING`。

## 3、读取上下文

根据 [TASK_BOARD_002/README.md](./TASK_BOARD_002/README.md) 的「当前迭代上下文」与 **已认领的 `tasks/T2-*.md`**，读取以下文档：

1. **openspec/specs 规范层**（须读、无省略）：含 [Constitution.md](../openspec/specs/Constitution.md)、[Product_Brief.md](../openspec/specs/Product_Brief.md)、[Architecture.md](../openspec/specs/Architecture.md)（索引）、`guides/` 下与工作相关的规范等。
2. **架构 / 工具长文**：在 [`docs/architecture/`](../docs/architecture/) 下**按任务卡点名的子路径**打开即可；**勿**将整棵 `docs/architecture` 当作默认附件通读。
3. **当前迭代立项 + 任务索引**：README §1 / §4；具体字段与验收以对应 **`tasks/T2-*.md`** 为准。

## 4、制定开发计划

读取完上下文后，**禁止直接编码**，须先制定并输出详细开发计划，**经用户确认后**方可进入开发阶段。

制定计划时须遵循 [plan/PLAN_SPEC.md](./plan/PLAN_SPEC.md) 中的**内容要求** 含全部 **7** 个维度，中小任务可先复制 [plan/PLAN_SKELETON.md](./plan/PLAN_SKELETON.md) 再按规范扩写，并参考 PLAN_SPEC 第四节中的完整案例索引。

## 5、开发

严格按 [Constitution.md](../openspec/specs/Constitution.md) 研发流程执行：

1. **开发前**：
   - 检查工作区状态，若处于 detached HEAD 则自动 checkout -b 工作分支
   - 从 develop 拉取最新代码
   - 创建任务分支（格式见下方「分支命名规范」）
   - 阅读 [编码规范](../openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
2. **开发流程**：
   - 编码（带注释）→ 测试 → 修 bug → 单测通过。
   - **全量集成测试前（必做）**：若本次变更新增或调整了 `tests/` 下需以 **integration 测试二进制**（`cargo test --test <name>` 之 `<name>`，与 `tests/<name>.rs` 对应）跑验收的用例，须在执行 `./scripts/run-integration-tests.sh` 等全量集成命令**之前**，按 [INTEGRATION_TEST_SPEC §7.2](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md) 判定该目标应进 **并发组** 还是 **串行组**，并更新 [`scripts/test-groups.sh`](../scripts/test-groups.sh)：`TOMCAT_INTEGRATION_PARALLEL_TESTS` 与 `TOMCAT_INTEGRATION_SERIAL_TESTS` **二者择一登记该二进制名**。不论最终归入哪一组，**都必须改 `test-groups.sh`**；漏登记则 `run-integration-tests.sh` **不会执行**该 crate。
   - 集成&E2E测试 → 写技术[文档](../docs/)：验收顺序与命令见 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](./INTEGRATION_MERGE_AND_ACCEPTANCE.md)、分类执行见 [INTEGRATION_TEST_SPEC §7.1 / §7.2](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)。
3. **提交前**：
   - 更新**当前 Git 分支**对应的 status 文件：文件名为「当前分支名（`/` 替换为 `-`）.md」，位于 `docs/status/` 目录；若在 develop 上开发则更新 `docs/status/develop.md`。填入 Cov% 等元数据。
   - 严格加载 [commit-guard.mdc](../.cursor/rules/commit-guard.mdc) 提交规则
4. **提交策略**：
   - 每个子任务完成提交一次，禁止囤积多个任务一次性提交
   - 提交到本地与远端

## 6、阻塞处理

遇依赖阻塞、技术问题、需求不明确时：

1. 在对应 **`TASK_BOARD_002/tasks/T2-*.md`** 中将任务状态改为 `BLOCKED`，填写**阻塞点**描述
2. 在**当前 Git 分支**对应的 status 文件（`docs/status/` 下「当前分支名，`/` 换 `-`」.md）中更新阻塞状态（含原因与预计解决时间）
3. 禁止静默阻塞

## 7、完成任务

1. 确认所有子项完成，通过门禁（rustfmt/clippy/单测），且已按 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](./INTEGRATION_MERGE_AND_ACCEPTANCE.md) 完成 **分支侧全量集成/E2E测试验收**（集成/E2E 失败须在本分支修复，禁止弱化断言通过）
2. 更新**当前 Git 分支**对应的 status 文件（即 `docs/status/` 下文件名为「当前分支名，`/` 替换为 `-`」的 .md 文件），标记任务完成；若在 develop 上开发则更新 `docs/status/develop.md`。
3. 将 **`TASK_BOARD_002/tasks/T2-*.md`** 中该任务状态改为 `PENDING_INTEGRATION`。集成测试通过后，由合并/集成流程（见 Nibbles）将状态更新为 `DONE`；工程师只负责在自测完成并推送后标记为 `PENDING_INTEGRATION`。
4. **完成前自检（必做）**：
   - 已确认**当前分支**，并已更新 **docs/status/当前分支对应.md**（分支名中 `/` → `-`）。
   - **test-groups**：若本次有新增或调整 integration 测试二进制，已按 §5 更新 `scripts/test-groups.sh`（§7.2 并发/串行组二选一登记）。
   - **集成与 E2E**：已按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` 完成规格/场景库（若任务涉及用户可见行为或 P0/P1 故事）、集成测试与 E2E；失败项已在**本分支**修复；**无**为通过测试而弱化断言或糊弄 `#[ignore]` 的情况。
   - **覆盖率**（可选）：若需要测量覆盖率，可手动执行 `/update-coverage` Command 或 `cargo tarpaulin --lib --package tomcat`，将结果填入 status 文件元数据表的 Cov% 列；不强制执行，不阻塞任务完成。
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
- [INTEGRATION_MERGE_AND_ACCEPTANCE.md](./INTEGRATION_MERGE_AND_ACCEPTANCE.md) — 集成与 E2E：交付步骤与验收
- [集成测试规范](../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)（§7.2 与 [`scripts/test-groups.sh`](../scripts/test-groups.sh)）
- [E2E 测试规范](../openspec/specs/guides/testing/E2E_TEST_SPEC.md)
- [Status 规范](../openspec/specs/guides/workflow/STATUS_GUIDE.md)
- [Commit Message 规范](../openspec/specs/guides/workflow/COMMIT_MESSAGE_SPEC.md)
- [技术文档规范](../openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md)
