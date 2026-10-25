# Agent Loop 与 core 层说明 (core)

## 1. 概述 (Overview)

- **职责**：编排「用户输入 → LLM 流式调用 → 工具执行 → 结果回注 → 再调 LLM」的三层嵌套循环，支持 Steering（中途改向）、FollowUp（同上下文追问）、Abort（Ctrl+C 中断）、AgentEvent 全生命周期发布、错误分类与指数退避重试；与 **token-aware 上下文管理**、**四层 Compaction 防护**（`compaction.rs`）协同。
- **所在层级**：宿主核心能力层（`src/core/agent_loop/` 等），被 `src/api/chat.rs` 调用，依赖 `LlmProvider`、`PrimitiveExecutor`、`EventBus`。
- **核心文件**：
  - `src/core/agent_loop/` — ToolCallInfo、AgentLoopConfig、AgentLoop、LoopError
  - `src/core/compaction/` — 四层上下文防护算法（Layer 0~3）、Compaction Prompt 模板、context overflow 检测
  - `src/core/session/manager/` — ContextState、init_context_state、estimate_msg_chars
  - `src/core/llm/types.rs` — ChatMessage（含 MessageKind）、ChatRequest/ChatResponse
  - `src/lib.rs` — 对外导出 AgentLoop、AgentLoopConfig、AgentRunResult、ChatMessage、ContextState、ContextConfig 等

### 1.1 三层嵌套循环 + 干预点（ASCII）

```text
Layer 1  Conversation Loop：输入、FollowUp、最终收尾
    |
    v
Layer 2  Reasoning Loop：保留已接受正文与工具轮次
    +--> Layer 3 Request Retry：本次模型请求的有限尝试
    |      guard / 组装 / 完整消费流 / 响应判定
    |      压缩、降级、空答不清零；成功后返回
    +--> 提交回复 / 执行工具 / 检查 Steering、Abort
    |      下一逻辑请求获得新额度，已完成工具不重放
    v
  final_text；消息按合法 append 边界持久化
```

- **消息类型**：统一使用 `ChatMessage`（OpenAI wire format），通过 `MessageKind` 字段区分 Steering/CompactionSummary 等语义。
- **总览**：与 [src 模块索引](../README.md)「图 1」中 `core/agent_loop` 位置对照。

## 2. 设计方案 (Design Details)

- **设计模式**：三层嵌套循环（Conversation → Reasoning → Request Retry），职责分离；消息统一使用 `ChatMessage`，无需在 LLM 边界做类型转换。`MessageKind` 枚举区分 Normal/Steering/CompactionSummary 等内部语义。
- **关键权衡**：会话消息只有一份。回合间停放在 `ContextState.messages`，进入 `AgentLoop::run` 时 move 到局部工作列表，所有退出路径再停回该字段。`system_prompt` 由 `AgentLoopConfig` 保存，只在构造 `ChatRequest` 时临时前置；它不属于会话历史，也不进入 transcript。每条消息由 AgentLoop 的 append sink 落盘，调用方在 `run()` 返回后只持久化观测与 checkpoint。
- **线程安全/并发**：`steering_queue`、`follow_up_queue` 为 `Arc<Mutex<Vec<ChatMessage>>>`，`cancel_token` 为 `CancellationToken`；`steer()`、`follow_up()`、`abort()` 可从其他线程调用，`run()` 内读队列与取消信号，无数据竞争。流式 delta 通过 `EventBus` 的 `message_update` 事件推送，调用方（如 `chat.rs`）通过 `event_bus.on("message_update", ...)` 订阅。

## 3. 核心 API 与数据结构 (API Definitions)

- **ChatMessage**：统一消息类型（OpenAI wire format），含 `role`/`content`/`tool_calls`/`tool_call_id` 等字段。三个 `#[serde(skip)]` 字段 `msg_id`/`kind`/`timestamp` 携带内部元数据。
- **MessageKind**：`Normal`/`Steering`/`CompactionSummary`，区分不同语义的 user 消息。
- **ToolCallInfo**：`{ id, name, arguments }`，仅在流式积累 + 工具执行阶段使用的临时类型。
- **AgentLoopConfig**：`max_attempts`、`max_tool_rounds`、`retry_base_delay_ms`、`model`、`session_id`、`tool_definitions`、`context_config`，以及仅用于请求组装的 `system_prompt: Option<String>`。
- **AgentRunResult**：`{ final_text: String, new_messages: Vec<ChatMessage> }`，run 成功时最后一轮 LLM 文本回复及本次产生的所有新消息。
- **AgentLoop::new(llm, primitive, event_bus, config, cancel_token)**：标准构造函数；内部创建默认的 steering_queue、follow_up_queue。
- **AgentLoop::run_turn(&mut self, messages, turn_start) -> AgentRunOutcome**：主入口；`turn_start` 由调用方在追加用户输入时精确记录，执行第一层 Conversation Loop（含 FollowUp 检查）、第二层 Reasoning Loop（LLM ↔ 工具）、第三层 Request Retry（单请求重试与分类）。**`run(messages)`** 仅是兼容封装，适用于列表以且仅以一条用户消息收尾的调用方。
- **AgentLoop::steer(&self, msg: String)**：向 steering_queue 推入 `ChatMessage::steering(msg)`；Reasoning 每工具执行完后检查，非空则注入并跳过剩余工具进入下一轮 LLM。
- **AgentLoop::follow_up(&self, msg: String)**：向 follow_up_queue 推入 `ChatMessage::user(msg)`；第一层循环尾部检查，非空则 drain 追加到 messages 并 continue。
- **AgentLoop::abort(&self)**：调用 `cancel_token.cancel()`；Reasoning 每工具执行前检查取消状态，命中后返回中断结果并发布 `agent_end(interrupted)`。
- **LoopError**（内部）：`Retryable(AppError)`、`Fatal(AppError)`、`Aborted`；`classify_error(AppError)` 不再靠字符串猜测，而是优先基于结构化 `LlmError { stage, http_status, summary }` 判定：`401` → Fatal，`is_context_overflow(&AppError)`（如 `http_status=400 + context_length_exceeded`）→ Retryable 并触发 L3 截断，其余 `400` → Fatal，`429/500/502/503/504` 与传输阶段错误 → Retryable。

### 3.2 上下文管理 API（TASK-17）

- **ContextState**：回合间的停车位和上下文预算/预热/观测状态，包含 `messages: Vec<ChatMessage>`、`estimate_context_chars`、`context_budget_chars`。运行 `AgentLoop` 时 `messages` 必须为空；工作列表在 `run_turn()` 返回时统一停回。Turn 边界由追加输入的外层精确传入，steering 随后追加到该受保护尾部。
- **init_context_state(session, config, system_text) -> ContextState**：从 transcript 加载历史，解析为 ChatMessage 列表，识别已有 Compaction entry 标记为 `kind: CompactionSummary`。
- **消息请求组装**：`assemble_request_messages()` 是唯一出口，生成 `[system_prompt?] + working_messages + [ephemeral_tail?]`；system 和 ephemeral tail 都不写入停车列表。
- **ContextConfig**：上下文管理配置，含 `context_window`、`max_output_tokens`、`keep_recent_turns`、`layer0_single_result_max_chars`、`layer0_placeholder_threshold_chars`、`current_tail_compactable_min_chars`、`current_tail_single_result_max_chars`、`compaction_model`、`compaction_max_tokens`。

### 3.3 四层防护算法（`compaction/`）

| Layer | 函数 | 机制 | 触发条件 |
|-------|------|------|----------|
| 0 | `layer0_persist_large_results` | 超大 tool result 落盘 + preview 占位符 | boundary 应用成功后（② / ⑤ / mid-turn） |
| 1 | `compact_tool_results` | compactable zone 旧 tool result 替换为占位符 | boundary 应用成功后（② / ⑤ / mid-turn） |
| 2 | preheat + `apply_boundary` | LLM 异步摘要 + boundary switch | timing ②/⑤ |
| 3 | `force_drop_oldest_to_target` | 强制删除最旧 turn（防御性兜底） | context overflow |

- **is_context_overflow(&AppError)**：Agent Loop 生产分类入口；结构化识别 `http_status=400 + context_length_exceeded / maximum context length / reduce the length` 等上下文溢出。
- **is_context_overflow_text(text)**：仅底层文本规则 helper；供少量纯文本场景与测试复用，不再作为 Agent Loop 生产分类入口。

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
| max_attempts | u32 | 4 | 每次逻辑模型请求的尝试上限（含首次），无人值守临时故障/空答10次；详见架构文档 |
| max_tool_rounds | usize | usize::MAX | 单次 Reasoning 的工具轮次上限；请求重试不重置、不额外占用轮次 |
| retry_base_delay_ms | u64 | 500 | 指数退避，含 jitter；交互封顶8s、无人值守30s，Retry-After 优先 |
| model | String | — | LLM 模型名，由调用方从 Session/Config 填入 |
| session_id | String | — | 会话 ID，随 AgentEvent 发布 |
| tool_definitions | Vec<serde_json::Value> | [] | 传入 LLM 的工具 JSON Schema |
| context_config | ContextConfig | ContextConfig::default() | 上下文管理配置 |

### 5.2 ContextConfig（`[context]` 配置节）

| 字段 | 类型 | 默认值 | 环境变量 | 说明 |
|------|------|--------|----------|------|
| context_window | usize | 400,000 | `PI_CONTEXT_WINDOW` | 模型 context window（tokens） |
| max_output_tokens | usize | 128,000 | `PI_MAX_OUTPUT_TOKENS` | 模型最大输出（tokens） |
| keep_recent_turns | usize | 5 | — | Layer 1 历史 placeholder 保护区 turn 数（最近几轮不动） |
| layer0_single_result_max_chars | usize | 50,000 | — | Layer 0 单条超大 tool result 落盘 + preview 阈值（chars） |
| layer0_placeholder_threshold_chars | usize | 10,000 | — | Layer 0/L1 历史 tool result placeholder 阈值（chars） |
| current_tail_compactable_min_chars | usize | 1 | — | mid-turn current-tail guard 的候选最小字符数 |
| current_tail_single_result_max_chars | usize | 10,000 | — | mid-turn current-tail guard 的大结果落盘阈值（chars） |
| compaction_model | String | gpt-5.2 | — | Compaction LLM 调用使用的模型 |
| compaction_max_tokens | usize | 10,000 | — | Compaction 摘要路径的 max tokens 配置 |

预算公式：`contextBudgetChars = (context_window - max_output_tokens) × 4`（GPT-5.4 默认 = 1,088,000 chars）。

## 6. 交互流程 (Workflow)

```mermaid
flowchart TD
    Caller["调用方 chat_loop"]
    Run["AgentLoop::run(initial_messages)"]
    Conv["第一层 Conversation Loop"]
    Attempt["第三层 Request Retry"]
    Reason["第二层 Reasoning Loop"]
    LLM["llm.chat_stream"]
    Tools["execute_tool 循环"]
    SteeringCheck["steering_queue 非空?"]
    FollowUpCheck["follow_up_queue 非空?"]
    Persist["调用方持久化 Transcript"]

    Caller --> Run
    Run --> Conv
    Conv --> Reason
    Reason --> Attempt
    Attempt --> LLM
    LLM -->|失败且有额度| Attempt
    LLM -->|回复可接受| Tools
    Tools --> SteeringCheck
    SteeringCheck -->|是| Reason
    SteeringCheck -->|否| Reason
    Reason -->|Ok final_text| FollowUpCheck
    FollowUpCheck -->|是| Conv
    FollowUpCheck -->|否| Persist
    Persist --> Caller
```

- 第一层：处理输入、FollowUp 和任务收尾；工具预算耗尽不能靠追加输入重新开启循环。
- 第二层：模型与工具交替运行；保留已接受的正文、工具结果、工具轮次，提交失败不重发模型请求。
- 第三层：每次逻辑请求重新计数；网络、空答、压缩、附件降级共享该请求额度。完整回复通过判定后，先结束请求重试，再交回第二层提交 / 执行工具。取消、Fatal、压缩无进展与额度耗尽都结束重试事件。
- provider 内的首个 delta 前重试不变；出字后的断流上抛至单请求重试，不执行失败流的工具预览。
- Git `eb2cd0a9` → `9f25ac4e` 的旧设计把 Attempt 包住整个 Reasoning；`09333a6d` 只增加无人值守次数。本次调整归属，不提高数值。完整规则及扩展五分钟冷却见 [Agent Loop 设计](../../docs/architecture/agent-loop.md)。

### 6.1 上下文管理集成流程（TASK-17）

```text
  chat_loop (api/chat.rs)
      |
      v  init_context_state() ← 从 transcript 重建 ContextState（仅首次）
      |
      v  每轮用户输入:
      |    1. 更新 estimateContextChars（新消息）
      |    2. mem::take ContextState.messages，直接追加并落盘本轮输入
      |    3. timing ② check_before_request；若成功 apply boundary（内含 L0），继续使用同一工作列表
      |    4. set_context_state（停车列表为空）→ AgentLoop
      |
      v  AgentLoop::run()
      |    - Layer 2: timing ②/⑤/mid-turn 的 ready preheat 直接改写工作列表；Layer 0 严格止于 history_end
      |    - Request Retry: ContextOverflow → force_drop_oldest；历史为空则同次 collapse → 同一额度内重试
      |    - 动态维护 estimate_context_chars
      |
      v  take_context_state ← 取回 ContextState
      |    - 列表已由 run() 停回；new_messages 仅用于 checkpoint 行 id
      |    - 下一轮继续使用同一 ContextState
```

## 7. 示例代码 (Usage Examples)

chat 层构造并调用 AgentLoop 的典型片段（见 `src/api/chat.rs`）：

```rust
let mut messages = std::mem::take(&mut context_state.messages);
append_planned_messages(&mut messages, ChatMessage::user(&input))?;

let config = AgentLoopConfig {
    max_attempts: 3,
    system_prompt: Some(system_text),
    model: model.clone(),
    session_id: ctx.session.current_session_key().to_string(),
    tool_definitions: build_tool_definitions(),
    context_config: context_config.clone(),
    ..Default::default()
};
let mut agent_loop = AgentLoop::new(
    ctx.llm.clone(),
    ctx.primitive.clone(),
    ctx.event_bus.clone(),
    config,
    ctx.cancelled.clone(),
);

agent_loop.set_context_state(Some(context_state));
let result = agent_loop.run(messages).await?;
let context_state = agent_loop
    .take_context_state()
    .expect("run parks the working list on every exit");
persist_context_observability(&context_state)?;
record_turn_checkpoint(result.new_messages.iter().filter_map(|m| m.msg_id.as_deref()))?;
```

## 8. 验收标准 (Testing & QA)

- **单测**：`cargo test -j 1 --lib -- --test-threads=1` 全通过；覆盖 `core::agent_loop`、`core::compaction`、`infra::config`（含 ContextConfig）、`core::session::manager`（init_context_state 等）。
- **集成**：`tests/context_management_tests.rs` — 端到端上下文与 Compaction 场景；`context_management.md` / User_Stories Story 8 对齐。
- **门禁**：`cargo clippy --all-targets -- -D warnings` 无警告。
- **事件**：agent_start、turn_start/end、message_start/update/end、tool_execution_start/end、tool_call/tool_result、auto_retry_start/end、**auto_compaction_start/end**、**tool_result_truncated**、**compaction_error**、agent_end(success|error|interrupted)。
