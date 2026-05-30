| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-05-31 01:56 +0800 | PENDING_INTEGRATION | feature/current-tail-aggregate-guard | - |

### DONE
- [x] [P1] 认领 `T2-P1-011`，确认任务卡为 `DOING` / `Spike`
- [x] [P1] 切换到 `feature/current-tail-aggregate-guard`
- [x] [P1] 收口 `ContextConfig` 与 `truncation.rs` 的阶段二配置漂移
- [x] [P1] 接入 mid-turn precheck + aggregate reduction + single branch_summary collapse
- [x] [P1] 补齐单测、集成测试、文档同步与交付收口

### INTERFACE
- `context.compaction_turns` 已移除。
- 新增 `context.current_tail_compactable_min_chars`（默认 `1`）与 `context.current_tail_single_result_max_chars`（默认 `10_000`）。
- `context.keep_recent_turns` 现在真实驱动历史 placeholder 保护区（默认 `5`）。
- current-tail guard 在每次工具轮结束后、下一次 `llm.chat_stream(...)` 前执行：先吃历史收益，再减 current tail，不够时整份 collapse 为单条 `branch_summary + keepalive`。

### BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### TEST
- 已跑：`cargo fmt --check -- src/core/agent_loop/mod.rs src/core/agent_loop/reasoning_loop.rs src/core/agent_loop/current_tail_guard.rs src/core/agent_loop/tests/mod.rs src/core/agent_loop/tests/current_tail_guard_test.rs src/core/compaction/mod.rs src/core/compaction/truncation.rs src/core/compaction/tests/context_layer0_v2_test.rs src/core/compaction/tests/preheat_and_truncation_test.rs src/core/compaction/tests/turn_boundaries_l3_test.rs src/core/session/manager/types.rs src/core/session/manager/tests/context_state_test.rs src/core/session/mod.rs src/core/session/transcript.rs src/core/session/tests/transcript_mutate_test.rs src/core/tools/config_tool/allowlist.rs src/core/tools/contract/catalog.rs src/infra/config/types/context.rs src/infra/config/tests/context_cfg_test.rs tests/context_management_tests.rs`
- 已跑：`cargo clippy --all-targets -- -D warnings`
- 已跑：`cargo test --lib context_config_default_values`
- 已跑：`cargo test --lib l1_keep_recent_turns_reads_config_value`
- 已跑：`cargo test --lib current_tail_guard_test`
- 已跑：`cargo test --lib rewrite_message_text_entries_by_id_updates_target_messages_only`
- 已跑：`cargo test --lib rewrite_local_tail_chars_updates_estimate_and_post_usage`
- 已跑：`cargo test --test context_management_tests`
- 已跑：`cargo test --test context_management_tests test_reasoning_loop_mid_turn_precheck_rewrites_before_second_llm -- --nocapture`
- 已核对：`openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 已补阶段二 current-tail guard 场景，并登记 `AgentLoop::run()` 集成链路；本轮复用既有 integration crate，`scripts/test-groups.sh` 无需改动
