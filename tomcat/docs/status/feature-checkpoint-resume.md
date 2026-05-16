| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-05-16 19:28 | PENDING_INTEGRATION | feature/checkpoint-resume | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P1]** 认领 `T2-P1-001`：任务卡状态改为 `DOING`、负责人改为 Jerry，并同步看板索引状态与负责人字段。
- [✓] **[P1]** 创建并切换工作分支 `feature/checkpoint-resume`。
- [✓] **[P1]** 已落地 `CheckpointStore` / `ShadowGitStore` / `NoopStore` / `SwitchingCheckpointStore`，接入 chat TurnEnd/Interrupt checkpoint、`/ckpt` `/restore`、`superseded`、`last_checkpoint_id`、启动期后台 prune 与 git preflight。
- [✓] **[P1]** 已补齐 checkpoint 相关单测 / 集成测试 / CLI E2E 与文档收口：新增 `checkpoint_integration_tests`、`checkpoint_cli_e2e`、`chat_git_preflight_tests`、`resume_plan_always_continue`、`startup_prune_scheduled_without_blocking_readline`、`turn_end_writes_checkpoint`、`interrupt_writes_checkpoint_after_partial_persist`，并修复 macOS `/var` vs `/private/var` 工作区别名导致的 checkpoint 仓分裂问题；`tool-catalog.md` 已回归到生成器单一事实源。
- [✓] **[P1]** 本卡边界内门禁已完成：已按 [`INTEGRATION_MERGE_AND_ACCEPTANCE.md`](../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) 使用 `scripts/run-integration-tests.sh integration` / `all` 后台日志模板执行，`.integration_test_output.log` 末尾两轮均为 `EXIT_CODE=0`；通过日志已留存为 `.integration_test_output.integration-pass.log` / `.integration_test_output.final-all-pass.log`。

### 🔌 INTERFACE (接口变更)
- 已新增 `core::checkpoint::*` 公共类型、`ShadowGitStore` / `NoopStore` / `SwitchingCheckpointStore`，并完成 `ChatContext` / `AgentLoopConfig` 的 `Arc<dyn CheckpointStore>` 注入路径。
- 已新增 `tomcat chat` 本地命令 `/ckpt list|show|diff`、`/restore <id> [--path ...] [--dry-run]`；`SessionEntry` 已扩展 `last_checkpoint_id`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 全仓 `cargo fmt --check` 仍有无关漂移 | 未触达的既有 `tests/*.rs` 存在格式差异；本卡修改文件已逐个 `rustfmt`/`--check` 并通过 | 如需清仓格式化，建议拆独立任务处理 |
| `wasmedge_e2e_tests` 不在本卡范围 | 用户已明确该套件与本次 checkpoint 卡无关；本卡验收以 `run-integration-tests.sh integration/all` 的 checkpoint 相关门禁为准 | 若要做仓库级 all-green，再单独处理该套件 |
