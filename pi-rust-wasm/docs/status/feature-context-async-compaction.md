| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-12 | PENDING_INTEGRATION | feature/context-async-compaction | - |

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
- [x] **ContextState 嵌套** `session_obs: SessionContextObservation`（刷盘子集）与 `live: ContextLiveMetrics`；`AgentLoop` 移除独立 `metrics`，瞬时指标只写 `context_state.live`
- [x] **可观测性与日志可靠性**：`target: pi_wasm_chat_diag` 结构化 info（用户追加后、timing② 后、agent run 前、`classify_error` 各分支、L3 重试与 trim 结果、首 turn 指标、流连接错误、timing② `check_before_request` 出入路径）；CLI 在 `init_logging` 之前加载 `assets/.env` 使 `RUST_LOG` 参与 `EnvFilter`；文件与 stderr 共用同一 `EnvFilter`，`non_blocking` 的 `WorkerGuard` 在 `try_init` 成功后 `mem::forget` 避免落盘线程提前退出
- [x] **会话级 stderr 监听**：`chat_loop` 入口一次性注册 `session_stderr_listeners`（context_metrics / L1 compaction / boundary 等），跨用户输入轮次保留，避免 Layer1 在 readline 空闲 emit 时因 per-run `off` 无人消费；`event_bus` 单测覆盖「摘掉占位监听后延迟 emit 仍可送达」
- [x] **TASK-21 §5.7 消息级 ID**：`UserTurn` 增加 `start_id`/`end_id` 与 `compound_turn_id`；`append_message`/`try_append_message` 返回新 message 行 id；`chat` 先写满 transcript 再 `on_new_user_turn`；`fold_entries_to_turns` 从首尾 message id 还原；`insert_entry_after_message_id`（锚点缺失 warn + `append_entry`）；Preheat 快照 `S`/`E`、`BranchSummaryEntry.id = S::E`；`apply_boundary` 以 `end_id` 为主 + 旧 id 回退；`restore_completed` 与 `end_id`/suffix 匹配
- [x] **门禁（2026-04-12）**：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings` 通过；全量子集 `cargo test -j 1 -p pi_wasm --lib --bins` + 除 `cli_tests`/`llm_tests` 外全部 `--test` 目标，`--test-threads=1`，日志 `pi-rust-wasm/.integration_test_output.log` 末尾 `EXIT_CODE=0`。**LLM/CLI**：同日在 `pi-rust-wasm` 下 `source .env`（与 `scripts/verify-openai-apis.sh` 一致）后 `cargo test -j 1 -p pi_wasm --test cli_tests --test llm_tests -- --nocapture --test-threads=1` 全绿（77 + 2 passed）；详见 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md)「OpenAI API Key」节

### 🔌 INTERFACE (接口变更)

**TASK-21（§5.7 消息级 ID，2026-04）**

- `SessionManager::append_message` / `try_append_message`：返回 `Result<String, AppError>`，成功值为新 `MessageEntry.id`。
- `compound_turn_id` / `compound_id_prefix` / `compound_id_suffix`：公开辅助（crate 根或 `session::manager` 导出路径以代码为准）。
- `transcript::insert_entry_after_message_id`：在锚点 message 行之后插入条目；锚点缺失时 warn 并回退尾部追加。

**20.5′（单行 + 原地升级，2026-04）**

- `CompactionResult`：`transcript_compaction_entry_id: Option<String>` — 对应 JSONL 中该批次 `type: branch_summary` 行的 `id`，apply 时用于 `set_branch_summary_entry_is_boundary_true`。
- `BranchSummaryEntry`（serde）：可选 `preheatCompactionId`（与行 `id` 对齐写入，便于外部工具）。
- `transcript::set_branch_summary_entry_is_boundary_true`：按 `id` 定位 `branch_summary` 行并改写 `isBoundary=true`（整文件读改写）。
- `Preheat::restore_completed` + 内部 `CachedCompleted`：重载后恢复「LLM 已写完磁盘、尚未 apply」的摘要；`poll_result`/`await_result` 与正常完成同语义；`abort` 清除缓存。
- `apply_boundary`：仅按 **`covered_end_id`** 在 `user_turns_list` 中定位最小 `k`；无匹配 → **`AppError::ApplyBoundaryStale`**，Layer 2 删对应 **`branch_summary`** 行且不 **`restore_pending_result`**（见 `context-management.md` §5.7.5.1）。
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
