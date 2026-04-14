# Tool 消息链违规根因报告

> **⚠️ 过时提示（2026-04）**：本文中提及的 `AgentMessage`、`convert_to_llm_format`、`TurnEntry` 等类型已在 `feature/collapse-to-chatmsg` 分支中删除。消息链校验现在直接在 `ChatMessage` 层面操作。现行架构见 [collapse-to-chatmsg-guide.md](./collapse-to-chatmsg-guide.md)。

> 创建：2026-04-04
> 状态：根因已定位，修复待实施
> 关联错误：`OpenAI API 400: messages with role 'tool' must be a response to a preceeding message with 'tool_calls'`

---

## 1. 现象

用户在 `pi chat` 多轮对话中，执行过包含工具调用的请求后，后续请求稳定报错：

```
API 错误 400: {
  "error": {
    "message": "Invalid parameter: messages with role 'tool' must be a response
               to a preceeding message with 'tool_calls'.",
    "param": "messages.[79].role"
  }
}
```

复现条件：前序轮次触发了 proactive compaction（上下文压缩），之后所有请求均 400。

## 2. 违规样本

来自 transcript `1774521308274_47a17397cf2478e2.jsonl` 第 79-81 行：

```
JSONL 行号   role              tool_call_id / tool_calls
─────────   ──────────────    ─────────────────────────
   78       assistant         (纯文本，无 tool_calls)    ← 前一轮的最终回复
   79       user              —                          ← 本轮用户输入
   80       tool              call_npMgy...              ← 违规！前一条是 user
   81       assistant         (分析 compaction.rs)       ← 本轮最终回复
```

OpenAI 的规则：`role: tool` 的紧邻前驱必须是「带非空 `tool_calls` 的 `assistant`」或另一条 `tool`。
第 80 行前面是 `user`（第 79 行），违规。

缺失的消息：在第 79（user）和第 80（tool）之间，应有一条 `assistant` + `tool_calls`（LLM 请求读取 compaction.rs）。该消息**在内存中存在过**，但**未被写入 transcript**。

## 3. 根因：proactive compaction 重建 messages 后 start_idx 过期

### 3.1 正常流程（无 compaction 时）

```
chat.rs                          AgentLoop::run()
────────                         ─────────────────
messages = build_from_ctx_state()
messages.insert(0, System)
messages.push(User)
append_message(user) → JSONL
                                 start_idx = messages.len()  ← 比如 79
                                 ┌─ reasoning loop ──────────────────────┐
                                 │ LLM call → assistant+tc  → msgs[79]  │
                                 │ tool exec → ToolResult   → msgs[80]  │
                                 │ LLM call → assistant     → msgs[81]  │
                                 └───────────────────────────────────────┘
                                 new_messages = msgs[79..82] (取79 80 81)
                                   = [Asst+tc, ToolResult, Asst]  ✓ 合法

convert_to_llm_format(new_msgs)
for msg: append_message → JSONL   写入: Asst+tc, Tool, Asst  ✓
```

### 3.2 Bug 流程（触发 proactive compaction 时）

下面用 ASCII 图完整还原 bug 时序。假设初始 messages 有 79 条。

```
                    AgentLoop::run()
                    ════════════════
                    messages = initial_messages (len=79)
                    start_idx = 79          ← 固定不变，这就是 bug

    ┌─── reasoning loop ─────────────────────────────────────────────────┐
    │                                                                     │
    │  Step 1: LLM 调用                                                   │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ LLM 返回 assistant + tool_calls(读 compaction.rs)            │   │
    │  │ messages.push(Asst+tc) → messages[79]                        │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    │  Step 2: 工具执行                                                    │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ 执行 read_file(compaction.rs) → 1228 行                      │   │
    │  │ messages.push(ToolResult) → messages[80]                     │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    │  Step 3: proactive compaction 触发                                   │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ ctx_state 中历史 turn 有大量文件读取结果                        │   │
    │  │ Layer 1: 替换旧 turn 中 >20K 的 tool result 为占位符           │   │
    │  │ layers_executed = [1] → 非空 → 触发 messages 重建             │   │
    │  │                                                              │   │
    │  │  *** 关键操作 ***                                             │   │
    │  │  *messages = build_context_from_state(ctx_state)              │   │
    │  │                                                              │   │
    │  │  ctx_state 不含当前轮：                                       │   │
    │  │    - 不含 User("帮我指出来")                                   │   │
    │  │    - 不含 Asst+tc (读 compaction.rs)                          │   │
    │  │    - 不含 ToolResult (compaction.rs 内容)                      │   │
    │  │                                                              │   │
    │  │  重建后 messages = [历史 turns] ≈ 77 条                        │   │
    │  │  messages.insert(0, System) → len = 78                        │   │
    │  │                                                              │   │
    │  │  start_idx 仍然 = 79，但 messages 只有 78 条!                  │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    │  messages 重建前后对比：                                              │
    │                                                                     │
    │  重建前 (len=81):                                                    │
    │  ┌────┬──────────────────────┬──────────────────┬──────────────┐    │
    │  │ 0  │ 1 ··· 78            │ 79               │ 80           │    │
    │  │Sys │ 历史消息              │ Asst+tc(读文件)  │ ToolResult   │    │
    │  └────┴──────────────────────┴──────────────────┴──────────────┘    │
    │                                  ↑ start_idx=79                     │
    │                                                                     │
    │  重建后 (len=78):                                                    │
    │  ┌────┬──────────────────────┐                                      │
    │  │ 0  │ 1 ··· 77            │  ← 当前轮全部丢失！                    │
    │  │Sys │ 压缩后的历史消息       │                                      │
    │  └────┴──────────────────────┘                                      │
    │                                  ↑ start_idx 仍=79（越界）           │
    │                                                                     │
    │  Step 4: reasoning loop 继续 — LLM 再次被调用                         │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ LLM 看到旧历史（最后一条是上一轮的 assistant 回复）             │   │
    │  │ 上一轮 assistant 末尾写了"你要我继续读这个文件吗？"             │   │
    │  │ LLM 决定读 compaction.rs → 返回新的 assistant+tool_calls      │   │
    │  │                                                              │   │
    │  │ messages.push(Asst+tc) → messages[78]   ← 注意：78 < 79!     │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    │  Step 5: 工具再次执行                                                │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ messages.push(ToolResult) → messages[79]  ← 刚好 = start_idx │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    │  Step 6: LLM 最终回复                                                │
    │  ┌──────────────────────────────────────────────────────────────┐   │
    │  │ messages.push(Assistant) → messages[80]                      │   │
    │  └──────────────────────────────────────────────────────────────┘   │
    │                                                                     │
    └─────────────────────────────────────────────────────────────────────┘

    返回: new_messages = messages[start_idx..] = messages[79..81]

    messages 最终状态:
    ┌────┬──────────────────────┬──────────┬───────────┬───────────┐
    │ 0  │ 1 ··· 77            │ 78       │ 79        │ 80        │
    │Sys │ 压缩后的历史          │ Asst+tc  │ ToolResult│ Assistant │
    └────┴──────────────────────┴──────────┴───────────┴───────────┘
                                    ↑           ↑
                                    │           └── start_idx = 79（切片起点）
                                    │
                                    └── index 78 < 79，被 start_idx 切掉了！

    new_messages = [ToolResult, Assistant]   ← 缺少 Asst+tc！
```

### 3.3 写入 transcript 的后果

```
chat.rs 成功分支：

    // 已经写过：
    JSONL 行 79: {"role":"user", "content":"帮我指出来"}

    // 现在写入 new_messages（缺少 Asst+tc）：
    let chat_msgs = convert_to_llm_format(&result.new_messages);
    for msg in &chat_msgs {
        session.append_message(msg);   // 逐条追加
    }

    JSONL 行 80: {"role":"tool", "tool_call_id":"call_npMgy..."}   ← 违规！
    JSONL 行 81: {"role":"assistant", "content":"...分析..."}

    结果：transcript 中 user → tool，缺少 assistant+tool_calls
```

### 3.4 下次请求 400

```
    下一次用户输入 "帮我读 dispatcher.rs"：

    init_context_state() 从 transcript 忠实还原 →
    build_context_from_state() 展平 →
    convert_to_llm_format() →

    发给 OpenAI 的 messages:
    ┌─────┬──────────────────┬──────┬──────┬───────────┬──────┐
    │ ... │ [78] assistant   │ [79] │ [80] │ [81]      │ [82] │
    │     │ (纯文本)          │ user │ tool │ assistant │ user │
    └─────┴──────────────────┴──────┴──────┴───────────┴──────┘
                                       ↑
                                       OpenAI: "messages.[79+1].role = tool,
                                       但前一条是 user，违规！" → 400
```

## 4. 涉及代码

### 4.1 bug 所在

`agent_loop.rs` 第 513 行：

```rust
let start_idx = messages.len();   // 只在 run() 入口赋值一次
```

但 `messages` 在以下两处被完全重建（`start_idx` 未更新）：


| 位置                                          | 行号   | 触发条件                       |
| ------------------------------------------- | ---- | -------------------------- |
| `run_reasoning_loop` 中 proactive compaction | ~935 | 工具执行后 cascade 任一层执行        |
| `run_attempt_loop` 中 overflow retry         | ~617 | LLM 返回 context overflow 错误 |


### 4.2 为什么 ctx_state 不含当前轮

`ctx_state` 的当前轮只在 `chat.rs` 成功分支才追加：

```rust
// chat.rs 约 442 行，AgentLoop::run 返回之后
context_state.on_new_user_turn(current_turn);
```

所以 AgentLoop 执行期间，`ctx_state.user_turns_list` 只有历史轮次。
`build_context_from_state(ctx_state)` 重建的 messages 不含当前轮的任何消息。

### 4.3 为什么 transcript 无 type:compaction 但仍触发重建

Layer 0（大 tool result 落盘）和 Layer 1（>20K 占位符替换）均**不写 BranchSummaryEntry 到 transcript**。
只有 Layer 2（LLM 摘要）才写。因此 transcript 中没有 `type: compaction`，但 Layer 0/1 仍可触发
`layers_executed` 非空，进而执行 `*messages = build_context_from_state(ctx_state)` 重建。

## 5. 修复方向

在每次 `*messages = build_context_from_state(...)` 重建后，将 `start_idx` 更新为
`messages.len()`，确保后续 `messages[start_idx..]` 能正确捕获重建后所有新增消息。

具体做法：将 `start_idx` 从 `run()` 的局部变量改为通过 `&mut usize` 传递给
`run_attempt_loop` 和 `run_reasoning_loop`，在两处重建后更新。

## 6. 已加入的诊断日志

作为排查辅助，已在以下位置加入了消息链校验日志（`[chain_violation]` 前缀）：


| 打点  | 文件              | 位置                                    | 用途      |
| --- | --------------- | ------------------------------------- | ------- |
| A   | `openai.rs`     | `chat_stream` / `chat_inner` 发 POST 前 | 发送前检测违规 |
| B   | `chat.rs`       | `append_message` 循环前                  | 落盘前检测违规 |
| C   | `agent_loop.rs` | 两处 `build_context_from_state` 后       | 重建后检测违规 |
| D   | `dispatcher.rs` | 三处 `append_message`                   | 插件路径审计  |


