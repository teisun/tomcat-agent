| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-05-01 12:07 | PENDING_INTEGRATION | fix/drag-deny-cwd-remediation | - |

### T2-P0-013 基线 — Drag Deny / Bash / CWD 整改启动

**任务与计划**

- 看板任务：`T2-P0-013 | drag-deny-cwd-remediation`。
- 计划：`/Users/yankeben/.cursor/plans/drag-deny-cwd-remediation_8b74f52d.plan.md`。
- 分支：从 `develop` 切出 `fix/drag-deny-cwd-remediation`。

**当前基线**

- 目标：拖拽仅处理整行纯路径；deny/cancel 不进入 LLM；bash assignment RHS 进入权限预检；当前目录语义统一为 `agent_workspace_dir`；删除 legacy whitelist 配置入口。
- 已知工作区状态：`agents/TASK_BOARD_002.md` 为本任务认领修改；仓库外层存在既有 `../cc-fork-01` 与 `../hermes-agent/` 变动，非本任务范围。
- 预计测试矩阵：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、拖拽 / bash_parser / system_prompt / config 定向单测、`cwd_lazy_prompt_e2e`、新增 `bash_assignment_deny` 与 `system_prompt_cwd_priority` E2E。

### INTERFACE

- 已变更：`DragOutcome` 删除 `AutoAllow`；`DragHandleResult` 增加 `RecordUserAndSkip`；目录字段统一为 `agent_workspace_dir` / `agent_definition_dir` / `agent_trail_dir`；`PrimitiveConfig` 删除 legacy whitelist 字段。

### T2-P0-013 验收 — Drag Deny / Bash / CWD 整改完成

**实现结论**

- 拖拽授权收敛为「整行纯路径」才进入授权菜单；`PATH + 意图` 原样进入 LLM 后续工具授权流程。
- deny/cancel 拖拽只记录 `[drag-cancel]` 合成用户消息并跳回输入，不重建 `cancel_token`、不进入 `AgentLoop`。
- Bash `NAME=/path` 在命令前缀、位置参数、子命令首段三种形态均进入路径权限预检。
- 系统提示统一介绍 `agent_workspace_dir` / `agent_definition_dir` / `agent_trail_dir`，其中 `agent_workspace_dir` 只是当前目录语义来源；`WorkspaceStateSection` 仅列权限分类清单。
- `primitive.path_whitelist` / `primitive.bash_whitelist` / `primitive.auto_confirm_whitelist` 已从 schema、默认配置、工具读写面和本机 `~/.pi_/pi.config.toml` 中移除；旧 TOML 命中时显式报错并提示迁移。

**验证命令**

- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test --all-targets`：通过。
- `cargo test -- --ignored`：通过；真实 OpenAI ignored 用例已运行，doctest 中非可编译示例已改为 `text` 文档块。

**新增/重点覆盖**

- `tests/dragged_path_e2e.rs`：拖拽 deny/cancel 与 `PATH + 意图` silent passthrough。
- `tests/bash_assignment_deny.rs`：Bash assignment RHS deny path rule。
- `tests/system_prompt_cwd_priority.rs`：三目录说明与当前目录优先级。
- 单测覆盖 `dragged_path`、`dragged_handler`、`bash_parser`、`system_prompt`、`config_tool`、`infra::config`。

**提交**

- 实现提交：`981775c`（`fix(integration): 收敛拖拽 Bash CWD 语义`），已推送 `origin/fix/drag-deny-cwd-remediation`。

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
