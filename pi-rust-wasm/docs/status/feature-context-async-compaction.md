| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-08 | PENDING_INTEGRATION | feature/context-async-compaction | - |

### ✅ DONE (已完成)
- [x] **[P0]** 20.1 TurnEntry 增加 `id: String`；ContextState 增加 `transcript_path`/`compaction_summary`
- [x] **[P0]** 20.2 `CompactionSummary`/`CompactionResult` + `abort_preheat()` + `apply_boundary()`
- [x] **[P0]** 20.3 Layer 0 阈值可配化 (50K/10K) + `run_layer0_cleanup`
- [x] **[P0]** 20.3 `preheat.rs` — `maybe_start_preheat` + `generate_summary` 异步预热
- [x] **[P0]** 20.3 `apply.rs` — `check_after_reply` / `check_before_request` / `apply_boundary_switch`
- [x] **[P0]** 20.3 agent_loop/run.rs 时机 ⑤ 集成（Layer 0 cleanup → preheat → check_after_reply）
- [x] **[P0]** 20.4 chat/mod.rs 时机 ② 集成（`check_before_request`）
- [x] **[P1]** 20.5 Transcript **单行 compaction 不变式**：预热追加一行 `is_boundary=false`（含行 `id` / 可选 `preheatCompactionId`）；应用时 **原地** 将该行 `isBoundary` 改为 `true`（不追加第二份全文）。`init_context_state` 在 `fold_entries_to_turns` 单遍推断最后一条未应用 preheat 时调用 `preheat.restore_completed`，重载后与运行时一致（§5.5′）
- [x] **[P1]** 20.6 UPDATE_SUMMARIZATION_PROMPT 增量摘要支持
- [x] **[P1]** 20.7 Layer 3 仅在 overflow 后触发（`force_drop_oldest_to_target`）
- [x] **[P1]** 20.8 ContextMetrics 新增 `preheat_in_progress`
- [x] **[P1]** 20.9 AgentEvent 新增 `PreheatStarted`/`PreheatCompleted`/`PreheatError`/`BoundarySwitched`
- [x] **[P1]** 20.10 移除同步 cascade 入口（`run_compaction_cascade_v2`/`determine_cascade_params`/`summary.rs`）
- [x] **[P1]** 20.11 Session 退出 `abort_preheat()`
- [x] **[P2]** 20.12 单元测试（`abort_preheat`/`apply_boundary`/`check_after_reply`/`layer0_threshold_from_config`）
- [x] **[P2]** 20.13 集成测试（`session_reload_boundary_false_skipped`/`session_reload_with_boundary`/`session_reload_pending_preheat_restore`）
- [x] **`compact_tool_results` 读取 `ContextConfig.layer0_placeholder_threshold_chars`**（默认 10K，可 `[context]` 覆盖；替换原硬编码 20K）
- [x] **`context_metrics_update` 单次 `run_reasoning_loop` 内至多两次**（首轮 LLM 前 + 收尾 / `max_tool_rounds` 耗尽；中间 tool round 不发）

### 🔌 INTERFACE (接口变更)

**20.5′（单行 + 原地升级，2026-04）**

- `CompactionResult`：`transcript_compaction_entry_id: Option<String>` — 对应 JSONL 中该批次 compaction 行的 `id`，apply 时用于 `set_compaction_entry_is_boundary_true`。
- `CompactionEntry`（serde）：可选 `preheatCompactionId`（与行 `id` 对齐写入，便于外部工具）。
- `transcript::set_compaction_entry_is_boundary_true`：按 `id` 定位 compaction 行并改写 `isBoundary=true`（整文件读改写）。
- `Preheat::restore_completed` + 内部 `CachedCompleted`：重载后恢复「LLM 已写完磁盘、尚未 apply」的摘要；`poll_result`/`await_result` 与正常完成同语义；`abort` 清除缓存。
- `apply_boundary`：`covered_start_id` 在列表中缺失但 `covered_end_id` 仍命中时，替换区间为 `user_turns_list[0..=end]`（Layer3 删前缀场景），并 `warn`。
- **不向前兼容**：开发阶段不实现「同一逻辑批次两条全文 compaction（false 行 + 再 append true 行）」；历史 JSONL 需手工整理或新 session。
- `TurnEntry::UserTurn`/`SummaryTurn`: 新增 `id: String`
- `ContextState`: 新增 `transcript_path: PathBuf`、`compaction_summary: Option<CompactionSummary>`；移除 `compaction_consecutive_failures`
- `ContextConfig`: 新增 `compaction_max_tokens`；移除 `single_tool_result_max_chars`、`layer0_turn_aggregate_max_chars`
- `compute_context_budget_chars`: 公式从 `(cw - mo) * 4 * 0.75` 改为 `(cw - mo) * 4`
- `AgentEvent`: 新增 `PreheatStarted`/`PreheatCompleted`/`PreheatError`/`BoundarySwitched` 变体
- `ContextMetrics`: 新增 `preheat_in_progress: bool`
- 新增 `CompactionSummary`/`CompactionResult` structs
- 新增 `maybe_start_preheat()`、`check_after_reply()`、`check_before_request()`
- 新增 `run_layer0_cleanup()`
- `compact_tool_results(state, config, m)`：第二参为 `&ContextConfig`（占位符阈值取自 `layer0_placeholder_threshold_chars`）
- 删除 `run_compaction_cascade_v2`、`run_compaction_cascade`、`determine_cascade_params`、`force_drop_oldest`、`truncate_tool_result_if_needed`、`summary.rs`

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
