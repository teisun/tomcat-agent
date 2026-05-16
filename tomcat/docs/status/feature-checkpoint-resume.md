| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-05-16 19:55 | DONE | develop @ 78d7518 | - |

### ✅ DONE (已完成)
- [✓] **[P1]** 认领 `T2-P1-001`：任务卡状态由 `PENDING_INTEGRATION` 推进至 `DONE`，看板索引同步。
- [✓] **[P1]** 全量 review `feature/checkpoint-resume` 对齐 [`docs/architecture/tools/checkpoint-resume.md`](../architecture/tools/checkpoint-resume.md)：
  - §4.1 决策表 C1–C14：影子 Git / `NoopStore` / `SwitchingCheckpointStore`、仅 TurnEnd+Interrupt 自动写、`(session_id, turn_id, kind)` dedup、不进 tool catalog、`/restore [--path][--dry-run]` + `/ckpt list|show|diff`、启动期后台 prune、`auto_install_git` preflight、`compute_resume_plan` 恒 `Continue`、TurnEnd 前 pre-rollback 失败 fatal — 与代码完全一致。
  - §5 协议：`CheckpointStore` trait / `CheckpointKind` / `CheckpointMeta` / `last_checkpoint_id` / `superseded` + `Custom{checkpoint.restore}` 仅 TurnEnd/Interrupt restore；§9 配置 `[checkpoint] retention_max/retention_days` + `[preflight] auto_install_git` 已对接 `AppConfig`。
  - §11 测试矩阵：`checkpoint_integration_tests`、`checkpoint_cli_e2e`、`chat_git_preflight_tests`、`init_context_state_skips_superseded_messages`、`resume_plan_always_continue`、`turn_end_writes_checkpoint`、`interrupt_writes_checkpoint_after_partial_persist` 均落地并通过。
  - 结论：**未发现规范偏差**，本卡边界内无须修复。
- [✓] **[P1]** 合并 `feature/checkpoint-resume` → `develop` @ `78d7518`（`--no-ff`，保留 PR-CKA..F 历史）。
- [✓] **[P1]** 按 [`INTEGRATION_MERGE_AND_ACCEPTANCE.md §4`](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) 在 `develop` 上复跑全量门禁：
  - §4-1 `cargo build --release` + `cargo clippy --all-targets -- -D warnings` + `cargo test --lib`：**lib 890/0 passed**，clippy 无 warning。
  - §4-3 `scripts/run-integration-tests.sh integration`（并发组 + 串行组）：
    - parallel：`agent_loop_tests` 11、`audit_tests` 1、`bash_assignment_deny` 1、`chat_git_preflight_tests` 3、`checkpoint_integration_tests` 4、`context_management_tests` 19、`cwd_lazy_prompt_e2e` 6、`event_tests` 4、`llm_tests` 2、`openai_responses_integration_tests` 7、`path_command_e2e` 4、`plugin_tests` 3、`read_tool_tests` 6、`robustness_tests` 5、`search_files_tests` 10、`session_tests` 4、`system_prompt_cwd_priority` 1 — **全绿**。
    - serial：`checkpoint_cli_e2e` **5/0**、`cli_tests` **78/0**、`hostcall_tests` 5、`js_api_alignment_tests` 2、`long_lived_vm_tests` 13、`primitives_tools_tests` 10、`tool_catalog_doc` 1、`wasmedge_e2e_tests` **39/0** — **全绿**。
    - `.integration_test_output.log` 末尾 `EXIT_CODE=0`。

### 🔌 INTERFACE (接口已生效在 develop)
- `core::checkpoint::*`：`CheckpointStore`、`ShadowGitStore`、`NoopStore`、`SwitchingCheckpointStore`、`CheckpointId/Kind/Meta`、`RetentionPolicy`、`ResumePlan`（恒 `Continue`）已对外。
- `AgentLoopConfig.checkpoint_store: Arc<dyn CheckpointStore>` / `ChatContext.checkpoint_store` 注入闭环。
- `tomcat chat` 内建命令：`/ckpt list|show|diff`、`/restore <id> [--path …] [--dry-run]`；`SessionEntry.last_checkpoint_id` 持久化于 sessions 索引；`Custom{checkpoint.restore}` 审计事件写入 transcript。
- 配置：`[checkpoint] retention_max` / `retention_days`、`[preflight] auto_install_git`（`tomcat.config.toml.example` 已更新）。

### ⚠️ BLOCKED / 风险
| 项 | 状态 |
| :--- | :--- |
| 全仓 `cargo fmt --check` 历史漂移 | 本卡修改文件已 `rustfmt --check`；其余无关漂移建议拆独立清仓任务。 |
| 无 git 环境 | `NoopStore` 兜底；启动期 `start_git_preflight` 后台探测，发现 git 后惰性切到 `ShadowGitStore`，不阻塞 `tomcat chat`。 |
