# Specs 归档目录

> 本目录用于存放已关闭（不再维护）的迭代规划文档与任务看板。仅追加，不修改历史，不从此处删除。

## 归档约定

1. **仅追加**：禁止从归档目录删除或改动已有正文。如需补充说明，只可在文件顶部加新的「归档说明」块。
2. **路径对照表**：每次归档必须登记在下方「归档清单」，含归档日期、迭代代号、原路径 → 新路径、归档原因、后续读者入口。
3. **归档触发条件**：
   - 迭代目标已达成或被明确关闭；
   - 新迭代启动且新看板已就位；
   - 决定将剩余 TODO 迁移到新优先级档位（在原 task.md 顶部登记迁移表）。
4. **后续读者指引**：
   - 如需复现历史决策：从本目录读取；
   - 如需认领新任务：去 `agents/TASK_BOARD_00X.md`（当前 = 002）；
   - 如需查看路线图：去 `openspec/specs/Product_Brief.md`；
   - 如需浏览全集 TODO：去 `docs/TODOS.md`。

## 归档清单

| 归档日期 | 迭代代号 | 原路径 | 新路径 | 归档原因 | 新迭代入口 |
|----------|----------|--------|--------|----------|------------|
| 2026-04-22 | `001-mvp` | `openspec/changes/001-mvp/` | `openspec/specs/archive/001-mvp/` | MVP 主体已交付，切换到「单 Agent 完善期」P0-P9 路线图；剩余 TODO 迁移到 `002-single-agent-complete` 或延后档位 | [`agents/TASK_BOARD_002.md`](../../../agents/TASK_BOARD_002.md) |
| 2026-04-22 | `001-mvp` 看板 | `agents/TASK_BOARD.md` | `openspec/specs/archive/TASK_BOARD_001-mvp.md` | 随 `001-mvp` 一并归档；新迭代使用独立看板 | [`agents/TASK_BOARD_002.md`](../../../agents/TASK_BOARD_002.md) |

## 变更管理简化说明（2026-04-22 起）

从 `002` 迭代开始，不再创建 `openspec/changes/00X-<codename>/` 的「proposal.md / task.md / tasks_details.md / design.md」四件套。代之以：

- **规格层**：`openspec/specs/Product_Brief.md`（路线图）+ `Architecture.md`（架构）；
- **执行层**：`docs/TODOS.md`（全集想法池）+ `agents/TASK_BOARD_00X.md`（当前迭代立项 + 执行一体）。

Board 本身吸收了原 proposal.md 的「迭代目标 / 不做范围 / 验收口径 / 风险」四段。若未来某次大立项确实需要评审文档，再临时创建 `changes/` 子目录不迟。
