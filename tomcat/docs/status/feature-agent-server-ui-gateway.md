| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-06-19 15:48 +0800 | PENDING_INTEGRATION | feature/agent-server-ui-gateway | — |

### ✅ DONE (已完成/进行中)
- [x] **[P1]** 认领 `T2-P1-018`，同步任务卡/任务板并建立功能分支与状态台账 @2026-06-19
- [x] **[P1]** `tomcat serve --stdio` Phase 1 主体实现（CLI 入口、协议、writer、多会话、control、ask_question、schema） @2026-06-19
- [x] **[P1]** serve 专项单元/集成/E2E、schema fixture、test-groups 与跨文档收口完成 @2026-06-19
- [x] **[P1]** 分支级 full acceptance / 任务卡移交 `PENDING_INTEGRATION` @2026-06-19

### 2026-06-19 | acceptance: branch ready for integration

- **验证**：`./scripts/run-integration-tests.sh integration-serial` 通过；`./scripts/run-integration-tests.sh integration-openai-responses-wire` 通过。
- **补修**：修复 `cli_tests` 自定义 `models.toml` 被 helper 覆盖、`init`/`resume` 迁移后旧断言、`docs/tool-catalog.md` 快照漂移。
- **说明**：`openai_files_integration_tests` 继续保留 `#[ignore]`，并新增 `PI_LIVE_OPENAI_FILES=1` 显式 opt-in；默认 wire 验收不再因网关未开 Files 能力而误报失败。

### 🔌 INTERFACE (接口变更)
- 已新增 `tomcat serve --stdio` CLI 子命令、serve wire 协议类型、`tomcat serve --print-schema` schema / `.d.ts` 工件导出路径。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
