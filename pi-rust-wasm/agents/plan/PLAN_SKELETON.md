# 开发计划骨架（复制后填空）

> 符合 [PLAN_SPEC.md](./PLAN_SPEC.md)。章节顺序与 [PLAN_EXAMPLE_TASK21.md](./PLAN_EXAMPLE_TASK21.md) 一致；填完对照范例查漏补缺。

## 认领与分支（Dispatcher）

- [TASK_BOARD_002.md](../TASK_BOARD_002.md)：负责人、状态、**分支名**（与 Git 一致）；依赖任务是否已 `DONE` / 是否需先合并 `develop`。
- 行为规范：[Constitution.md](../../openspec/specs/Constitution.md) 及任务引用的规范链接。

## 研发流程（对照 Dispatcher）

| 阶段 | 动作 |
|------|------|
| 读上下文 | specs、task.md、tasks_details 原子子项与边界；todo：`context-read`（若采用） |
| 开发前 | 非 detached、目标分支、与 `develop` 同步；编码规范 |
| 开发中 | 单测 / 集成测；**小步提交** |
| 门禁 | `cargo fmt`、`cargo clippy -D warnings`；§4 全量按 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../INTEGRATION_MERGE_AND_ACCEPTANCE.md)；todo：`dev-gates` |
| 提交与进度 | [commit-guard.mdc](../../.cursor/rules/commit-guard.mdc)；`docs/status/{branch}.md`；todo：`status-ship` |
| 完成 | TASK_BOARD → `PENDING_INTEGRATION`、推送 |

## 子项清单与状态（对照看板 x.y）

| 子项 | 内容 | 计划状态 |
|------|------|----------|
| x.1 | … | 待做 |
| x.2 | … | 待做 |

---

## 目标与验收（含作用/意义）

**要做出什么**：（一句话）  
**验收**：（fmt、clippy、测试、§4 若适用）  
**用户故事/场景与意义**：（分步；纯内部写「无用户面」+ 不做的后果）

---

## 现状与差距（关键代码，可选但强规格任务建议写）

- 当前类型/入口/旧行为与目标的一行对比（附 `src/` 路径）。

---

## 子项与 API 一览（可选）

| 子项 | 主要已有接口 | 计划新建或变更 |
|------|----------------|----------------|
| x.1 | … | … |

---

## 各子项：文件、思路、接口、测试

### x.1 标题

- **文件**：`src/...`；参考 `openspec/...`
- **思路**：调用链、关键时机；async/阻塞约束
- **接口**：已有 / 新建
- **测试要点**：正常 / 边界 / 错误表现

### x.2 标题

（同上结构复制）

---

## 实施顺序与依赖

（列表或 Mermaid `flowchart`；标明串行 / 可并行）

---

## 风险与备选

| 风险 | 备选 |
| :--- | :--- |
| … | … |

---

## 集成与 E2E（条件触发）

- 用户面 / P0–P1：场景库编号、集成测、`cli_tests` / Wasm E2E；用例名与仓库一致或标「待新增」
- 纯内部：「不适用」+ 理由
- §4 全量：引用 INTEGRATION 文档，写明后台日志 + 轮询方式

---

## 计划输出前自检

对照 [PLAN_SPEC.md](./PLAN_SPEC.md) **第五节**逐项勾选。

---

## 完成后的 Dispatcher 动作（实现阶段）

status 持续更新；子项与门禁通过后标 `PENDING_INTEGRATION`；按 commit-guard 提交推送；对外可见行为按 DOCUMENTATION_GUIDE 更新已有 `docs/`，不擅自新建长篇文档。

---

## 七、Todo 总表（与 YAML / 正文对照，[PLAN_SPEC.md](./PLAN_SPEC.md) 一.7）

| id | 类型 | 对应正文 |
| :--- | :--- | :--- |
| `claim-board-branch` | 流程 | 认领与分支 |
| `context-read` | 流程 | 读上下文 |
| `impl-x.1` | 实施 | **x.1** |
| `impl-x.2` | 实施 | **x.2** |
| `dev-gates` | 流程 | 门禁与 §4 全量 |
| `status-ship` | 流程 | status / commit / PENDING_INTEGRATION |

**写后复核**：每条 todo 与正文子项双向一致（见 PLAN_SPEC **第六节**）。
