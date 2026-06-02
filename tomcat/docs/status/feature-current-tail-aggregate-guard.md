| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-05-31 13:07 +0800 | DONE | feature/current-tail-aggregate-guard | - |

### DONE
- [x] [P1] 认领 `T2-P1-011`，确认任务卡为 `DOING` / `Spike`
- [x] [P1] 切换到 `feature/current-tail-aggregate-guard`
- [x] [P1] 收口 `ContextConfig` 与 `truncation.rs` 的阶段二配置漂移
- [x] [P1] 接入 mid-turn precheck + aggregate reduction + single branch_summary collapse
- [x] [P1] 补齐单测、集成测试、文档同步与交付收口
- [x] [P1] 补齐 keepalive A/B/C 真实 LLM 验证，`integration-real-llm` 分组全绿

### INTERFACE
- `context.compaction_turns` 已移除。
- 新增 `context.current_tail_compactable_min_chars`（默认 `1`）与 `context.current_tail_single_result_max_chars`（默认 `10_000`）。
- `context.keep_recent_turns` 现在真实驱动历史 placeholder 保护区（默认 `5`）。
- `context.compaction_model` 默认值改为 `gpt-5.2`；current-tail collapse / preheat 摘要默认沿用这条压缩模型口径。
- current-tail guard 在每次工具轮结束后、下一次 `llm.chat_stream(...)` 前执行：先吃历史收益，再减 current tail，不够时整份 collapse 为单条 `branch_summary + keepalive`。
- 新增 `tests/current_tail_guard_real_llm_tests.rs`，把 keepalive 真实验证拆成 A/B/C 三个串行 real-LLM cases，并已登记到 `scripts/test-groups.sh` 的 `TOMCAT_INTEGRATION_REAL_LLM_TESTS`。

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
- 已跑：`cargo test --lib current_tail_guard_behavior_test -- --nocapture`
- 已跑：`cargo test --lib current_tail_guard_runtime_test -- --nocapture`
- 已跑：`cargo test --lib steering_followup_test -- --nocapture`
- 已跑：`cargo test --lib rewrite_message_text_entries_by_id_updates_target_messages_only`
- 已跑：`cargo test --lib rewrite_local_tail_chars_updates_estimate_and_post_usage`
- 已跑：`cargo test --test context_management_tests`
- 已跑：`cargo test --test context_management_tests test_reasoning_loop_mid_turn_precheck_rewrites_before_second_llm -- --nocapture`
- 已跑：`cargo test --test current_tail_guard_real_llm_tests -- --nocapture --test-threads=1`
- 已跑：`set -a && source .env && set +a && cargo test --test plan_real_llm_inprocess_tests -- --nocapture --test-threads=1`
- 已跑：`set -a && source .env && set +a && ./scripts/run-integration-tests.sh integration-real-llm`
- 已核对：`docs/openspec/specs/User_Stories.md`、`docs/openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 与当前实现一致，无需额外补充用户面场景
- 已核对：新 real-LLM target 已接入 `scripts/test-groups.sh`；普通 `integration` / `all` 仍不跑 keepalive A/B/C，需显式执行 `integration-real-llm`
- 已提交：`1bb4d66`（阶段二实现）+ 后续 `style(rust)` 提交（补齐分支内遗留的 `cargo fmt` 格式化，无行为变更）
