| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Cursor | 2026-05-07 16:13 | ACTIVE | develop | — |

### 2026-05-07 | merge `feature/strengthen-four-core-tools` → develop @ a09ac01

- **阶段 R（评审）**：`scripts/test-groups.sh` 与合入变更面对照完整；`tests/` 下无无理由 `#[ignore]`；User_Stories / E2E 场景库与 read、bash 等已有自动化条目一致。
- **阶段 T（门禁）**：`RUST_LOG=pi_wasm=debug,info ./scripts/run-integration-tests.sh all` → `.integration_test_output.log` 末尾 `EXIT_CODE=0`（含 release、clippy、lib、integration 并行/串行、`cli_tests`、`wasmedge_e2e_tests` 等）。
- **看板**：T2-P0-016、T2-P0-017 本提交置为 `DONE`。

### 文档与 OpenSpec（无代码变更）

- [✓] **openspec `edit.md` 小节标题**：§7–§12 在 `##` 下补 `###` 子节并扩展「目录」锚点，改善侧栏大纲与渲染层次。
- [✓] **[P0]** 看板 **TASK_BOARD_002**（`README` + `tasks/`）：插入 **T2-P0-017**（`edit` 独立任务，锚 [edit.md](../docs/architecture/tools/edit.md)）；收敛 **T2-P0-016**；§1 交付任务数 18→19；§5 拓扑 `P005→P017`。
- [✓] **规范**：`ARCHITECTURE_SPEC` / `PLAN_SPEC` / `DOCUMENTATION_GUIDE` / `MODULE_README_SPEC` / `DEBUG_SPEC` / `PLAN_SKELETON` —「说人话」段落 + 表格列、去掉 12 岁表述；`Constitution` 二.10 改为先专业后口语。
- [✓] **read.md**：与 `edit`/`search_files` 同类章节编排收缩；**edit.md** 新增为冻结版 `edit` 工具方案。
- [✓] **其它**：`Architecture.md` / `interrupt-and-cancellation.md` / `search_files.md` / `plan-mode-execution-playbook` 小步对齐引用或措辞。

