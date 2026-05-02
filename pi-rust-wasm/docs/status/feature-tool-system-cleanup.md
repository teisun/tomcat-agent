# feature/tool-system-cleanup 状态

| 字段 | 值 |
|---|---|
| Owner | Spike |
| State | PENDING_INTEGRATION |
| Branch | `feature/tool-system-cleanup` |
| Task | `T2-P0-005 | tool-system-cleanup` |
| Update Time | 2026-05-02 15:03 |
| Cov% | - |

## Step-by-Step

### 2026-05-02 14:50 | A.0 Bash audit 错配定位

1. 读取 `src/core/permission/gate.rs` 与 `src/core/tools/primitive/executor.rs` 的 Bash 授权链路。
2. 当前 `check_bash()` 默认允许路径返回 `PermissionScope::Bash`，approval-required 路径返回 `PermissionScope::BashApproval`。
3. `execute_bash()` 成功审计记录使用 Bash 决策结果，而不是 cwd/path 预检的 `Read/Write` 结果。
4. 因此计划中提到的 `FS → Exec` 更像旧任务表/报告口径残留；实际修复落点是把旧 `PermissionLevel` / `permission_level` 命名整体收敛为 `PermissionScope` / `permission_scope`，并补测试保证 Bash 审计不会回落到 `read` / `write` / `fs_*`。

## 当前进展

- 已创建并切换分支 `feature/tool-system-cleanup`。
- 已完成核心编译口径改造：`PermissionLevel`（权限 gate 语义）→ `PermissionScope`，`PrimitiveAuditEntry.permission_level` → `permission_scope`。
- 已新增内置工具 catalog 初版与 `gen-tool-catalog` 生成器，`cargo check` 通过。

### 2026-05-02 15:03 | 完成实现并进入集成复核

- T-033：Bash audit 现在记录 `permission_scope = "bash" / "bash_approval"`，新增 `execute_bash_audit_records_bash_scope` 断言 `grant_type = "bash_policy"` 且无 `fs_*`。
- T-034：新增 `src/core/tools/catalog.rs` 作为内置工具单一事实源；`build_tool_definitions()`、`CoreIdentitySection`、`docs/tool-catalog.md` 均从 catalog 派生；新增 `src/bin/gen-tool-catalog.rs` 与 `tests/tool_catalog_doc.rs` 防漂移。
- T-036：cwd lazy prompt 选择符修为 `[s/w/c]`；未识别输入会 warning 后按取消处理；拒绝授权后的失败回执提示下次触达 cwd 可重弹 `[s]/[w]/[c]`，或执行 `pi workspace add <cwd>` 永久授权。
- 文档同步：`docs/user-guide.md`、`openspec/specs/architecture/permission-system.md`、`docs/tool-catalog.md`、任务看板已更新。

## 门禁

- `cargo test -j 1 execute_bash_audit_records_bash_scope -- --test-threads=1`：PASS
- `cargo test -j 1 catalog -- --test-threads=1`：PASS
- `cargo test -j 1 cwd_lazy -- --test-threads=1`：PASS
- `cargo test -j 1 --test cwd_lazy_prompt_e2e -- --test-threads=1`：PASS
- `cargo test -j 1 --test cli_tests test_workspace_add_cwd_e2e -- --test-threads=1`：PASS
- Full gate：`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test -j 1 -- --test-threads=1`：PASS（`.integration_test_output.log`，`EXIT_CODE=0`）
