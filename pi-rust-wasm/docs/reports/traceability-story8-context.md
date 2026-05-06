# Story 8 上下文 / JSONL / 压缩 — 可追溯性对照表

本文档将 [User_Stories.md](../../openspec/specs/User_Stories.md) **Story 8** 中与对话、会话、上下文压缩相关的验收项，与 [E2E_SCENARIO_LIBRARY.md](../../openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md) 及仓库内 **Rust 测试符号** 对齐，便于审查与回归。

**产品前提（开发阶段）**：JSONL 摘要行正式类型为 **`type: branch_summary`**；**不**读盘兼容历史 `type: compaction`；无法反序列化的行由 `read_entries_tail` **warn + skip**（见 `src/core/session/transcript.rs`）。

## Story 8 验收项 ↔ E2E ↔ 测试

| Story 8 条（概括） | E2E 编号 | 测试符号 / 位置 |
|-------------------|----------|-----------------|
| `pi chat`、流式 | E2E-CLI-081 | `tests/cli_tests.rs`：`test_user_chat_non_interactive_with_prompt_flag` |
| `pi chat --resume`、JSONL 加载 | E2E-CLI-082 | `test_user_chat_resumes_last_session` |
| 多轮上下文 | E2E-CLI-083 | `test_user_chat_multi_turn_context_retained` |
| `pi session` 生命周期 | E2E-CLI-071～074 及 session 相关 `test_user_*` | `cli_tests.rs` |
| Layer 0 大 tool result | E2E-CLI-084 | `test_layer0_persist_and_readback`、`test_compact_tool_results_*`；`layer0_threshold_from_config`（`src/core/compaction/tests.rs`） |
| Context overflow → 压缩重试 | E2E-CLI-085 | `test_context_overflow_triggers_compaction_and_retries`（`tests/context_management_tests.rs`） |
| Session 重载 + `branch_summary` 折叠 | E2E-CLI-086 | `test_session_reload_with_branch_summary_entries`、`test_session_reload_with_boundary` |
| Preheat pending / restore | E2E-CLI-087 | `preheat_*`（`compaction/tests.rs`）；`test_session_reload_pending_preheat_restore` |
| ratio≥0.98 同步等待（时机 ②） | E2E-CLI-088 | 无独立黑盒名；`check_before_request`（`apply.rs`）+ 集成/cli 观测 |
| Boundary / ratio 分档 | E2E-CLI-089 | `apply_boundary_replaces_covered_range` 等（`compaction/tests.rs`） |
| `is_boundary=false` 跳过 | E2E-CLI-090 | `test_session_reload_boundary_false_skipped` |
| 上下文指标与 sessions.json | E2E-CLI-091 | `test_context_metrics_update_event_published`（`agent_loop_tests.rs`）；`persist_context_observability_writes_sessions_json`（`manager/tests.rs`） |
| §5.7.5.1 陈旧 apply：删行 + preheat idle | **E2E-CLI-092** | `check_after_reply_stale_apply_removes_branch_summary_and_keeps_preheat_idle`（`src/core/compaction/tests.rs`） |
| JSONL tail：无法解析行不崩溃 | **E2E-CLI-093** | `read_entries_tail_skips_unknown_type_line`（`src/core/session/transcript/tests.rs`） |

## 无 E2E 表编号的历史缺口（已补）

- **§5.7.5.1**：原场景表未单独列出 → **E2E-CLI-092** 与 TASK-21 备注挂钩。
- **read_entries_tail skip**：原无表项 → **E2E-CLI-093**（仅锁「不 panic + 可解析行保留」）。

## 相关规格路径

- [context-management.md §5.7 / §5.7.5.1](../../docs/architecture/context-management.md)
- [session-storage.md — transcript / BranchSummaryEntry](../../docs/architecture/session-storage.md)
