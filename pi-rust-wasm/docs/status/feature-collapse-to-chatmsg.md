| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-14 | DEV | feature/collapse-to-chatmsg | - |

### ✅ DONE (已完成)
- [x] **[P0]** Phase 1: 扩展 ChatMessage — 新增 msg_id / kind / timestamp + MessageKind enum + 便捷构造方法
- [x] **[P0]** Phase 2: 重写 ContextState — messages: Vec<ChatMessage> 替代 user_turns_list
- [x] **[P0]** Phase 3: 重写 context.rs — fold_entries_to_messages / filter_messages_by_day / build_context_from_state
- [x] **[P0]** Phase 4: 重写 compaction 层 — truncation / cascade / preheat 全部操作 Vec<ChatMessage>
- [x] **[P0]** Phase 5: 重写 agent_loop — 消除 AgentMessage，直接使用 ChatMessage
- [x] **[P0]** Phase 6: 重写 chat/mod.rs — 消除 convert_to_llm_format
- [x] **[P0]** Phase 7: 删除旧类型 — TurnEntry / AgentMessage / convert_to_llm_format / agent_messages_from_chat
- [x] **[P1]** Phase 8: 更新全部测试 + 文档，cargo test 全绿
- [x] **[P1]** 门禁: clippy --all-targets -D warnings 零 warning + cargo test 606 passed 0 failed
- [x] **[P1]** Compaction 测试计划: 新增 8 个单元测试 + 5 个集成测试，覆盖 L0/L1/L2/L3 + 事件断言
- [x] **[P2]** 重构导读: `docs/reports/collapse-to-chatmsg-guide.md`
- [x] **[P2]** 文档更新: 全项目文档术语同步（20+ 文件）

### 🔌 INTERFACE (接口变更)
- `ChatMessage`: 新增 `msg_id: Option<String>`, `kind: MessageKind`, `timestamp: Option<String>` (均 `#[serde(skip)]`)
- 新增 `MessageKind` enum (`Normal`, `Steering`, `CompactionSummary`)
- 新增 `ChatMessage::steering()`, `ChatMessage::compaction_summary()`, `ChatMessage::set_text_content()` 方法
- `ContextState`: `user_turns_list: Vec<TurnEntry>` → `messages: Vec<ChatMessage>`
- 新增 `ContextState::turn_count()`, `estimate_msg_chars()`
- 删除: `TurnEntry`, `AgentMessage`, `convert_to_llm_format`, `agent_messages_from_chat`, `compact_messages`, `on_new_user_turn`
- `AgentRunResult.new_messages`: `Vec<AgentMessage>` → `Vec<ChatMessage>`
- `compound_turn_id`: 从 `pub` 降为 `pub(crate)`

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
