# 分支状态：feature/chat-path-env

## 本轮交付（多 agent / 全局工作区授权）

- **`AppConfig.workspace`**：`[workspace] workspace_roots`（`Vec<String>`），默认空；`validate_config` 与 `resolve_workspace_roots_paths` 做路径规范化、去重、须为已存在目录。
- **废弃** `{agent_dir}/ext_workspaces.json`：`pi workspace` 与 `ChatContext` / `DefaultPrimitiveExecutor` 均只读写 **`~/.pi_/pi.config.toml`**（`add/remove` 用 `load_config_toml_file` + 整表 `toml::to_string_pretty` + `write_file_atomic`；`list` 用 `load_config` 以反映环境变量覆盖）。
- **文档 / 看板**：`user-guide.md`、`work-dir-and-data-layout.md`、`E2E_SCENARIO_LIBRARY.md`、`TASK_BOARD.md`（TASK-09 P1 表述）已同步为 TOML 全局列表模型。

## 验证

- `cargo fmt`、`cargo clippy`、`cargo test`（含 `cli_tests` / 相关单测）应在当前分支通过。
