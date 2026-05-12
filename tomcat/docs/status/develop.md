| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-05-08 10:30 | ACTIVE | develop | — |
| Jerry | 2026-05-10 23:20 | PENDING_INTEGRATION | feature/llm-files-upload-manager | — |

### 2026-05-10 | T2-P0-015 OpenAI Files 上传管理进入集成前状态

- **范围**：落地 `OpenAiFilesClient`（`upload/get/delete/list` + retry + `DELETE 404` 幂等）、`ChatMessageContentPart::{image_upload,file_upload}`、会话级双索引 cache（path/hash + inflight 单飞）、`read` inline/upload 决策注入、CLI/chat 退出 cleanup、`[llm.files] expires_after_seconds` 配置链路与文档同步。
- **阶段 T（门禁）**：已跑关键单测/焦小测（openai_files 模块、tool_exec oversize 路由、chat/session cleanup、config 边界与 env 覆盖、provider lazy files client）；`openai_files_integration_tests` 已新增并完成编译（默认 `#[ignore]`，手动触发真实 API）。
- **看板**：`TASK_BOARD_002` 中 `T2-P0-015` 更新为 `PENDING_INTEGRATION`（任务卡与总览同步）。

### 2026-05-08 | 架构文档 `llm-stream-events-cli-pipeline.md` 增补 §5.3

- **范围**：Thinking 端到端 ASCII 图、Responses SSE 解析流程图、完整 JSON 样例（A–F）与字段速查，便于对照 `openai_responses/stream.rs` 实现。
- **阶段 T**：文档-only，未重跑全量门禁。

### 2026-05-08 | merge `feature/thinking-api-display` → develop @ 8c7b86e

- **范围**：T2-P0-006（Thinking/Reasoning 事件链、`CliTurnRenderer`、`/thinking` 命令、persist transcript、协议解析与测试补全）+ 后续稳定性修复（`11739bc`：Responses reasoning 事件兜底 + 真实环境 E2E 稳健性）。
- **阶段 R（评审）**：对 `develop...feature/thinking-api-display` 的新增/变更代码做全量 review，发现并修复 1 个合并阻塞项：`openai_responses_integration_tests::test_openai_responses_chat_stream_reasoning_emits_thinking` 在上游不稳定无 thinking 输出时会误红（已改为有限重试 + 正文兜底断言，并补 parser 对 `reasoning_summary_part.*` / reasoning `output_item.done` 的提取能力与单测）。
- **阶段 T（门禁）**：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all` → `.merge_acceptance_after_fix3.log` 末尾 `EXIT_CODE=0`（含 release、clippy、lib、integration 并行/串行、`cli_tests`、`wasmedge_e2e_tests`）。
- **看板**：`T2-P0-006` 由 `PENDING_INTEGRATION` 置为 `DONE`（同步 `TASK_BOARD_002/README.md` 与 `tasks/T2-P0-006.md`）。

### 2026-05-07 | merge `feature/stream-timeout` → develop @ a067393

- **范围**：T2-P0-003（`openai` / `openai-responses` 流式 bytes 层空闲超时、`#T-132` 关闭、单测与 `docs/TODOS.md` 核对）。
- **阶段 T（门禁）**：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all` → 末尾 `EXIT_CODE=0`（含 release、clippy、lib、integration 并行/串行、`cli_tests`、`wasmedge_e2e_tests`）。
- **看板**：`T2-P0-003` 由 `PENDING_INTEGRATION` 置为 `DONE`。

### 2026-05-07 | merge `feature/strengthen-four-core-tools` → develop @ a09ac01

- **阶段 R（评审）**：`scripts/test-groups.sh` 与合入变更面对照完整；`tests/` 下无无理由 `#[ignore]`；User_Stories / E2E 场景库与 read、bash 等已有自动化条目一致。
- **阶段 T（门禁）**：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all` → `.integration_test_output.log` 末尾 `EXIT_CODE=0`（含 release、clippy、lib、integration 并行/串行、`cli_tests`、`wasmedge_e2e_tests` 等）。
- **看板**：T2-P0-016、T2-P0-017 本提交置为 `DONE`。

### 文档与 OpenSpec（无代码变更）

- [✓] **openspec `edit.md` 小节标题**：§7–§12 在 `##` 下补 `###` 子节并扩展「目录」锚点，改善侧栏大纲与渲染层次。
- [✓] **[P0]** 看板 **TASK_BOARD_002**（`README` + `tasks/`）：插入 **T2-P0-017**（`edit` 独立任务，锚 [edit.md](../docs/architecture/tools/edit.md)）；收敛 **T2-P0-016**；§1 交付任务数 18→19；§5 拓扑 `P005→P017`。
- [✓] **规范**：`ARCHITECTURE_SPEC` / `PLAN_SPEC` / `DOCUMENTATION_GUIDE` / `MODULE_README_SPEC` / `DEBUG_SPEC` / `PLAN_SKELETON` —「说人话」段落 + 表格列、去掉 12 岁表述；`Constitution` 二.10 改为先专业后口语。
- [✓] **read.md**：与 `edit`/`search_files` 同类章节编排收缩；**edit.md** 新增为冻结版 `edit` 工具方案。
- [✓] **其它**：`Architecture.md` / `interrupt-and-cancellation.md` / `search_files.md` / `plan-mode-execution-playbook` 小步对齐引用或措辞。

