| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-04-28 14:25 | DONE | feature/drag-deny-cwd-runtime | - |

### DONE（拖拽授权、deny 规则与目录语义整改）

- [✓] 修复拖拽 AutoAllow、菜单授权、CwdLazyPrompt 与 config_set 等入口绕过 deny 的风险。
- [✓] `PermissionGate` 新增当前会话运行时 path_rules，`[r]/[d]` 与 `config_set primitive.path_rules` 写盘后立即生效。
- [✓] 统一 `cwd_snapshot`、`agent_workspace_trail`、`agent_workspace_definition` 的代码与 Prompt 语义。
- [✓] 将 Layer0 tool-results 从设计态 workspace 迁移到运行态 agent 目录：`agent_workspace_trail/tool-results/{session_id}/`。
- [✓] 补齐拖拽路径切分、运行时 path_rules、生效顺序、Layer0 路径、二进制读取反馈回归测试。

### INTERFACE（接口变更）

- `PermissionGate` 新增 `grant_path_rule(PathRule)`，用于当前会话内热追加 deny / readonly。
- `ConfigToolContext` 可携带共享 `PermissionGate`，用于 `config_set` 前置 deny 预检与 path_rules 热生效。
- `AgentLoopConfig.agent_workspace_trail`（原字段名 `work_dir`）：Agent 运行态轨迹目录字符串，用作 Layer0 落盘根；命名与 `ChatContext::agent_workspace_trail` 对齐，避免与 `storage.work_dir` / cwd 混淆。

### TEST（门禁）

- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo build --release`：通过。
- `cargo test -j 1 -- --nocapture --test-threads=1`：通过，lib 571 passed / 0 failed / 1 ignored；integration 201 passed / 0 failed；doc 0 passed / 1 ignored。

### BLOCKED（阻塞/风险）

- 无。
