本文为 [Architecture](../Architecture.md) 中「上下文管理」的详细设计，总览见主文档。关联文档：[Agent Loop 设计](agent-loop.md)、[会话存储数据结构](session-storage.md)。研究报告：[context-management-deep-dive.md](../../../docs/reports/context-management-deep-dive.md)。

---

# 上下文管理技术方案

## 1. 概述

### 1.1 背景

当前 pi-rust-wasm 的上下文管理存在两个维度的缺陷：

- **reasoning loop 内**：`max_tool_rounds=10` 仅限轮数，每轮全量累积 messages 发给 LLM，10 轮大文件 read_file 可达 50,000+ token，**无 token 级检查**。
- **跨轮次 session 历史**：`DEFAULT_CONTEXT_CAP=10`（条数），按消息条数裁剪——一条 tool result 可能 50,000 字符，条数不等于 token 量。

两个维度叠加后，任一场景都可能导致 **context window 溢出**（LLM 报错或静默截断），且按条数裁剪会**丢失早期关键上下文**（用户约束、决策、目标），直接影响回答质量。

### 1.2 设计目标

1. **防溢出**：所有发给 LLM 的 prompt 估算 token 不超过安全水位，消除 context overflow。
2. **语义完整**：被裁剪的旧消息通过 LLM 结构化摘要保留核心语义（Goal / Constraints / Progress）。
3. **低额外开销**：优先用零成本的占位符替换释放空间，仅在不够时才触发 LLM 摘要调用。
4. **可配置**：`context_window`、`max_output_tokens`、`compaction_turns`、`keep_recent_turns`、`single_tool_result_max_chars`、`compaction_model` 均可在配置文件中覆盖。

---

## 2. 术语表

| 术语 | 说明 |
|------|------|
| **user turn** | 一条 `role=user` 消息 + 其后所有 `role=assistant` / `role=tool` 消息，直到下一条 `role=user`。上下文管理的最小粒度单位。 |
| **context_window** | 模型固有的最大上下文长度（输入 + 输出），由模型提供商决定（如 GPT-4o 128K, GPT-5.2 400K）。 |
| **contextBudgetChars** | 滑动窗口的字符预算上限，`(context_window - max_output_tokens) × 4 × 0.75`。 |
| **estimateContextChars** | 当前 `userTurnsList` 中所有消息的估算总字符数，内存中动态维护。 |
| **compactable zone** | `userTurnsList` 中可被压缩的区间（排除保护区）。 |
| **protected zone** | 最近 `keep_recent_turns`（默认 3）个 user turns，**不参与任何压缩/替换**。 |
| **placeholder** | `"[compacted: tool output removed to free context]"`，替换旧 tool result 正文的常量。 |
| **compaction summary** | LLM 对一批 user turns 生成的结构化摘要，一条消息替换整批原始 turns（多变一）。 |
| **chars/4 启发式** | token 估算公式：`estimated_tokens ≈ text.len() / 4`，业界广泛使用，pi-mono `estimateTokens` 同此实现。 |

---

## 3. 核心架构图

### 图一：消息膨胀与两维度风险

```
  ═══════════════════════════════════════════════════════════════════════
  维度 1：reasoning loop 内消息膨胀（单次用户输入）
  ═══════════════════════════════════════════════════════════════════════

  用户输入 ──► AgentLoop.run(initial_messages)
               │
               │  思考-行动循环（第三层，见 agent-loop.md §13.3）
               │  ┌─────────────────────────────────────────────────┐
               │  │ 每轮：                                           │
               │  │   messages += assistant(tool_calls)  ~200 tok    │
               │  │   messages += tool(result)           ~2500 tok   │
               │  │   下一轮把【全量 messages】发给 LLM                │
               │  └─────────────────────────────────────────────────┘
               │
               │  520 tok → 3,220 → 4,220 → ... → 50,000+ tok（10 轮大文件）
               │
               │  当前防线：仅 max_tool_rounds=10 硬上限
               │  缺失防线：无 token 估算、无 tool result 截断
               │  ► 本方案补充：每轮工具执行后检查 estimateContextChars

  ═══════════════════════════════════════════════════════════════════════
  维度 2：跨 user turn 的 session 历史裁剪
  ═══════════════════════════════════════════════════════════════════════

  transcript（磁盘持久化全量 JSONL）
  ┌────────────────────────────────────────────────────┐
  │ turn_0(user + assistant + tool × N)                 │
  │ turn_1(user + assistant + tool × N)                 │
  │ ...                                                 │
  │ turn_n-1(user)  ← 当前输入                           │
  └────────────────────────────────────────────────────┘
               │
               │ 当前：build_context_messages(cap=10 条) → 按条数裁剪
               │ 问题：一条 tool result 可能 50,000 chars → 溢出
               │       早期 user 声明的约束被裁掉 → 语义丢失
               │
               │ ► 本方案：改为 token-aware 滑窗 + 占位符 + Compaction
```

### 图二：预算计算与滑动窗口结构

```
  ════════════════════ 预算计算链 ════════════════════

  context_window (模型固有)           如 GPT-5.2 = 400,000 tokens
       │
       ├─ max_output_tokens (模型固有)  如 GPT-5.2 = 128,000 tokens
       │
       ▼
  prompt_tokens_max = context_window - max_output_tokens
                    = 400,000 - 128,000 = 272,000 tokens
       │
       │  × 4 (chars/4 启发式, 反向换算)
       ▼
  prompt_chars_max = 272,000 × 4 = 1,088,000 chars
       │
       │  × 0.75 (安全水位, 吸收估算误差 + provider framing)
       ▼
  contextBudgetChars = 1,088,000 × 0.75 = 816,000 chars
       │
       │  这是 estimateContextChars 的触发阈值
       │  超过此值 → 进入 Layer 1~3（Layer 0 在工具返回时单独触发）
       ▼

  ════════════════════ 滑动窗口结构 ════════════════════

  userTurnsList (内存中维护)：

  [turn_0] [turn_1] ... [turn_n-4] │ [turn_n-3] [turn_n-2] [turn_n-1]
  ◄──────── compactable zone ──────►│◄────── protected zone (3) ─────►
                                    │
  Layer 1/2 仅操作此区间              │  永远不压缩，保证当前对话完整
```

### 图三：四层防护流程

```
  ┌──────────────────────────────────────────────────────────────────┐
  │ Layer 0: 新 tool result 入口截断（reasoning loop 内，工具返回时）  │
  │                                                                  │
  │  tool_result.len() > SINGLE_TOOL_RESULT_MAX_CHARS ?              │
  │    YES → 截断到 70%~100% 区间的换行处，拼 [truncated] 后缀        │
  │    NO  → 保留原文                                                 │
  │  ► 每条 tool result 写入 messages 前执行                          │
  └──────────────────────────────────────────────────────────────────┘

  每次 build_context_messages（向 LLM 发请求前）触发：

  ┌──────────────────────────────────────────────────────────────────┐
  │ estimateContextChars ≤ contextBudgetChars ?                      │
  │   YES → 直接发送，无需处理                                        │
  │   NO  → 进入防护流程 ↓                                           │
  └──────────────────────────────────────────────────────────────────┘
                                  │
                 ┌────────────────▼─────────────────┐
                 │  Layer 1: 占位符替换               │
                 │                                   │
                 │  scope = compactable zone          │
                 │  for turn in turns[0..n-3]:        │
                 │    for msg where role=tool:         │
                 │      msg.content = PLACEHOLDER      │
                 │      reduced += (before - after)    │
                 │      if reduced >= charsNeeded:     │
                 │        break ──────────────►        │
                 │  charsNeeded = estimate - budget     │
                 └────────────────┬─────────────────┘
                                  │
                 estimateContextChars ≤ budget ?
                      YES → 发送
                      NO  ↓
                 ┌────────────────▼─────────────────┐
                 │  Layer 2: 循环 Compaction           │
                 │                                   │
                 │  compactable = turns[0..n-3]       │
                 │                                   │
                 │  while estimate > budget            │
                 │     && compactable 非空:             │
                 │                                   │
                 │    batch = 最旧未压缩 ≤10 turns     │
                 │    summary = LLM(batch)            │
                 │    一条 summary 替换整批 turns       │
                 │    更新 estimateContextChars        │
                 │    compactable 移除已压缩 batch     │
                 └────────────────┬─────────────────┘
                                  │
                 estimateContextChars ≤ budget ?
                      YES → 发送
                      NO  ↓ (compactable zone 已耗尽)
                 ┌────────────────▼─────────────────┐
                 │  Layer 3: 强制删除（极端兜底）      │
                 │                                   │
                 │  从最旧 summary/turn 起删除          │
                 │  直到 estimate 回到 budget 内        │
                 │  （几乎不可达；防御性保底）            │
                 └──────────────────────────────────┘
```

### 图四：Compaction 多轮摘要演进

```
  ════════════════════ 初始状态 ════════════════════

  [turn_0][turn_1]...[turn_9] [turn_10]...[turn_n-3] │ [turn_n-2][turn_n-1][turn_n]
  ◄────── batch 1 (10 turns) ──► ◄── 剩余可压缩 ──► │ ◄──── protected zone ──────►

  ════════════════════ 第一轮 Compaction ════════════════════

  LLM(turn_0..turn_9) → summary_A（一条消息，含 Goal/Constraints/Progress...）

  [summary_A] [turn_10]...[turn_n-3] │ [turn_n-2][turn_n-1][turn_n]
  ◄─ 1 条 ──► ◄── 剩余可压缩 ───────► │ ◄──── protected zone ──────►

  ════════════════════ 若仍超预算 → 第二轮 ════════════════════

  LLM(turn_10..turn_19) → summary_B

  [summary_A][summary_B] [turn_20]...[turn_n-3] │ [turn_n-2][turn_n-1][turn_n]
  ◄────── 摘要链 ──────► ◄── 剩余可压缩 ────────► │ ◄──── protected zone ──────►

  ════════════════════ 最终稳定 ════════════════════

  [summary_A][summary_B]...[summary_X] │ [turn_n-2][turn_n-1][turn_n]
  ◄──────────── 摘要链 ──────────────► │ ◄──── protected zone ──────►

  已有旧 summary 时 → UPDATE 模式合并（参考 pi-mono UPDATE_SUMMARIZATION_PROMPT）
```

---

## 4. 预算计算

### 4.1 公式

```
prompt_tokens_max   = context_window - max_output_tokens
prompt_chars_max    = prompt_tokens_max × 4
contextBudgetChars  = prompt_chars_max × 0.75
```

### 4.2 配置项

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `context_window` | `usize` | `400_000` | 默认对齐 **GPT-5.2**（400K）；其他模型请在配置中覆盖 |
| `max_output_tokens` | `usize` | `128_000` | 默认对齐 **GPT-5.2** 单轮最大输出；对齐 API 的 `max_tokens` |
| `compaction_turns` | `usize` | `10` | 每批 Compaction 摘要处理的最大 user turns 数 |
| `keep_recent_turns` | `usize` | `3` | 保护区大小，最近 N 个 user turns 不参与压缩 |
| `single_tool_result_max_chars` | `usize` | `400_000` | 单条 tool result 最大字符数，超出截断（Layer 0），与 openclaw 硬上限量级一致 |
| `compaction_model` | `String` | `"gpt-5.2"` | Compaction 摘要专用模型 ID（与主对话 `model` 可相同或不同） |

> 配置位于 `pi.config.toml` 的 `[context]` 节，或通过 `PrimitiveConfig` 结构体注入。

### 4.3 典型值

| 模型 | context_window | max_output_tokens | prompt_tokens_max | contextBudgetChars |
|------|---------------|-------------------|-------------------|--------------------|
| GPT-4o | 128,000 | 16,384 | 111,616 | 334,848 |
| GPT-5.2 | 400,000 | 128,000 | 272,000 | 816,000 |
| Claude 3.5 Sonnet | 200,000 | 8,192 | 191,808 | 575,424 |
| DeepSeek-V3 | 64,000 | 8,192 | 55,808 | 167,424 |

---

## 5. 初始化与动态维护

### 5.1 会话启动时初始化

```
fn init_context_state(session: &Session, config: &ContextConfig) -> ContextState:
    turns = load_user_turns_from_transcript(session.transcript_path)

    # 优先取当天所有 turns
    today_turns = turns.filter(|t| t.date == today())

    # 不足 10 则向前补全
    if today_turns.len() < 10:
        extra = turns.before(today_turns.first())
                     .rev()
                     .take(10 - today_turns.len())
        today_turns = extra.rev() + today_turns

    estimate = sum(today_turns.map(|t| estimate_turn_chars(t)))

    return ContextState {
        user_turns_list: today_turns,
        estimate_context_chars: estimate,
        context_budget_chars: compute_budget(config),
    }
```

### 5.2 动态更新

每次 reasoning loop 中追加新 turn（包括 assistant、tool 消息），同步更新 `estimate_context_chars`：

```
fn on_message_appended(state: &mut ContextState, msg: &Message):
    state.estimate_context_chars += msg.content.len()

fn on_new_user_turn(state: &mut ContextState, turn: UserTurn):
    state.user_turns_list.push(turn)
    state.estimate_context_chars += estimate_turn_chars(&turn)
```

无需每次从 transcript 文件全量扫描。

### 5.3 system prompt 纳入估算

`estimateContextChars` 应包含 system prompt 的字符数。system prompt 在会话期间通常不变，初始化时计算一次即可：

```
estimate = system_prompt.len() + sum(today_turns.map(|t| estimate_turn_chars(t)))
```

> 若 system prompt 较短（< 5K chars），25% 安全水位已足够覆盖。但为准确性，仍建议显式计入。

### 5.4 Session 重载时处理已有 Compaction entry

从 transcript JSONL 加载 user turns 时，需识别 `SessionEntry::Compaction` entry：

1. 遇到 `Compaction` entry → 作为 `SummaryTurn` 加入 `userTurnsList`，**不展开**为原始消息
2. 已被 Compaction 覆盖的原始 turns **不重复加载**（Compaction entry 中记录了覆盖的 turn range）
3. 后续 Layer 2 的 `find_last_summary` 可直接定位已有 summary，进入 UPDATE 模式

**被压缩的 user turn 是否仍留在 transcript JSONL 中？**

采用 **仅追加（append-only）** 约定，与 pi 系 transcript 一致：

- **保留**：原先写入的 `Message` 行（user / assistant / tool）**不删除、不改写**，仍在 `.jsonl` 中，便于审计、回放与调试。
- **追加**：Compaction 发生时 **再追加一行** `type: compaction` 的 `CompactionEntry`（含摘要正文与覆盖范围元数据，字段以 [session-storage.md](session-storage.md) 为准）。
- **构建 LLM 上下文**：`userTurnsList` / `build_context_messages` 在内存中按 Compaction 元数据 **折叠**——已摘要区间只表现为一条 summary，**不把同一区间的原始 Message 再次拼进 prompt**（避免双倍 token）。

若未来需要「物理瘦身」大文件，可作为独立运维能力（压缩归档副本），**不**作为默认行为。

### 5.5 `userTurnsList` 与现有消息结构的关系

#### 现有三种消息类型

```
  ┌─ transcript JSONL ─┐    ┌── chat.rs ──────┐    ┌── AgentLoop ──────────┐
  │ serde_json::Value   │    │ ChatMessage      │    │ AgentMessage           │
  │ (磁盘持久化格式)    │    │ (LLM 请求格式)  │    │ (agent 内部富类型)     │
  └─────────┬──────────┘    └───────┬─────────┘    └─────────┬─────────────┘
            │                       │                         │
            │ build_context_messages│ agent_messages_from_chat │ convert_to_llm_format
            │ (JSON → ChatMessage)  │ (ChatMessage → Agent)   │ (Agent → ChatMessage)
            ▼                       ▼                         ▼
  历史加载 ────────────► 拼装初始消息 ──────────► reasoning loop 每轮发 LLM
```

- **`serde_json::Value`**：transcript JSONL 的原始 JSON 行，`build_context_messages` 从中读取。
- **`ChatMessage`**：LLM 请求/响应格式（`role` + `content` + `tool_calls`），由 `src/core/llm/types.rs` 定义。
- **`AgentMessage`**：agent loop 内部富类型（User / Assistant / ToolResult / System / **Steering** / **CompactionSummary**），比 `ChatMessage` 多出 `Steering`（用户中途注入指令）和 `CompactionSummary`（摘要）。

#### `userTurnsList` 的定位

`userTurnsList` 是 **上下文管理模块的内存视图**，它**不是**第四种消息类型，而是对上述结构的**逻辑分组**：

```
  userTurnsList: Vec<UserTurn>
      │
      ├─ UserTurn { messages: Vec<AgentMessage> }   // 一条 user + 后续 assistant/tool
      ├─ UserTurn { ... }
      └─ SummaryTurn { summary: String }             // Compaction 产物（对应 AgentMessage::CompactionSummary）
```

- 每个 `UserTurn` 内部持有 `Vec<AgentMessage>`，按 user turn 粒度分组。
- `SummaryTurn` 展平为一条 `AgentMessage::CompactionSummary`（在 `convert_to_llm_format` 中映射为 `ChatMessage::user`）。

#### 发给 LLM 的完整链路

```
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ① 会话初始化（用户首次输入前，一次性）                                    │
  │                                                                         │
  │  transcript JSONL ──[BufReader 逐行]──► 按 user turn 分组               │
  │       │                                      │                          │
  │       │  识别 Compaction entry                │ 折叠已摘要区间            │
  │       ▼                                      ▼                          │
  │  userTurnsList: [SummaryTurn?, UserTurn, UserTurn, ...]                  │
  │  estimateContextChars = system_prompt.len() + Σ turn_chars              │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ② 每轮对话进入前（用户按下回车）                                          │
  │                                                                         │
  │  userTurnsList.flatten() ──► Vec<AgentMessage>                          │
  │       + 注入 system prompt                                              │
  │       + 追加本轮 AgentMessage::User                                     │
  │       ──► initial_messages: Vec<AgentMessage>                           │
  │                                                                         │
  │  ◆ 此处触发 Layer 1~3 预算检查（若 estimateContextChars > budget）       │
  │                                                                         │
  │  AgentLoop::run(initial_messages)                                       │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ③ reasoning loop 内（每轮 LLM ↔ 工具）                                   │
  │                                                                         │
  │  messages: &mut Vec<AgentMessage>    ← AgentLoop 工作集                  │
  │       │                                                                 │
  │       │  convert_to_llm_format(messages)                                │
  │       ▼                                                                 │
  │  Vec<ChatMessage> ──► ChatRequest.messages ──► llm.chat_stream          │
  │       │                                                                 │
  │       │  LLM 返回 assistant + tool_calls                                │
  │       │  工具执行 → tool result                                         │
  │       ▼                                                                 │
  │  ◆ Layer 0: truncate_tool_result_if_needed(result)                      │
  │  messages.push(AgentMessage::ToolResult { .. })                         │
  │  estimateContextChars += result.len()       ← 实时更新                  │
  │                                                                         │
  │  注意：userTurnsList 此时【不更新】                                      │
  │        当前 turn 正在进行中，尚未完成                                    │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ④ reasoning loop 结束后（当前 user turn 完成）                            │
  │                                                                         │
  │  当前 turn 内的全部 messages 打包：                                      │
  │    current_turn = UserTurn {                                            │
  │        messages: [User, Assistant, ToolResult, ..., Assistant(final)]    │
  │    }                                                                    │
  │  userTurnsList.push(current_turn)          ← 此时才追加                  │
  │  写入 transcript JSONL（各 Message entry）                               │
  └─────────────────────────────────────────────────────────────────────────┘
```

**`userTurnsList` 与 `messages` 的关系**：

- **`userTurnsList`**：管理**已完成的历史 turns**。只在 ② 进入前读取（flatten）、④ 结束后追加。
- **`messages`**：reasoning loop 的**实时工作集**，包含历史 + 当前 turn 正在产生的新消息。
- **`estimateContextChars`**：**两处都更新**——② 初始化时计算历史总量，③ 每次 push 时实时累加。这样 Layer 0~3 在 reasoning loop 内也能正确触发。

即：**`userTurnsList` 是持久化 transcript 与 `AgentMessage` 之间的中间层**——负责分组、Compaction 折叠与估算维护；最终转为 `AgentMessage` 后走已有的 `convert_to_llm_format` 链路，**不改变** reasoning loop 内部已有的消息流转方式。

---

## 6. 防护算法（Layer 0~3）

### 6.1 触发点

在 `build_context_messages`（向 LLM 发请求前）调用，以及 reasoning loop 内每轮工具执行后更新估算。

与 Agent Loop 第二层（容错重试循环）的 ContextOverflow 路径配合：当 LLM 返回 ContextOverflow 错误时，也进入本模块的 Layer 2。

### 6.2 Layer 0：新 tool result 单条截断（入口防线）

在 reasoning loop 中，每次工具执行返回结果后、**写入 messages 前**，检查单条 tool result 大小。超过上限则就地截断，防止单条巨型结果直接撑爆 context。

```
const SINGLE_TOOL_RESULT_MAX_CHARS: usize = 400_000;  // 默认 400K chars，与 openclaw HARD_MAX_TOOL_RESULT_CHARS 同量级
const TRUNCATION_SUFFIX: &str = "\n\n[truncated: result exceeded size limit, showing first portion]";

fn truncate_tool_result_if_needed(content: &mut String):
    if content.len() <= SINGLE_TOOL_RESULT_MAX_CHARS:
        return

    # 在 70% 处之后找换行，避免截断到 JSON/代码中间
    let cut_zone_start = SINGLE_TOOL_RESULT_MAX_CHARS * 70 / 100
    let cut_pos = content[cut_zone_start..SINGLE_TOOL_RESULT_MAX_CHARS]
                    .rfind('\n')
                    .map(|i| cut_zone_start + i)
                    .unwrap_or(SINGLE_TOOL_RESULT_MAX_CHARS)

    content.truncate(cut_pos)
    content.push_str(TRUNCATION_SUFFIX)
```

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `single_tool_result_max_chars` | `usize` | `400_000` | 单条 tool result 最大字符数，超出截断 |

> 参考 openclaw `SINGLE_TOOL_RESULT_CONTEXT_SHARE=0.5`（动态语境下单条占比）与 `HARD_MAX_TOOL_RESULT_CHARS=400,000`。本方案默认上限与后者对齐，仍可通过配置调低。

### 6.3 Layer 1：占位符替换

零 LLM 开销。仅对 compactable zone 内的 `role=tool` 消息，**从最旧到最新**逐条替换正文为常量 PLACEHOLDER，减够即停。

```
const KEEP_RECENT_TURNS: usize = 3;
const PLACEHOLDER: &str = "[compacted: tool output removed to free context]";

fn compact_tool_results(state: &mut ContextState) -> usize:
    let compactable_end = state.user_turns_list.len().saturating_sub(KEEP_RECENT_TURNS)
    let chars_needed = state.estimate_context_chars - state.context_budget_chars
    let mut reduced = 0

    for turn in state.user_turns_list[..compactable_end]:
        for msg in turn.messages where msg.role == Tool:
            let before = msg.content.len()
            if before <= PLACEHOLDER.len():
                continue
            msg.content = PLACEHOLDER
            reduced += before - PLACEHOLDER.len()
            state.estimate_context_chars -= (before - PLACEHOLDER.len())
            if reduced >= chars_needed:
                return reduced
    return reduced
```

### 6.4 Layer 2：循环 Compaction 摘要

Layer 1 不够时触发。对 compactable zone 内最旧的未压缩 turns，每批 ≤ `compaction_turns` 条，调用 LLM 生成一条结构化摘要消息替换整批。循环执行直到回预算或 compactable zone 耗尽。

```
fn run_compaction_loop(state: &mut ContextState, llm: &LlmProvider):
    let compactable_end = state.user_turns_list.len().saturating_sub(KEEP_RECENT_TURNS)
    let mut cursor = 0

    while state.estimate_context_chars > state.context_budget_chars
       && cursor < compactable_end:

        let batch_end = min(cursor + COMPACTION_TURNS, compactable_end)
        let batch = state.user_turns_list[cursor..batch_end]

        let previous_summary = find_last_summary(state.user_turns_list[..cursor])
        let summary_text = llm.generate_summary(batch, previous_summary)

        let batch_chars = sum(batch.map(|t| estimate_turn_chars(t)))
        let summary_chars = summary_text.len()

        # 替换：移除原 batch，插入一条 summary 消息
        state.user_turns_list.splice(cursor..batch_end, [SummaryTurn(summary_text)])
        state.estimate_context_chars -= batch_chars
        state.estimate_context_chars += summary_chars

        # 更新索引（splice 后 compactable_end 缩小了）
        compactable_end = state.user_turns_list.len().saturating_sub(KEEP_RECENT_TURNS)
        cursor += 1  # 跳过刚插入的 summary

    # 写入 Transcript：追加 Compaction entry（type=compaction）
    session.append_entry(CompactionEntry { summary: summary_text, ... })
```

### 6.5 Layer 3：强制删除（极端兜底）

Layer 2 耗尽 compactable zone 后仍超预算——说明 protected zone 本身已超大。从 `user_turns_list[0]`（最旧 summary/turn）起逐个删除，直到估算回到预算内。几乎不可达，仅作防御性保底。

### 6.6 Compaction 失败处理

Layer 2 调用 LLM 生成摘要可能因网络错误、速率限制等失败：

| 情况 | 处理 |
|------|------|
| LLM 调用超时/网络错误 | 重试 1 次；仍失败则跳过本批次，尝试下一批；最终降级到 Layer 3 |
| LLM 返回空摘要 | 视为失败，同上 |
| 摘要比原文还大（极端） | 丢弃摘要，保留原文，尝试下一批 |

> 失败时发布 `compaction_error` 事件（`{ batch_index, error }`），由日志记录。

### 6.7 与 `max_tool_rounds` 的关系

> **TODO**：`max_tool_rounds` 硬限暂时**移除**（不再限制 reasoning loop 工具轮数）。防死循环由上下文预算 + 后续独立的 tool-loop-detection 方案负责。等 tool-loop-detection 方案落地后再评估是否需要恢复硬限。

**对照 openclaw / pi-mono**

- **openclaw**：无对等固定轮数上限；靠 **tool-loop-detection**（重复/无进展/乒乓）+ **tool-result 上下文守卫** + **Compaction / overflow 恢复**组合约束。
- **pi-mono（coding-agent）**：无 `max_tool_rounds`；由 **token 预算 + Compaction** + 用户中止约束行为。

两者均**不**硬编码轮数上限，pi-rust-wasm 对齐此策略：工具轮次受 **上下文预算（本方案）** 自然约束——token 用尽时 Compaction 压缩或兜底中止，无需额外硬限。

---

## 7. Compaction 摘要模板

### 7.1 首次摘要（SUMMARIZATION_PROMPT）

```
Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.
```

### 7.2 Compaction 模型选择

| 策略 | 说明 | 推荐 |
|------|------|------|
| **默认：`gpt-5.2`** | 与主对话同代模型，摘要质量与长上下文能力一致；`compaction_model` 默认值为 `"gpt-5.2"` | **当前默认** |
| 与主对话对齐 | 将 `compaction_model` 设为与 `model` 相同，行为与「全用主模型」一致 | 可选 |
| 轻量模型 | 如 `gpt-4o-mini` / DeepSeek-V3，成本低但需自行评估摘要质量 | 成本敏感时可改配置 |

> 实现上 Compaction 的 LLM 调用使用 **`compaction_model`**，与 `ChatRequest.model`（主对话）分离配置；若希望完全一致，将两项设为同一模型 ID 即可。

### 7.3 增量更新（UPDATE_SUMMARIZATION_PROMPT）

当 `user_turns_list` 中已有上一次 summary 时，采用 UPDATE 模式合并：

```
Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it
```

### 7.4 摘要消息格式

摘要以 `Compaction` entry 写入 transcript JSONL（类型已在 [session-storage.md](session-storage.md) 的 `SessionEntry::Compaction` 定义）。在内存中作为一条 `role=user` 消息（content 为摘要文本）放入 `user_turns_list`，替换被压缩的原始 turns。

---

## 8. 超出本方案范围（Out of Scope）

以下机制在研究报告中分析过，但不纳入本方案首期实现：

| 机制 | 说明 | 后续计划 |
|------|------|---------|
| **工具循环检测（tool-loop-detection）** | openclaw 的滑窗重复检测 + steering 注入 + 熔断。当前 `max_tool_rounds` 已作为简单兜底。 | 可作为独立方案在 agent-loop 中实现 |
| **RAG 检索增强** | 旧消息向量化 + 按相关性检索注入。效果好但需向量库依赖。 | 长期方向 |
| **System Prompt 自动注入** | 从对话中自动提取约束/偏好到 system prompt（类似 Cursor rules）。 | 可与 Compaction 互补，后续独立方案 |

---

## 9. 涉及改动文件

| 文件 | 改动内容 |
|------|---------|
| [`src/core/session/manager.rs`](../../../src/core/session/manager.rs) | `build_context_messages` 改为 token-aware（基于 `estimateContextChars` 而非 `context_cap` 条数）；初始化时构建 `userTurnsList` + `estimateContextChars`；每次追加消息同步更新估算 |
| [`src/core/agent_loop.rs`](../../../src/core/agent_loop.rs) | reasoning loop 每轮工具执行后：① 调用 `truncate_tool_result_if_needed`（Layer 0）② 调用 `on_message_appended` 更新估算；`build_context_messages` 前触发 Layer 1~3 防护检查 |
| [`src/infra/config.rs`](../../../src/infra/config.rs) | 新增 `[context]` 配置节：`context_window`、`max_output_tokens`、`compaction_turns`（默认 10）、`keep_recent_turns`（默认 3）、`single_tool_result_max_chars`（默认 400K）、`compaction_model`（默认 `gpt-5.2`） |
| `src/core/compaction.rs`（**新建**） | `truncate_tool_result_if_needed`（Layer 0）、`compact_tool_results`（Layer 1）、`run_compaction_loop`（Layer 2）、`generate_summary` / `update_summary`（LLM 摘要）、SUMMARIZATION_PROMPT / UPDATE_SUMMARIZATION_PROMPT 模板 |

---

## 10. 与其他模块的关联

### 10.1 Agent Loop（agent-loop.md §13.3）

- **思考-行动循环（第三层）**：每轮工具执行后调用 `on_message_appended` 更新 `estimateContextChars`。
- **容错重试循环（第二层）**：LLM 返回 ContextOverflow 错误时，发布 `auto_compaction_start` 事件，调用本模块 Layer 2（`run_compaction_loop`），完成后发布 `auto_compaction_end`。
- **对话管理循环（第一层）**：每次 `build_context_messages` 前执行 Layer 1~3；Layer 0 在工具返回写入 messages 时执行。

### 10.2 会话存储（session-storage.md）

- Compaction 摘要以 `SessionEntry::Compaction` entry 类型写入 transcript JSONL。
- 初始化时从 transcript 流式读取 user turns（遵守「禁止全量加载」约定，使用 `BufReader` 逐行解析）。

### 10.3 配置管理（infrastructure-layer.md）

- `[context]` 配置节由 `PrimitiveConfig` 加载，支持 `pi.config.toml` 覆盖。
- 不同模型可通过 `[model.<name>]` 节覆盖 `context_window` 和 `max_output_tokens`。

### 10.4 事件系统（events.md）

本模块发布的事件：

| 事件 | 时机 | payload |
|------|------|---------|
| `auto_compaction_start` | Layer 2 开始前 | `{ reason: "context_overflow" \| "budget_exceeded" }` |
| `auto_compaction_end` | Layer 2 完成后 | `{ summary_count, chars_before, chars_after }` |
| `compaction_error` | Layer 2 单批次失败 | `{ batch_index, error }` |
| `tool_result_truncated` | Layer 0 截断新 tool result | `{ tool_name, original_chars, truncated_chars }` |
