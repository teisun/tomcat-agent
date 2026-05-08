| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Cursor Agent | 2026-05-08 11:05 | ACTIVE | feature/project_rename | — |

### 2026-05-08 | 宿主叙事对齐 tomcat + pi-mono 术语收敛

- **范围**：`tomcat/` 文档与 OpenSpec（Constitution、Product_Brief、Architecture 等）、架构与会话模块注释/说明；`compaction-prompt` / `context-management` 等报告；`edit.md` / `write.md` 横向表 OpenClaw 列措辞；`plugin_systems_*` / `pi_mono_gap_analysis` 等少量消歧。保留 `globalThis.pi`、`pi_bridge`、`pi-mono` 契约表述不变。
- **阶段 T（门禁）**：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all` → `tomcat/.integration_test_output.log` 末尾 `EXIT_CODE=0`（release、clippy、lib、integration 含 `cli_tests` / `wasmedge_e2e_tests`）。

### ✅ DONE（本分支交付意图）

- [✓] **[P0]** 仓库根以 **`tomcat/`** 为唯一 crate 树承接原 `pi-rust-wasm/`，并移除旧路径下已跟踪文件。
- [✓] **[P1]** 文档与规范中宿主名、数据路径与 **pi-mono 生态** 表述分层，避免将本仓误读为上游 `pi` CLI。

### 🔌 INTERFACE（接口变更）

- 无对外 Rust API 签名变更；CLI 与配置路径以 `tomcat` / `~/.tomcat/` 为准（详见 `README.md` 与 `work-dir-and-data-layout.md`）。

### ⚠️ BLOCKED（阻塞/风险）

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
