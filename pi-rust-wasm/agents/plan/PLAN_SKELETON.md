# 开发计划骨架（复制后填空）

> 符合 [PLAN_SPEC.md](./PLAN_SPEC.md)。适用于单模块或单周可完成的中小任务；大任务请拆 Phase 并另附专文或对照 [PLAN_TASK05_PI_MONO_COMPAT.md](./PLAN_TASK05_PI_MONO_COMPAT.md)。

## 元信息

| 字段 | 内容 |
| :--- | :--- |
| 任务 ID / 名称 | （TASK-XX / 看板标题） |
| 分支 | `feature/...`（与 TASK_BOARD 一致） |
| 规格单一来源 | （openspec 路径，无则写「无，以 TASK_BOARD 为准」） |

## 〇、Todo 总表（与下文章节一一对应，勿留悬空项）

> 流程类（认领/分支/status/提交/集成）与实施类（每个子项或 Phase）**均须出现**；可与「一、子项清单」合并为同一组勾选，但须保证每条能指回正文一节。

| id | 类型 | 动作摘要 | 对应正文 |
| :--- | :--- | :--- | :--- |
| `ops-claim` | 流程 | 认领 TASK、checkout 分支、拉 develop | （Dispatcher §1–§5） |
| `ops-ship` | 流程 | 子项完成后 status → commit → push | （Dispatcher §5） |
| `ops-integration` | 流程 | 分支侧门禁通过 → PENDING_INTEGRATION → 推送 | （Dispatcher §7） |
| `impl-x.1` | 实施 | … | 见 **三、子项 x.1** |
| `impl-x.2` | 实施 | … | 见 **三、子项 x.2** |

**写后复核**（定稿前勾选）：每条 Todo 有正文展开；每个正文交付点有 Todo；若用 YAML 与 Markdown 双轨则两处一致（见 [PLAN_SPEC.md](./PLAN_SPEC.md) 第六节）。

## 一、子项清单

- [ ] x.1 …（与上表 `impl-x.1` 同步）
- [ ] x.2 …（与上表 `impl-x.2` 同步）

## 二、目标与验收

**要做出什么**：（一句话）  
**验收**：（可测条件：测试命令、clippy、文档等）  
**用户场景 / 作用 / 意义**：（若仅内部可写「无用户面」+ 不做的技术后果）

## 三、子项详情（每个子项复制本块）

### x.n 标题

- **文件与模块**：`src/...`；参考 `openspec/...`
- **实现思路**：调用链、关键时机；若涉及 async/阻塞，写明禁止 `block_on` 嵌套等约束
- **已有接口**：…
- **新建接口**：…（无则写无）
- **测试要点**：正常 / 边界 / 错误表现

## 四、实施顺序与依赖

（列表或 `A → B → C`；可并行的写「A ∥ B」）

## 五、风险与备选

| 风险 | 备选 |
| :--- | :--- |
| … | … |

## 六、集成与 E2E（条件触发）

- 涉及用户面 / P0–P1 故事：场景库条目编号、集成测试文件、`cli_tests` / `wasmedge_e2e_tests` 意图；**用例名与仓库一致或标「待新增」**  
- 纯内部：写「不适用」+ 理由  
- 全量门禁：交付前按 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../INTEGRATION_MERGE_AND_ACCEPTANCE.md) 执行（不在此重复全文）
