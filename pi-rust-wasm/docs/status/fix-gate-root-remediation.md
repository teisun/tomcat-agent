| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-05-01 17:45 | PENDING_INTEGRATION | fix/gate-root-remediation | - |

### Gate Root Remediation — 实现与文档同步

**范围**

- `GateConfig` / `DefaultPermissionGate`：默认可写根为 `agent_definition_dir`；启动 `cwd` / `agent_workspace_dir` 仅作 prompt 语义，访问需 `extra_roots`、会话授权、拖拽或 cwd lazy。
- `DefaultPrimitiveExecutor`：构造强制传入 `Arc<dyn PermissionGate>`，移除无 gate 的 legacy 路径。
- 文档与规格：`permission-system.md` 重写；`directory-structure`、`work-dir-and-data-layout`、`user-guide`、`User_Stories`、E2E 场景库、相关 status 与 `TASK_BOARD_002` changelog 对齐；`docs/TODOS.md` 安全项记为 `T-151`（避免与既有 `T-147` 编号冲突）。
- 代码注释：`types.rs`、`audit`、`confirmation`、`primitives` 等与审计字段、确认路径表述一致。

**验证命令（本提交前）**

- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- `cargo test --all-targets`：通过。

### INTERFACE

- `GateConfig.agent_definition_dir` 取代原 gate 侧「默认 workspace 根」语义；`GrantSource::AgentWorkspace` 仍指该目录（审计名保留）。
- `DefaultPrimitiveExecutor::new(..., gate: Arc<dyn PermissionGate>)` 必填；无 `with_gate` / `with_extra_roots` builder。

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
