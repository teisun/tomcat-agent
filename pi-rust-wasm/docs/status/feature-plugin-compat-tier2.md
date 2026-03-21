### 元数据

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-03-22 | PENDING_INTEGRATION | feature/plugin-compat-tier2 | （未跑 tarpaulin） |

### 任务

- [x] **[P1]** TASK-05c：pi-mono Tier 2（命令 + exec argv + 基础 UI + TypeBox parameters 规整 + sendMessage 写会话）

### INTERFACE

- `PrimitiveExecutor::execute_bash(..., argv: Option<&[String]>)`：无 argv 时 `sh -c`；有 argv 时 `Command::new(cmd).args(argv)`
- `HostApiDispatcher::normalize_tool_parameters`（`pub(crate)`）、`registered_plugin_commands`
- `context.uiSelect` / `uiConfirm` / `uiInput` / `uiSetStatus`：确定性宿主响应（非 `{stub:true}`）
- `agent.sendMessage` / `sendUserMessage`：注入 `SessionManager` 时写入当前会话 transcript；`options.silent` 跳过追加
- `assets/js/pi_bridge.js`：`__pi_commands`、`registerCommand` 存 handler、`__pi_invoke_command`、`ctx.ui.setStatus`

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 验收自测

- `cargo test -j 1 -p pi_wasm --lib --tests -- --test-threads=1` 全通过
- Wasm E2E：`test_wasmedge_e2e_tier2_compat_script`、`test_wasmedge_e2e_tier2_transpiled_export_default_plugin`
- 场景库：E2E-WASM-037、E2E-WASM-038
