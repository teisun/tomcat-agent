| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-06 | DOING | feature/context-async-compaction | - |

### ✅ DONE (已完成)
- [x] **[P0]** 20.1 TurnEntry 增加 `id: String`；ContextState 增加 `transcript_path`/`compaction_summary`
- [x] **[P0]** 20.2 `CompactionSummary`/`CompactionResult` + `abort_preheat()` + `apply_boundary()`
- [x] **[P0]** 20.3 Layer 0 阈值可配化 (50K/10K) + `run_layer0_cleanup`
- [x] **[P0]** 20.3 `preheat.rs` — `maybe_start_preheat` + `generate_summary` 异步预热
- [x] **[P0]** 20.3 `apply.rs` — `check_after_reply` / `check_before_request` / `apply_boundary_switch`
- [x] **[P0]** 20.3 agent_loop/run.rs 时机 ⑤ 集成（Layer 0 cleanup → preheat → check_after_reply）
- [x] **[P0]** 20.4 chat/mod.rs 时机 ② 集成（`check_before_request`）
- [x] **[P1]** 20.5 Transcript 双阶段写入（`is_boundary: false` 预热 / `is_boundary: true` 边界）
- [x] **[P1]** 20.6 UPDATE_SUMMARIZATION_PROMPT 增量摘要支持
- [x] **[P1]** 20.7 Layer 3 仅在 overflow 后触发（`force_drop_oldest_to_target`）
- [x] **[P1]** 20.8 ContextMetrics 新增 `preheat_in_progress`
- [x] **[P1]** 20.9 AgentEvent 新增 `PreheatStarted`/`PreheatCompleted`/`PreheatError`/`BoundarySwitched`
- [x] **[P1]** 20.10 移除同步 cascade 入口（`run_compaction_cascade_v2`/`determine_cascade_params`/`summary.rs`）
- [x] **[P1]** 20.11 Session 退出 `abort_preheat()`
- [x] **[P2]** 20.12 单元测试（`abort_preheat`/`apply_boundary`/`check_after_reply`/`layer0_threshold_from_config`）
- [x] **[P2]** 20.13 集成测试（`session_reload_boundary_false_skipped`/`session_reload_with_boundary`）

### 🔌 INTERFACE (接口变更)
- `TurnEntry::UserTurn`/`SummaryTurn`: 新增 `id: String`
- `ContextState`: 新增 `transcript_path: PathBuf`、`compaction_summary: Option<CompactionSummary>`；移除 `compaction_consecutive_failures`
- `ContextConfig`: 新增 `compaction_max_tokens`；移除 `single_tool_result_max_chars`、`layer0_turn_aggregate_max_chars`
- `compute_context_budget_chars`: 公式从 `(cw - mo) * 4 * 0.75` 改为 `(cw - mo) * 4`
- `AgentEvent`: 新增 `PreheatStarted`/`PreheatCompleted`/`PreheatError`/`BoundarySwitched` 变体
- `ContextMetrics`: 新增 `preheat_in_progress: bool`
- 新增 `CompactionSummary`/`CompactionResult` structs
- 新增 `maybe_start_preheat()`、`check_after_reply()`、`check_before_request()`
- 新增 `run_layer0_cleanup()`
- 删除 `run_compaction_cascade_v2`、`run_compaction_cascade`、`determine_cascade_params`、`force_drop_oldest`、`truncate_tool_result_if_needed`、`summary.rs`

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
