# Agent Loop 与 core 层说明 (core)

## 1. 概述 (Overview)

- **职责**：编排「用户输入 → LLM 流式调用 → 工具执行 → 结果回注 → 再调 LLM」的三层嵌套循环，支持 Steering（中途改向）、FollowUp（同上下文追问）、Abort（Ctrl+C 中断）、AgentEvent 全生命周期发布、错误分类与指数退避重试；与 **token-aware 上下文管理**、**四层 Compaction 防护**（`compaction.rs`）协同。
- **所在层级**：宿主核心能力层（`src/core/agent_loop.rs` 等），被 `src/api/chat.rs` 调用，依赖 `LlmProvider`、`PrimitiveExecutor`、`EventBus`。
- **核心文件**：
  - `src/core/agent_loop.rs` — AgentMessage、ToolCallInfo、convert_to_llm_format、agent_messages_from_chat、AgentLoopConfig、AgentLoop、LoopError
  - `src/core/compaction.rs` — 四层上下文防护算法（Layer 0~3）、Compaction Prompt 模板、context overflow 检测、`run_compaction_cascade`
  - `src/core/session/manager.rs` — TurnEntry、ContextState、init_context_state、build_context_from_state、estimate_turn_chars
  - `src/core/mod.rs` — core 层 re-export
  - `src/lib.rs` — 对外导出 AgentLoop、AgentLoopConfig、AgentRunResult、AgentMessage、ContextState、TurnEntry、ContextConfig 等

### 1.1 三层嵌套循环 + 干预点（ASCII）

```text
Layer 1  Conversation Loop
    |     FollowUp 队列非空 -> 注入 User 再继续
    v
Layer 2  Attempt Loop (max_attempts, 指数退避)
    |     ContextOverflow 检测 -> 触发 Layer 1~3 Compaction -> 重试
    v
Layer 3  Reasoning Loop
    |     LLM 流式 -> tool_calls?
    |     +-- 执行工具 -> Layer 0 截断超大 ToolResult -> ToolResult 回注
    |     +-- Steering 队列 -> 改向，跳过后续工具
    |     +-- Abort 信号 -> 中断
    |     +-- ContextState 动态估算更新
    v
  final_text (由调用方决定是否写 Session)
```

- **消息边界**：内部 `AgentMessage`，调用 `LlmProvider` 前经 `convert_to_llm_format` 转为 `ChatMessage`（与 [agent-loop 规格](../../openspec/specs/architecture/agent-loop.md) 13.4 一致）。
- **总览**：与 [src 模块索引](../README.md)「图 1」中 `core/agent_loop` 位置对照。

## 2. 设计方案 (Design Details)

- **设计模式**：三层嵌套循环（Conversation → Attempt → Reasoning），职责分离；内部使用富类型 `AgentMessage`，仅在调 LLM 边界通过 `convert_to_llm_format` 转为 `ChatMessage`，与 [agent-loop.md](../../openspec/specs/architecture/agent-loop.md) 13.4 消息类型边界一致。
- **关键权衡**：System Prompt 与工具定义由**调用方**（如 chat）拼装并注入：AgentLoop 只接受已拼好的 `initial_messages`（含首条 System 若需要）和构造时传入的 `config.tool_definitions`，不在 Loop 内再拼 system，便于多调用方复用同一 Loop 逻辑。Transcript 持久化由调用方在 `run()` 返回后根据 `AgentRunResult.final_text` 自行 append 并写入 Session，AgentLoop 不依赖 SessionManager。
- **线程安全/并发**：`steering_queue`、`follow_up_queue` 为 `Arc<Mutex<Vec<AgentMessage>>>`，`abort_signal` 为 `Arc<AtomicBool>`；`steer()`、`follow_up()`、`abort()` 可从其他线程调用，`run()` 内读队列与信号，无数据竞争。`run()` 需 `&mut self` 因持有 `on_stream_delta: Option<Box<dyn FnMut(&str) + Send>>`。

## 3. 核心 API 与数据结构 (API Definitions)

- **AgentMessage**：Agent 内部富类型消息；变体包括 `User`、`Assistant`（含 `tool_calls: Vec<ToolCallInfo>`）、`ToolResult`（含 `is_error`）、`System`、`Steering`（含 `timestamp`）、`CompactionSummary`。
- **ToolCallInfo**：`{ id, name, arguments }`，与 LLM 流式 tool_calls 对应。
- **convert_to_llm_format(messages: &[AgentMessage]) -> Vec<ChatMessage>**：将 AgentMessage 序列转为 LlmProvider 使用的 ChatMessage；User/Steering/CompactionSummary → user，System → system，Assistant 按有无 tool_calls 分别转为 assistant 或 assistant_with_tool_calls，ToolResult → tool。
- **agent_messages_from_chat(messages: &[ChatMessage]) -> Vec<AgentMessage>**：反向转换，供 chat 从 Session 加载历史后拼装 `initial_messages`。
- **AgentLoopConfig**：`max_attempts`（默认 3）、`max_tool_rounds`（默认 `usize::MAX`，由 token 预算与工具轮次逻辑兜底）、`retry_base_delay_ms`（默认 300）、`model`、`session_id`、`tool_definitions: Vec<serde_json::Value>`（由调用方 `build_tool_definitions()` 等生成）、`context_config`。
- **AgentRunResult**：`{ final_text: String }`，run 成功时最后一轮 LLM 文本回复。
- **AgentLoop::new(llm, primitive, event_bus, config, abort_signal)**：标准构造函数；内部创建默认的 steering_queue、follow_up_queue。
- **AgentLoop::run(&mut self, initial_messages: Vec<AgentMessage>) -> Result<AgentRunResult, AppError>**：主入口；执行第一层 Conversation Loop（含 FollowUp 检查）、第二层 Attempt Loop（重试与 classify_error）、第三层 Reasoning Loop（LLM 流式 + 工具执行 + Steering/Abort 检查）。
- **AgentLoop::steer(&self, msg: String)**：向 steering_queue 推入 `AgentMessage::Steering { text, timestamp }`；第三层每工具执行完后检查，非空则注入并跳过剩余工具进入下一轮 LLM。
- **AgentLoop::follow_up(&self, msg: String)**：向 follow_up_queue 推入 `AgentMessage::User { text }`；第一层循环尾部检查，非空则 drain 追加到 messages 并 continue。
- **AgentLoop::abort(&self)**：将 `abort_signal` 置为 true；第三层每工具执行前检查，为 true 则返回 `Err` 并发布 agent_end(interrupted)。
- **AgentLoop::set_on_stream_delta(&mut self, f)**：设置流式 delta 回调，供 chat 做 Markdown 渲染等。
- **LoopError**（内部）：`Retryable(String)`、`Fatal(String)`、`Aborted`；`classify_error(AppError)` 将 429/5xx/超时/请求失败等归为 Retryable，401/400 归为 Fatal。
- **compact_messages(messages, keep_recent)**：（已废弃）MVP 压缩。由 ContextState + 四层防护替代。

### 3.2 上下文管理 API（TASK-17）

- **ContextState**：运行时上下文状态，包含 `user_turns_list: Vec<TurnEntry>`、`estimate_context_chars: usize`、`context_budget_chars: usize`。在 `chat_loop` 外层初始化一次、跨迭代复用。
- **TurnEntry**：上下文分组单位——`UserTurn { messages: Vec<AgentMessage> }` 或 `SummaryTurn { summary: String }`。
- **init_context_state(session, config, system_text) -> ContextState**：从 transcript 加载历史，按 user turn 分组，识别已有 Compaction entry 折叠为 SummaryTurn。
- **build_context_from_state(state) -> Vec<AgentMessage>**：将 ContextState 的 turns 展平为 AgentMessage 列表。
- **ContextConfig**：上下文管理配置，含 `context_window`、`max_output_tokens`、`compaction_turns`、`keep_recent_turns`、`single_tool_result_max_chars`、`compaction_model`。

### 3.3 四层防护算法（`compaction.rs`）

| Layer | 函数 | 机制 | 触发条件 |
|-------|------|------|----------|
| 0 | `truncate_tool_result_if_needed` | 单条 tool result 超限截断（Unicode 安全） | 每次 tool 执行完毕后 |
| 1 | `compact_tool_results` | 可压缩区内旧 tool result 替换为占位符 | `estimateContextChars > budget` |
| 2 | `run_compaction_loop` | LLM 循环摘要（结构化 Compaction） | Layer 1 后仍超预算 |
| 3 | `force_drop_oldest` | 强制删除最旧 turn（防御性兜底） | Layer 2 后仍超预算 |

- **is_context_overflow_error(err)**：检测 LLM 错误是否为 context overflow（含 "context" + "length"/"token"/"limit"）。
- **Compaction Prompts**：`SUMMARIZATION_PROMPT`（首次摘要）和 `UPDATE_SUMMARIZATION_PROMPT`（增量合并已有摘要）。

## 4. core 层其它子模块（索引）

以下模块不单独拆 README，职责与主文件如下：

| 模块 | 主文件 | 职责摘要 |
|------|--------|----------|
| `executor` | `executor.rs` | `DefaultPrimitiveExecutor`，4 原语执行 |
| `primitives` | `primitives.rs` | 原语类型与 `PrimitiveExecutor` trait |
| `tools` | `tools.rs` | `Tool`/`ToolRegistry`/`DefaultToolRegistry` |
| `confirmation` | `confirmation.rs` | `UserConfirmationProvider`（允许/拒绝/交互） |
| `system_prompt` | `system_prompt.rs` | 系统提示拼装辅助 |
| `session` | `session/` | 见 [session/README.md](./session/README.md) |
| `llm` | `llm/` | 见 [llm/README.md](./llm/README.md) |

## 5. 配置项 (Configuration)

### 5.1 AgentLoopConfig

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| max_attempts | u32 | 3 | 第二层 Attempt 最大重试次数（含首次） |
| max_tool_rounds | usize | usize::MAX | 单次 Attempt 内第三层最大工具轮次（不再硬限，由 token 预算兜底） |
| retry_base_delay_ms | u64 | 300 | 指数退避基准延迟（ms），实际 delay = base × 2^(attempt-1) |
| model | String | — | LLM 模型名，由调用方从 Session/Config 填入 |
| session_id | String | — | 会话 ID，随 AgentEvent 发布 |
| tool_definitions | Vec<serde_json::Value> | [] | 传入 LLM 的工具 JSON Schema |
| context_config | ContextConfig | ContextConfig::default() | 上下文管理配置 |

### 5.2 ContextConfig（`[context]` 配置节）

| 字段 | 类型 | 默认值 | 环境变量 | 说明 |
|------|------|--------|----------|------|
| context_window | usize | 400,000 | `PI_CONTEXT_WINDOW` | 模型 context window（tokens） |
| max_output_tokens | usize | 128,000 | `PI_MAX_OUTPUT_TOKENS` | 模型最大输出（tokens） |
| compaction_turns | usize | 10 | — | 每批 Compaction 的 turn 数上限 |
| keep_recent_turns | usize | 3 | — | 保护区 turn 数（不参与压缩） |
| single_tool_result_max_chars | usize | 400,000 | — | Layer 0 单条 tool result 截断阈值（chars） |
| compaction_model | String | 与主模型相同 | — | Compaction LLM 调用使用的模型 |

预算公式：`contextBudgetChars = (context_window - max_output_tokens) × 4 × 0.75`（GPT-5.2 默认 = 816,000 chars）。

## 6. 交互流程 (Workflow)

```mermaid
flowchart TD
    Caller["调用方 chat_loop"]
    Run["AgentLoop::run(initial_messages)"]
    Conv["第一层 Conversation Loop"]
    Attempt["第二层 Attempt Loop"]
    Reason["第三层 Reasoning Loop"]
    LLM["llm.chat_stream"]
    Tools["execute_tool 循环"]
    SteeringCheck["steering_queue 非空?"]
    FollowUpCheck["follow_up_queue 非空?"]
    Persist["调用方持久化 Transcript"]

    Caller --> Run
    Run --> Conv
    Conv --> Attempt
    Attempt --> Reason
    Reason --> LLM
    LLM --> Tools
    Tools --> SteeringCheck
    SteeringCheck -->|是| Reason
    SteeringCheck -->|否| Reason
    Reason -->|Ok final_text| FollowUpCheck
    FollowUpCheck -->|是| Conv
    FollowUpCheck -->|否| Persist
    Persist --> Caller
```

- 第一层：处理用户输入与 FollowUp；每次循环开始注入 steering_queue 中已有消息；Attempt 成功后在循环尾检查 follow_up_queue，非空则 drain 追加后 continue，否则 return。
- 第二层：按 attempt 计数，Retryable 错误时指数退避后重试，Fatal 或 Aborted 则终止并返回 Err。**ContextOverflow 检测**：若错误匹配 `is_context_overflow_error`，触发 Layer 1→2→3 Compaction 后以压缩后的上下文重试。
- 第三层：turn_start → chat_stream → message_start/update/end → 若有 tool_calls 则逐个 execute_tool（**Layer 0 截断**检查），每工具前检查 abort、每工具后检查 steering_queue；同时**动态更新** `ContextState.estimate_context_chars`。

### 6.1 上下文管理集成流程（TASK-17）

```text
  chat_loop (api/chat.rs)
      |
      v  init_context_state() ← 从 transcript 重建 ContextState（仅首次）
      |
      v  每轮用户输入:
      |    1. 更新 estimateContextChars（新消息）
      |    2. is_over_budget? → 预飞 Layer 1~3 Compaction
      |    3. build_context_from_state → messages
      |    4. set_context_state → AgentLoop
      |
      v  AgentLoop::run()
      |    - Layer 0: 每次 tool 执行后截断超大 result
      |    - Layer 2 Attempt: ContextOverflow → Layer 1~3 → 重试
      |    - 动态维护 estimate_context_chars
      |
      v  take_context_state ← 取回 ContextState
      |    - on_new_user_turn（追加本轮 turn）
      |    - 下一轮继续使用同一 ContextState
```

## 7. 示例代码 (Usage Examples)

chat 层构造并调用 AgentLoop 的典型片段（见 `src/api/chat.rs`）：

```rust
let messages = agent_messages_from_chat(&chat_messages); // 从 Session 历史 + 当前用户消息
let config = AgentLoopConfig {
    max_attempts: 3,
    max_tool_rounds: 10,
    retry_base_delay_ms: 300,
    model: model.clone(),
    session_id: ctx.session.current_session_key().to_string(),
    tool_definitions: build_tool_definitions(),
};
let mut agent_loop = AgentLoop::new(
    ctx.llm.clone(),
    ctx.primitive.clone(),
    ctx.event_bus.clone(),
    config,
    ctx.cancelled.clone(),
);
agent_loop.set_on_stream_delta(Box::new(move |delta: &str| { /* 渲染 delta */ }));

match agent_loop.run(messages).await {
    Ok(result) => {
        // AgentLoop 不负责写入 Session；调用方自行 append 并持久化
        if !result.final_text.is_empty() {
            let assistant_msg = ChatMessage::assistant(&result.final_text);
            ctx.session.append_message(serde_json::to_value(&assistant_msg)?)?;
        }
    }
    Err(e) => return Err(e),
}
```

## 8. 验收标准 (Testing & QA)

- **单测**：`cargo test -j 1 --lib -- --test-threads=1` 全通过；覆盖 `core::agent_loop`、`core::compaction`、`infra::config`（含 ContextConfig）、`core::session::manager`（init_context_state 等）。
- **集成**：`tests/context_management_tests.rs` — 端到端上下文与 Compaction 场景；`context_management.md` / User_Stories Story 8 对齐。
- **门禁**：`cargo clippy --all-targets -- -D warnings` 无警告。
- **事件**：agent_start、turn_start/end、message_start/update/end、tool_execution_start/end、tool_call/tool_result、auto_retry_start/end、**auto_compaction_start/end**、**tool_result_truncated**、**compaction_error**、agent_end(success|error|interrupted)。
