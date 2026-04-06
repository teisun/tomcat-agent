# Claude Code 与 pi-rust-wasm 摘要（Compaction）模板对比

> 范围：Claude Code（`cc-fork-01`）AutoCompact 摘要提示词 vs pi-rust-wasm 的 Compaction 摘要模板。  
> 参考源码：`cc-fork-01/src/services/compact/prompt.ts`；`pi-rust-wasm/src/core/compaction/summary.rs`；openspec `context-management.md` §7。

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

## 2. pi-rust-wasm 模板概览

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


| 对比项    | Claude Code                          | pi-rust-wasm（代码）                   | pi-rust-wasm（openspec §7）           |
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

**Pi（当前代码）更利于**

- Prompt **短**，单次 Compaction **输入/输出 token 相对可控**。
- 结构简单，实现成本低。

**Pi（openspec §7）目标形态**

- 在 **结构化 checkpoint**（Goal / Progress / Next Steps）与 **~8K 软引导** 之间折中，接近「可续写的产品级摘要」，但仍 **弱于 CC 对用户原话与代码块的硬性枚举**。

---

## 5. 整合建议：取长补短后的统一模板

### 5.1 取舍原则

**Pi 的骨架更好**——结构化 checkpoint（Goal / Progress / Next Steps）为「下一轮 LLM 接续工作」而设计，以 **可行动** 为核心。CC 的 9 节偏「审计回溯」，内容全但冗余、token 开销大。

整合方向：**以 Pi 的 checkpoint 结构为骨架，从 CC 借鉴 4 个单点增强。**

### 5.2 从 CC 借鉴的特性与取舍

| CC 特性 | 价值 | 取舍 |
|---------|------|------|
| `<analysis>` 草稿区 | 先推理再输出，摘要质量更高 | **不采纳**——Pi 不走 fork 子代理，多一轮草稿 = 多一倍输出 token，性价比不好 |
| All user messages 全列 | 防任务漂移 | **缩小范围**：只要求摘录「最近 2~3 条用户原话」，不全列 |
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
- [Verbatim or near-verbatim quote of the 2~3 most recent non-tool user messages, to preserve task intent]

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

### 5.5 与原 Pi §7 的变化总结

| 维度 | 原 Pi（openspec §7） | 整合后 | 来源 |
|------|---------------------|-------|------|
| 节数 | 7 | **9** | +Errors Encountered, +Recent User Messages |
| 禁工具 | 无 | 首行一句话 | CC 简化 |
| 用户原话 | 无 | 最近 2~3 条 | CC 缩小范围 |
| 错误追踪 | 散在 Progress/Critical Context | 独立小节 | CC |
| Next Steps | 有序列表 | 第一条带 **verbatim 短引用** | CC |
| Progress Done | 纯文本 | 带文件路径 `(file: ...)` | CC Files & Code 精简版 |
| 草稿区 | 无 | **不加** | 省 token |
| 篇幅 | ~8K | ~8K | 保持 |

核心增加只有两个小节（Errors、Recent User Messages）+ 一句禁工具 + Next Steps 引用。总 prompt 长度增加约 200 字，不影响 ~8K 产出预算。

### 5.6 落地 TODO

- [ ] 将 §5.3 / §5.4 模板同步到 `context-management.md` §7（替换现有 §7.1 / §7.3）
- [ ] 将 `summary.rs` 中 `SUMMARIZATION_PROMPT` / `UPDATE_SUMMARIZATION_PROMPT` 与新模板对齐
- [ ] Compaction 专用 ChatRequest 中显式不传 `tools`（或传空），配合首行禁工具声明

---

## 6. 参考路径速查

| 项目 | 路径 |
|------|------|
| CC 摘要 prompt | `cc-fork-01/src/services/compact/prompt.ts` |
| CC 上下文总览 | `cc-fork-01/docs/CONTEXT_MANAGEMENT.md` |
| Pi 运行时 prompt | `pi-rust-wasm/src/core/compaction/summary.rs` |
| Pi 规格模板 | `pi-rust-wasm/openspec/specs/architecture/context-management.md` §7 |


