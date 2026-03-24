| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Agent | 2026-03-24 15:00 | DONE | feature/directory_refactor | — |

### 运行时目录与 Agent 配置重构

**目标**：将默认数据根从 `~/.pi_wasm/agents/default/...` 迁移至 `~/.pi_/` 布局；主配置更名为 `pi.config.toml`；默认 agent id 为 `main`；新增 `[agent]`（`id` / `agent_dir` / `workspace`）；sessions/logs/audit 独立从 `work_dir/agents/{id}/` 推导；插件与 QuickJS wasm 分别位于全局 `plugins/` 与 `assets/wasm/`。

#### 验收

| 项 | 结果 |
| :--- | :--- |
| `cargo test --lib` | PASS（全量单测） |
| `cargo clippy --lib` | PASS |
| `cli_tests` + `llm_tests`（需网络/代理与 OPENAI_API_KEY） | 用户本机代理就绪后 PASS |

### 🔌 INTERFACE（接口变更）

- `AppConfig` 新增 `agent: AgentConfig`（`id` 默认 `main`、`agent_dir`、`workspace` 可选）；crate 公开 `resolve_agent_dir`、`resolve_memory_dir`、`resolve_assets_dir`。
- `DEFAULT_SESSION_KEY`、`DEFAULT_CONFIG_PATH`、默认 `work_dir` 与配置文件名变更见上；路径推导：sessions/logs/audit 不经 `agent_dir`；`resolve_workspace_dir` → `workspace-{id}` 或可配置；`resolve_plugins_dir` → 全局 `plugins/`；`resolve_quickjs_path` → `assets/wasm/`。
- `ensure_work_dir_structure` 与 [directory-structure.md](../reports/directory-structure.md)、[work-dir-and-data-layout.md](../../openspec/specs/architecture/work-dir-and-data-layout.md) 对齐。

### ⚠️ BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
