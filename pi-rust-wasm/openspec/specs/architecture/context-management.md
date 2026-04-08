本文为 [Architecture](../Architecture.md) 中「上下文管理」的详细设计，总览见主文档。关联文档：[Agent Loop 设计](agent-loop.md)、[会话存储数据结构](session-storage.md)。研究报告：[context-management-deep-dive.md](../../../docs/reports/context-management-deep-dive.md)。重构建议报告：[context-management-refactoring-proposal.md](../../../docs/reports/context-management-refactoring-proposal.md)。

---

# 上下文管理技术方案

## 1. 概述

### 1.1 背景

TASK-17 落地了四层同步防护（Layer 0 截断 → Layer 1 占位符 → Layer 2 LLM 摘要 → Layer 3 强制删除）和 token-aware 滑窗。后续基于 [Claude Code 上下文管理机制](../../../docs/reports/context-management-refactoring-proposal.md) 的对比分析，升级为 ratio 水位线 + 级联降压模式。

本轮重构将 **LLM 摘要从同步阻塞改为异步预热 + 延迟应用**，核心目标是 **主线程零等待**，避免压缩操作卡住 UI。四层重新定义为：


| 层级      | 名称             | 执行模式               |
| ------- | -------------- | ------------------ |
| Layer 0 | tool_result 清理 | 同步（每轮必跑，纯内存操作极快，用户无感知） |
| Layer 1 | 异步预热           | 异步（后台 Task，主线程不等待） |
| Layer 2 | 检查与应用          | 非阻塞检查 / 仅极端时同步等待   |
| Layer 3 | 物理截断           | 同步（API 报错后兜底）      |


**关键时序定义**：

- **「LLM 回复后」**：指 reasoning loop 的**最终 assistant 回复**——此时当前 user turn 内所有 tool 已执行完毕，LLM 给出了无 tool_calls 的文本回答，reasoning loop 结束。**不是** reasoning loop 内每次中间 LLM 调用之后。
- **「发起下一次 LLM 请求前」**：指**下一个 user turn** 进入时，在构建 `messages` 并调用 LLM 之前。两个时机之间是用户阅读/思考/输入的间隔期，异步预热在此期间后台运行。

核心改进点：

- **Token 计数精度**：从纯字符估算升级为 API Usage 优先 + 字符 fallback
- **异步摘要**：LLM 摘要从同步阻塞改为 Layer 1 异步预热 + Layer 2 延迟应用，LLM 回复后主线程零等待
- **Preheat 状态机（单任务）**：`Preheat` 保证后台同一时间至多一个预热 task，防止竞态和 token 浪费
- **信息保全**：Layer 0 从「截断丢弃」升级为「落盘 + preview 占位符」，大 tool_result 内容不丢失、可按需读回
- **UI 不卡顿**：LLM 回复后（L0/L1 时机）绝不阻塞主线程；仅在发起下一次 LLM 请求前、且 ratio >= 0.98 时才可能同步等待
- **可观测性**：`ContextMetrics` 追踪 token 使用率、压缩次数、释放量等指标；UI 状态栏反馈压缩进度

### 1.2 设计目标

1. **防溢出**：所有发给 LLM 的 prompt 估算 token 不超过安全水位，消除 context overflow
2. **语义完整**：被压缩的旧消息通过 LLM 结构化摘要保留核心语义（Goal / Constraints / Progress）
3. **信息保全**：超大 tool_result 落盘保全，不截断丢弃，未来可按需读回
4. **主线程零等待**：LLM 回复后绝不阻塞 UI；压缩通过异步预热 + 延迟应用实现
5. **主动降压**：ratio 水位线驱动的分级主动压缩，而非被动等 API 报错
6. **防御性兜底**：Layer 3 物理截断确保极端场景不崩溃
7. **可配置**：`context_window`、`max_output_tokens`、`compaction_model` 等均可在配置中覆盖
8. **可观测**：`ContextMetrics` 提供上下文健康度实时指标；UI 反馈压缩状态

---

## 2. 术语表


| 术语                     | 说明                                                                                                                                                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **user turn**          | 一条 `role=user` 消息 + 其后所有 `role=assistant` / `role=tool` 消息，直到下一条 `role=user`。上下文管理的最小粒度单位。                                                                                                                                |
| **context_window**     | 模型固有的最大上下文长度（输入 + 输出），由模型提供商决定（如 GPT-4o 128K, GPT-5.2 400K）。                                                                                                                                                              |
| **input_budget**       | 输入 token 预算，`context_window - max_output_tokens`。分母，用于计算 ratio。                                                                                                                                                           |
| **ratio**              | 上下文使用率，`estimated_token_count / input_budget`，取值 0.0 ~ 1.0+。驱动多级水位线触发。                                                                                                                                                    |
| **compactable zone**   | `userTurnsList` 中可被 Layer 0 占位符替换的区间 `[0, N-m)`，排除保护区。m 固定为 5。**仅适用于 Layer 0**。                                                                                                                                           |
| **protected zone**     | 最近 `m` 个 user turns（m=5），**不参与 Layer 0 占位符替换**。Layer 1 摘要压缩**整个 userTurnsList**，不受保护区限制。                                                                                                                                  |
| **m 值**                | 保护区大小，固定为 5。**仅影响 Layer 0** 占位符替换范围。                                                                                                                                                                                      |
| **preview 占位符**        | Layer 0 落盘后替换 tool_result 的短文本，包含路径 + 工具名 + 前 500 chars 预览。                                                                                                                                                               |
| **placeholder**        | Layer 0 替换旧 turn 中 tool_result 的常量文本 `[Previous tool result replaced to save context space]`。                                                                                                                             |
| **CompactionSummary**  | 多指 `AgentMessage::CompactionSummary`：摘要展平到消息列表中的形态。运行时 Layer 1 由 **`Preheat`** 封装 task、3× retry 与 `Idle`/`Running`/`ExhaustedPending`（及重载用的 **`CachedCompleted`**）；成功产物类型为 **`CompactionResult`**（文本 + 覆盖 id + **`transcript_compaction_entry_id`**），供 Layer 2 取出并应用。任务完成时 **追加一行** transcript compaction（`is_boundary=false`）作为持久化备份。 |
| **预热（Preheat）**        | Layer 1 的异步压缩任务。在 ratio >= 0.5 时启动，克隆 `userTurnsList` 后台调用 `compaction_model` 生成摘要，主线程不等待。                                                                                                                                |
| **Boundary 切换**        | Layer 2 从 **`preheat`** 取得已完成的 **`CompactionResult`** 并应用到 `userTurnsList`：清空被摘要覆盖的旧 turns，插入摘要消息，**更新内存中的 `start_idx`**（reasoning loop 的消息起始位置），并按 `transcript_compaction_entry_id` **原地**将 JSONL 中对应 compaction 行的 `isBoundary` 改为 `true`（不追加第二份全文）。切换后水位从 ~~70% 瞬降至 10~~20%。                                    |
| **compaction summary** | Layer 1 LLM 对**整个 `userTurnsList`** 生成的结构化摘要，一条消息替换整批 turns。                                                                                                                                                              |
| **compact boundary**   | `TranscriptEntry::Compaction` 中的 `is_boundary: bool` 标记。每个逻辑批次在 JSONL 中 **仅一行**：预热追加 `boundary=false`（fold 时跳过）；应用 **原地升级** 为 `boundary=true`（`init_context_state` 遇到后丢弃其前所有 entry）。重载时若最后一行 compaction 仍为 `false`，`init_context_state` 通过 **`restore_completed`** 将摘要 Hydrate 回 `Preheat`。                                                                     |
| **API Usage**          | LLM API 返回的 `usage` 字段（`prompt_tokens` + `completion_tokens`），用于精确 token 计数。                                                                                                                                              |


---

## 3. 核心架构图

### 图一：Token 计数与 Ratio 计算

```
  ════════════════════ Token 计数策略 ════════════════════

  方式 A（优先）：API Usage 精确计数
  ──────────────────────────────────────
  LLM 响应结束时，API 返回本次请求的 token 用量：
      → prompt_tokens  = 180,000  （本次请求的输入 token 数，API 精确值）
      → completion_tokens = 2,000  （本次 LLM 生成的输出 token 数，API 精确值）
      │
      │ prompt_tokens + completion_tokens = 下一轮请求的基线输入量
      │ （因为 LLM 的回复也会成为下一轮的历史消息）
      │
      │ 在 API 响应之后、下一次 LLM 调用之前，
      │ 可能追加了新消息（如 tool result），这部分没有精确 token 数，
      │ 只能用字符数 / 4 估算：
      │   post_usage_appended_chars = 12,000 chars → ~3,000 tokens
      ▼
  estimated_token_count = (prompt_tokens + completion_tokens)
                        + post_usage_appended_chars / 4
                        = (180,000 + 2,000) + 3,000
                        = 185,000

  注意：绝大部分 token 计数来自 API 返回的精确值（prompt_tokens + completion_tokens），
  字符数 / 4 仅用于估算「最近一次 API 响应之后新追加的消息」这一小段增量。

  方式 B（fallback）：字符启发式
  ──────────────────────────────────────
  首轮无 usage / Boundary 切换后旧 usage 失效时
      → estimated_token_count = estimate_context_chars / 4
  此模式仅短暂使用，等下一次 LLM 响应即可切回方式 A。


  ════════════════════ Ratio 与水位线 ════════════════════

  input_budget = context_window - max_output_tokens
               = 400,000 - 128,000 = 272,000 tokens（GPT-5.2）

  ratio = estimated_token_count / input_budget

    0%          50%     70%    85%           98%  100%    API Error
    ├───────────┼───────┼──────┼─────────────┼───┤         │
    │  正常区    │ L1    │ L2   │L1+L2        │L2 │         ▼
    │  无压缩    │预热   │请求前│回复后检查+   │请求前:    L3 物理截断
    │  (L0每轮)  │async  │检查  │请求前检查+   │同上+      │ 目标<0.50
    │           │       │      │可能启动新预热│sync wait  │

  注：Layer 0 每轮必跑（同步清理），不受 ratio 控制，图中省略。
      「LLM 回复后」= user turn 结束（reasoning loop 最终回复，所有 tool 已执行）。
      「发起 LLM 请求前」= 下一个 user turn 进入时。
      100% 水位本身不触发 Layer 3；Layer 3 仅在 API 明确返回 Context Overflow 错误时触发。
      LLM 回复后 ratio 允许暂时超过 0.98 甚至 >1.0（input_budget 已扣除 max_output_tokens，有余量）。
      仅在发起下一次 LLM 请求前，ratio >= 0.98 时才可能同步等待。
```

### 图二：四层防护流程

```
  User turn 结束：reasoning loop 最终回复（所有 tool 已执行，无 tool_calls）
  当前 turn 打包追加到 userTurnsList + 写入 transcript
      │
      ▼
  ┌─ Layer 0（每轮必跑，同步，纯内存操作极快）─────────────┐
  │  A. 单条 tool_result >= 50K chars？                    │
  │     → 落盘 + 500 chars preview 占位符                  │
  │  B. compactable zone (turn 0..N-5) 中                  │
  │     tool_result >= 10K chars？                         │
  │     → 占位符替换（不落盘）                             │
  │  C. 写入 transcript JSONL（新 message entry）           │
  │  D. 重新估算 tokens 用量和水位                          │
  └────────────────────────────────────────────────────────┘
      │
      ▼ 计算 ratio
      │
      ratio >= 0.50 且无进行中的异步任务？
      │  ──Yes──► 触发 Layer 1（异步，不等待）
      │
      ▼
  ┌─ Layer 1（异步预热，主线程不等待）─────────────────────┐
  │  1. 克隆当前 user_turns_list                           │
  │  2. 启动后台 Task：                                    │
  │     → 按模板压缩整个 user turn list（记录首尾 id）      │
  │     → 调用 compaction_model，限制 <= 10K tokens         │
  │     → 写入 transcript: type=compaction, boundary=false  │
  │  3. 产物 → CompactionResult（由 Preheat 持有至 Layer 2 消费）   │
  │  单例：后台只允许一个压缩任务                           │
  └────────────────────────────────────────────────────────┘
      │
      ▼ 主线程继续（不等待）
      │
      ratio >= 0.85？
      │  ──Yes──► Layer 2 - LLM 回复后检查（非阻塞）
      │            preheat 已有可应用结果？
      │              → Yes: 立即 Boundary 切换
      │              → No:  跳过，不等待
      │
      ▼ 当前 user turn 处理完毕
      ·
      · （用户阅读回复、思考、输入下一条消息）
      · （异步预热在此期间后台运行）
      ·
      ▼
  ┌─ 下一个 user turn 发起 LLM 请求前 ─────────────────────┐
  │  ratio >= 0.70？                                       │
  │    → try_restart_if_pending；preheat 已有结果则 Boundary 切换 │
  │                                                        │
  │  ratio >= 0.98？                                       │
  │    → Layer 2 - 发请求前检查：                          │
  │      已有结果？→ 直接 Boundary 切换                        │
  │      未完成？→ **化异步为同步**（await_result）           │
  │        阻塞等待摘要完成，再 Boundary 切换               │
  │        （阻塞的是推理启动，UI 已完成渲染）              │
  └────────────────────────────────────────────────────────┘
      │
      ▼ 发起 LLM 请求
      ·
      · （若 API 返回 Context Overflow 错误）
      ·
      ▼
  ┌─ Layer 3（物理截断，防御性兜底）───────────────────────┐
  │  从最旧 summary/turn 起逐条删除，直到 ratio < 0.50     │
  │  （几乎不可达的安全网）                                │
  └────────────────────────────────────────────────────────┘
```

### 图三：滑动窗口与保护区

```
  userTurnsList (内存中维护)：

  m = 5（固定，仅用于 Layer 0）:
  [turn_0] [turn_1] ... [turn_n-6] │ [turn_n-5] ... [turn_n-1]
  ◄──── compactable zone ─────────►│◄──── protected zone (5) ──►
  （仅 Layer 0 占位符替换适用此分区）

  Layer 0 占位符替换：作用于 compactable zone（turn 0..N-5）中 tool_result >= 10K 的消息
  Layer 1 异步预热：摘要覆盖**整个 userTurnsList**（记录首尾 id），不区分保护区
  Layer 2 Boundary 切换：用摘要替换 **CompactionResult** 覆盖范围内的 turns，保留之后新增的 turns
```

### 图四：异步预热与 Boundary 切换演进

```
  ════════════════════ 初始状态 ════════════════════

  [turn_0][turn_1]...[turn_9] [turn_10]...[turn_n-1]
  ◄──────────── 整个 userTurnsList ─────────────────►

  ════════════════════ ratio >= 0.50 → Layer 1 异步预热 ════════════

  后台 Task：克隆整个 user_turns_list → 调用 compaction_model
  → 生成 summary_A（Goal/Constraints/Progress...）
  → 追加 transcript: { type: compaction, is_boundary: false, id: ... }（持久化备份；每批次单行）
  → CompactionResult { summary_text: summary_A, covered: turn_0..turn_n-1, ... }（由 Preheat 暂存）

  主线程不等待，对话正常继续。

  ════════════════════ ratio >= 0.70 → 发起 LLM 请求前检查 ════════════

  preheat 已有可应用结果？
    → Yes: 执行 Boundary 切换（非阻塞）
           原地更新已存在 compaction 行: is_boundary: true（同一 id，不追加第二行）
    → No:  跳过，不阻塞

  ════════════════════ ratio >= 0.85 → LLM 回复后检查（⑤，非阻塞）════════════

  preheat 已有可应用结果？
    → Yes: 立即执行 Boundary 切换
           原地更新已存在 compaction 行: is_boundary: true

  [summary_A] [turn_new_1]...[turn_new_k]
  ◄─ 1 条摘要 ──► ◄── Layer 1 快照后新增的 turns ──►

  ratio 降回 ~10-20%（常规场景；若快照后有大量新增 turns，降幅可能不到位，
  会触发新一轮 Layer 1 预热）

  ════════════════════ ratio >= 0.98 → 发起 LLM 请求前强制检查（②）════════════

  try_restart_if_pending；已有结果？→ 直接 Boundary 切换
  未完成？→ 化异步为同步（await_result），再 Boundary 切换

  ════════════════════ 已有旧 summary 时 ════════════════════

  若后续 ratio 再次达 0.50，summary_A 与新 turn 均在 userTurnsList 中：
  Layer 1 使用 UPDATE 模式合并旧 summary（参考 UPDATE_SUMMARIZATION_PROMPT）
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

- `**last_api_usage**`：每次 LLM 响应结束后，从 `StreamEvent::Usage` 更新
- `**post_usage_appended_chars**`：自最近一次 API 返回 usage 之后，新追加到对话中的消息字符数（如 tool result、用户新消息等）。由于这些消息没有 API 精确 token 数，只能用 `字符数 / 4` 近似估算其 token 数，作为增量叠加到 API Usage 基线上
- **compact 后**：`last_api_usage` 失效（上下文已变），清零回退到字符 fallback，等下次 API 响应刷新

> **关于 `estimate_context_chars` 的度量单位**：Rust 的 `String::len()` 返回 UTF-8 字节数而非 Unicode 字符数。`CHARS_PER_TOKEN_ESTIMATE = 4` 对英文内容（1 byte ≈ 1 char）较为准确；对中文内容（3 bytes/char，约 1.5 token/char），4 bytes/token 的估算会偏保守（低估 token 数），可能导致压缩触发略晚。**API Usage 优先模式下此偏差被消除**，字符 fallback 仅在首轮和 compact 后短暂使用。

### 4.2 Ratio 水位线

`ratio = estimated_token_count / input_budget`，其中 `input_budget = context_window - max_output_tokens`。

分母是输入 token 预算（已扣除输出预留），ratio 衡量的是输入空间的使用率，不会挤占输出空间。


| ratio 档位         | 触发层       | 检查时机              | 动作                                                                      |
| ---------------- | --------- | ----------------- | ----------------------------------------------------------------------- |
| 每轮结束             | Layer 0   | LLM 回复后（⑤）        | 同步清理 tool_result（主线程同步但极快，用户无感知）                                          |
| `>= 0.50`        | Layer 1   | LLM 回复后（⑤）        | `try_restart_if_pending` → 异步预热 `preheat.try_start`（若无进行中的任务），主线程不等待                                                   |
| `>= 0.70`        | Layer 2   | **发起 LLM 请求前（②）** | `try_restart_if_pending` → 检查 `preheat` 结果，完成则 Boundary 切换（非阻塞）                               |
| `>= 0.85`        | Layer 1+2 | LLM 回复后（⑤）        | `try_restart_if_pending` → 先 `poll_result`：**已有结果则立即 Boundary 切换**（非阻塞）；再判断是否需要新一轮 `try_start`      |
| `>= 0.98`        | Layer 2   | **发起 LLM 请求前（②）** | `try_restart_if_pending` → **已有结果则直接 Boundary 切换**（非阻塞）；仅**未完成**时 `await_result` 化异步为同步阻塞等待 |
| Context Overflow | Layer 3   | API 返回错误后（③内）     | 物理截断至 ratio < 0.50                                                      |


**设计原则**：

- **LLM 回复后绝不阻塞主线程**，即使 ratio 暂时超过 0.98 甚至 >1.0 也只做 L0 清理和 L1 异步预热，不卡 UI
- 因为 `input_budget = context_window - max_output_tokens`，LLM 回复完成时实际还有 `max_output_tokens` 的空间余量，ratio 超过 1.0 不代表立即 Context Overflow
- **仅在发起下一次 LLM 请求前**才可能阻塞（L2 化异步为同步），此时 UI 已完成当前轮的渲染，阻塞的是推理启动而非 UI 交互

**触发总表**（含 Layer 0 细节）：


| 触发条件                                                      | 层级        | 时机           | 动作                                   |
| --------------------------------------------------------- | --------- | ------------ | ------------------------------------ |
| 单条 tool_result >= 50K chars                               | Layer 0   | ⑤            | 落盘 + 500 chars preview 占位符           |
| compactable zone (turn 0..N-5) 中 tool_result >= 10K chars | Layer 0   | ⑤            | 占位符替换（不落盘）                           |
| ratio >= 0.50 且 preheat 可启动                                   | Layer 1   | ⑤            | `try_restart_if_pending` → `preheat.try_start`（后台 Task，内 3× retry）                   |
| ratio >= 0.70                                             | Layer 2   | ② 发起 LLM 请求前 | `try_restart_if_pending` → `poll_result`/`apply`，完成则 Boundary 切换 |
| ratio >= 0.85                                             | Layer 1+2 | ⑤ LLM 回复后    | `try_restart_if_pending` → 非阻塞 `poll_result` + 切换 + 可能 `try_start`                 |
| ratio >= 0.98                                             | Layer 2   | ② 发起 LLM 请求前 | `try_restart_if_pending` → 已完成→切换；未完成→`await_result` 同步等待                      |
| API 返回 Context Overflow                                   | Layer 3   | ③ 内          | 物理截断至 ratio < 0.50                   |


### 4.3 配置项


| 配置项                                  | 类型       | 默认值         | 说明                                                                                                                                |
| ------------------------------------ | -------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `context_window`                     | `usize`  | `400_000`   | 默认对齐 **GPT-5.2**（400K）；其他模型请在配置中覆盖                                                                                                |
| `max_output_tokens`                  | `usize`  | `128_000`   | 默认对齐 **GPT-5.2** 单轮最大输出；对齐 API 的 `max_tokens`                                                                                     |
| `layer0_single_result_max_chars`     | `usize`  | `50_000`    | Layer 0 触发条件 A：单条 tool_result 超过此值则落盘 + preview 占位符                                                                               |
| `layer0_placeholder_threshold_chars` | `usize`  | `10_000`    | Layer 0 触发条件 B：compactable zone 中 tool_result 超过此值则占位符替换                                                                          |
| `compaction_model`                   | `String` | `"gpt-5.2"` | Compaction 摘要专用模型 ID（与主对话 `model` 可相同或不同）                                                                                         |
| `compaction_max_tokens`              | `usize`  | `10_000`    | Layer 1 异步预热生成摘要的 token 上限（预留）。当前**不设 API `max_tokens` 硬限制**以保证摘要语义完整性；仅在 prompt 中软引导 LLM 控制在 ~8K tokens 篇幅。未来若摘要频繁超标，可启用 API 硬限制 |


> 配置位于 `pi.config.toml` 的 `[context]` 节，或通过 `PrimitiveConfig` 结构体注入。

### 4.4 典型值


| 模型                | context_window | max_output_tokens | input_budget | ratio=0.50 时已用 |
| ----------------- | -------------- | ----------------- | ------------ | -------------- |
| GPT-4o            | 128,000        | 16,384            | 111,616      | 55,808         |
| GPT-5.2           | 400,000        | 128,000           | 272,000      | 136,000        |
| Claude 3.5 Sonnet | 200,000        | 8,192             | 191,808      | 95,904         |
| DeepSeek-V3       | 64,000         | 8,192             | 55,808       | 27,904         |


### 4.5 与旧方案对比

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
        preheat: Preheat::default(),  # Layer 1 异步预热状态机
    }
```

> **边界情况说明**：
>
> - **跨午夜会话**：用户 23:55 开始对话，重启后 `today()` 返回新日期。当天 turns 为空时，向前补全最近 10 条覆盖前一天的对话，不影响正确性。
> - **长期不活跃**：transcript 最后活跃在数天前，补全的 10 条为旧消息。上下文可能已不相关，但不影响正确性——后续新对话产生后旧 turns 会自然进入 compactable zone 被压缩。
> - 此策略优先保证**不丢失近期上下文**；上下文"相关性"由 Layer 1 摘要在运行过程中自然优化。

### 5.2 `Preheat` 与 `CompactionResult`

**`CompactionResult`**（预热成功时的产物，与 Layer 2 应用、`AgentMessage::CompactionSummary` 展平形态对应）：

```
struct CompactionResult {
    summary_text: String,
    covered_start_id: String,
    covered_end_id: String,
    covered_count: usize,
}
```

**`Preheat`**：封装 Layer 1 的完整状态机（内部 `Idle` / `Running` / `ExhaustedPending`，对调用方不可见）。`ContextState` 字段为 **`preheat: Preheat`**（非 `Option`）。

**对外方法**：

- `try_start(...)`：ratio 等条件满足且当前可启动时 spawn **唯一**后台 task；`Running` 或已有可应用的完成结果时不再重复启动。
- `try_restart_if_pending(...)`：在 **`ExhaustedPending`**（3× retry 耗尽）时，若条件仍满足则重新启动；与 §6.6、步骤 ⑤/② 双点调用配合。
- `poll_result()` / `await_result()`：供 Layer 2 非阻塞探测或发请求前同步等待。
- `abort()`：任意状态 → `Idle`，取消 task、清理 pending。

**生命周期**：Session 销毁或用户退出时应调用 **`preheat.abort()`**，等价于取消 `JoinHandle` 并复位状态；勿依赖仅 drop handle 来取消 task。

### 5.3 动态更新

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

> `invalidate_api_usage` 在 Boundary 切换后调用——上下文已变，旧 usage 不再有效。

### 5.4 system prompt 纳入估算

`estimateContextChars` 应包含 system prompt 的字符数。system prompt 在会话期间通常不变，初始化时计算一次即可：

```
estimate = system_prompt.len() + sum(today_turns.map(|t| estimate_turn_chars(t)))
```

> 若 system prompt 较短（< 5K chars），水位线已足够覆盖。但为准确性，仍建议显式计入。
> 若 system prompt 在会话中可能变化（如工具动态注册/卸载导致工具描述段变化），应在每轮 ② 构建 `messages` 时重新计算 system prompt 字符数并更新 `estimateContextChars`。

### 5.5 Session 重载与 Compact Boundary

从 transcript JSONL 加载 user turns 时，需识别 `SessionEntry::Compaction` entry 并处理 boundary 语义：

1. 遇到 `Compaction` entry 且 `is_boundary=true` → 作为 `SummaryTurn` 加入 `userTurnsList`，**丢弃其前**已暂存的所有 entry
2. 遇到 `Compaction` entry 且 `is_boundary=false` → **跳过**（这是预热阶段的备用记录，尚未被应用；**不在** `userTurnsList` 中生成 `SummaryTurn`）
3. 已被 Compaction 覆盖的原始 turns **不重复加载**
4. 后续 Layer 1 可直接定位已有 summary，进入 UPDATE 模式
5. **重载 Hydrate**：在 `fold_entries_to_turns` 与 `init_context_state` 使用的 **同一 entry 切片** 内，正向扫描维护「最后一条未应用 preheat」：遇 `is_boundary=false` 且摘要与 `covered_*` 齐全则更新；遇下一条 `is_boundary=true` 则清空。切片结束后若仍保留该 pending，且当前 `userTurnsList`（经日筛选后）仍含 `covered_end_id`，则调用 **`preheat.restore_completed`**，使下一轮 `poll_result` 与「任务刚完成」一致（无需再 spawn LLM）。

**Compact Boundary 处理（单行不变式）**：

```
Transcript 文件（JSONL；每个压缩逻辑批次仅一行 compaction）
═════════════════════════════════════════════

  entry 1~8:  原始消息（已被摘要覆盖）
  entry 9:    Compaction { id, summary: "...", is_boundary: false }  ← 预热追加
              … apply 成功后同一行原地改为 is_boundary: true（不追加第二行）
  entry 10~11: 新消息

init_context_state 处理流程：
  读到 entry 1~8 → 暂存
  读到 entry 9：若仍为 false → fold 跳过；pending_preheat → restore_completed
            若已 true → 丢弃暂存的 1~8，保留 summary
  读到 entry 10~11 → 构建 UserTurn

  结果: [SummaryTurn(该行), UserTurn(entry 10~11)]  （与运行时一致，无重复全文 compaction）
```

**被压缩的 user turn 是否仍留在 transcript JSONL 中？**

采用 **消息行仅追加、compaction 行可原地改写 `isBoundary`** 约定，与 pi 系 transcript 一致：

- **保留**：原先写入的 `Message` 行（user / assistant / tool）**不删除、不改写**，仍在 `.jsonl` 中，便于审计、回放与调试。
- **Compaction**：预热 **追加** 一行 `type: compaction`，`is_boundary=false`，并写入稳定 **`id`**（及可选 `preheatCompactionId`）；Boundary 切换时 **按 `id` 原地**将 `isBoundary` 置为 `true`（**不**再追加一条带全文摘要的新行）。开发阶段 **不** 向前兼容历史上「false 一行 + true 一行双份全文」JSONL。
- **构建 LLM 上下文**：`userTurnsList` / `build_context_messages` 在内存中按 Compaction 元数据 **折叠**——已摘要区间只表现为一条 summary，**不把同一区间的原始 Message 再次拼进 prompt**（避免双倍 token）。

若未来需要「物理瘦身」大文件，可作为独立运维能力（压缩归档副本），**不**作为默认行为。

### 5.6 `userTurnsList` 与现有消息结构的关系

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

- `**serde_json::Value**`：transcript JSONL 的原始 JSON 行，`build_context_messages` 从中读取。
- `**ChatMessage**`：LLM 请求/响应格式（`role` + `content` + `tool_calls`），由 `src/core/llm/types.rs` 定义。
- `**AgentMessage**`：agent loop 内部富类型（User / Assistant / ToolResult / System / **Steering** / **CompactionSummary**），比 `ChatMessage` 多出 `Steering`（用户中途注入指令）和 `CompactionSummary`（摘要）。

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
  │  ◆ 发起 LLM 请求前检查（详见下方 ⑤→② 循环）：                            │
  │    preheat.try_restart_if_pending(...)（② 补偿 ExhaustedPending）        │
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
  │ ③ reasoning loop 内（LLM ↔ 工具循环，可能多轮）                           │
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
  │  reasoning loop 内 messages 自由增长，不做压缩。                          │
  │  若 API 返回 Context Overflow → 触发 Layer 3 物理截断（见 §6.4）         │
  │  最终 LLM 回复（无 tool_calls）→ 退出 reasoning loop                    │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ④ user turn 完成：打包 + 持久化                                          │
  │                                                                         │
  │  当前 turn 内的全部 messages 打包：                                      │
  │    current_turn = UserTurn {                                            │
  │        messages: [User, Assistant, ToolResult, ..., Assistant(final)]    │
  │    }                                                                    │
  │  userTurnsList.push(current_turn)          ← 此时才追加                  │
  │  写入 transcript JSONL（各 Message entry）                               │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ⑤ LLM 回复后：上下文管理检查（绝不阻塞 UI）                               │
  │                                                                         │
  │  此时 userTurnsList 已包含刚完成的 turn。                                │
  │                                                                         │
  │  → preheat.try_restart_if_pending(...)（⑤ 与 ② 双点恢复）               │
  │  → Layer 0（同步清理 userTurnsList 中的 tool_result）                    │
  │  → 计算 ratio → 若 >= 0.50：`preheat.try_start(...)`（异步预热，不等待）   │
  │  → 若 ratio >= 0.85：Layer 2 回复后检查                                 │
  │    （preheat 已有结果？→ 立即 Boundary 切换；未完成→跳过）                 │
  └─────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
               用户阅读回复、思考、输入下一条消息
              （异步预热在此期间后台运行）
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │ ② 下一个 user turn 进入前（用户按下回车）                                 │
  │                                                                         │
  │  ◆ 发起 LLM 请求前检查：                                                │
  │    → preheat.try_restart_if_pending(...)                                │
  │    → 若 ratio >= 0.70：Layer 2 检查（完成则 Boundary 切换）              │
  │    → 若 ratio >= 0.98：Layer 2 发请求前检查                              │
  │      （完成→切换；未完成→化异步为同步，阻塞等待）                        │
  │                                                                         │
  │  userTurnsList.flatten() ──► Vec<AgentMessage>                          │
  │       + 注入 system prompt                                              │
  │       + 追加本轮 AgentMessage::User                                     │
  │       ──► initial_messages: Vec<AgentMessage>                           │
  │                                                                         │
  │  AgentLoop::run(initial_messages) → 进入 ③                              │
  └─────────────────────────────────────────────────────────────────────────┘
```

`**userTurnsList` 与 `messages` 的关系**：

- `**userTurnsList`**：管理**已完成的历史 turns**。只在 ② 进入前读取（flatten）、④ 结束后追加、⑤ 被 L0/L1/L2 修改。
- `**messages`**：reasoning loop 的**实时工作集**，包含历史 + 当前 turn 正在产生的新消息。每次进入 ② 时从 `userTurnsList` 重新构建。
- **估算更新**：`estimateContextChars` 在 ③ 每次 push 时实时累加，`last_api_usage` 在每次 LLM 响应后刷新。
- **上下文管理与 `messages` 无交集**：L0/L1/L2 在 ⑤ 操作 `userTurnsList`；reasoning loop 内的 `messages` 不受压缩影响。下一轮 ② 时 `messages` 从更新后的 `userTurnsList` 重建，自然包含 Boundary 切换后的摘要。

即：`**userTurnsList` 是持久化 transcript 与 `AgentMessage` 之间的中间层**——负责分组、Compaction 折叠与估算维护；最终转为 `AgentMessage` 后走已有的 `convert_to_llm_format` 链路，**不改变** reasoning loop 内部已有的消息流转方式。

---

## 6. 防护算法（Layer 0~3）

### 6.1 Layer 0：tool_result 清理（每轮同步）

在 user turn 完成后（步骤⑤，reasoning loop 已结束，当前 turn 已追加到 `userTurnsList`）立即执行，**不受 ratio 控制**。操作对象是 `userTurnsList`（包含刚完成的当前 turn）。合并了旧方案的 Layer 0 落盘和 Layer 1 占位符为一个同步步骤。

**步骤 A：大结果落盘**

单条 tool_result >= `layer0_single_result_max_chars`（默认 **50K chars**，~12.5K token）→ 落盘 + 500 chars preview 占位符。**先让 LLM 看到完整内容，再收纳落盘**——LLM 在本轮已正常分析和使用了完整结果，落盘是为了未来轮次的上下文不膨胀。

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

**步骤 B：compactable zone 占位符替换**

compactable zone（turn 0..N-5）中 tool_result >= `layer0_placeholder_threshold_chars`（默认 **10K chars**）→ 占位符替换（不落盘）。

```
const PLACEHOLDER: &str = "[Previous tool result replaced to save context space]";

fn compact_old_tool_results(state: &mut ContextState) -> usize:
    let m = 5
    let compactable_end = state.user_turns_list.len().saturating_sub(m)
    let mut reduced = 0

    for turn in state.user_turns_list[..compactable_end]:
        for msg in turn.messages where msg.role == Tool:
            if msg.content.len() >= 10_000:
                if msg.content.starts_with("[Tool result persisted:") || msg.content == PLACEHOLDER:
                    continue  # 已处理
                let before = msg.content.len()
                msg.content = PLACEHOLDER
                reduced += before - PLACEHOLDER.len()
                state.estimate_context_chars =
                    state.estimate_context_chars.saturating_sub(before - PLACEHOLDER.len())
    return reduced
```

**步骤 C：写入 transcript JSONL**

Layer 0 处理后的新 message entry 写入 transcript（落盘的 tool_result 以 preview 占位符形式写入，确保 transcript 中记录的是处理后的版本）。

**步骤 D：重新估算 tokens 和水位**

更新 `estimate_context_chars`，重新计算 `usage_ratio()`，供后续 Layer 1/2 触发判断。

**留 preview 的理由**：仅靠路径和工具名，LLM 在未来轮次无法判断内容是否与当前任务相关。500 chars 的 preview 成本极低（~125 token），但能帮助 LLM 决定是否需要按需读回。

### 6.2 Layer 1：异步预热

Layer 0 完成后（仍在步骤⑤），先 **`preheat.try_restart_if_pending(...)`**，再计算 ratio；若 **ratio >= 0.50** 且 `preheat.try_start(...)` 接受启动，则 spawn 异步预热。

**主线程不等待**，当前 user turn 处理完毕。异步预热在用户阅读/思考/输入期间后台运行。

```
fn layer1_preheat(state: &mut ContextState, llm: Arc<dyn LlmProvider>, config: &ContextConfig, ...):
    state.preheat.try_restart_if_pending(state, llm, config, ...)  # 与 ② 对称；见 §6.6

    if state.usage_ratio() < 0.50:
        return
    if state.user_turns_list.is_empty():
        return

    # try_start 内部：Idle 且无双任务时克隆 snapshot、spawn task；
    # task 内 generate_summary 最多重试 3 次（§6.6），成功发 AutoCompactionEnd，耗尽发 CompactionError 并转入 ExhaustedPending
    state.preheat.try_start(state, llm, config, ...)
```

**异步任务单例性**：

- 由 `Preheat` 内部状态保证：同一时间至多一个 `Running` task
- 防止 51%、52% 连续触发多个 Task 导致 token 浪费和竞态
- 任务结果被 Layer 2 `poll_result` / `await_result` 消费并 `apply` 后，`preheat` 回到可再次 `try_start` 的状态；**`ExhaustedPending`** 依赖 **`try_restart_if_pending`**（⑤ 与 ②）恢复

### 6.3 Layer 2：检查与应用

Layer 2 在 **两个时机** 检查预热结果是否可取用，对应步骤⑤和②；两时机均应先 **`preheat.try_restart_if_pending`**（与 §6.6 一致）。

#### LLM 回复后检查（ratio >= 0.85）

在 Layer 0 和 Layer 1 之后执行。**绝不阻塞主线程**——使用 **`poll_result()`**（或等价非阻塞路径），`Completed(result)` 则应用，否则跳过。

```
fn check_preheat_after_reply(state: &mut ContextState):
    state.preheat.try_restart_if_pending(...)
    if state.usage_ratio() < 0.85:
        return
    if let PreheatOutcome::Completed(_) = state.preheat.poll_result() {
        apply_boundary_switch(state)
    }
```

> 不区分 ratio 是否 >= 0.98——高水位时尽早非阻塞应用可减少下一轮 ② 发请求前同步等待的概率。

#### 发起 LLM 请求前检查（ratio >= 0.70）

```
fn check_preheat_before_request(state: &mut ContextState):
    state.preheat.try_restart_if_pending(...)
    let ratio = state.usage_ratio()
    if ratio < 0.70:
        return

    match state.preheat.poll_result() {
        PreheatOutcome::Completed(_) => apply_boundary_switch(state),
        PreheatOutcome::NotReady => {
            if ratio >= 0.98 {
                # 化异步为同步：await_result / 阻塞直至完成或失败
                if let PreheatOutcome::Completed(_) = state.preheat.await_result() {
                    apply_boundary_switch(state)
                }
            }
        }
        _ => {}
    }
```

#### Boundary 切换动作（两个检查时机共用）

```
fn apply_boundary_switch(state: &mut ContextState):
    let result = match state.preheat.poll_result() {
        PreheatOutcome::Completed(r) => r,
        _ => return,
    }
    # 消费结果后 preheat 内部回到 Idle（或等价可再 try_start）

    # 在 user_turns_list 中找到被覆盖的范围并替换
    # find_covered_range：若 start_id 因 Layer3 删前缀已不在列表而 end_id 仍在，则降级为 [0..=end]（并打 warn）
    let covered_range = find_covered_range(
        &state.user_turns_list,
        &result.covered_start_id,
        &result.covered_end_id,
    )
    let batch_chars = sum(state.user_turns_list[covered_range].map(|t| estimate_turn_chars(t)))
    let summary_chars = result.summary_text.len()

    state.user_turns_list.splice(covered_range, [SummaryTurn(result.summary_text.clone())])
    # 注意：使用 saturating_sub 防止 usize 下溢（累积估算误差可能导致 batch_chars > estimate）
    state.estimate_context_chars = state.estimate_context_chars.saturating_sub(batch_chars)
    state.estimate_context_chars += summary_chars

    invalidate_api_usage(state)

    # 按 transcript 行 id 原地将 isBoundary 改为 true（`Some(id)` → set_compaction_entry_is_boundary_true(path, id)；无 id 则 warn）

    # apply 路径从 preheat 取出并消费 CompactionResult，随后可再次 try_start
```

### 6.4 Layer 3：物理截断（防御性兜底）

API 返回 Context Overflow 错误时触发。从 `user_turns_list[0]`（最旧 summary/turn）起逐个删除，**直到 ratio < 0.50**。

```
fn force_delete_oldest(state: &mut ContextState):
    while state.usage_ratio() >= 0.50 && !state.user_turns_list.is_empty():
        let oldest = state.user_turns_list.remove(0)
        state.estimate_context_chars =
            state.estimate_context_chars.saturating_sub(estimate_turn_chars(&oldest))
    invalidate_api_usage(state)
```

**为什么目标是 0.50 而不是刚好 < 1.0**：若只降到 < 1.0，下一条消息或工具调用就可能再次触发 Layer 3，形成频繁振荡。删到 0.50 一次性创造充足缓冲，远低于 Layer 1 首次触发线（0.50），确保 Layer 3 触发后有足够的对话增长空间。

**设计定位**：几乎不可达的安全网。正常运行中，0.50 的 Layer 1 异步预热 + Layer 2 应用通常已足够将 ratio 降回 0.1~0.2。Layer 3 是最后兜底。

> **Layer 3 不受 m 值保护区约束**：当所有 turn 都在 protected zone 内（`compactable_end = 0`）时，Layer 0/1/2 无法工作。Layer 3 作为最后兜底，**必须能删除任何 turn**（包括 protected zone 内的），否则极端场景下无法降压。

### 6.5 防振荡设计

落盘后如果 LLM 再次全量读取同一文件，新 tool_result 仍可能超阈值、再次落盘，形成「读 → 落盘 → 再读 → 再落盘」的无效循环。

防范策略：

1. **分页读取引导**：system prompt 中明确告知 LLM「已落盘的工具结果可通过 `read_file` 的 offset/limit 参数按需读取指定行范围，无需全量读取」
2. **占位符自包含**：preview（前 500 chars）+ 来源工具名 + 参数 + 大小，让 LLM 有足够信息决定是否需要读回、读哪部分
3. **兜底保障**：即使 LLM 仍然全量读取，Layer 0 会再次正常落盘。流程上不会死循环（每轮仍正常推进），只是浪费了一次全量读取的 token。这属于 LLM 行为问题，通过优化 system prompt 引导来改善，不需要在代码层做硬拦截

### 6.6 异步预热失败处理（Preheat 状态机）

Layer 1 由 **`Preheat`** 封装，取代原先在 `ContextState` 上直接持有 `Option<CompactionSummary>` 的做法。

**内部状态（实现细节，不对外暴露）**：`Idle` / `Running` / `ExhaustedPending`。

**对外 API（仅此与预热交互）**：`try_start`、`try_restart_if_pending`、`poll_result`、`await_result`、`abort`。

**3× retry（在 spawn 的 task 内部）**：

- 对 `generate_summary` **最多连续尝试 3 次**。
- **成功**：发出 **`AutoCompactionEnd`**（L1 可观测性），返回 `CompactionResult`，供 Layer 2 Boundary 应用。
- **三次均失败（耗尽）**：发出 **`CompactionError`**，含 `exhausted_after_retries: true`、`attempts: 3`、`source: "preheat"` 等字段；task 以 **`Err`** 结束；状态转入 **`ExhaustedPending`**（不会在同一失败点自动再 spawn，需走恢复路径）。

**⑤ + ② 双点 `try_restart_if_pending`**：

- 时机 **⑤**（LLM 回复后）与时机 **②**（下一次发起 LLM 请求前）**都调用** `preheat.try_restart_if_pending(...)`。
- 这样即使 **⑤ 未执行到**（例如走了 **tool_calls** 分支、提前结束本轮），仍可在 **②** 补上恢复，避免长期卡在 `ExhaustedPending`。

**`abort`**：

`preheat.abort()` 将状态从 **任意** 状态收束到 **`Idle`**：取消运行中的 task、清除 pending / 未完成句柄，与 Session 销毁或用户中止时释放资源一致。

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

Keep each section concise. The entire summary should be under ~8K tokens.
Preserve exact file paths, function names, and error messages.
Prioritize actionable information over verbose descriptions.
```

### 7.2 Compaction 模型选择


| 策略               | 说明                                                         | 推荐        |
| ---------------- | ---------------------------------------------------------- | --------- |
| **默认：`gpt-5.2`** | 与主对话同代模型，摘要质量与长上下文能力一致；`compaction_model` 默认值为 `"gpt-5.2"` | **当前默认**  |
| 与主对话对齐           | 将 `compaction_model` 设为与 `model` 相同，行为与「全用主模型」一致           | 可选        |
| 轻量模型             | 如 `gpt-4o-mini` / DeepSeek-V3，成本低但需自行评估摘要质量                | 成本敏感时可改配置 |


> 实现上 Compaction 的 LLM 调用使用 `**compaction_model`**，与 `ChatRequest.model`（主对话）分离配置；若希望完全一致，将两项设为同一模型 ID 即可。

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
- The complete updated summary (which REPLACES the old one entirely) should be under ~8K tokens
- When the old summary is already large, compress older/less relevant details to stay within budget

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

摘要以 `Compaction` entry 写入 transcript JSONL（类型已在 [session-storage.md](session-storage.md) 的 `SessionEntry::Compaction` 定义）。**每个压缩批次仅一行 compaction**：

- **预热阶段**（Layer 1 异步任务完成时）：**追加**一行，`is_boundary: false`，分配行 **`id`**，写入 `CompactionResult.transcript_compaction_entry_id`；`init_context_state` fold 时**跳过**该行（不生成 `SummaryTurn`），并可 **`restore_completed`** 注入 `Preheat`。
- **应用阶段**（Layer 2 Boundary 切换时）：**原地**将该行的 `isBoundary` 改为 `true`（summary / `covered_*` 不变）。`init_context_state` 遇到 `is_boundary=true` 后**丢弃其前所有 entry**，使重启时重建结果与运行时一致。

在内存中作为一条 `role=user` 消息（content 为摘要文本）放入 `user_turns_list`，替换被压缩的原始 turns。

---

## 8. 超出本方案范围（Out of Scope）

以下机制在研究报告中分析过，但不纳入本方案实现：


| 机制                              | 说明                                                                                                | 后续计划                     |
| ------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------ |
| **Snip（中间段删除）**                 | CC Level 1，删除中间历史保留头尾，零 API 成本。当前 Layer 1 异步摘要可覆盖此场景。                                             | 若 Layer 1 触发过于频繁/费用高，再评估 |
| **Prompt Cache 管理**             | CC 的 `cache_control` / `cache_reference` / `cache_edits` 三原语为 Anthropic API 专属，OpenAI 自动缓存无需客户端配置 | 不适用                      |
| **Cached Microcompact**         | 依赖 `cache_edits` 服务端打洞能力                                                                          | 不适用                      |
| **Session Stability Latching**  | 锁定运行时状态防 cache bust，Pi 无 Prompt Cache                                                             | 不适用                      |
| **Context Collapse**            | CC 实验性 commit-log 视图投影，通用性差                                                                       | 不纳入                      |
| **工具循环检测（tool-loop-detection）** | openclaw 的滑窗重复检测 + steering 注入 + 熔断                                                               | 独立方案在 agent-loop 中实现     |
| **RAG 检索增强**                    | 旧消息向量化 + 按相关性检索注入                                                                                 | 长期方向                     |
| **System Prompt 自动注入**          | 从对话中自动提取约束/偏好到 system prompt                                                                      | 可与 Compaction 互补，后续独立方案  |


---

## 9. 涉及改动文件


| 文件                                                                                | 改动内容                                                                                                                                                                        |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `[src/core/session/manager/types.rs](../../../src/core/session/manager/types.rs)` | `ContextState` 持有 `preheat: Preheat`（Layer 1 状态机）、`start_idx` 等；`CompactionResult` 含 `transcript_compaction_entry_id`；`apply_boundary` 支持仅尾锚命中时的 `[0..=end]` 降级                                           |
| `[src/core/session/transcript.rs](../../../src/core/session/transcript.rs)` | `CompactionEntry` 可选 `preheatCompactionId`；`set_compaction_entry_is_boundary_true` 按 id 原地升级                                                     |
| `[src/core/agent_loop/run.rs](../../../src/core/agent_loop/run.rs)`               | reasoning loop 每轮 LLM 回复后：① Layer 0 同步清理 ② ratio check → Layer 1 异步预热 ③ 0.85<=r<0.98 时 Layer 2 回复后检查；发起 LLM 请求前：r>=0.70 时 Layer 2 检查、r>=0.98 时发请求前检查（可能同步等待）                |
| `[src/infra/config/types.rs](../../../src/infra/config/types.rs)`                 | `[context]` 配置节更新 `layer0_single_result_max_chars` 为 50K、新增 `layer0_placeholder_threshold_chars`（10K）、新增 `compaction_max_tokens`（10K）                                       |
| `[src/core/compaction/](../../../src/core/compaction/)`                           | 重构模块结构：`layer0.rs`（同步清理：落盘 + 占位符）、`preheat.rs`（异步预热：克隆整个 userTurnsList + 后台 Task + 写 transcript）、`apply.rs`（检查与应用：两个检查时机 + Boundary 切换）、`truncation.rs`（物理截断）               |
| `[src/core/system_prompt.rs](../../../src/core/system_prompt.rs)`                 | 新增分页读取引导 section                                                                                                                                                            |
| `[src/infra/events/mod.rs](../../../src/infra/events/mod.rs)`                     | 压缩可观测性：`AutoCompactionStart`/`End`、`CompactionError`、`BoundarySwitched`、`ContextOverflowTrimStart`/`End` 等（wire 名见 §10.4）                                                                 |
| `[src/core/context_metrics.rs](../../../src/core/context_metrics.rs)`             | `ContextMetrics` 结构体：`input_tokens_used`、`context_utilization_ratio`、`compaction_count`、`compaction_tokens_freed`、`total_tool_result_bytes_persisted`、`preheat_in_progress` |


---

## 10. 与其他模块的关联

### 10.1 Agent Loop（agent-loop.md §13.3）

Agent Loop 中有 **三个检查时机** 与上下文管理交互（对应 §5.6 步骤编号）：

- **⑤ LLM 回复后**（user turn 完成，绝不阻塞）：
  - `preheat.try_restart_if_pending(...)`（与 ② 双点恢复 ExhaustedPending）
  - Layer 0 同步清理 `userTurnsList` 中的 tool_result（落盘 + 占位符）
  - ratio check → `preheat.try_start(...)`（Layer 1 异步预热，不等待）
  - ratio >= 0.85 → Layer 2 回复后检查（`poll_result` 已有 `CompactionResult` 则立即切换，非阻塞）
- **② 发起下一次 LLM 请求前**（下一个 user turn 进入时）：
  - `preheat.try_restart_if_pending(...)`
  - ratio >= 0.70 → Layer 2 检查（完成则 Boundary 切换）
  - ratio >= 0.98 → Layer 2 发请求前检查（未完成则**化异步为同步**阻塞等待）
  - Boundary 切换后 `userTurnsList` 已更新，`messages` 从中重建
- **③ reasoning loop 内 API 返回 Context Overflow 错误**：
  - Layer 3 物理截断 + 重试

**容错重试循环（第二层）**：LLM 返回 ContextOverflow 错误时，发布 **`context_overflow_trim_start` / `context_overflow_trim_end`**（L3），驱动 Layer 3 物理截断与可选重试；异步预热进度仍由 L1 的 **`auto_compaction_*`** 表示。

### 10.2 会话存储（session-storage.md）

- Compaction 摘要以 `SessionEntry::Compaction` entry 类型写入 transcript JSONL（**每批次单行**）。
  - 预热阶段 **追加** `is_boundary: false`（含行 `id`）
  - 应用阶段 **原地**将该行改为 `is_boundary: true`（重启时生效；不追加第二份全文）
- Tool result 落盘文件存储在 `{work_dir}/agents/{session_id}/tool-results/` 目录。
- 初始化时从 transcript 流式读取 user turns（遵守「禁止全量加载」约定，使用 `BufReader` 逐行解析），识别 compact boundary，跳过 `is_boundary=false` 的预热记录。

### 10.3 配置管理（infrastructure-layer.md）

- `[context]` 配置节由 `PrimitiveConfig` 加载，支持 `pi.config.toml` 覆盖。
- 新增/更新 `layer0_single_result_max_chars`（50K）、`layer0_placeholder_threshold_chars`（10K）、`compaction_max_tokens`（10K）配置项。
- 不同模型可通过 `[model.<name>]` 节覆盖 `context_window` 和 `max_output_tokens`。

### 10.4 事件系统（events.md）

本模块发布的压缩相关事件按 **L1 / L2 / L3** 分层（Rust variant ↔ wire name）：


| Layer | Rust variants | wire names |
| ----- | ------------- | ---------- |
| **L1（异步预热）** | `AutoCompactionStart { covered_count, ratio_before }` / `AutoCompactionEnd { elapsed_ms, summary_chars, covered_count, ratio_after }` / `CompactionError { exhausted_after_retries, attempts, error, source, ratio }` | `auto_compaction_start` / `auto_compaction_end` / `compaction_error` |
| **L2（边界切换）** | `BoundarySwitched { ratio_before, ratio_after, covered_count, was_sync_wait }` | `boundary_switched` |
| **L3（溢出裁剪）** | `ContextOverflowTrimStart { reason, ratio }` / `ContextOverflowTrimEnd { ratio_before, ratio_after, will_retry }` | `context_overflow_trim_start` / `context_overflow_trim_end` |


其他与本模块相关的通用事件（未归入上表分层）：`tool_result_persisted`（Layer 0 落盘）、`context_metrics_update`（⑤ LLM 回复后指标刷新）等，见 `events` 模块定义。

**CLI / 宿主按 wire name 订阅时的语义对应**：

- `auto_compaction_start` / `auto_compaction_end` → **L1** 异步预热进度
- `compaction_error`（当 **`exhausted_after_retries == true`** 时）→ **待恢复**提示（需结合 ⑤/② 的 `try_restart_if_pending` 或用户操作）
- `context_overflow_trim_start` / `context_overflow_trim_end` → **L3** Context Overflow 后的物理裁剪
- `boundary_switched` → **L2** 摘要已应用、边界重置


### 10.5 UI 反馈


| ratio 档位            | UI 表现               |
| ------------------- | ------------------- |
| ratio >= 0.50 且预热中  | 状态栏转圈图标："后台准备压缩..." |
| Boundary 切换完成       | 闪过提示："上下文已重置"       |
| ratio >= 0.98 同步等待中 | 状态栏："等待压缩完成..."     |


