| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| — | 2026-03-25 12:30 | ACTIVE | feature/user-guide-remediation | - |

### ✅ DONE（本提交相关）

- [✓] **VmActor 关停路径可观测性**：`waitForEvent` 入口 debug、`end_session` 各步 elapsed、`cleanup_instance` 的 `try_send(__shutdown)` 失败升级为 warn、`run_vm` / `_start` 耗时、`_start` 返回后 drain `cmd_rx` 并 warn
- [✓] **`docs/reports/vm-actor-shutdown-dead-code-analysis.md`**：三套管道（cmd_rx / 未用 event_rx / dispatcher 事件通道）、`Shutdown` 死代码根因与推荐改造（先 cleanup、JoinHandle 可选超时）

### 🔌 INTERFACE

无显著对外 API 变更（日志与内部诊断为主）。

### ⚠️ BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

---

# feature-user-guide-remediation Status

**Task**: TASK-12 | user-guide.md 整改与 bug 修复
**Branch**: feature/user-guide-remediation
**Status**: PENDING_INTEGRATION
**Date**: 2026-03-24

## Completed Items

1. **fix-stream-flush** — chat.rs 流式回调中 `print!` 后加 `flush`
2. **fix-prompt** — AI> 改为 `pi.{agentId}>`，你> 改为 `u>`
3. **audit-doc-fix** — user-guide 审计章节改为「默认开启」措辞
4. **remove-config-flag** — 移除 init/doctor/config edit 的 `--config` 标志，固定 DEFAULT_CONFIG_PATH，测试改 HOME env 隔离
5. **init-path-hint** — pi init 完成后输出 PATH 提示
6. **remove-export-import** — 移除 config export/import 子命令、测试、E2E、user-guide
7. **remove-env-overrides** — 移除 WASMEDGE_QUICKJS_PATH 死代码和 user-guide 环境变量附录
8. **fix-tool-call-history** — AgentRunResult 返回 new_messages，chat_loop 逐条写入 session
9. **workspace-command** — 新增 workspace add/list/remove + `pi.config.toml` `[workspace] extra_roots`（全局）
10. **plugin-registry** — 新增 plugins/registry.json 全局注册表
11. **TASK-16 / init-3step-workspace-cwd** — `pi init` 三步（环境 / doctor 同源检查 skip API Key / API Key）、默认 `gpt-5.2`、已有配置不覆盖、PATH 写入 shell（`# Added by pi init`）；`pi workspace add --cwd`；附录补充 `PI_WASM__LLM__DEFAULT_MODEL` 说明

## Verification

- `cargo build --release` ✓
- `cargo test -p pi_wasm --lib` 299 passed, 0 failed, 1 ignored ✓
- `cargo test -p pi_wasm --test cli_tests` 77 passed ✓
- `cargo clippy --all-targets -- -D warnings` 0 warnings ✓
- user-guide.md §2 init 三步、§5 `workspace add --cwd`、附录 `PI_WASM__LLM__DEFAULT_MODEL` ✓
- `openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` Story 1 与 E2E-CLI-001/004/005/010/017 等已同步（含 PATH 幂等补充说明）✓
- `tests/cli_tests.rs`：`test_init_auto_adds_path_to_shell_profile`、`test_init_path_export_idempotent_in_shell_profile`、`test_workspace_add_cwd_e2e`、`test_chat_without_config_exits_with_error`（隔离 `HOME` + `env_remove(PI_WASM__LLM__DEFAULT_MODEL)`）✓

## Breaking Changes

- `--config` flag removed from `pi init`, `pi doctor`, `pi config edit`
- `pi config export` / `pi config import` subcommands removed
- `pi session new --cwd` flag removed (replaced by `pi workspace add`)
- `WASMEDGE_QUICKJS_PATH` environment variable no longer recognized
