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
9. **workspace-command** — 新增 workspace add/list/remove + ext_workspaces.json
10. **plugin-registry** — 新增 plugins/registry.json 全局注册表

## Verification

- `cargo build --release` ✓
- `cargo test --lib` 298 passed, 0 failed ✓
- `cargo clippy -- -D warnings` 0 warnings ✓
- user-guide.md 章节编号和内容已更新 ✓
- `openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 已同步：Story 1 调整为 9 条，新增 E2E-CLI-004（workspace），001 含 PATH 断言；071 含 `workspace` 帮助 ✓
- `tests/cli_tests.rs`：`test_workspace_add_list_remove_e2e`、`--help` 含 workspace、init 输出含 PATH ✓

## Breaking Changes

- `--config` flag removed from `pi init`, `pi doctor`, `pi config edit`
- `pi config export` / `pi config import` subcommands removed
- `pi session new --cwd` flag removed (replaced by `pi workspace add`)
- `WASMEDGE_QUICKJS_PATH` environment variable no longer recognized
