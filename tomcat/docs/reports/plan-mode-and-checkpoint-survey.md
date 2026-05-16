# PLAN 模式与 Checkpoint：多 Agent 实现调研报告

> **范围**：本仓库内可达源码：`cc-fork-01`、`codex`、`GenericAgent`、`hermes-agent`、`openclaw`、`pi_agent_rust`、`pi-mono`，以及 **Tomcat** 任务板 T2-P1-001 / T2-P1-002 的规划表述。  
> **方法**：精读任务卡与代表性源文件；部分目录用系统 `grep`/`find` 补充（工作区内置 `rg` 对部分路径不可用）。  
> **日期**：2026-05-15

---

## 1. 术语对齐

| 术语 | 本文含义 |
|------|-----------|
| **PLAN 模式（类）** | 显式或约定地把「规划 / 拆解 / 跟踪」与「执行」分离的机制：专用工具、权限子模式、或 SOP+状态位。不等同于 Cursor IDE 的 Ask/Plan 产品开关（Tomcat 另有 [`plan-mode-execution-playbook-T2-P0-001.md`](./plan-mode-execution-playbook-T2-P0-001.md) 复盘 IDE 流程）。 |
| **Checkpoint** | **A. 工作区快照 / 可回滚点**（磁盘状态）；**B. 会话/压缩检查点**（持久化或摘要语义）；**C. 运行态「工作记忆」**（短字符串锚点，非完整快照）。下文按实现归类。 |

---

## 2. 各项目：类 PLAN 模式如何实现

### 2.1 cc-fork-01（Claude Code 系 fork）

- **机制**：一等工具 **`EnterPlanMode`** / **`ExitPlanMode`**（及 v2 变体）。  
- **行为要点**（`EnterPlanModeTool.ts`）：  
  - 调用 `handlePlanModeTransition` 与 `prepareContextForPlanMode`，把应用态里的 **`toolPermissionContext.mode` 设为 `plan`**（会话级）。  
  - 工具声明为 **只读**（`isReadOnly: true`），结果里指示模型：**先探索与设计，不要写文件**；准备好后用 **`ExitPlanMode`** 提交计划待审批。  
  - 子 Agent 上下文（`context.agentId`）下禁止进入 Plan，避免嵌套陷阱。  
- **配套**：**`TodoWrite`** 或 Todo v2 下的 **`TaskCreate` / `TaskGet` / `TaskList` / `TaskUpdate`**（见 [`agent-tools-comparison.md`](./agent-tools-comparison.md) §4.6）。  
- **与 IDE Cursor PLAN**：同源产品族思路（工具切换「规划 vs 执行」），但实现是 **运行时工具 + 权限模式**，不是 Cursor 侧 `CreatePlan` 文件流。

### 2.2 openclaw

- **机制**：可选工具 **`update_plan`**（`src/agents/tools/update-plan-tool.ts`）。  
- **行为**：模型提交 **`plan: [{ step, status }]`**，`status ∈ {pending, in_progress, completed}`，且 **至多一个 `in_progress`**；返回结构化 `details`（供 UI/ runner 消费）。  
- **门控**：配置项显式启用；测试描述中 **对 GPT-5 strict-agentic 等场景可自动启用**（见 `openclaw-tools.update-plan.test.ts`）。  
- **说明**：**不切换全局工具白名单**；与 cc-fork 的「整段会话进入 plan 权限模式」不同，更偏 **结构化进度表**。

### 2.3 hermes-agent

- **机制**：无与 cc-fork 同名的 Enter/Exit Plan；**规划面**主要靠：  
  - **`todo`** 工具：会话内 **内存 TodoStore**，`write`/`merge` 更新，压缩后可再注入上下文（见 `tools/todo_tool.py` 头注释）。  
  - **`kanban_*`** 等任务看板工具族（对比表见 [`agent-tools-comparison.md`](./agent-tools-comparison.md) §4.6）。  
- **定位**：分解任务、跨轮次保持焦点；**不是**「只读探索直到用户批准」的硬闸门。

### 2.4 pi_agent_rust / pi-mono

- **机制**：**无一等 Plan / Todo 工具**（对比表同上）。规划依赖模型自然语言 + 用户工作流，或通过 **扩展/MCP** 自建。  
- **pi-mono**：`SUMMARIZATION_SYSTEM_PROMPT`（`packages/coding-agent/src/core/compaction/utils.ts`）仅约束 **压缩摘要** 的输出形态，**不是**计划模式。

### 2.5 GenericAgent

- **机制**：混合 **SOP 文档**（如 `memory/plan_sop.md`）+ **运行态标志**。  
- **代码**：`ga.py` 中 `enter_plan_mode(plan_path)` 写入 `working['in_plan_mode']`；`_check_plan_completion()` 扫描 `plan.md` 中未勾选 `[ ]`；`no_tool` 路径里对「声称完成」做 **VERIFY / VERDICT** 拦截；周期性注入 **必须 `file_read(plan.md)`** 的提示。  
- **`update_working_checkpoint`**：把 `key_info` / `related_sop` 写入 **`working`**，并注入到 `_get_anchor_prompt()` 的 **`<key_info>`** 段——属于下文 **Checkpoint-C（工作锚点）**，不是磁盘快照。

### 2.6 codex

- **机制**：**无**面向用户的「PLAN 模式」工具；多步工作由 **产品 UX / 提示词 / rollout 诊断** 等承担，不在本仓枚举为 `EnterPlanMode` 类 API。

### 2.7 Tomcat（规划态）

- **当前**：无内建 PlanRuntime；任务 **T2-P1-002** 描述目标闭环（执行面板、子 Agent review、文件锁、`agents/plan/<timestamp>.md`、里程碑拆分等）。  
- **Cursor 侧**：仓库内 [`plan-mode-execution-playbook-T2-P0-001.md`](./plan-mode-execution-playbook-T2-P0-001.md) 描述的是 **IDE PLAN 工作流**，与 Tomcat 运行时分离。

---

## 3. 各项目：Checkpoint 如何实现

### 3.1 hermes-agent — **Checkpoint-A（工作区快照）**

- **实现**：`tools/checkpoint_manager.py` — **影子 Git 仓库**（`GIT_DIR` 指向 `~/.hermes/checkpoints/<sha256(workdir)[:16]>/`，`GIT_WORK_TREE` 为真实项目目录）。  
- **触发**：在 **`write_file` / `patch`** 及若干 **破坏性终端命令** 之前自动快照；**每个目录每对话轮至多一次**（见 `website/docs/user-guide/checkpoints-and-rollback.md`）。  
- **用户接口**：**`/rollback`** 列出、恢复整树或单文件、diff 预览。  
- **性质**：**LLM 不可见**，配置 `checkpoints.enabled` 控制；与 `todo` 无代码级绑定。

### 3.2 cc-fork-01

- **用户态「会话 checkpoint」**：源码检索中 **未发现** 与 Hermes 同级的影子仓库式 rollback。  
- **名为 checkpoint 的符号**：主要为 **启动性能** `profileCheckpoint`（`main.tsx`），**不属于**工作区回滚。

### 3.3 openclaw

- **无**与 Hermes 文档等价的、agent 核心内置的 **工作区 checkpoint + /rollback** 链（agent 目录下检索未见同类管理器）。存在的是 **AssistantReplySnapshot**、插件元数据 snapshot 等 **别义「快照」**。

### 3.4 pi_agent_rust — **Checkpoint-B（会话文件完整性）**

- **`session.rs`**：`appends_since_checkpoint` / `compaction_checkpoint_interval` — 控制 **会话持久化文件**在若干次增量追加后 **做一次完整重写**，避免碎片与损坏风险。  
- **语义**：工程上的 **存储 checkpoint**，不是给用户「回到某任务节点」的语义。

### 3.5 pi-mono

- **Compaction**：长会话 **上下文压缩**（`packages/coding-agent/src/core/compaction/compaction.ts`），与 pi 系「摘要替代早期消息」一致。  
- **可选扩展示例**：`packages/coding-agent/examples/extensions/git-checkpoint.ts` — **每轮 git stash 式代码检查点**，服务 `/fork` 恢复；**示例扩展**，非核心默认行为。  
- **消息标签**：session-manager 支持 **`checkpoint` 标签**（测试见 `labels.test.ts`），属 **UI/元数据**，非文件系统快照。

### 3.6 codex — **Checkpoint-B（压缩 / 追踪）**

- **`codex-rs/rollout-trace`**：`CompactionCheckpoint` 等类型表示 **远程压缩安装**在 **诊断 trace** 中的事件节点（`rollout-trace/README.md` 图中 *inference + compaction … checkpoints*）。  
- **性质**：**可观测性 / 还原对话图**，不是终端用户一键「任务 checkpoint」商品。

### 3.7 GenericAgent — **Checkpoint-C（工作记忆）**

- **`do_update_working_checkpoint`**：更新 `working['key_info']` 等，进入后续 turn 的 **WORKING MEMORY** 提示块。  
- **与 plan_sop**：文档要求规划/执行各阶段 **更新 checkpoint 字符串**；**不**产生独立快照 ID 或自动回滚。

### 3.8 Tomcat — **规划中（T2-P1-001）**

- **目标**（`tomcat/agents/TASK_BOARD_002/tasks/T2-P1-001.md`）：`CheckpointStore`、写入时机、**`tomcat session rollback <id>`**、基于 transcript + checkpoint 的 **resume**。  
- **与上下文文档**：[`interrupt-and-cancellation.md`](../architecture/interrupt-and-cancellation.md) 将跨 session resume / Checkpoint 标为 **T2-P1-001** 范围。

---

## 4. Checkpoint 是否必须搭配类 PLAN 模式？

**结论：不必；取决于产品目标。**

| 场景 | 说明 |
|------|------|
| **Hermes** | **Checkpoint（磁盘）** 与 **`todo`（计划）** 正交；开/关 checkpoint 不依赖是否使用 todo。 |
| **cc-fork** | **Plan 模式**管权限与阶段；**无**同级内置工作区 checkpoint；二者无「硬绑定」。 |
| **openclaw** | **`update_plan`** 为可选进度工具；**无**内置 Hermes 式 rollback 时，更无「必须搭配」。 |
| **GenericAgent** | SOP **建议**在规划/执行中写 checkpoint 字符串，属于 **流程规范**，不是解释器级依赖。 |
| **Tomcat（看板）** | **T2-P1-002 显式依赖 T2-P1-001**，且文案要求消费 **`CheckpointStore`** 以实现文件锁、进展记录、里程碑 —— 这是 **产品层耦合**：希望 **PlanRuntime** 能把「计划态元数据」存进可回滚的存储，**不等于**业界通用定理。 |

---

## 5. 各 Agent 中「Checkpoint × PLAN」关系小结

| 项目 | 类 PLAN | Checkpoint 主语义 | 关系 |
|------|---------|---------------------|------|
| **cc-fork-01** | Enter/Exit + Todo | 无用户态工作区 checkpoint（仅有 profiler 名） | **独立**（Plan 不自带磁盘快照） |
| **openclaw** | `update_plan`（可选） | 无核心 Hermes 式快照 | **独立** |
| **hermes-agent** | `todo` / kanban | 影子 Git `/rollback` | **正交** |
| **pi_agent_rust** | 无 | 会话持久化 checkpoint 计数 | **无关** |
| **pi-mono** | 无 | 压缩 + 示例 git-checkpoint 扩展 | **无关 / 可选扩展** |
| **codex** | 无 | rollout trace 中 compaction checkpoint | **无关** |
| **GenericAgent** | `plan_sop` + `in_plan_mode` | `update_working_checkpoint` 工作记忆 | **流程上常一起用**；非硬编码依赖 |
| **Tomcat** | T2-P1-002 规划中 | T2-P1-001 规划中 | **看板设计：Plan 消费 CheckpointStore** |

---

## 6. 对 Tomcat 落地的启示（简要）

1. **若目标是 Hermes 级「改码可撤销」**：应对齐 **Checkpoint-A**（工具前钩子 + 用户命令回滚），与是否实现 **PlanRuntime** 无关。  
2. **若目标是 cc-fork 级「先读后写」**：应对齐 **权限 mode + Exit 审批**，可与 **CheckpointStore** 分阶段交付。  
3. **若目标是 OpenClaw 级「轻量计划表」**：可先上 **结构化 `update_plan` 等价物**（面板消费），再决定是否把 **里程碑** 持久化进 checkpoint 元数据。  
4. **T2-P1-002 依赖 T2-P1-001** 合理：**文件锁 / 多进程写 `agents/plan/*.md`** 需要稳定快照或版本向量；但 **语义上**仍建议保持 **「会话快照」与「计划工具」模块边界清晰**，避免把「压缩摘要」与「工作区回滚」混名。

---

## 7. 参考路径（仓库内）

- Tomcat 任务：`tomcat/agents/TASK_BOARD_002/tasks/T2-P1-001.md`、`T2-P1-002.md`  
- 工具对比总表：`tomcat/docs/reports/agent-tools-comparison.md`  
- cc-fork-01：`src/tools/EnterPlanModeTool/EnterPlanModeTool.ts`、`src/tools/ExitPlanModeTool/ExitPlanModeV2Tool.ts`  
- openclaw：`src/agents/tools/update-plan-tool.ts`、`src/agents/openclaw-tools.update-plan.test.ts`  
- hermes-agent：`tools/checkpoint_manager.py`、`website/docs/user-guide/checkpoints-and-rollback.md`、`tools/todo_tool.py`  
- GenericAgent：`ga.py`（`enter_plan_mode` / `do_update_working_checkpoint`）、`memory/plan_sop.md`  
- pi_agent_rust：`src/session.rs`（`appends_since_checkpoint`）  
- pi-mono：`packages/coding-agent/src/core/compaction/utils.ts`、`examples/extensions/git-checkpoint.ts`  
- codex：`codex/codex-rs/rollout-trace/README.md`  

---

## 8. Tomcat：Checkpoint 与 PLAN 分开做还是一起做？

**结论**：**模块上分开设计与实现、按里程碑分阶段交付**；在少数 **契约面**（谁持久化、谁拿锁、崩溃后如何恢复）上 **一起做集成设计**，但不要从第一天把两块代码糊进同一个「大泥球」。

### 8.1 为什么适合先分开

1. **问题域不同**：`CheckpointStore` 管 **可恢复状态 / rollback / resume**（偏基础设施）；`PlanRuntime` 管 **计划拆解、review、执行面板、`agents/plan/*.md`、文件锁**（偏产品编排）。边界清晰后，单测与回滚验收不依赖计划面板是否已存在。  
2. **依赖方向保持单向**：与看板 **T2-P1-002 依赖 T2-P1-001** 一致——**Plan 消费 CheckpointStore（或其稳定子集）**，而不是「没有 PLAN 就不能做 checkpoint」。Hermes 式「只开回滚、不用 todo」在 Tomcat 仍应可行。  
3. **验收可拆分**：先完成 **中断 → 重启 → resume** 与 **`session rollback`**，再叠 **计划 / review / 锁**；避免一个 PR 同时扛存储语义与交互状态机。

### 8.2 建议「一起做」的集成点（仍是两模块、一条契约）

| 集成点 | 做法 |
|--------|------|
| **计划文件写入与锁** | Plan 侧持锁与写路径；若锁或草稿需跨崩溃可解释，由 **CheckpointStore 记录版本或元数据**（或等价 WAL），双方在 **写入提交 API** 上对齐，而非合并实现类。 |
| **里程碑完成** | 「完成里程碑 M」是否 **自动打一个命名 checkpoint** → 作为 **PlanRuntime 策略**，内部调用 `CheckpointStore::record(...)`，可选、可配置。 |
| **混名防范** | 会话/工作区 **rollback 点**、上下文 **compaction 摘要**、Layer0 **tool-result 落盘** 在文档与类型命名上 **不要都叫 checkpoint**，减少排障成本。 |

### 8.3 与看板依赖的对应关系

- **T2-P1-001 先落地**：`CheckpointStore` + 写入时机 + rollback + resume 的 **稳定 API**。  
- **T2-P1-002 后落地**：`PlanRuntime` **只依赖上述 API**；看板里的「文件锁 / 进展落盘」通过 **显式接口**（而非共享可变全局状态）接在 Store 上。

---

*本报告仅基于当前 checkout 源码与文档；各上游 main 分支若新增能力，需以当时仓库为准。*
