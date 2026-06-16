# 002-single-agent-complete · 立项与上下文

> 本页为 [README.md](./README.md) 的 **§1–§2** 拆出正文；任务状态说明、索引表与依赖拓扑仍以 README 为准。

---

## 1. 迭代立项块

| 字段 | 值 |
|------|----|
| **迭代代号** | `002-single-agent-complete` |
| **启动日期** | 2026-04-22 |
| **迭代主题** | 把单 Agent 的基础体验、状态管理、任务循环做到极致，再考虑扩展到多 Agent / Skill / 插件生态 |
| **对应档位** | P0（基础体验）+ P1（状态管理） |
| **路线图** | [`Product_Brief.md`](../../openspec/specs/Product_Brief.md)（P0-P9 执行编排） |

### 1.1 迭代目标

1. **单 Agent 基础体验无 P0 bug**：工具授权、工作目录权限、中断/恢复、长任务后台化、TUI 体验 6 个方向全部达标；
2. **摘要与 compaction 行为对齐** [`docs/reports/compaction-prompt-cc-vs-pi.md`](../../reports/compaction-prompt-cc-vs-pi.md) §5.3/§5.4，升级 9 节模板 + 禁 tools 调用 + context v2 剩余落地；
3. **Agent Loop 模块化**：`src/core/agent_loop/run.rs`（832 行）拆为 dispatcher / tool_exec / stream_handler / error_classifier 子模块；三套管道（`src/ext/`）语义统一；
4. **Thinking API + 展示**：`src/core/llm/` 接入 Claude/GPT/Qwen 的 thinking 协议，TUI 支持可折叠展示；
5. **状态管理**：Checkpoint 机制 + 断点续跑、PLAN 模式增强、提问/应答机制、结果验证 review 子流程、集成测试规范达标；**Feedback 回路（T2-P1-005）002 迭代内取消**；
6. **交付口径**：20 个 T2-P0/P1 任务全部 DONE；macOS/Linux 上单 Agent 可连续运行 ≥ 4 小时不退化；compaction 触发不产生死循环。

### 1.2 不做的范围

与 `Product_Brief.md` 新档位对齐，002 迭代明确**不做**：

1. **多 Agent / Agent 编排 / 安全体系 / 多会话**（P5）；
2. **插件系统新特性**（冻结区，P6；只做 T-001 VMActor shutdown 等维护性修复）；
3. **Skill 系统**（P2）；
4. **记忆系统 / USER.md / MEMORY.md**（P3）；
5. **自进化 / 学习回路**（P4）；
6. **跨平台（Android、openclaw 兼容与平台适配）**（P7）；
7. **多 LLM 适配 / 多 IM 网关**（P8）；
8. **UI（Tauri、Android、插件可视化）**（P9）。

> 例外说明：`T2-P1-014 Skill 系统` 于 2026-06-06 由用户显式点名提前执行，用于按已定稿架构文档落地 P2 Skill 主干；该例外不改变 002 的主目标与其余 P2/P3/P4 档位默认冻结口径。

### 1.3 验收口径

1. 20 个 T2-P0/P1 任务全部标记 `DONE` 并通过 Nibbles 复核；
2. 单元测试覆盖率 ≥ 80%，集成测试（compaction、agent loop、权限、中断恢复）全部绿灯；
3. `docs/openspec/specs/Product_Brief.md` 与本看板在档位与任务 ID 映射上内部一致（归档回归检查以 Git 历史为准）。

### 1.4 风险与应对

| 风险 | 影响 | 应对 |
|------|------|------|
| Agent Loop 拆分涉及核心路径，可能引入回归 | 高 | 拆分前先补集成测试；分模块递进合并，每次 PR 附 e2e 截图/日志 |
| Thinking API 在 OpenAI / Anthropic / Qwen 协议不统一 | 高 | 先定义内部 `ThinkingEvent` 抽象，Provider 侧做适配；TUI 折叠面板复用 render 层 |
| TUI 体验重构（T2-P0-008）影响面大 | 中 | 合并到单个 T2 任务统一推进；先冻结现有 render 逻辑，新增面板分层叠加 |
| Checkpoint 设计需共识 | 中 | 先出 design 草案进 `docs/agents/plan/`，由 Nibbles 发起 review 后再开工 |
| Compaction prompt 改动可能触发旧 transcript 不兼容 | 中 | 保留旧摘要兜底路径，新 prompt 先做 A/B 观察 2 个会话周期 |
| 权限分级（T2-P0-004）与既有 4 原语 audit 日志耦合 | 中 | 先补 dry-run 模式，再切换；审计日志新增 `permission_level` 字段 |
| T2-P0-003 末位调度、迭代末仍未闭环 | 低 | 主动取消 + `reqwest` 整请求超时已兜底主要挂死面；下迭代首周可补热修 / 提优先级 |

### 1.5 优先级说明（必读）

> ⚠️ **本看板 P0/P1 含义与 `Product_Brief.md` 的 P0/P1 含义不同。**
>
> - `Product_Brief.md` P0-P9：**执行编排顺序**（全项目尺度），上一档完成后才投入下一档。
> - 本看板 P0 / P1：**当前迭代内部优先级**——`P0 = 当前迭代必做`，`P1 = 当前迭代应做`，两档都落在 `Product_Brief.md` 的 P0-P1 区间内。
> - 档位 `P0..P9` 以 `Product_Brief.md` 为准；本看板 `T2-PX-YYY` 为当前迭代任务 ID，与 Product_Brief 的 P0-P1 区间对齐。

---

## 2. 当前迭代上下文

| 字段 | 值 |
|------|----|
| 当前迭代 | `002-single-agent-complete` |
| 规格文档 | [Product_Brief.md](../../openspec/specs/Product_Brief.md) · [Architecture.md](../../openspec/specs/Architecture.md) · [Constitution.md](../../openspec/specs/Constitution.md) |
| 架构长文（按任务点名读） | [`docs/architecture/`](../../architecture/) — 勿整目录通读 |
| 关键设计报告 | [compaction-prompt-cc-vs-pi.md](../../reports/compaction-prompt-cc-vs-pi.md) · [plugin_skills_first_principles_pi_rust_wasm.md](../../reports/plugin_skills_first_principles_pi_rust_wasm.md) · [llm-tool-rounds-cli-display-thinking-protocol.md](../../reports/llm-tool-rounds-cli-display-thinking-protocol.md) · [agent_error_handling_cross_repo.md](../../reports/agent_error_handling_cross_repo.md) |
| 协作约定 | [Dispatcher.md](../Dispatcher.md) · [Nibbles.md](../Nibbles.md) · [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../INTEGRATION_MERGE_AND_ACCEPTANCE.md) · [Tom.md](../Tom.md) · [Jerry.md](../Jerry.md) · [Spike.md](../Spike.md) |

