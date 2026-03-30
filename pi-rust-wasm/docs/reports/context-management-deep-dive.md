# 工具轮次与上下文管理深度分析

> 创建：2026-03-29 | 最后更新：2026-03-30
> 范围：pi-rust-wasm 现状分析 + pi-mono / openclaw 设计对照 + 业界方案 + **最终设计决策**
> 来源：从综合报告 `llm-tool-rounds-cli-display-thinking-protocol.md` 第一章独立拆出并整合 Q&A

---

## 目录

- [1. pi-rust-wasm 现状与问题](#1-pi-rust-wasm-现状与问题)
- [2. 参考项目策略](#2-参考项目策略)
- [3. 关键概念与数学模型](#3-关键概念与数学模型)
- [4. 三项目对比](#4-三项目对比)
- [5. 上下文语义完整性](#5-上下文语义完整性)
- [6. 最终设计决策（pi-rust-wasm）](#6-最终设计决策pi-rust-wasm)

---

## 1. pi-rust-wasm 现状与问题

### 1.1 当前参数

| 参数 | 值 | 位置 |
|------|---|------|
| `max_tool_rounds` | 10 | `agent_loop.rs` L326 `AgentLoopConfig::default()` |
| `DEFAULT_CONTEXT_CAP` | 10（条） | `session/manager.rs` L19 |

- **reasoning loop**（第三层循环）每轮把 **全量累积的 messages** 经 `convert_to_llm_format` 发给 API；`turn_index` 到达 `max_tool_rounds` 才退出。
- **session 历史**：`build_context_messages(recent_n)` 从 transcript 取最近 `recent_n` 条 Message 型 entry，向前回退到首条 `role=user`。
- 两处都**没有** token 级别检查。

### 1.2 两个维度的风险

**维度 1：reasoning loop 内消息膨胀**

```
  用户输入 "帮我重构 main.rs"，进入 reasoning loop：

  ┌─────────── initial messages ───────────┐
  │ [system] ~500 tok  [user] ~20 tok      │  合计 ≈ 520 tok
  └────────────────────────────────────────┘

  第 1 轮：发 520 tok → LLM 返回 tool_calls ~200 tok → 执行 read_file ~2500 tok → 累计 ≈ 3,220
  第 2 轮：发 3220 tok → edit_file ~800 tok → 结果 ~200 tok                    → 累计 ≈ 4,220
  第 N 轮：每轮全量发送；最坏 10 轮 × 大文件 → 50,000+ tok

  关键：max_tool_rounds=10 限制的是「轮数」，真正瓶颈是 token 总量是否超 context_window
       当前代码【没有】token 级检查
```

**维度 2：跨轮次 session 历史裁剪**

```
  transcript 全量:
  ┌────────────────────────────────────────┐
  │ msg_1(user)  msg_2(assistant+tools)    │
  │ msg_3(tool result = 50000 chars!)      │  ← 巨大
  │ ...  msg_18(user)  msg_19(assistant)   │
  │ msg_20(user) ← 当前输入                │
  └────────────────────────────────────────┘
           │ build_context_messages(cap=10)
           │ 取最近 10 条，向前锚到 role=user
           ▼
  ┌────────────────────────────────────────┐
  │ msg_11(user) ~ msg_20  共 10 条        │
  └────────────────────────────────────────┘
  问题：50000 chars 的 tool result 若落在窗口内仍会溢出；按「条数」裁剪 ≠ 按「token」裁剪
```

---

## 2. 参考项目策略

### 2.1 pi-mono

- **无 `max_tool_rounds` 上限**，工具循环由 **token 预算** 兜底。
- **Compaction 机制**（`compaction.ts`）：
  - 触发：`contextTokens > contextWindow - reserveTokens`（默认 `reserveTokens = 16384`）
  - 切分：`findCutPoint` 从尾部向前累加 `estimateTokens`（chars/4），达到 `keepRecentTokens`（默认 20000）后在合法边界截断（不能切在孤立 toolResult 上）
  - 摘要：`generateSummary` 用结构化模板（Goal / Progress / Critical Context），tool 结果截 2000 字进入摘要输入；若已有旧 summary 则用 `UPDATE_SUMMARIZATION_PROMPT` 合并
- **触发时机**：每轮 agent 结束后 + 新 `prompt()` 发送前

### 2.2 openclaw

依赖 pi-coding-agent 的 agent loop（无独立 `max_tool_rounds`），在外围叠加多层防线：

| 防线 | 机制 | 位置 |
|------|------|------|
| 历史轮次截断 | `limitHistoryTurns`：按最近 N 个 **user 轮次** 裁剪（N 由 `historyLimit` / `dmHistoryLimit` 配置，无硬编码默认值，未配置时不裁剪，实际部署一般 3~10） | `history.ts` |
| 单条 tool 截断 | 上限 `floor(contextWindowTokens × 2 × 0.5)` 字符；在主体后 30% 内找换行切断，拼 `[truncated: ...]` 后缀 | `tool-result-context-guard.ts` |
| 总上下文超标 | 估算字符超 `tokens×4×0.75` → 从最旧 tool result 起逐个换占位符 `[compacted: ...]`，累计省够即停（**非全量替换**） | 同上 |
| 溢出预警 | 处理后仍超 `tokens×4×0.9` → 抛错走 overflow 恢复 | 同上 |
| 硬字符上限 | `HARD_MAX_TOOL_RESULT_CHARS = 400,000` | `tool-result-truncation.ts` |
| 工具循环检测 | 滑动窗口 30 次调用，记录 `(name, args_hash)`；10 次 warning → 注入 steering msg；20 次 critical → 强制中止；30 次全局熔断。三种检测器：`generic_repeat`、`known_poll_no_progress`、`ping_pong` | `tool-loop-detection.ts` |
| Compaction | `reserveTokens` / `keepRecentTokens` / `maxHistoryShare`，溢出恢复最多 3 次 | `run.ts` |

**user turn 定义**：一条 `role=user` + 其后所有 `role=assistant` / `role=tool`，直到下一条 `role=user`。

```
  示例 limit=2（保留最近 2 个 user turns）：

  [user] "重构 config"   ← turn 1  → 被裁掉
  [assistant] [tool] [assistant]    → 被裁掉
  ─────────────────────────────────
  [user] "error handling" ← turn 2  → 保留
  [assistant] [tool] [assistant]    → 保留
  ─────────────────────────────────
  [user] "重构 main.rs"  ← turn 3  → 保留
```

---

## 3. 关键概念与数学模型

### 3.1 context_window

模型**固有参数**，非计算所得，通过配置传入或查表获取。**单次请求的输入 + 输出（含 reasoning）共同受此封顶**。

| 模型 | context_window | max output |
|------|---------------|------------|
| GPT-4o | 128,000 | 16,384 |
| GPT-5.2 | 400,000 | 128,000 |
| Claude 3.5 Sonnet | 200,000 | 8,192 |
| DeepSeek-V3 | 64,000 | 8,192 |
| Doubao Pro 128K | 128,000 | — |
| Qwen2.5 | 131,072 | — |

### 3.2 reserve_tokens

为模型回复预留的 token 空间。**`reserve_tokens` 应 ≥ 业务允许的最长回复（`max_tokens`）+ 缓冲**。

```
  context_window = 128,000
  ┌──────────────────────────────────────────────────┐
  │  prompt tokens（你发的）  │  completion tokens    │
  │  ◄── 越大越好 ──►        │  ◄── 必须留够 ──►    │
  └──────────────────────────────────────────────────┘

  安全线：prompt_tokens ≤ context_window - reserve_tokens
  pi-mono 默认 reserve = 16,384 → prompt ≤ 111,616
```

以 GPT-5.2（400K）为例：

| 场景 | reserve_tokens | prompt 可用 |
|------|---------------|-------------|
| 编程 agent 短回复 | 16K~32K | ~384K |
| 长文生成 | 64K~128K | ~336K~272K |
| 用满 max output 128K | ≥128K | ≤272K |

### 3.3 chars/4 启发式

精确 tokenize 需 tiktoken 等依赖，`text.len() / 4` 是业界广泛使用的近似公式（pi-mono `estimateTokens` 即此实现）。

| 语言 | 实际 | chars/4 估算 | 安全性 |
|------|------|-------------|--------|
| 英文 | ~4 chars/tok | 准确 | ✓ |
| 中文 | ~1.5 chars/tok | 偏保守（高估） | ✓ 安全 |
| 代码 | ~3-5 chars/tok | 近似准确 | ✓ |

### 3.4 核心不等式与 max_safe_rounds

```
  核心不等式：estimated_prompt_tokens + reserve_tokens < context_window

  工具轮次约束：
  max_safe_rounds ≈ (context_window - system_tokens - history_tokens - reserve_tokens)
                    / avg(assistant_delta + tool_result)

  示例（GPT-4o 128K）：(128000 - 500 - 5000 - 16384) / (200 + 2500) ≈ 39 轮
  极端情况（read_file 大文件 20000 tok/轮）：106116 / 20200 ≈ 5 轮即溢出
```

仅靠 `max_tool_rounds` 硬限不够——**需要 token 级别的动态检查**。

---

## 4. 三项目对比

```
             │ 轮次上限  │ 上下文裁剪       │ token 级防护          │ 工具结果截断
  ───────────┼──────────┼─────────────────┼──────────────────────┼──────────────
  pi-wasm    │ 10 轮    │ 最近 10 条消息   │ 无                   │ 无
  pi-mono    │ 无上限   │ token 滑窗      │ shouldCompact 阈值    │ 摘要时截 2000 字
  openclaw   │ 无上限   │ user 轮次截断   │ toolResultGuard 多层  │ 单条 50% / 硬限 400K
```

| 策略 | 优点 | 缺点 | 适合场景 |
|------|------|------|----------|
| **pi-wasm 当前** | 最简单、可预测 | 不感知 token；大文件溢出；丢失早期上下文 | 原型阶段 |
| **pi-mono** | 语义保全最好（摘要保留核心语义）；token 预算兜底 | 额外 LLM 调用；实现复杂 | 长对话编程 agent |
| **openclaw** | 多层防线稳健；无额外 LLM；可配置 | 按 turns 裁剪可能丢早期信息；配置项多 | 社交 bot、高并发 |

> **结论**：pi-rust-wasm 是编程 agent，应采用 **pi-mono 思路**分阶段实施。

---

## 5. 上下文语义完整性

> **核心问题**：截取最近 N 条消息时，被裁掉的更早消息可能包含关键约束/决策/目标，导致 LLM 对当前话题的理解不完整——这不仅是 token 成本问题，更是**回答质量**问题。

```
  全量消息:
  │ msg_1(user): "不要用 unsafe"  ← 关键约束! │
  │ msg_2(assistant) ... msg_12(assistant)    │
  │ msg_13(user): "重构 main.rs"  ← 当前输入  │

  context_cap=10 → 取 msg_4~msg_13 → ❌ 丢失 msg_1 的约束
```

### 业界解决方案

| 方案 | 原理 | 代表项目 | 效果 | 成本 |
|------|------|----------|------|------|
| **Compaction 摘要** | 旧消息 → LLM 结构化摘要 → 替换原文 | pi-mono, Cursor, Aider | ★★★★★ | 额外 LLM 调用 |
| **System Prompt 注入** | 约束/偏好自动提取到 system prompt | Cursor rules, Windsurf | ★★★★☆ | 需维护提取逻辑 |
| **滑窗 + 锚点** | 额外保留会话首条 user / 显式边界 | Aider | ★★★★☆ | 简单 |
| **RAG 检索增强** | 旧消息 → 向量库，按相关性检索注入 | MemGPT, Langchain | ★★★★★ | 需向量库 |

### 多任务会话中的锚点问题

「首条 user = 任务起点」是简化假设。多任务会话中常见做法：

| 做法 | 思路 |
|------|------|
| **显式边界** | 用户点「新任务」/ `/new`；锚点 = 边界后第一条 user |
| **最近 K 个 user turns** | 整段保留最近 K 次，自然覆盖当前任务 |
| **Compaction 摘要** | 多段目标合并到 Goal/Progress，弱化对单条锚点依赖 |

### Compaction 裁切规则

pi-mono 实现**不是**按语义判断重要性，而是 **token 滑窗 + 结构合法性**：

1. 从最新消息向前累加 token，直到 ≥ `keepRecentTokens`
2. 在合法边界（user / 带 tool_calls 的 assistant）处切分
3. **切分点之前整段** → `generateSummary` → **一条** summary 消息替换

```
  transcript:  [… 旧消息整段 …][ 保留区 ← 从尾部向前累加 ]
                ◄── 待摘要前缀 ──►│◄── 保留原文 ──►
                      ↑ 切分点

  generateSummary(待摘要前缀全部消息) → 一条 summary
  最终: [summary] + [保留区原文…]
```

若已有上一次 summary → 增量合并（UPDATE 模式），不从头重扫。

---

## 6. 最终设计决策（pi-rust-wasm）

> 日期：2026-03-30
> 状态：已确定，待实现

### 6.1 总体思路

**滑动窗口（token 预算） + 占位符替换 + Compaction 摘要**，三层递进防护。上下文粒度以 **user turn** 为单位管理。

### 6.2 关键概念定义

**user turn**：一条 `role=user` 消息 + 其后所有 `role=assistant` / `role=tool` 消息，直到下一条 `role=user`（与 openclaw `limitHistoryTurns` 裁剪单位一致）。

### 6.3 初始化与 user turns 收集策略

| 条件 | 行为 |
|------|------|
| 当天（按本地日期）session 内有 user turns | 优先加载当天所有 user turns 转化为 prompt token 估算 |
| 当天 user turns 数 < 10 | 向前补全，直到累计满 **10 个 user turns** |
| 进入会话第一次用户输入时 | 初始化内存中的 `userTurnsList` + `estimateContextChars`，后续**动态维护**，不再从文件全量扫描 |

> **设计意图**：保证上下文对「当前活跃会话」有足够的新鲜历史；同时对短会话补全到 10 turns，避免上下文过短。

### 6.4 滑动窗口预算计算

```
context_window        由模型固有参数决定（如  GPT-5.2 = 400,000）
max_output_tokens     模型单轮最大输出（如 GPT-5.2 = 128,000）

prompt_tokens_max   = context_window - max_output_tokens
prompt_chars_max    = prompt_tokens_max × 4          （chars/4 启发式）
contextBudgetChars  = prompt_chars_max × 0.75        （75% 作为安全水位）

示例（GPT-5.2）:
  prompt_tokens_max  = 400,000 - 128,000 = 272,000
  prompt_chars_max   = 272,000 × 4       = 1,088,000 chars
  contextBudgetChars = 1,088,000 × 0.75  =   816,000 chars
```

### 6.5 动态上下文维护

- **内存中维护**：`userTurnsList: Vec<UserTurn>` + `estimateContextChars: usize`（滚动累加，避免每次全量扫描 transcript）
- **每次追加新 turn**：同步更新 `estimateContextChars`
- **估算函数**：`estimate_chars(text: &str) -> usize { text.len() }` / token 估算 = `chars / 4`

### 6.6 触发条件与三层防护流程

**约束**：最近 3 个 user turns 永远不参与 Compaction（保证当前对话上下文完整）。

```
  每次 build_context_messages（向 LLM 发请求前）:

  userTurnsList = [turn_0(最旧), turn_1, ..., turn_n-3, turn_n-2, turn_n-1(最新)]
                   ◄────────────── 可压缩区间 ──────────────►│◄── 保护区(3) ──►

  ┌──────────────────────────────────────────────────────────────────────────┐
  │ estimateContextChars ≤ contextBudgetChars?                               │
  │   YES → 直接发送，无需处理                                                │
  │   NO  → 进入防护流程 ↓                                                   │
  └──────────────────────────────────────────────────────────────────────────┘
                                      │
                     ┌────────────────▼────────────────┐
                     │  Layer 1: 占位符替换              │
                     │                                  │
                     │  仅在「可压缩区间」内从旧→新扫描   │
                     │  遇 role=tool → 换占位符           │
                     │  累计 reduced >= charsNeeded → break│
                     │  charsNeeded = estimateContextChars│
                     │             - contextBudgetChars  │
                     └────────────────┬────────────────┘
                                      │
                     estimateContextChars ≤ contextBudgetChars?
                          YES → 完成，继续发送
                          NO  ↓
                     ┌────────────────▼────────────────┐
                     │  Layer 2: 循环 Compaction 摘要    │
                     │                                  │
                     │  compactable = turns[0..n-3]      │
                     │  （保留最近 3 个 turns 不压缩）    │
                     │                                  │
                     │  while estimateContextChars       │
                     │        > contextBudgetChars       │
                     │    && compactable 非空:            │
                     │                                  │
                     │    batch = compactable 中最旧的    │
                     │            未压缩 turns（≤10条）  │
                     │    summary = LLM(batch)           │
                     │    → 一条 summary 消息替换整批     │
                     │    更新 estimateContextChars      │
                     │    compactable 移除已压缩 batch    │
                     └────────────────┬────────────────┘
                                      │
                     estimateContextChars ≤ contextBudgetChars?
                          YES → 完成，继续发送
                          NO  ↓（可压缩区间已全部 compact，
                                 仍超预算：极端边缘情况）
                     ┌────────────────▼────────────────┐
                     │  Layer 3: 强制删除（极端兜底）    │
                     │                                  │
                     │  可压缩区间已无可用空间            │
                     │  从最旧 summary/turn 起删除        │
                     │  直到 estimateContextChars 回预算 │
                     │  （几乎不可达；防御性保底）         │
                     └────────────────────────────────┘
```

**Compaction 产物**：每批次（≤10 个 user turns）→ LLM 生成**一条** summary 消息，替换原始那批 turns（多变一）。若该 session 已有上一次 summary，则采用 UPDATE 模式合并（参考 pi-mono `UPDATE_SUMMARIZATION_PROMPT`）。

```
  压缩前:  [turn_0][turn_1]...[turn_9][turn_10]...[turn_n-3] | [turn_n-2][turn_n-1][turn_n]
  第一轮:  [summary_A]        [turn_10]...[turn_n-3] | [turn_n-2][turn_n-1][turn_n]
  第二轮:  [summary_A][summary_B]      [turn_n-3]   | [turn_n-2][turn_n-1][turn_n]
  ...最终: [summary_A][summary_B]...[summary_X]     | [turn_n-2][turn_n-1][turn_n]
           ◄──────────── 摘要链 ─────────────────►   ◄──── 保护区 ────►
```

### 6.7 占位符替换详细规则

参考 openclaw `compactExistingToolResultsInPlace` 逻辑：

```rust
// 伪代码
const KEEP_RECENT_TURNS: usize = 3; // 最近 3 个 user turns 受保护，不参与占位符替换
const PLACEHOLDER: &str = "[compacted: tool output removed to free context]";

fn compact_existing_tool_results(
    user_turns: &mut Vec<UserTurn>,
    chars_needed: usize,
    estimate_cache: &mut EstimateCache,
) -> usize {
    let compactable_end = user_turns.len().saturating_sub(KEEP_RECENT_TURNS);
    let mut reduced = 0;

    for turn in user_turns[..compactable_end].iter_mut() { // 仅在可压缩区间内操作
        for msg in turn.messages.iter_mut() {
            if msg.role != Role::Tool { continue; }         // 只处理 tool result
            let before = estimate_chars(msg, estimate_cache);
            if before <= PLACEHOLDER.len() { continue; }

            msg.content = PLACEHOLDER.to_string();
            estimate_cache.invalidate(msg);
            let after = estimate_chars(msg, estimate_cache);
            reduced += before - after;

            if reduced >= chars_needed { return reduced; }  // 够了就停
        }
    }
    reduced
}
```

### 6.8 Compaction 摘要模板（参考 pi-mono）

调用 LLM 对最旧的 10 个 user turns 生成结构化摘要：

```
## Goal
[用户在这段对话里想完成的目标，可多项]

## Constraints & Preferences
- [用户声明的约束、偏好、不允许事项]

## Progress
### Done
- [x] [已完成的子任务]

### In Progress
- [ ] [进行中的工作]

### Blocked
- [阻塞项，若无写 "(none)"]

## Key Decisions
- **[决策名]**: [简要理由]

## Critical Context
- [后续继续工作所必须知道的关键数据、文件路径、错误信息等]
```

摘要将以 `role=user`（或独立的 `role=summary`，视实现决定）消息形式插回消息列表，替换原 10 个 user turns。

### 6.9 涉及改动文件

| 文件 | 改动内容 |
|------|---------|
| `src/core/session/manager.rs` | `build_context_messages` 改为 token-aware；内存维护 `userTurnsList` + `estimateContextChars` |
| `src/core/agent_loop.rs` | reasoning loop 每轮工具执行后更新 `estimateContextChars`；触发占位符替换 |
| `src/infra/config.rs` | 新增 `context_window`、`max_output_tokens`、`compaction_turns`（默认 10）等配置项 |
| `src/core/compaction.rs`（新建） | Compaction 摘要逻辑，复用 pi-mono 摘要 prompt 模板 |
