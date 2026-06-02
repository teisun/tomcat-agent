# Claude Code 与 tomcat 摘要（Compaction）模板对比

> 范围：Claude Code（`cc-fork-01`）AutoCompact 摘要提示词 vs tomcat 的 Compaction 摘要模板。  
> 参考源码：`cc-fork-01/src/services/compact/prompt.ts`；`tomcat/src/core/compaction/summary.rs`；openspec `context-management.md` §7。
>
> **术语**：正文已统一用 **tomcat** 指本仓库宿主；文件名与旧 openspec 仍含 `pi` 字样。若在其他文档见到单独「**Pi**」指本仓实现，含义与 **tomcat** 同，**不是** pi-mono 上游终端里的 `pi` CLI。

---

## 1. Claude Code 模板概览

### 1.1 文件与入口

- **主文件**：`cc-fork-01/src/services/compact/prompt.ts`
- **导出**：`getCompactPrompt()`（全量对话）、`getPartialCompactPrompt()`（部分/分段场景，含 `direction: 'from' | 'up_to'`）
- **后处理**：`formatCompactSummary()` 剥离 `<analysis>`，将 `<summary>` 转为可读文本；`getCompactUserSummaryMessage()` 在摘要外包一层「会话从压缩点继续」的说明，并可附带 transcript 路径、是否保留近期原文等。

### 1.2 结构特点


| 维度       | CC 做法                                                                                                                                                                                                                                                        |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **工具调用** | 开头 `NO_TOOLS_PREAMBLE` + 结尾 `NO_TOOLS_TRAILER`：强制纯文本，禁止 Read/Bash 等工具，避免 fork 摘要浪费唯一一轮                                                                                                                                                                       |
| **草稿区**  | 要求先输出 `<analysis>...</analysis>` 再输出 `<summary>...</summary>`；上线文只保留 summary，`analysis` 被剥离                                                                                                                                                                  |
| **章节**   | **9 个编号小节**（全量 `BASE_COMPACT_PROMPT`）：Primary Request and Intent、Key Technical Concepts、Files and Code Sections（强调完整代码片段）、Errors and fixes、Problem Solving、**All user messages**（非 tool 用户消息全列）、Pending Tasks、Current Work、Optional Next Step（要求引用最近对话原文防漂移） |
| **变体**   | `PARTIAL_COMPACT_PROMPT`：只摘要「近期」；`PARTIAL_COMPACT_UP_TO_PROMPT`：摘要将作为前缀，后接新消息，第 8/9 节改为 Work Completed / Context for Continuing Work                                                                                                                         |
| **可扩展**  | `customInstructions` 可追加「Compact Instructions」类用户/配置指令                                                                                                                                                                                                       |
| **篇幅**   | **无显式 ~8K token 软限制**；强调 thorough、detailed、full code snippets                                                                                                                                                                                                |


### 1.3 设计意图（归纳）

- 偏 **审计级可追溯**：逐条列出用户原话、错误与修复、文件与代码块。
- 与 **fork 子代理 + prompt cache 共享** 路径配合（见 `CONTEXT_MANAGEMENT.md`），工具禁言是为避免单轮摘要失败。
- 输出偏长，以 **完整性** 优先于极致压缩。

---

## 2. tomcat 模板概览

### 2.1 运行时实现（`summary.rs`）

当前 **实际编译进产物** 的常量为较短版本：

- **首次**：`SUMMARIZATION_PROMPT` — 5 个 Markdown 小节：`Goal`、`Constraints`、`Progress`、`Key Decisions`、`Critical Context`。
- **增量**：`UPDATE_SUMMARIZATION_PROMPT` — 注入 `{existing_summary}`，要求合并为同一结构的单份摘要。

特点：无 `<analysis>`/`<summary>` XML、无「禁止工具」长前言（Compaction 调用由宿主单独发请求，是否带 tools 由实现决定）。

### 2.2 架构规格（`openspec/.../context-management.md` §7）

规格中的模板 **比 `summary.rs` 更细**：含 `Constraints & Preferences`、`Progress` 下 Done/In Progress/Blocked、`Next Steps`、以及 **~8K tokens 软引导** 与 UPDATE 规则（保留/合并/删减、完整替代摘要预算）。  
→ **实现与规格存在差距**：若要对齐规格，需在 `summary.rs`（或统一 prompt 源）中同步 §7 全文。

---

## 3. 逐项对比


| 对比项    | Claude Code                          | tomcat（代码）                   | tomcat（openspec §7）           |
| ------ | ------------------------------------ | ---------------------------------- | ----------------------------------- |
| 章节数量   | 9 节（+ analysis 草稿）                   | 5 节                                | 7 块（含子列表）                           |
| 用户原话全列 | **强制**（All user messages）            | 无单独章节，可散落在 Progress                | 无等价「全列用户消息」                         |
| 文件/代码  | 强调 full snippets、为何重要                | Critical Context 中保留路径/错误等         | 强调路径、函数名、错误原文                       |
| 错误与修复  | 独立小节 + 用户反馈                          | 可写入 Progress / Critical Context    | Progress/Blocked + Critical Context |
| 下一步    | Optional Next Step + **verbatim 引用** | Key Decisions / 无单独 Next Steps（代码） | 有 **Next Steps** 有序列表               |
| 草稿区    | `<analysis>` 提升质量后丢弃                 | 无                                  | 无                                   |
| 禁工具声明  | 强约束（防 fork 单轮失败）                     | 无（依赖调用方不传 tools）                   | 未写                                  |
| 篇幅控制   | 无 ~8K 软限制                            | 无（代码）                              | **~8K 软引导** + UPDATE 合并预算           |
| 自定义指令  | `Additional Instructions`            | 无                                  | 无                                   |
| 会话续写包装 | `getCompactUserSummaryMessage`       | 由 Agent/transcript 边界语义承担          | Compaction entry + boundary         |


---

## 4. 差异带来的取舍

**CC 更利于**

- 长会话后仍能从摘要里 **还原用户措辞与任务边界**，减少「摘要漂移」。
- 调试与合规（**谁说了什么**、**改了哪些文件**）。

**tomcat（当前代码）更利于**

- Prompt **短**，单次 Compaction **输入/输出 token 相对可控**。
- 结构简单，实现成本低。

**tomcat（openspec §7）目标形态**

- 在 **结构化 checkpoint**（Goal / Progress / Next Steps）与 **~8K 软引导** 之间折中，接近「可续写的产品级摘要」，但仍 **弱于 CC 对用户原话与代码块的硬性枚举**。

---

## 5. 整合建议：取长补短后的统一模板

### 5.1 取舍原则

**tomcat 的骨架更好**——结构化 checkpoint（Goal / Progress / Next Steps）为「下一轮 LLM 接续工作」而设计，以 **可行动** 为核心。CC 的 9 节偏「审计回溯」，内容全但冗余、token 开销大。

整合方向：**以 tomcat 的 checkpoint 结构为骨架，从 CC 借鉴 4 个单点增强。**

### 5.2 从 CC 借鉴的特性与取舍

| CC 特性 | 价值 | 取舍 |
|---------|------|------|
| `<analysis>` 草稿区 | 先推理再输出，摘要质量更高 | **不采纳**——tomcat 不走 fork 子代理，多一轮草稿 = 多一倍输出 token，性价比不好 |
| All user messages 全列 | 防任务漂移 | **缩小范围**：只要求摘录「最近 10 条用户原话」，不全列 |
| Files & Code Sections | 文件变更可追溯 | **合并进 Progress Done**——每个已完成项带文件路径即可，不需要独立节 |
| Errors & Fixes 独立节 | 避免重复踩坑 | **采纳**——作为独立小节，简短列出错误 + 修复方式 |
| Next Step + verbatim 引用 | 防「摘要漂移」 | **采纳**——要求 Next Steps 第一条附带最近对话原文的简短引用 |
| NO_TOOLS preamble | 防摘要轮调工具 | **简化采纳**——一句话 "Respond with text only, do not call any tools" 即可 |
| customInstructions | 可扩展 | **预留**——实现时 prompt 末尾可追加，模板里不占空间 |

### 5.3 整合后首次摘要模板（SUMMARIZATION_PROMPT）

```
Respond with text only. Do not call any tools.

Create a structured context checkpoint that another LLM will use to continue the work.
The entire summary should be under ~8K tokens. Prioritize actionable information.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed task] (file: path/to/file, if applicable)
- [x] ...

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Errors Encountered
- **[Error description]**: [How it was fixed / current status]
- [Or "(none)" if no errors]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Recent User Messages
- [Verbatim or near-verbatim quote of the 10 most recent non-tool user messages, to preserve task intent]

## Next Steps
1. [Most immediate next step. Include a short quote from the latest conversation showing what was being worked on.]
2. [Subsequent steps]

## Critical Context
- [Any data, file paths, variable names, error messages, or references needed to continue]
- [Or "(none)" if not applicable]
```

### 5.4 整合后增量更新模板（UPDATE_SUMMARIZATION_PROMPT）

```
Respond with text only. Do not call any tools.

Update the existing structured summary with new information. The output REPLACES the old summary entirely.

RULES:
- PRESERVE information from the previous summary that is still relevant
- ADD new progress, decisions, errors, and context from the new messages
- UPDATE Progress: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" and "Recent User Messages" to reflect the latest state
- REMOVE information that is no longer relevant to free space
- The complete updated summary should be under ~8K tokens
- When the old summary is already large, compress older details to stay within budget
- PRESERVE exact file paths, function names, and error messages

Use the EXACT same format as the original summary (Goal / Constraints & Preferences / Progress / Errors Encountered / Key Decisions / Recent User Messages / Next Steps / Critical Context).
```

### 5.5 与 openspec §7（tomcat）的演化总结

| 维度 | 原 openspec §7（tomcat） | 整合后 | 来源 |
|------|---------------------|-------|------|
| 节数 | 7 | **9** | +Errors Encountered, +Recent User Messages |
| 禁工具 | 无 | 首行一句话 | CC 简化 |
| 用户原话 | 无 | 最近 10 条 | CC 缩小范围 |
| 错误追踪 | 散在 Progress/Critical Context | 独立小节 | CC |
| Next Steps | 有序列表 | 第一条带 **verbatim 短引用** | CC |
| Progress Done | 纯文本 | 带文件路径 `(file: ...)` | CC Files & Code 精简版 |
| 草稿区 | 无 | **不加** | 省 token |
| 篇幅 | ~8K | ~8K | 保持 |

核心增加只有两个小节（Errors、Recent User Messages）+ 一句禁工具 + Next Steps 引用。总 prompt 长度增加约 200 字，不影响 ~8K 产出预算。

### 5.6 落地 TODO

- [x] 将 §5.3 / §5.4 模板同步到 `context-management.md` §7（已通过 [§7.5 Compaction v2 修订](../../docs/architecture/context-management.md) 简明补充落地，单一事实来源指向 `preheat.rs` 的 `pub(super) const`）
- [x] 将 `preheat.rs` 中 `SUMMARIZATION_PROMPT` / `UPDATE_SUMMARIZATION_PROMPT` 与新模板对齐（含 `Recent User Messages` 最近 10 条 + `First reason internally, then output the final summary.`；测试锁点 [`prompt_snapshot.rs`](../../src/core/compaction/tests/prompt_snapshot.rs) 13 用例）
- [x] Compaction 专用 ChatRequest 中显式 `tools: None`（双保险：模板首行 `Respond with text only. Do not call any tools.` + 请求体不携带 tool schema）

> 落地工作由 plan [`compaction-prompt-9section`](../../../.cursor/plans/compaction_prompt_9-section_41653219.plan.md) 承接（T2-P0-002 / Phase B），架构 spec 补充见 [`context-management.md` §7.5](../../docs/architecture/context-management.md)。

### 5.7 明确不做的事项（Anti-goals）

下列事项在 plan T2-P0-002 阶段决议为「**不实施**」，避免后续接手者按字面读到 §5 时误以为「凡是 CC 有的都要补齐」「凡是 TODOS.md 列出的都得在 compaction 层做」。每项都附决议理由 + 回链，方便审阅追溯。

| 不做的事 | 来源 | 决议 | 理由 | 回链 |
|---------|------|------|------|------|
| **Two-pass `<analysis>` 草稿区** | 本报告 §5.2 第 1 行（已标"不采纳"） | 不实施；模板指令区追加 `First reason internally, then output the final summary.` 做隐式诱导 | CC 走 fork 子代理 + prompt cache 抵消草稿成本；tomcat 单次 LLM 直发，多一轮草稿 = 输出 token 翻倍 + 撞 max_tokens 上限，性价比不好 | [§5.7.1 Two-pass 决议固化](#571-two-pass-summary-不实施决议固化关闭-t-044)；TODOS [#T-044](../TODOS.md) |
| **在 compaction 层对超大单条消息做字符硬截断 + 哨兵** | TODOS [#T-040](../TODOS.md)、[T2-P0-002](docs/agents/TASK_BOARD_002/tasks/T2-P0-002.md) 子项 3 | 不实施 / 关闭归并 | ① compaction 是「读 → 总结 → 写」的二次加工层，**不应兼任输入校验**——若真要限制单条字符，正确位置是在用户输入入口或 LLM 调用 provider 边界，不是 compaction；② 核实代码后发现 [`preheat.rs::messages_to_text`](../../src/core/compaction/preheat.rs) 对 User/Assistant **不做切片**，**不存在字符边界 panic**——原 #T-040 描述把 Layer 0 的 `floor_char_boundary` 风险（已多年稳定）误外推到 messages_to_text；③ 唯一剩余风险（拼出超长 batch_text → LLM 拒绝）由 plan Phase D 的退避 + transcript 失败留痕直接承接，**用户视角是「ratio 高 + 一条 fail entry」，不 panic / 不 abort，可接受**；④ 引入 `truncate_with_sentinel` + 配置项会新增高风险代码（字符边界 panic）+ 新测试 + spec 字段，收益与成本不对称 | plan [§6.C 决议段](../../../.cursor/plans/compaction_prompt_9-section_41653219.plan.md)；TODOS [#T-040](../TODOS.md) 行尾备注 |
| **在 compaction 层为多次落盘建立 `_index.jsonl` 合并锚点** | TODOS [#T-043](../TODOS.md)、[T2-P0-002](docs/agents/TASK_BOARD_002/tasks/T2-P0-002.md) 子项 5 | 不实施 / #T-043 改判**归属错位**，已抽出独立任务 [T2-P0-011](docs/agents/TASK_BOARD_002/tasks/T2-P0-011.md) 承接 | ① **TODO 原始文案错配**——[#T-043](../TODOS.md) 写的是「**更新大文件时应该多次编辑写入**」，最自然读法是「agent 写大文件时偏好多次小 `edit_file` 而不是一次性 overwrite」，归属应在 `src/core/executor/primitives.rs::edit_file`，看板把它分进 compaction 任务（关联模块 `src/core/compaction/`）属分类错配；② **方案是过度发挥**——`_index.jsonl` 这个具体方案是 plan 起草时根据"分块落盘 + 合并锚点"两个词凑出来的，本报告 / `context-management.md` / 任何 spec 都没明确要求过这个文件；③ **价值评估为零**——所提供的全部信息（落盘哪些块 / 时间序 / 原始大小 / 工具名）可由 **transcript 占位符 `[Tool result persisted: {path} ({len} chars)]` + 文件系统 mtime + 文件名（即 `tool_call_id`）** 完全重建；④ **无消费者**——主路径 [`PersistedResult.persisted_path`](../../src/core/compaction/truncation.rs) 直接持文件路径，没有任何代码读 `_index.jsonl`；⑤ 反观成本：~50 行业务 + ~80 行测试 + 1 份新 JSONL schema + spec 一节，收益 / 成本严重失衡 | plan [§6.E 决议段](../../../.cursor/plans/compaction_prompt_9-section_41653219.plan.md)；TODOS [#T-043](../TODOS.md) T2 映射改判 T2-P0-011；看板 [T2-P0-011](docs/agents/TASK_BOARD_002/tasks/T2-P0-011.md) 任务详情 |

**反向逃生口**：
- 若线上观测到「LLM `context_length_exceeded` 因超长 Assistant/User 消息触发」频率显著（例如 > 1%），单开 ticket 在 Phase D 的失败留痕路径之上加一道 batch_text 长度预检（短路浪费的 3 次 retry），改动 < 5 行，**与本报告 / 当前 plan 解耦**。
- 若将来真要做"按时间序可视化落盘历史"功能（例如 debug 工具），单开 ticket 加一份独立 `_index.jsonl`（< 30 行），**与本报告 / 当前 plan 解耦**。
- `#T-043` 的原始诉求（agent 写大文件时偏好多次 `edit_file`）由独立任务 [T2-P0-011 large-file-edit-strategy](docs/agents/TASK_BOARD_002/tasks/T2-P0-011.md) 承接（system prompt 引导 / `write_file` 大字符串 hint / tool description 增强 三选一或组合，由计划阶段决定，与 compaction 解耦）。

#### 5.7.1 Two-pass summary 不实施决议固化（关闭 [#T-044](../TODOS.md)）

**身份**：本节为 [#T-044](../TODOS.md)「Two-pass summary」的**最终决议落档点**——任何回链到 `compaction-prompt-cc-vs-pi.md§5.7.1` 即视作此决议生效；不再单独建立 ADR 文件（`docs/adr/T-044-two-pass-summary.md` 不存在，也不会创建）。

**背景**：

1. CC（Claude Code）在 [`prompt.ts`](../../../cc-fork-01/src/services/compact/prompt.ts) 里要求模型先在 `<analysis>` 标签内完整列出 1) Primary Request and Intent 2) Key Technical Concepts 3) Files and Code Sections 4) Errors and fixes 5) Problem Solving 6) All user messages 7) Pending Tasks 8) Current Work 9) Optional Next Step **草稿**，再在 `<summary>` 标签内输出最终摘要。这是**事实上的双轮推理**——只是借助单次模型调用 + 标签分区伪装成一次响应。
2. CC 能负担"草稿区翻倍"成本的两个**特殊条件**：
   - **fork 子代理路径**：CC 的 `runCompact` 调用单独 fork 一个子代理处理摘要，主对话上下文不被草稿占用；
   - **prompt cache 命中**：CC 与 Anthropic provider 之间走 `cache_control` 缓存机制，被压缩的历史消息（≥ 1024 token 的 system / user 块）在第二次调用时能命中 90% 以上输入 token 折扣（[Anthropic prompt-caching 文档](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching)），所以"两轮跑一次"的总输入 token 成本相比单轮仅多约 1.1 倍而非 2 倍。
3. tomcat 的部署形态**两个条件都不成立**：
   - tomcat 的 `Compactor::summarize` 走与主 reasoning loop 同一个 `LlmProvider::chat`，没有 fork 子代理；
   - 当前 OpenAI 接入未启用 prompt cache（OpenAI 自身也只对 ≥ 1024 token 的 prefix 命中 50% 折扣，且 cache 命中需要请求间稳定 prefix——压缩场景的 `batch_text` 每次都不同，命中率不稳定）。

因此 tomcat 若复刻 Two-pass，**实质等于把单次摘要的输出 token 直接乘以 2**——9 节模板已经接近 ~8K 输出预算，再翻倍会撞 provider 的 `max_tokens` 上限或显著抬升 latency。

**决议**：本计划（T2-P0-002）**不实施 Two-pass summary**。

**替代策略**（采纳）：

- 在 [§5.3 BASE](#53-整合后首次摘要模板summarization_prompt) / [§5.4 UPDATE](#54-整合后增量更新模板update_summarization_prompt) 模板的指令区追加一句 `First reason internally, then output the final summary.`，让模型走**内部隐式推理**（不输出可见草稿），相当于一次有引导的单轮推理；
- 现代 LLM（GPT-4 / Claude 3.5+ / Qwen-Max）对此句的内部推理已被广泛验证有效，落地成本 = 1 句 prompt，相对 Two-pass 节省约 1 倍输出 token；
- 验收**只校验** prompt 含此行，**不校验**模型是否真的进行了内部推理（无可观测端点，且超出本任务责任范围）。

**反向逃生口**：

- 若后续在某个 provider 上观测到 9 节摘要质量明显劣化（例如多个 in-progress 项被遗漏 / Next Steps verbatim 引用与最近用户消息不匹配 ≥ 30%），重新评估 Two-pass 时优先考虑：① 走 fork 子代理路径（需要主 reasoning loop 配合）；② 在 OpenAI / Anthropic provider 启用 prompt cache + 稳定 prefix（需要重排 message 顺序）；
- 真要做时单开 ticket，**与本任务 / 本报告解耦**；不在 plan T2-P0-002 中追加。

**关闭轨迹**：

- [`docs/TODOS.md`](../TODOS.md) 中 `#T-044` 的状态/备注同步标注为「**报告决议关闭**：详见 `compaction-prompt-cc-vs-pi.md §5.7.1`」（已于 2026-04-26 在看板侧落档；详见 **Git 历史**）。
- plan [`compaction_prompt_9-section_41653219.plan.md` §6.A](../../../.cursor/plans/compaction_prompt_9-section_41653219.plan.md) 的 `impl-A-adr-two-pass` 子项指向本节，作为决议落地凭证。

---

## 6. 参考路径速查

| 项目 | 路径 |
|------|------|
| CC 摘要 prompt | `cc-fork-01/src/services/compact/prompt.ts` |
| CC 上下文总览 | `cc-fork-01/docs/CONTEXT_MANAGEMENT.md` |
| tomcat 运行时 prompt | `tomcat/src/core/compaction/summary.rs` |
| tomcat 规格模板 | `tomcat/docs/architecture/context-management.md` §7 |


