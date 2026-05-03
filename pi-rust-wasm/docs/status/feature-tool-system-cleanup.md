# feature/tool-system-cleanup 状态

| 字段 | 值 |
|---|---|
| Owner | Spike |
| State | PENDING_INTEGRATION |
| Branch | `feature/tool-system-cleanup` |
| Task | `T2-P0-005 | tool-system-cleanup` + `T2-P1-007 | tool-system-deferred-followups / #T-152 search_files` |
| Update Time | 2026-05-03 19:16 |
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
- 用户确认本次 `search_files` 工作不另开分支，继续在当前分支 `feature/tool-system-cleanup` 开发。

### 2026-05-02 15:03 | 完成实现并进入集成复核

- T-033：Bash audit 现在记录 `permission_scope = "bash" / "bash_approval"`，新增 `execute_bash_audit_records_bash_scope` 断言 `grant_type = "bash_policy"` 且无 `fs_*`。
- T-034：新增 `src/core/tools/catalog.rs` 作为内置工具单一事实源；`build_tool_definitions()`、`CoreIdentitySection`、`docs/tool-catalog.md` 均从 catalog 派生；新增 `src/bin/gen-tool-catalog.rs` 与 `tests/tool_catalog_doc.rs` 防漂移。
- T-036：cwd lazy prompt 选择符修为 `[s/w/c]`；未识别输入会 warning 后按取消处理；拒绝授权后的失败回执提示下次触达 cwd 可重弹 `[s]/[w]/[c]`，或执行 `pi workspace add <cwd>` 永久授权。
- 文档同步：`docs/user-guide.md`、`openspec/specs/architecture/permission-system.md`、`docs/tool-catalog.md`、任务看板已更新。

## 门禁

口径：[INTEGRATION_TEST_SPEC §7](../../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)（§7.1 / §7.2 / §7.4）；新增/调整 integration 二进制须同步 [`scripts/test-groups.sh`](../../scripts/test-groups.sh)（见 [Dispatcher §5](../../agents/Dispatcher.md)）。

- 焦测（节选）：`execute_bash_audit_records_bash_scope`、`catalog`、`cwd_lazy`、`cwd_lazy_prompt_e2e`、`cli_tests test_workspace_add_cwd_e2e`：PASS
- Full gate：`cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` · 分类集成（`./scripts/run-integration-tests.sh integration` 或等价流程）：PASS（`.integration_test_output.log`，`EXIT_CODE=0`）

### 2026-05-02 22:25 | 认领 #T-152 search_files

- 看板登记：新增独立 `T2-P1-008 | search-files-tool` 承接 `#T-152`，负责人 Spike，状态 `DOING`。
- 实施范围：新增内置 `search_files` 只读工具，单入口支持 `target=content|files`；依赖系统 `rg`/`fd`，缺失时返回安装指引，不做 fallback。
- 流程约束：继续使用当前分支，不创建 `feature/search-files-tool`。

### 2026-05-02 22:47 | #T-152 完成并交集成

- 看板登记调整为独立 `T2-P1-008 | search-files-tool`，状态 `PENDING_INTEGRATION`；`T2-P1-007` 保持后置项池，不混入已完成子项。
- 已实现 `search_files` catalog entry、`PrimitiveExecutor::search_files`、`tool_exec` 路由、system prompt 使用指引与 `docs/tool-catalog.md` 派生文档。
- 行为覆盖：`target=content` 支持 `files_with_matches` / `content` / `count`，`target=files` 使用 `fd` glob；缺少 `rg`/`fd` 返回安装指引；`PermissionScope::Read` 与 deny path_rules 生效。
- 新增 `tests/search_files_tests.rs` 5 个集成用例，使用临时 fake `rg`/`fd` 覆盖分页、glob、content/count、缺二进制、head_limit 边界与 deny 过滤。
- 门禁（口径同上；焦测含 `search_files_tests`、`catalog` / `system_prompt` / `tool_catalog_doc`）：PASS；全量分类集成 PASS。

### 2026-05-03 12:50 | T2-P0-005 子项 search_files 兜底与预检 → PENDING_INTEGRATION

承接计划 `~/.cursor/plans/search_files_兜底选型_c8b4a778.plan.md`，作为 T2-P0-005 增量推进；T2-P1-008 历史口径（"缺 rg/fd 返回安装指引"）维持不变。

- **工具层（双实现 + 同一 schema）**：`src/core/tools/primitive/executor.rs` 缺一回落（`fd` 缺 → `target=files` 走 Tier2；`rg` 缺 → `target=content` 走 Tier2；都缺 → 全 Tier2）；`SearchFilesArgs` / `SearchFilesOutput` 不变，差异写 `warnings`；audit 标 `implementation=tier1|tier2`。
- **Tier2 兜底**：`Cargo.toml` 引入 `ignore = "0.4"`（替代 `walkdir`），默认遵守 `.gitignore`/`.ignore`；`filter_entry` 阶段对 deny 路径剪枝（避免越权 IO）+ 叶子路径再校验；regex 编译失败 → 空命中 + warning（**不 panic / 不 Err**）；> 5 MiB 文件与 NUL 嗅探判定为二进制 → 跳过 + warning；单查询墙钟默认 10 s，可经 `PI_SEARCH_TIER2_DEADLINE_MS` 覆盖；同步 IO 入 `tokio::task::spawn_blocking`。
- **预检层**：`src/api/chat/preflight.rs` 实现 `start_search_tools_preflight`，在 `chat_loop` 注册完 stderr 监听后后台启动；按 `cfg!(target_os)` + `TERMUX_VERSION` 决策 brew / winget / apt-get / dnf / pacman / pkg；事件经 `WIRE_SEARCH_TOOLS_PREFLIGHT` 推 stderr，全程不阻塞会话；普通 Android App 不自动装。
- **预检开关**：`PreflightConfig.auto_install_search_tools` 默认 `true`；env `PI_SKIP_SEARCH_TOOLS_PREFLIGHT=1` 跳过安装动作；优先级 env > config > 默认；`pi.config.toml.example` 与 `CONFIG_READ_ALLOWLIST/CONFIG_WRITE_ALLOWLIST` 同步加上 `preflight.auto_install_search_tools`。
- **catalog 描述更新**：`src/core/tools/catalog.rs` 写明双实现、`.gitignore` 默认尊重、Tier2 注意事项与超时变量；`docs/tool-catalog.md` 由 `gen-tool-catalog` 重新派生。
- **测试矩阵 T1–T10 落到具体用例名**：
  - T3：`test_search_files_tier2_count_and_deny`
  - T5：`test_search_files_missing_binary_uses_tier2_content_fallback` / `test_search_files_missing_fd_uses_tier2_files_fallback`
  - T8：`test_search_files_tier2_lookaround_returns_empty_with_warning`
  - T9：`test_search_files_tier2_skips_binary_and_large_files`
  - T10：`test_search_files_tier2_include_hidden_toggle`
  - 预检：`should_skip_preflight_when_env_set` / `should_skip_preflight_when_config_disables_auto_install` / `trim_for_event_truncates_when_too_long`
  - 配置：`load_config_accepts_preflight_section`
- **架构文档**：新增 `openspec/specs/architecture/search_files.md`（含 One-Glance Map + 行为对照 + 预检策略 + 测试映射）。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`：PASS；`search_files_tests`（10 passed）、分类集成全量：PASS（口径见 [INTEGRATION_TEST_SPEC §7](../../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)、[`scripts/test-groups.sh`](../../scripts/test-groups.sh)）。
- **看板**：`agents/TASK_BOARD_002.md` T2-P0-005 子项追加「search_files 兜底与预检」，状态 `PENDING_INTEGRATION`，并写明与 T2-P1-008 的口径关系。

### 2026-05-03 14:45 | 集成验收口径与门禁文档对齐提交

- 门禁与流程：`Dispatcher` 全量前 `test-groups`、`TASK_BOARD` / `develop` status / `UNIT_TEST_SPEC` 验收引用统一为 **INTEGRATION_TEST_SPEC §7**（不再从 specs 链到 `agents/INTEGRATION_MERGE_AND_ACCEPTANCE`）；§7.2 正文列举与 `scripts/test-groups.sh` 对齐（含 `search_files_tests`、`tool_catalog_doc`）。
- 仓库：`Tomcat/.gitignore` 与 `pi-rust-wasm/.gitignore`  scratch 目录 **`workspace-temp/`**；本地已将目录 **`workspace` → `workspace-temp`**（若其他克隆仍有旧名请自行 `mv`）。
- 代码要点：`truncation` 空 `work_dir` 不落盘 `tool-results`；测试用 `agent_definition_dir` 子目录名 **`workspace-temp`**；`test-groups` 已含 `search_files_tests`。

### 2026-05-03 13:40 | 文档编写规范拆分与引用对齐

- `openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md` 精简为索引页；新增 `MODULE_README_SPEC.md`、`ARCHITECTURE_SPEC.md`；`openspec/specs/Architecture.md`、`agents/plan/PLAN_SPEC.md`、`PLAN_SKELETON.md` 中架构方案与 One-Glance Map 硬约束改为指向 `ARCHITECTURE_SPEC.md`（标杆：`architecture/search_files.md`）。
- `openspec/specs/architecture/search_files.md` 扩充协议表、竞品分析、时序与状态机 ASCII 图。
- 仓库根 `pi-rust-wasm/.gitignore` 忽略本地 `tool-results/` 与 `workspace-temp/`（研发 scratch 约定见 `UNIT_TEST_SPEC.md` §1.2），避免误提交。

### 2026-05-03 19:16 | search_tools 预检可观测性与 CLI 展示

- **预检**：包管理器每次安装尝试将完整 stdout/stderr 落盘 `~/.pi_/agents/main/logs/preflight-file-log-<ts>.log`；`tracing` target `pi_wasm_preflight`（`RUST_LOG=debug`）；成功/失败事件 `extra` 带 `logPath`；文档明确 `Command::output` 无 pi 侧超时，勿与 `PI_SEARCH_TIER2_DEADLINE_MS` 混淆；`PreflightConfig` 注释同步。
- **stderr 监听**：`WIRE_SEARCH_TOOLS_PREFLIGHT` 优先经 `rustyline::ExternalPrinter` 输出，避免 `readline` 阻塞时 `[tools]` 与输入行错位；失败时附加截断后的 stderr / error / log 路径摘要。
- **架构**：`search_files.md` One-Glance Map 补充上述行为。
