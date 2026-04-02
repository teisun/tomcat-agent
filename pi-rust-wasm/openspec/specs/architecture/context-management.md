本文为 [Architecture](../Architecture.md) 中「上下文管理」的详细设计，总览见主文档。关联文档：[Agent Loop 设计](agent-loop.md)、[会话存储数据结构](session-storage.md)。研究报告：[context-management-deep-dive.md](../../../docs/reports/context-management-deep-dive.md)。重构建议报告：[context-management-refactoring-proposal.md](../../../docs/reports/context-management-refactoring-proposal.md)。

---

# 上下文管理技术方案

## 1. 概述

### 1.1 背景

TASK-17 已落地四层防护（Layer 0 截断 → Layer 1 占位符 → Layer 2 LLM 摘要 → Layer 3 强制删除）和 token-aware 滑窗，解决了原始的条数裁剪和 context overflow 问题。

本轮重构基于 [Claude Code 上下文管理机制](../../../docs/reports/context-management-refactoring-proposal.md) 的对比分析，对现有方案做渐进式升级，核心改进点：

- **Token 计数精度**：从纯字符估算（误差 30-50%）升级为 API Usage 优先 + 字符 fallback，直接影响所有压缩决策的准确性
- **主动压缩**：从被动 `is_over_budget()` 单点触发升级为多级 ratio 水位线（70%/85%/92%/98%）+ buffer 安全网的分级主动响应
- **信息保全**：Layer 0 从「截断丢弃」升级为「落盘 + preview 占位符」，大 tool_result 内容不丢失、可按需读回
- **级联降压**：Layer 0 → 1 → 2 → 3 按瀑布式逐层尝试，每层跑完重新计算 ratio，降压成功即停
- **防御增强**：Circuit Breaker（Layer 2 连续失败 3 次自动降级）+ PTL 重试（摘要请求超长时范围减半重试）
- **可观测性**：`ContextMetrics` 追踪 token 使用率、压缩次数、释放量等指标

### 1.2 设计目标

1. **防溢出**：所有发给 LLM 的 prompt 估算 token 不超过安全水位，消除 context overflow
2. **语义完整**：被压缩的旧消息通过 LLM 结构化摘要保留核心语义（Goal / Constraints / Progress）
3. **信息保全**：超大 tool_result 落盘保全，不截断丢弃，未来可按需读回
4. **低额外开销**：优先用零成本的 Layer 0（落盘）和 Layer 1（占位符）释放空间，仅在不够时才触发 LLM 摘要
5. **主动降压**：ratio 水位线驱动的分级主动压缩，而非被动等 API 报错
6. **防御性**：Circuit Breaker 防止 LLM 调用无限重试，Layer 3 兜底确保极端场景不崩溃
7. **可配置**：`context_window`、`max_output_tokens`、`autocompact_buffer_tokens`、`warning_buffer_tokens`、`compaction_model` 等均可在配置中覆盖
8. **可观测**：`ContextMetrics` 提供上下文健康度实时指标

---

## 2. 术语表

| 术语 | 说明 |
|------|------|
| **user turn** | 一条 `role=user` 消息 + 其后所有 `role=assistant` / `role=tool` 消息，直到下一条 `role=user`。上下文管理的最小粒度单位。 |
| **context_window** | 模型固有的最大上下文长度（输入 + 输出），由模型提供商决定（如 GPT-4o 128K, GPT-5.2 400K）。 |
| **input_budget** | 输入 token 预算，`context_window - max_output_tokens`。分母，用于计算 ratio。 |
| **ratio** | 上下文使用率，`estimated_input_tokens / input_budget`，取值 0.0 ~ 1.0+。驱动多级水位线触发。 |
| **cascade** | 级联降压流程。Layer 0 → 1 → 2 → 3 逐层尝试，每层完成后重新计算 ratio，降压成功即停。 |
| **compactable zone** | `userTurnsList` 中可被压缩的区间 `[0, N-m)`（左闭右开，Rust `[..compactable_end]` 语义），排除保护区。 |
| **protected zone** | 最近 `m` 个 user turns，**不参与任何压缩/替换**。`m` 值随 ratio 档位动态调整。 |
| **m 值** | 保护区大小，即 cascade 中保留的最近 turn 数。ratio 越高，m 越小，压缩越激进。 |
| **preview 占位符** | Layer 0 落盘后替换 tool_result 的短文本，包含路径 + 工具名 + 前 500 chars 预览。 |
| **placeholder** | Layer 1 替换旧 turn 中 tool_result 的常量文本 `[Previous tool result replaced to save context space]`。 |
| **compaction summary** | Layer 2 LLM 对一批 user turns 生成的结构化摘要，一条消息替换整批 turns。 |
| **compact boundary** | `TranscriptEntry::Compaction` 中的 `is_boundary: bool` 标记，`init_context_state` 遇到后丢弃其前所有 entry，防止跨重启时历史重复。 |
| **circuit breaker** | Layer 2 LLM 摘要连续失败 >= 3 次后自动跳过，直接 fallback 到 Layer 3。 |
| **buffer 安全网** | `autocompact_buffer_tokens` / `warning_buffer_tokens`，基于绝对剩余 token 的辅助触发线，主要保护小窗口模型。 |
| **API Usage** | LLM API 返回的 `usage` 字段（`prompt_tokens` + `completion_tokens`），用于精确 token 计数。 |

---

## 3. 核心架构图

### 图一：Token 计数与 Ratio 计算

```
  ════════════════════ Token 计数策略 ════════════════════

  方式 A（优先）：API Usage 精确计数
  ──────────────────────────────────────
  LLM 响应中的 StreamEvent::Usage
      → prompt_tokens = 180,000
      → completion_tokens = 2,000
      │
      │ 之后新增消息的字符数 / 4（增量估算）
      │   post_usage_appended_chars = 12,000 → ~3,000 tokens
      ▼
  estimated_input_tokens = prompt_tokens + incremental
                         = 180,000 + 3,000 = 183,000

  方式 B（fallback）：字符启发式
  ──────────────────────────────────────
  首轮无 usage / compact 后 usage 失效时
      → estimated_input_tokens = estimate_context_chars / 4


  ════════════════════ Ratio 与水位线 ════════════════════

  input_budget = context_window - max_output_tokens
               = 400,000 - 128,000 = 272,000 tokens（GPT-5.2）

  ratio = estimated_input_tokens / input_budget

    0%          70%     85%  92% 98% 100%
    ├───────────┼───────┼────┼───┼───┤
    │  正常区    │cascade│    │   │   │
    │  无压缩    │L1→L2  │L1→L2│L1→L2│L1→L2│ L3
    │           │  m=5  │m=3 │m=2│m=1│ 强制删除
    │           │       │    │   │阻止│ 目标<0.50
    │           │       │    │   │工具│

  注：Layer 0 每轮必跑，不受 ratio 控制，图中省略。
      cascade 启动后从 Layer 1 起逐层执行（L1→L2→L3），每层完成后重新算 ratio，降压成功即停。
```

### 图二：四层防护流程（级联降压）

```
  工具执行完毕，LLM 已完成本轮回复
      │
      ▼
  ┌─ Layer 0（每轮必跑）─────────────────────────────────┐
  │  单条 tool_result >= 30K chars？                     │
  │    → 落盘 + preview 占位符                           │
  │  单 user_turn tool_result 合计 >= 150K chars？       │
  │    → 挑最大 fresh 结果逐个落盘，直到合计 < 150K      │
  └──────────────────────────────────────────────────────┘
      │
      ▼ 重新算 ratio
      │
      ratio < 0.70 且剩余 > buffer？ ──Yes──► 停止，无需 cascade
      │
      No（cascade 启动）
      ▼
  ┌─ Layer 1 ────────────────────────────────────────────┐
  │  turn 0..(N-m) 中 tool_result > 20K chars            │
  │  → 占位符替换（不落盘）                              │
  └──────────────────────────────────────────────────────┘
      │
      ▼ 重新算 ratio
      │
      ratio 已降到安全线？ ──Yes──► 停止
      │
      No
      ▼
  ┌─ Layer 2 ────────────────────────────────────────────┐
  │  按当前 ratio 对应的 m 值，对 turn 0..(N-m) 做 LLM 摘要│
  │  （ratio 越高 m 越小，压缩越激进）                    │
  │  Circuit Breaker：连续失败 >= 3 → 跳过               │
  └──────────────────────────────────────────────────────┘
      │
      ▼ 重新算 ratio
      │
      ratio 已降到安全线？ ──Yes──► 停止
      │
      No
      ▼
  ┌─ Layer 3（防御性兜底，几乎不可达）───────────────────┐
  │  从最旧 summary/turn 起逐条删除，直到 ratio < 0.50   │
  │  （远低于 Layer 2 触发线，充足缓冲避免振荡）          │
  └──────────────────────────────────────────────────────┘
      │
      ▼
  ratio >= 0.98？ ──Yes──► 标记 block_tool_calls = true
                           reasoning loop 后续迭代中若 LLM 请求
                           工具调用，跳过执行并返回文本提示用户
                           （压缩已尽力，避免继续膨胀）
```

### 图三：滑动窗口与 m 值保护区

```
  userTurnsList (内存中维护)：

  ratio = 0.70 → m = 5:
  [turn_0] [turn_1] ... [turn_n-6] │ [turn_n-5] ... [turn_n-1]
  ◄──── compactable zone ─────────►│◄──── protected zone (5) ──►

  ratio = 0.85 → m = 3:
  [turn_0] [turn_1] ... [turn_n-4] │ [turn_n-3] [turn_n-2] [turn_n-1]
  ◄──── compactable zone ─────────►│◄──── protected zone (3) ──────►

  ratio = 0.98 → m = 1:
  [turn_0] [turn_1] ... [turn_n-2] │ [turn_n-1]
  ◄──── compactable zone ─────────►│◄── prot.(1)
```

### 图四：Compaction 摘要演进

```
  ════════════════════ 初始状态 ════════════════════

  [turn_0][turn_1]...[turn_9] [turn_10]...[turn_n-m] │ [turn_n-m+1]...[turn_n-1]
  ◄────── compactable zone ─────────────────────────►│◄─── protected zone (m) ──►

  ════════════════════ Layer 2 触发（ratio >= 0.70, m=5）════════════

  LLM(turn_0..turn_n-5) → summary_A（Goal/Constraints/Progress...）

  [summary_A] │ [turn_n-5]...[turn_n-1]
  ◄─ 1 条 ──► │ ◄─── protected zone (5) ──►

  ratio 降回 ~0.2，cascade 结束

  ════════════════════ 已有旧 summary 时 ════════════════════

  若后续 ratio 再次达 0.70，summary_A 与新 turn 均在 compactable zone 中：
  LLM 使用 UPDATE 模式合并旧 summary（参考 pi-mono UPDATE_SUMMARIZATION_PROMPT）
```

---

## 4. 预算计算与水位线

### 4.1 Token 计数策略

精确的 token 计数是所有压缩决策的基础。采用 **API Usage 优先 + 字符 fallback** 双模式：

```
fn estimated_token_count(state: &ContextState) -> usize:
    if let Some(usage) = state.last_api_usage:
        let base = usage.prompt_tokens + usage.completion_tokens
        let incremental = state.post_usage_appended_chars / CHARS_PER_TOKEN_ESTIMATE
        base + incremental
    else:
        state.estimate_context_chars / CHARS_PER_TOKEN_ESTIMATE

fn usage_ratio(state: &ContextState) -> f64:
    estimated_token_count(state) / state.context_budget_tokens
```

- **`last_api_usage`**：每次 LLM 响应结束后，从 `StreamEvent::Usage` 更新
- **`post_usage_appended_chars`**：自最近 usage 后新增的字符数，用于增量估算
- **compact 后**：`last_api_usage` 失效（上下文已变），清零回退到字符 fallback，等下次 API 响应刷新

> **关于 `estimate_context_chars` 的度量单位**：Rust 的 `String::len()` 返回 UTF-8 字节数而非 Unicode 字符数。`CHARS_PER_TOKEN_ESTIMATE = 4` 对英文内容（1 byte ≈ 1 char）较为准确；对中文内容（3 bytes/char，约 1.5 token/char），4 bytes/token 的估算会偏保守（低估 token 数），可能导致压缩触发略晚。**API Usage 优先模式下此偏差被消除**，字符 fallback 仅在首轮和 compact 后短暂使用。

### 4.2 Ratio 水位线

`ratio = estimated_input_tokens / input_budget`，其中 `input_budget = context_window - max_output_tokens`。

分母是输入 token 预算（已扣除输出预留），ratio 衡量的是输入空间的使用率，不会挤占输出空间。

| ratio 档位 | cascade 最高触及层 | m | 动作 |
|------------|-------------------|---|------|
| `< 0.70` | — | — | 正常，无 cascade（Layer 0 仍每轮执行） |
| `>= 0.70` | Layer 1 → 2 | 5 | 温和压缩，batch 小，摘要精 |
| `>= 0.85` | Layer 1 → 2 | 3 | 中等压力 |
| `>= 0.92` | Layer 1 → 2 | 2 | 高压，只留最近 2 轮 |
| `>= 0.98` | Layer 1 → 2 | 1 | 极限压缩 + 阻止后续工具调用 |
| `>= 1.0` | Layer 1 → 2 → 3 | — | 强制删除，**目标 ratio < 0.50** |

> cascade 始终从 Layer 0 起逐层执行（Layer 0 → 1 → 2 → 3），每层完成后重新计算 ratio，降压成功即停。上表的"cascade 最高触及层"表示该 ratio 下 cascade 最高会升级到的层级及其 m 值参数。

Layer 0 在**每轮 LLM 回复完毕后立即检查**，不受 ratio 控制。Layer 1/2/3 作为 cascade 的一环，由 ratio 或 buffer 驱动——只有 Layer 0 不够降压时才逐层升级。

**触发总表**（含 Layer 0/1）：

| 触发条件 | 层级 | m | 动作 |
|---------|------|---|------|
| 单条 tool_result >= 30K chars | Layer 0 | — | 落盘 + preview 占位符 |
| 单个 user_turn 的 tool_result 合计 >= 150K chars | Layer 0 | — | 挑最大的 fresh 结果逐个落盘，直到合计回到预算内 |
| Layer 0 后 ratio 仍 >= 0.70（或剩余 < buffer），旧 turn(0..N-m) 中 tool_result > 20K chars | Layer 1 | 同 Layer 2 | 占位符替换（不落盘），m 由当前 ratio 档位决定 |

### 4.3 Buffer 安全网

`autocompact_buffer_tokens`（默认 13000）和 `warning_buffer_tokens`（默认 20000）提供基于**绝对剩余 token** 的辅助触发线，映射到等价的 ratio 档位行为：

| Buffer 条件 | 等价 ratio（128K 窗口） | 动作 |
|------------|----------------------|------|
| 剩余 < `warning_buffer`(20K) | ≈ 0.82 | Layer 2，m=3 |
| 剩余 < `autocompact_buffer`(13K) | ≈ 0.88 | Layer 2，m=2 |

对大窗口模型（128K+），ratio 几乎总是先触发；buffer 的价值在**小窗口模型**。但 13K/20K 的绝对值对小窗口不合理（32K 窗口下几乎无法使用），因此需要 cap：**实际 buffer = min(配置值, input_budget × 0.3)**（其中 `input_budget = context_window - max_output_tokens`），确保至少保留 70% 的 input budget 给正常对话。

两套机制互补：**ratio 按比例适配所有窗口大小，buffer 为「剩余空间不足以完成一轮完整工具调用」提供绝对值保底**。

### 4.4 配置项

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `context_window` | `usize` | `400_000` | 默认对齐 **GPT-5.2**（400K）；其他模型请在配置中覆盖 |
| `max_output_tokens` | `usize` | `128_000` | 默认对齐 **GPT-5.2** 单轮最大输出；对齐 API 的 `max_tokens` |
| `layer0_single_result_max_chars` | `usize` | `30_000` | Layer 0 触发条件 A：单条 tool_result 超过此值则落盘 + preview 占位符 |
| `layer0_turn_aggregate_max_chars` | `usize` | `150_000` | Layer 0 触发条件 B：单个 user_turn 内所有 tool_result 合计超过此值则逐个落盘最大的 |
| `autocompact_buffer_tokens` | `usize` | `13_000` | 剩余 token 低于此值时触发 Layer 2（m=2），主要保护小窗口模型 |
| `warning_buffer_tokens` | `usize` | `20_000` | 剩余 token 低于此值时触发 Layer 2（m=3），与 autocompact_buffer 分级 |
| `compaction_model` | `String` | `"gpt-5.2"` | Compaction 摘要专用模型 ID（与主对话 `model` 可相同或不同） |

> 配置位于 `pi.config.toml` 的 `[context]` 节，或通过 `PrimitiveConfig` 结构体注入。

### 4.5 典型值

| 模型 | context_window | max_output_tokens | input_budget | ratio=0.70 时已用 |
|------|---------------|-------------------|-------------|-----------------|
| GPT-4o | 128,000 | 16,384 | 111,616 | 78,131 |
| GPT-5.2 | 400,000 | 128,000 | 272,000 | 190,400 |
| Claude 3.5 Sonnet | 200,000 | 8,192 | 191,808 | 134,266 |
| DeepSeek-V3 | 64,000 | 8,192 | 55,808 | 39,066 |

### 4.6 与旧方案对比

旧方案（TASK-17）使用 `contextBudgetChars = (context_window - max_output_tokens) × 4 × 0.75`，额外乘 0.75 安全系数用于补偿字符→token 估算误差。新方案有了精确 token 计数后，0.75 系数不再需要——ratio 水位线本身提供分级保护，且 `is_over_budget()` 改为基于 token 维度判断。

---

## 5. 初始化与动态维护

### 5.1 会话启动时初始化

```
fn init_context_state(session: &Session, config: &ContextConfig) -> ContextState:
    turns = load_user_turns_from_transcript(session.transcript_path)

    # 优先取当天所有 turns
    today_turns = turns.filter(|t| t.date == today())

    # 不足 10 则向前补全（确保短会话或跨午夜场景仍有足够上下文）
    if today_turns.len() < 10:
        extra = turns.before(today_turns.first())
                     .rev()
                     .take(10 - today_turns.len())
        today_turns = extra.rev() + today_turns

    let input_budget = config.context_window - config.max_output_tokens
    estimate = sum(today_turns.map(|t| estimate_turn_chars(t)))

    return ContextState {
        user_turns_list: today_turns,
        estimate_context_chars: estimate,
        context_budget_chars: input_budget * CHARS_PER_TOKEN_ESTIMATE,  # fallback 用
        context_budget_tokens: input_budget,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        compaction_consecutive_failures: 0,
    }
```

> **边界情况说明**：
> - **跨午夜会话**：用户 23:55 开始对话，重启后 `today()` 返回新日期。当天 turns 为空时，向前补全最近 10 条覆盖前一天的对话，不影响正确性。
> - **长期不活跃**：transcript 最后活跃在数天前，补全的 10 条为旧消息。上下文可能已不相关，但不影响正确性——后续新对话产生后旧 turns 会自然进入 compactable zone 被压缩。
> - 此策略优先保证**不丢失近期上下文**；上下文"相关性"由 Layer 2 摘要在运行过程中自然优化。

### 5.2 动态更新

每次 reasoning loop 中追加新 turn（包括 assistant、tool 消息），同步更新估算：

```
fn on_message_appended(state: &mut ContextState, msg: &Message):
    state.estimate_context_chars += msg.content.len()
    state.post_usage_appended_chars += msg.content.len()

fn on_new_user_turn(state: &mut ContextState, turn: UserTurn):
    let chars = estimate_turn_chars(&turn)
    state.estimate_context_chars += chars
    state.post_usage_appended_chars += chars
    state.user_turns_list.push(turn)

fn update_api_usage(state: &mut ContextState, prompt_tokens: u32, completion_tokens: u32):
    state.last_api_usage = Some(ApiUsage { prompt_tokens, completion_tokens })
    state.post_usage_appended_chars = 0

fn invalidate_api_usage(state: &mut ContextState):
    state.last_api_usage = None
    state.post_usage_appended_chars = 0
```

> `invalidate_api_usage` 在 compact 后调用——上下文已变，旧 usage 不再有效。

### 5.3 system prompt 纳入估算

`estimateContextChars` 应包含 system prompt 的字符数。system prompt 在会话期间通常不变，初始化时计算一次即可：

```
estimate = system_prompt.len() + sum(today_turns.map(|t| estimate_turn_chars(t)))
```

> 若 system prompt 较短（< 5K chars），水位线已足够覆盖。但为准确性，仍建议显式计入。

### 5.4 Session 重载与 Compact Boundary

从 transcript JSONL 加载 user turns 时，需识别 `SessionEntry::Compaction` entry 并处理 boundary 语义：

1. 遇到 `Compaction` entry → 作为 `SummaryTurn` 加入 `userTurnsList`，**不展开**为原始消息
2. 已被 Compaction 覆盖的原始 turns **不重复加载**（Compaction entry 中记录了覆盖的 turn range）
3. 后续 Layer 2 可直接定位已有 summary，进入 UPDATE 模式

**Compact Boundary 处理**：

`TranscriptEntry::Compaction` 中 `is_boundary: bool` 标记。`init_context_state` 遇到 `is_boundary=true` 的 Compaction entry 时，丢弃其前已暂存的所有 entry，使重建结果与运行时一致：

```
Transcript 文件（JSONL，按时间追加）
═════════════════════════════════════════════

  entry 1~8:  原始消息（已被摘要覆盖）
  entry 9:    Compaction { summary: "...", is_boundary: true }
  entry 10~11: 新消息

init_context_state 处理流程：
  读到 entry 1~8 → 暂存
  读到 entry 9 (is_boundary=true) → 丢弃暂存的 1~8，保留 summary
  读到 entry 10~11 → 构建 UserTurn

  结果: [SummaryTurn(entry 9), UserTurn(entry 10~11)]
  → 与运行时 drain 后的内存状态一致，无重复
```

**被压缩的 user turn 是否仍留在 transcript JSONL 中？**

采用 **仅追加（append-only）** 约定，与 pi 系 transcript 一致：

- **保留**：原先写入的 `Message` 行（user / assistant / tool）**不删除、不改写**，仍在 `.jsonl` 中，便于审计、回放与调试。
- **追加**：Compaction 发生时 **再追加一行** `type: compaction` 的 `CompactionEntry`（含摘要正文、覆盖范围元数据、`is_boundary` 标记，字段以 [session-storage.md](session-storage.md) 为准）。
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
  │       │  识别 Compaction entry + boundary     │ 折叠已摘要区间            │
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
  │  messages.push(AgentMessage::ToolResult { .. })                         │
  │  estimateContextChars += result.len()       ← 实时更新                  │
  │  update_api_usage(usage)                    ← 从 StreamEvent 更新       │
  │                                                                         │
  │  ◆ 本轮 LLM 回复完毕后：                                               │
  │    → Layer 0 检查（落盘超大 tool_result）                                │
  │    → 计算 ratio → 若需 cascade 则 Layer 1 → 2 → 3                      │
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
- **估算更新**：`estimateContextChars` 在 ③ 每次 push 时实时累加，`last_api_usage` 在每次 LLM 响应后刷新。cascade 在 ③ 内也能正确触发。

即：**`userTurnsList` 是持久化 transcript 与 `AgentMessage` 之间的中间层**——负责分组、Compaction 折叠与估算维护；最终转为 `AgentMessage` 后走已有的 `convert_to_llm_format` 链路，**不改变** reasoning loop 内部已有的消息流转方式。

---

## 6. 防护算法（Layer 0~3）

### 6.1 Layer 0：工具结果落盘（入口防线）

在 reasoning loop 中，每轮 LLM 回复完毕后，检查当前 turn 的 tool_result。**先让 LLM 看到完整内容，再收纳落盘**——LLM 在本轮已正常分析和使用了完整结果，落盘是为了未来轮次的上下文不膨胀。

**触发条件 A**：单条 tool_result >= `layer0_single_result_max_chars`（默认 **30K chars**，~7.5K token）

**触发条件 B**：单个 user_turn 内所有 tool_result 合计 >= `layer0_turn_aggregate_max_chars`（默认 **150K chars**，~37.5K token）→ 挑最大的 fresh 结果，逐个落盘并换成预览，直到合计回到预算内

> **fresh 定义**：本轮 reasoning loop 迭代内产生且尚未被 Layer 0 落盘处理的 tool_result。已替换为 preview 占位符的不再参与。

> **并发工具执行时序**：若 LLM 一次返回多个 tool_calls 并行执行，Layer 0 在**全部并行工具完成后**统一检查触发条件 A/B，确保合计计算覆盖所有结果。

**落盘动作**：

```
fn persist_tool_result(result: &ToolResult, work_dir: &Path) -> String:
    let path = format!("{}/agents/{}/tool-results/{}.txt",
                       work_dir, session_id, result.tool_call_id)
    fs::write(&path, &result.content)

    let preview = &result.content[..min(500, result.content.len())]
    format!("[Tool result persisted: {} (来源: {}(\"{}\"), {})]\\nPreview: {}...",
            path, result.tool_name, result.arg_summary,
            human_readable_size(result.content.len()), preview)
```

**留 preview 的理由**：仅靠路径和工具名，LLM 在未来轮次无法判断内容是否与当前任务相关（尤其是 `search`/`shell` 等输出不可预知的工具）。500 chars 的 preview 成本极低（~125 token），但能帮助 LLM 决定是否需要按需读回，避免盲目忽略或盲目全量读取。

**落盘时机与流程**：

```
                     当前轮（第 N 轮）
  ┌──────────────────────────────────────────────────────┐
  │  1. Agent 执行工具（如 read_file）                    │
  │     → 得到 tool_result（可能超过 30K 字符）            │
  │                                                      │
  │  2. tool_result 原样拼入 messages，发送给 LLM         │
  │     → LLM 看到完整内容，正常分析和回复 ✓              │
  │                                                      │
  │  3. 本轮 LLM 回复完毕后，检查该 tool_result：         │
  │     → 超过阈值？写入磁盘，将上下文中的 tool_result    │
  │       替换为 preview 占位符（路径 + 前 500 chars）    │
  └──────────────────────────────────────────────────────┘
                              │
                              ▼
                     未来轮次（第 N+1, N+2, ...）
  ┌──────────────────────────────────────────────────────┐
  │  组装上下文时，第 N 轮的 tool_result 已是短占位符     │
  │  → 不膨胀，正常构建 messages ✓                       │
  │                                                      │
  │  如果 LLM 需要再看原始内容：                         │
  │  → 按行范围读取（read_file + offset/limit），        │
  │    不要全量读，避免再次产生超大 tool_result            │
  └──────────────────────────────────────────────────────┘
```

### 6.2 Layer 1：旧 turn 批量占位符替换（不落盘）

**触发条件**：cascade 启动（ratio >= 0.70 或剩余 < buffer）且 Layer 0 不够降压时，对 turn 0..(N-m) 中 tool_result > **20K chars** 的做替换。m 由当前 ratio 档位决定。

**不会每轮独立触发**——ratio 低时旧 tool_result 保持完整，LLM 后续仍可引用。

```
const LAYER1_TOOL_RESULT_THRESHOLD: usize = 20_000;
const LAYER1_PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

fn compact_old_tool_results(state: &mut ContextState, m: usize) -> usize:
    let compactable_end = state.user_turns_list.len().saturating_sub(m)
    let mut reduced = 0

    for turn in state.user_turns_list[..compactable_end]:
        for msg in turn.messages where msg.role == Tool:
            if msg.content.len() > LAYER1_TOOL_RESULT_THRESHOLD:
                let before = msg.content.len()
                msg.content = LAYER1_PLACEHOLDER
                reduced += before - LAYER1_PLACEHOLDER.len()
                state.estimate_context_chars -= (before - LAYER1_PLACEHOLDER.len())
    return reduced
```

**为什么不落盘**：Layer 0 已把超大结果落盘保全；Layer 1 处理的是旧 turn 中「不算超大但仍占空间」的 tool_result，这些 turn 即将被 Layer 2 摘要覆盖，为每个都写磁盘的 I/O 和管理成本不值当。

**为什么不每轮无差别清理**：ratio 充裕时没必要丢掉旧 tool_result，且会使 m 保护变得无意义（保护了 turn 不被摘要，却偷偷换掉了其中的 tool_result）。

### 6.3 Layer 2：LLM 摘要压缩

Layer 1 不够时触发。按当前 ratio 对应的 m 值，一次性将 turn 0..(N-m) 提交给 LLM 生成一条 summary 替换。

```
fn run_compaction(state: &mut ContextState, llm: &LlmProvider, m: usize):
    if state.compaction_consecutive_failures >= 3:
        # Circuit Breaker 已触发，跳过 Layer 2
        return

    let compactable_end = state.user_turns_list.len().saturating_sub(m)
    if compactable_end == 0:
        return

    let batch = state.user_turns_list[..compactable_end]
    let previous_summary = find_last_summary(batch)

    # (actual_start, actual_end) 跟踪实际被摘要覆盖的范围
    # 正常情况 = (0, compactable_end)；PTL 重试时取较新半段
    let (summary_text, actual_start, actual_end) =
        match llm.generate_summary(batch, previous_summary):
            Ok(text) => (text, 0, compactable_end)
            Err(e) if is_ptl_error(&e) =>
                # PTL 重试：取较新半段 [mid..compactable_end)，保留近期上下文
                retry_with_half_range(state, llm, compactable_end)?
            Err(e) =>
                state.compaction_consecutive_failures += 1
                return

    let covered = state.user_turns_list[actual_start..actual_end]
    let batch_chars = sum(covered.map(|t| estimate_turn_chars(t)))
    let summary_chars = summary_text.len()

    # 在 splice 前提取覆盖范围的 entry id（splice 后原数据已被替换）
    let first_id = covered.first_entry_id()
    let last_id = covered.last_entry_id()

    # splice 范围必须与实际摘要覆盖范围一致
    state.user_turns_list.splice(actual_start..actual_end, [SummaryTurn(summary_text)])
    state.estimate_context_chars -= batch_chars
    state.estimate_context_chars += summary_chars
    state.compaction_consecutive_failures = 0

    invalidate_api_usage(state)

    # 字段对齐 CompactionEntry 结构体（session-storage.md），新增 is_boundary
    session.append_entry(CompactionEntry {
        id: generate_entry_id(),
        parent_id: last_id.clone(),
        timestamp: Utc::now().to_rfc3339(),
        summary: Some(summary_text),
        covered_start_id: first_id,
        covered_end_id: last_id,
        covered_count: Some(actual_end - actual_start),
        is_boundary: actual_start == 0,  # 仅从头开始压缩时标记 boundary（截断前序）
    })
```

**不再使用循环 batch 模式**：旧设计中 `run_compaction_loop` 按 batch 分组逐步压缩；新设计直接按 ratio 对应的 m 值确定压缩范围，一次调用完成。

### 6.4 Layer 3：强制删除（极端兜底）

Layer 2 被 Circuit Breaker 跳过后仍超预算，或 ratio >= 1.0。从 `user_turns_list[0]`（最旧 summary/turn）起逐个删除，**直到 ratio < 0.50**。

```
fn force_delete_oldest(state: &mut ContextState):
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty():
        let oldest = state.user_turns_list.remove(0)
        state.estimate_context_chars -= estimate_turn_chars(&oldest)
    invalidate_api_usage(state)
```

**为什么目标是 0.50 而不是刚好 < 1.0**：若只降到 < 1.0，下一条消息或工具调用就可能再次触发 Layer 3，形成频繁振荡。删到 0.50 一次性创造充足缓冲，远低于 Layer 2 首次触发线（0.70），确保 Layer 3 触发后有足够的对话增长空间。

**设计定位**：几乎不可达的安全网。正常运行中，0.70 的 Layer 2 压缩通常已足够将 ratio 降回 0.1~0.3；高档位（0.85/0.92/0.98）在实际运行中极少触发。Layer 3 是最后兜底。

> **Layer 3 不受 m 值保护区约束**：当所有 turn 都在 protected zone 内（`compactable_end = 0`）时，Layer 1/2 无法工作。Layer 3 作为最后兜底，**必须能删除任何 turn**（包括 protected zone 内的），否则极端场景下无法降压。

### 6.5 Circuit Breaker

Layer 2 依赖外部 LLM 调用，可能因网络错误、速率限制等反复失败。

```
fn check_circuit_breaker(state: &ContextState) -> bool:
    state.compaction_consecutive_failures >= 3
```

- `compaction_consecutive_failures` 在 Layer 2 LLM 调用失败时递增，成功时清零
- 连续失败 >= 3 次时跳过 Layer 2，直接 fallback 到 Layer 3
- 通过 EventBus 发出 `CompactionCircuitBreakerTriggered` 事件

### 6.6 PTL 重试

Layer 2 的 LLM 摘要请求如果因上下文过长（Prompt Too Long）失败：

```
fn retry_with_half_range(state, llm, compactable_end)
    -> Result<(String, usize, usize)>:
    # 取较新半段：优先保留近期上下文的摘要，旧半段留给后续 cascade 或 Layer 3
    let mut range_start = compactable_end / 2
    for attempt in 0..2:
        let sub_batch = state.user_turns_list[range_start..compactable_end]
        let prev = find_last_summary(sub_batch)
        match llm.generate_summary(sub_batch, prev):
            Ok(text) => return Ok((text, range_start, compactable_end))
            Err(e) if is_ptl_error(&e) =>
                # 仍然太长，再从中点往后取（丢弃更多旧 turns）
                range_start = range_start + (compactable_end - range_start) / 2
            Err(e) => return Err(e)
    Err(PtlRetryExhausted)
```

- 错误含 context/token 关键词（PTL）→ **取较新半段** `[mid..compactable_end)` 重试，优先保留近期上下文的摘要
- 最多重试 2 次，每次 `range_start` 向后推移（丢弃更多旧 turns），范围持续缩小
- 未被摘要覆盖的旧半段 `[0..range_start)` 留给后续 cascade 或 Layer 3 处理
- 仍失败则跳过 Layer 2，由 Circuit Breaker 计数，fallback 到 Layer 3

### 6.7 防振荡设计

落盘后如果 LLM 再次全量读取同一文件，新 tool_result 仍可能超阈值、再次落盘，形成「读 → 落盘 → 再读 → 再落盘」的无效循环。

防范策略：

1. **分页读取引导**：system prompt 中明确告知 LLM「已落盘的工具结果可通过 `read_file` 的 offset/limit 参数按需读取指定行范围，无需全量读取」
2. **占位符自包含**：preview（前 500 chars）+ 来源工具名 + 参数 + 大小，让 LLM 有足够信息决定是否需要读回、读哪部分
3. **兜底保障**：即使 LLM 仍然全量读取，Layer 0 会再次正常落盘。流程上不会死循环（每轮仍正常推进），只是浪费了一次全量读取的 token。这属于 LLM 行为问题，通过优化 system prompt 引导来改善，不需要在代码层做硬拦截

### 6.8 Compaction 失败处理

| 情况 | 处理 |
|------|------|
| LLM 调用超时/网络错误 | Circuit Breaker 递增；连续 3 次后跳过 Layer 2，降级到 Layer 3 |
| LLM 返回空摘要 | 视为失败，同上 |
| 摘要比原文还大（极端） | 丢弃摘要，保留原文，Circuit Breaker 递增 |
| PTL 错误 | 范围减半重试（最多 2 次），仍失败则 Circuit Breaker 递增 |

> 失败时发布 `compaction_error` 事件（`{ error, consecutive_failures }`），由日志记录。

### 6.9 与 `max_tool_rounds` 的关系

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

Use this EXACT format (same as the original summary):

## Goal
[Updated goal]

## Constraints & Preferences
- [Updated constraints]

## Progress
### Done
- [x] [Completed tasks]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Updated ordered list]

## Critical Context
- [Updated references]
```

### 7.4 摘要消息格式

摘要以 `Compaction` entry 写入 transcript JSONL（类型已在 [session-storage.md](session-storage.md) 的 `SessionEntry::Compaction` 定义，新增 `is_boundary` 字段）。在内存中作为一条 `role=user` 消息（content 为摘要文本）放入 `user_turns_list`，替换被压缩的原始 turns。

---

## 8. 超出本方案范围（Out of Scope）

以下机制在研究报告中分析过，但不纳入本方案实现：

| 机制 | 说明 | 后续计划 |
|------|------|---------|
| **Snip（中间段删除）** | CC Level 1，删除中间历史保留头尾，零 API 成本。当前 Layer 2 可覆盖此场景。 | 若 Layer 2 触发过于频繁/费用高，再评估插入 Layer 1~2 之间 |
| **Prompt Cache 管理** | CC 的 `cache_control` / `cache_reference` / `cache_edits` 三原语为 Anthropic API 专属，OpenAI 自动缓存无需客户端配置 | 不适用 |
| **Cached Microcompact** | 依赖 `cache_edits` 服务端打洞能力 | 不适用 |
| **Session Stability Latching** | 锁定运行时状态防 cache bust，Pi 无 Prompt Cache | 不适用 |
| **Context Collapse** | CC 实验性 commit-log 视图投影，通用性差 | 不纳入 |
| **工具循环检测（tool-loop-detection）** | openclaw 的滑窗重复检测 + steering 注入 + 熔断 | 独立方案在 agent-loop 中实现 |
| **RAG 检索增强** | 旧消息向量化 + 按相关性检索注入 | 长期方向 |
| **System Prompt 自动注入** | 从对话中自动提取约束/偏好到 system prompt | 可与 Compaction 互补，后续独立方案 |

---

## 9. 涉及改动文件

| 文件 | 改动内容 |
|------|---------|
| [`src/core/session/manager.rs`](../../../src/core/session/manager.rs) | `ContextState` 增加 `context_budget_tokens`、`last_api_usage`、`post_usage_appended_chars`、`compaction_consecutive_failures` 字段；新增 `usage_ratio()`、`estimated_token_count()`、`update_api_usage()`、`invalidate_api_usage()` 方法；`init_context_state` 增加 compact boundary 处理 |
| [`src/core/agent_loop.rs`](../../../src/core/agent_loop.rs) | reasoning loop 每轮 LLM 回复后：① 捕获 `StreamEvent::Usage` 更新 `last_api_usage` ② 调用 Layer 0（落盘） ③ 计算 ratio 触发 cascade（Layer 1 → 2 → 3）；ratio >= 0.98 时标记阻止新工具调用 |
| [`src/infra/config.rs`](../../../src/infra/config.rs) | `[context]` 配置节新增 `layer0_single_result_max_chars`、`layer0_turn_aggregate_max_chars`、`autocompact_buffer_tokens`、`warning_buffer_tokens` |
| [`src/core/compaction.rs`](../../../src/core/compaction.rs) | Layer 0 从截断改为落盘 + preview；Layer 1 改为 cascade 内触发（旧 turn > 20K 占位符替换，不落盘）；Layer 2 从循环 batch 改为按 m 值一次调用；Layer 3 目标 ratio < 0.50；新增 Circuit Breaker + PTL 重试；新增 cascade 流程编排 |
| [`src/core/system_prompt.rs`](../../../src/core/system_prompt.rs) | 模块化改造（`SystemPromptSection` trait + 注册机制）；新增分页读取引导 section |
| [`src/infra/events.rs`](../../../src/infra/events.rs) | 新增 `ContextMetricsUpdate`、`CompactionCircuitBreakerTriggered` 事件 |
| `src/core/context_metrics.rs`（**新建**） | `ContextMetrics` 结构体：`input_tokens_used`、`context_utilization_ratio`、`compaction_count`、`compaction_tokens_freed`、`total_tool_result_bytes_persisted` |

---

## 10. 与其他模块的关联

### 10.1 Agent Loop（agent-loop.md §13.3）

- **思考-行动循环（第三层）**：每轮 LLM 回复完毕后，捕获 `StreamEvent::Usage` 更新 token 计数，执行 Layer 0 检查，计算 ratio 触发 cascade。
- **容错重试循环（第二层）**：LLM 返回 ContextOverflow 错误时，发布 `auto_compaction_start` 事件，直接触发 cascade 流程（从 Layer 0 起逐层执行）。
- **工具调用阻止**：ratio >= 0.98 时标记 `block_tool_calls`，reasoning loop 跳过工具执行、返回文本提示用户。

### 10.2 会话存储（session-storage.md）

- Compaction 摘要以 `SessionEntry::Compaction` entry 类型写入 transcript JSONL，新增 `is_boundary: bool` 字段。
- Tool result 落盘文件存储在 `{work_dir}/agents/{session_id}/tool-results/` 目录。
- 初始化时从 transcript 流式读取 user turns（遵守「禁止全量加载」约定，使用 `BufReader` 逐行解析），识别 compact boundary。

### 10.3 配置管理（infrastructure-layer.md）

- `[context]` 配置节由 `PrimitiveConfig` 加载，支持 `pi.config.toml` 覆盖。
- 新增 `autocompact_buffer_tokens`、`warning_buffer_tokens` 配置项。
- 不同模型可通过 `[model.<name>]` 节覆盖 `context_window` 和 `max_output_tokens`。

### 10.4 事件系统（events.md）

本模块发布的事件：

| 事件 | 时机 | payload |
|------|------|---------|
| `auto_compaction_start` | cascade 启动前 | `{ reason: "ratio_threshold" \| "buffer_threshold" \| "context_overflow", ratio, m }` |
| `auto_compaction_end` | cascade 完成后 | `{ layers_executed, ratio_before, ratio_after }` |
| `compaction_error` | Layer 2 单次失败 | `{ error, consecutive_failures }` |
| `compaction_circuit_breaker_triggered` | 连续失败 >= 3 | `{ consecutive_failures }` |
| `tool_result_persisted` | Layer 0 落盘 tool result | `{ tool_name, original_chars, persisted_path }` |
| `context_metrics_update` | 每轮 LLM 回复后 | `{ ratio, input_tokens, compaction_count, tokens_freed }` |
