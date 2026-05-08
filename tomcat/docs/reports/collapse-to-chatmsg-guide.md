# 重构导读：四层降两层 — 统一到 ChatMessage

> 对应分支：`feature/collapse-to-chatmsg`
> 核心提交：`d482725 refactor(core): 四层降两层——统一到 ChatMessage，消除 TurnEntry 与 AgentMessage`
> 状态文档：[docs/status/feature-collapse-to-chatmsg.md](../status/feature-collapse-to-chatmsg.md)

---

## 一、动机

重构前，一条对话消息在内存中存在 **四种形状**：

```
transcript JSONL (serde_json::Value)
       ↓ fold_entries_to_turns
  TurnEntry (UserTurn / SummaryTurn)
       ↓ build_context_from_state → flatten
  AgentMessage (User / Assistant / ToolResult / System / Steering / CompactionSummary)
       ↓ convert_to_llm_format
  ChatMessage (OpenAI wire format)
```

问题：
- 同一事实三种类型表示，字段映射散落在 `fold`、`convert`、`splice`、`compound_id` 等处
- `TurnEntry` 把「一条 user + 后续 assistant/tool」打成一包，引入 `start_id` / `end_id` / 复合 id 后缀等规则
- `AgentMessage` 是与 `ChatMessage` 几乎 1:1 的冗余层，唯一差别是多了 `Steering` / `CompactionSummary` 变体
- Compaction 各层（L0/L1/L2/L3）在 `TurnEntry` 和 `AgentMessage` 之间来回切换

---

## 二、目标架构

重构后只剩 **两层**：

```
transcript JSONL (serde_json::Value)
       ↓ fold_entries_to_messages
  ChatMessage（扩展了 msg_id / kind / timestamp 字段）
       ↓ clone
  发给 LLM（同一结构，无需转换）
```

核心思路：**在 `ChatMessage` 上加 `#[serde(skip)]` 内部元数据字段**，不改变 wire format，消除中间层。

---

## 三、删除了什么

| 删除项 | 原位置 | 说明 |
|--------|--------|------|
| `TurnEntry` 枚举（`UserTurn` + `SummaryTurn`） | `session/manager/types.rs` | 分组抽象完全移除 |
| `AgentMessage` 枚举 | `agent_loop/types.rs` | 冗余中间层完全移除 |
| `convert_to_llm_format()` | `agent_loop/convert.rs` | 不再需要 AgentMessage → ChatMessage 转换 |
| `agent_messages_from_chat()` | `agent_loop/convert.rs` | 不再需要 ChatMessage → AgentMessage 反向转换 |
| `on_new_user_turn()` | `session/manager/types.rs` | 不再用 TurnEntry 登记一轮 |
| `compact_messages()` | `agent_loop/run.rs` | 已弃用的 MVP 压缩逻辑 |
| `estimate_turn_chars()` | `session/manager/types.rs` | 改为 `estimate_msg_chars()` |
| `user_turn_matches_covered_end()` | `session/manager/types.rs` | 边界应用改为按 `msg_id` 匹配 |
| `compound_id_suffix()` / `compound_id_prefix()` | `session/manager/types.rs` | 不再需要复合 id 解析 |

---

## 四、新增了什么

### 4.1 `MessageKind` 枚举（`src/core/llm/types.rs`）

```rust
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub enum MessageKind {
    #[default]
    Normal,
    Steering,
    CompactionSummary,
}
```

区分：普通消息、Steering 指令（role=user 但不算 turn 边界）、压缩摘要（role=user 但语义为历史摘要）。

### 4.2 `ChatMessage` 新增字段

```rust
pub struct ChatMessage {
    // --- 原有 wire format 字段（参与序列化）---
    pub role: ChatMessageRole,
    pub content: Option<ChatMessageContent>,
    pub name: Option<String>,
    pub tool_calls: Option<Vec<serde_json::Value>>,
    pub tool_call_id: Option<String>,

    // --- 新增内部元数据（#[serde(skip)]，不影响 wire）---
    pub msg_id: Option<String>,      // transcript 行 id
    pub kind: MessageKind,           // 语义标签
    pub timestamp: Option<String>,   // ISO 时间
}
```

### 4.3 便捷构造方法

```rust
ChatMessage::steering(text)            // kind = Steering, role = User
ChatMessage::compaction_summary(text)  // kind = CompactionSummary, role = User
ChatMessage::set_text_content(text)    // 原地替换 content（L0/L1 用）
```

---

## 五、核心数据结构变化

### ContextState

```rust
// 重构前
pub struct ContextState {
    pub user_turns_list: Vec<TurnEntry>,
    // ...
}

// 重构后
pub struct ContextState {
    pub messages: Vec<ChatMessage>,
    // ...
}
```

### Turn 边界推导

不再由 `TurnEntry` 显式分组。Turn 的起始点通过以下规则在 `Vec<ChatMessage>` 上动态推导：

```rust
fn is_turn_start(m: &ChatMessage) -> bool {
    (m.role == ChatMessageRole::User && m.kind != MessageKind::Steering)
        || m.kind == MessageKind::CompactionSummary
}
```

`ContextState::turn_count()` 方法统计当前有多少个 turn。

### build_context_from_state

```rust
// 重构前：展平 TurnEntry → Vec<AgentMessage>，再 convert_to_llm_format → Vec<ChatMessage>
// 重构后：
pub fn build_context_from_state(state: &ContextState) -> Vec<ChatMessage> {
    state.messages.clone()
}
```

---

## 六、各层 Compaction 的变化

### L0（`truncation.rs` — `layer0_persist_large_results`）

- **重构前**：操作最后一个 `UserTurn` 内的 `AgentMessage::ToolResult`
- **重构后**：找到 `messages` 中最后一个 turn 起始索引，扫描该索引之后的 `role == Tool` 消息，直接修改 `ChatMessage.content`

### L1（`truncation.rs` — `compact_tool_results`）

- **重构前**：遍历 `user_turns_list[..len-m]` 中每个 turn 的 `ToolResult`
- **重构后**：用 `find_protected_turn_start()` 算出保护区起始索引，处理 `messages[..protected_start]` 中的 `role == Tool` 消息

### L2（`preheat.rs` + `apply.rs`）

- **重构前**：快照 `TurnEntry` 列表，用 `start_id/end_id` 定位，`SummaryTurn` 替换 turn 列表
- **重构后**：快照 `Vec<ChatMessage>`，用 `msg_id` 定位，`ChatMessage::compaction_summary()` 替换消息区间

### L3（`cascade.rs` — `force_drop_oldest_to_target`）

- **重构前**：`user_turns_list.remove(0)` 删除整个 `TurnEntry`
- **重构后**：`messages.drain(..turn_end)` 删除从头到下一个 turn 起始点之间的所有消息

### apply_boundary

- **重构前**：在 `user_turns_list` 中找匹配 `covered_end_id` 的 `UserTurn`（含复合 id 后缀），`splice` 成 `SummaryTurn`
- **重构后**：`rposition` 找 `msg_id == covered_end_id`，`messages.splice(..=end_idx, [summary_msg])`

---

## 七、Transcript 折叠流程变化

```
// 重构前
fold_entries_to_turns: JSONL entries → Vec<TurnEntry>
  - Message entry → 积累到当前 UserTurn 的 messages 列表
  - BranchSummary (is_boundary=true) → 清空 turns，插入 SummaryTurn
  - 遇到新 User entry → 先 flush 上一个 UserTurn

// 重构后
fold_entries_to_messages: JSONL entries → Vec<ChatMessage>
  - Message entry → chat_message_from_entry() 生成 ChatMessage，直接 push
  - BranchSummary (is_boundary=true) → 清空 messages，插入 CompactionSummary
  - 不需要 "flush" 操作，一条 entry 对应一条 ChatMessage
```

---

## 八、Agent Loop 路径变化

### 发给 LLM

```rust
// 重构前
let agent_messages = build_context_from_state(&ctx_state); // Vec<AgentMessage>
let chat_messages = convert_to_llm_format(&agent_messages); // Vec<ChatMessage>
let req = ChatRequest { messages: chat_messages, .. };

// 重构后
let messages = build_context_from_state(&ctx_state); // Vec<ChatMessage>
let req = ChatRequest { messages: messages.clone(), .. };
```

### AgentLoop::run 签名

```rust
// 重构前
pub async fn run(&mut self, initial_messages: Vec<AgentMessage>) -> Result<AgentRunResult, ..>

// 重构后
pub async fn run(&mut self, initial_messages: Vec<ChatMessage>) -> Result<AgentRunResult, ..>
```

### steer / follow_up

```rust
// 重构前：push AgentMessage::Steering { text }
// 重构后：push ChatMessage::steering(text)
```

### 对话消息写入

```rust
// 重构前：构造 UserTurn，调用 on_new_user_turn
// 重构后：逐条 append_message → msg.msg_id = row_id → messages.push(msg)
```

---

## 九、测试覆盖

本次重构新增 13 个测试（8 个单元 + 5 个集成）：

### 单元测试（`src/core/compaction/tests.rs`）

| 测试 | 覆盖 |
|------|------|
| `run_layer0_cleanup_persists_then_compacts` | L0+L1 组合：大 tool 落盘 + 可压缩区 placeholder |
| `run_layer0_cleanup_no_tool_results_is_noop` | 无 tool 消息时 no-op |
| `run_layer0_cleanup_mixed_sizes` | 混合体量：60K persist、15K placeholder、5K 保留 |
| `run_layer0_cleanup_freed_values_consistent_with_estimate` | 释放统计与 estimate 下降一致 |
| `l1_turn_boundary_with_steering_messages` | Steering 不参与 turn 边界计数 |
| `l3_drop_oldest_with_compaction_summary_as_first` | 最老 turn 以 CompactionSummary 开头时的裁剪 |
| `apply_boundary_with_msg_id_matching` | 按 msg_id 区间替换为摘要 |
| `messages_to_text_format_all_roles` | 摘要输入文本格式正确性 |

### 集成测试（`tests/context_management_tests.rs`）

| 测试 | 覆盖 |
|------|------|
| `test_check_after_reply_emits_boundary_switched_on_apply` | BoundarySwitched 事件 payload |
| `test_check_after_reply_stale_emits_compaction_error` | CompactionError 事件（stale apply） |
| `test_check_before_request_emits_boundary_switched` | async 路径 BoundarySwitched 事件 |
| `test_full_compaction_pipeline_l0_l1_l2_l3_with_event_sequence` | L0→L1→L2→L3 全链路 |
| `test_context_overflow_trim_events_have_correct_payload` | overflow trim 事件 payload 字段 |

---

## 十、变更文件清单

| 文件 | 变更类型 |
|------|----------|
| `src/core/llm/types.rs` | 新增 `MessageKind`、`msg_id`/`kind`/`timestamp` 字段、构造方法 |
| `src/core/session/manager/types.rs` | 删除 `TurnEntry`，`messages: Vec<ChatMessage>` 替代 `user_turns_list` |
| `src/core/session/manager/context.rs` | `fold_entries_to_messages`、`filter_messages_by_day` |
| `src/core/agent_loop/types.rs` | 删除 `AgentMessage`，签名改 `Vec<ChatMessage>` |
| `src/core/agent_loop/convert.rs` | 删除 `convert_to_llm_format` 等（文件内容清空） |
| `src/core/agent_loop/run.rs` | 消息列表类型改 `Vec<ChatMessage>` |
| `src/api/chat/mod.rs` | 直接使用 `ChatMessage`，不再经 `AgentMessage` |
| `src/core/compaction/truncation.rs` | L0/L1 操作 `Vec<ChatMessage>` |
| `src/core/compaction/cascade.rs` | L3 操作 `Vec<ChatMessage>` |
| `src/core/compaction/preheat.rs` | 快照改 `&[ChatMessage]`，`messages_to_text` |
| `src/core/compaction/apply.rs` | `turn_count()` 替代 `user_turns_list.len()` |
| `src/core/compaction/tests.rs` | 新增 8 个单元测试 |
| `tests/context_management_tests.rs` | 新增 5 个集成测试 |
| `src/core/mod.rs` | 导出链更新 |
| `src/core/session/mod.rs` | 导出链更新 |
| `src/lib.rs` | 导出链更新 |
