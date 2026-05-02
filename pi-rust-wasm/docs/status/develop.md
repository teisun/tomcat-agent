| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-05-02 10:16 | INTEGRATED | develop | — |

### 集成测试报告 — `feat/path-command`（chat 命令模块化 + `/path` 显式授权，T2-P0-013/014 follow-up）

**合并信息**

- 用户登记的提交范围：`981775c`、`6e3138b`、`30ecf02`、`623e94c`、`274fe3d`。`git branch --contains` 与 `git log develop..HEAD` 显示前 4 个提交已随 `feature/permission-source-redesign` `--no-ff` 合并入 develop（merge `623e94c`，集成报告见下一块），本次实际只新引入 `274fe3d`。
- 源分支（tip）：`feat/path-command` @ `f0ed1b5`（含 `274fe3d` + Nibbles 集成补漏 commit `f0ed1b5`）
- 合并 commit：`398e1a6 merge: feat/path-command (chat 命令模块化 + /path 显式授权)`
- 合并策略：`--no-ff`，ort 无冲突
- 负责任务：T2-P0-013 / T2-P0-014 follow-up（主任务状态保持 `DONE`）

**§1 规格 & 场景库核对**

- `User_Stories.md` Story 2 已切换至 `/path <路径>` + `/help` 语义：拖拽/粘贴路径回车按普通聊天发给 LLM，仅显式 `/path` 进入授权菜单；`/help` 列出本地命令；条目与代码完全对应。
- `E2E_SCENARIO_LIBRARY.md` 覆盖 `/path` 命令路径：E2E-CLI-018 `path_with_intent_silent_passthrough_contract`（自动）、E2E-CLI-019 `manual_path_command_denied_shows_cancel_only`（人工 + 自动回归 `path_menu_with_deny_rule_hides_authorization_choices`）、E2E-CLI-023 `deny_path_command_menu_only_allows_cancel_contract`（自动）、E2E-CLI-026 `path_help_command_contract`（自动）。
- `permission-system.md`、`work-dir-and-data-layout.md`、`docs/user-guide.md` 已对齐 `/path` 命令为路径授权 UI；`DraggedPathMenu` trigger 名称为兼容历史审计保留。

**§2 / §3 测试 review + Nibbles 补漏**

- 集成 review 发现 `274fe3d` 在 `commands/{cmd_path,cmd_help,parse}.rs` 末尾保留了 `#[cfg(test)] mod tests { ... }` 内联块，违反 [`RUST_FILE_LINES_SPEC.md §A.7`](../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)（业务源文件不内联 tests）。Nibbles 在 `feat/path-command` 分支提交 `f0ed1b5 test(chat): 拆分 commands inline tests 并补 /help E2E 契约`：把 18 个单测整体迁到 `src/api/chat/commands/tests/{parse,cmd_help,cmd_path}.rs`（默认父目录 `tests/mod.rs` 挂载，未触发 `#[path]` 例外，未为测试放宽可见性），业务文件回到 L-1 黄金区间——`cmd_path.rs` 481→304、`parse.rs` 108→85、`cmd_help.rs` 45→22。
- 集成 review 发现 `tests/path_command_e2e.rs` 仅 2 个用例，与 `E2E_SCENARIO_LIBRARY` 标注「E2E-CLI-026 自动」不一致。`f0ed1b5` 同步追加 `help_command_lists_path_and_help_contract`（断言 `parse_chat_command("/help") == Help`，并锁定 `help_text` 文案含 `/path` / `/help` / 「绝对路径」）与 `path_command_usage_errors_e2e_contract`（缺参 / 多参 → `UsageError`，大写 → `NotACommand`）；为支撑离线 e2e，`commands/mod.rs` 新增最小 `pub fn help_text()` 门面，未升 `cmd_help::help_text` 私有可见性。
- 编码规范家族家族对照：分层 / idioms / 注释 PASS。模块拆分把 `chat/{commands,events,permission}/` 与 `core/tools/{primitive,config,registry}/` 拆出后，权限决策仍单点收敛于 `PermissionGate`，`/path` 菜单 `[a]` 通过 `SessionGrants` 与 executor / system prompt 共享同一份视图，无重复实现。
- 既有 L-2 黄金预警留痕：`api/chat/mod.rs` 916、`core/tools/config.rs` 824、`core/tools/primitive/executor.rs` 653 仍在黄区，与上一轮集成报告口径一致，记 follow-up 不阻塞合并。

**§4 全量门禁**（在 develop 合并后、`pi-rust-wasm` 根目录；`source .env` + `source ~/.wasmedge/env` + `DYLD_FALLBACK_LIBRARY_PATH=$HOME/.wasmedge/lib`）

| 命令 | 结果 |
| :--- | :--- |
| `cargo fmt --check` | 通过 |
| `cargo clippy --all-targets -- -D warnings` | 零警告 |
| `cargo build --release` | 通过 |
| `RUST_LOG=pi_wasm=debug,info cargo test -j 1 -- --nocapture --test-threads=1` | **lib 572 passed / 0 failed / 1 ignored；integration 19 crate 207 passed / 0 failed；doc 0 passed；EXIT_CODE=0**，日志 `.integration_test_output.log` |
| `RUST_LOG=pi_wasm=debug,info cargo test -j 1 --test '*' -- --nocapture --test-threads=1` | **integration 18 crate 207 passed / 0 failed**（含 `cli_tests` 77、`wasmedge_e2e_tests` 39、`path_command_e2e` 4），日志 `.integration_only.log` |

**编码规范家族对照**

| 规范 | 结果 | 备注 |
| :--- | :--- | :--- |
| `Codeing&Architecture_Spec.md` | 通过 | 分层清晰：commands 子模块负责 chat 本地命令解析与分发；core/tools 集中聚合 4 原语执行器与 config 工具后端；权限单点收敛于 `PermissionGate` |
| `RUST_FILE_LINES_SPEC.md` | 通过 | 本次新增 `commands/{cmd_path,cmd_help,parse}.rs` 经 `f0ed1b5` 修复后均落 L-1；既有 L-2 大文件预警保留 follow-up |
| `RUST_IDIOMS_SPEC.md` | 通过 | `clippy --all-targets -D warnings` 零警告 |
| `COMMENT_SPEC.md` | 通过 | 新模块均带模块级 `//!` 与关键决策注释；无降级断言 / `#[ignore]` 糊弄 |

**结论**

`feat/path-command` 集成验收**通过**：`274fe3d` + Nibbles 补漏 `f0ed1b5` 通过单次 `--no-ff` 合并 tip 进入 develop，全量门禁绿灯。本次涉及任务（T2-P0-013 / T2-P0-014）在看板上的 `DONE` 状态保持不变，仅在「6. 变更记录」追加 follow-up 一行。

---
