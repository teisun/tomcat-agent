| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-06-19 07:53 +0800 | BLOCKED | feature/agent-server-ui-gateway | — |

### ✅ DONE (已完成/进行中)
- [x] **[P1]** 认领 `T2-P1-018`，同步任务卡/任务板并建立功能分支与状态台账 @2026-06-19
- [x] **[P1]** `tomcat serve --stdio` Phase 1 主体实现（CLI 入口、协议、writer、多会话、control、ask_question、schema） @2026-06-19
- [x] **[P1]** serve 专项单元/集成/E2E、schema fixture、test-groups 与跨文档收口完成 @2026-06-19
- [ ] **[P1]** 分支级 full acceptance / 任务卡移交 `PENDING_INTEGRATION` @2026-06-19

### 🔌 INTERFACE (接口变更)
- 已新增 `tomcat serve --stdio` CLI 子命令、serve wire 协议类型、`tomcat serve --print-schema` schema / `.d.ts` 工件导出路径。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| `cli_tests::test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli` | 仓库现有 real-LLM CLI 用例在本机完整 `integration-serial` 套件里偶发失败（单独重跑可通过）；与 serve 热区无关，但会阻塞任务卡从 `DOING` 升到 `PENDING_INTEGRATION` | 待单独修复/稳定该 CLI 用例后，重跑 `./scripts/run-integration-tests.sh integration-serial` 并完成移交 |
