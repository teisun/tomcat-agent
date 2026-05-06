| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-05-07 18:30 | ACTIVE | feature/strengthen-four-core-tools | - |

### 2026-05-07 | bash AST 默认 `enabled=false` + P1 任务 T2-P1-009

- **动机**：`detect_unsupported` 手写粗匹配易误伤（意图/对标见本机 Cursor 计划 `~/.cursor/plans/bash_detect_unsupported_对比_ac5d829c.plan.md`）。
- **代码**：`DefaultPrimitiveExecutor::new` 默认 `BashAstChecker::new(false, [], [])`；`ToolsBashAstConfig::default().enabled = false`；[bash-pr-l-scope.md §4](../architecture/tools/bash-pr-l-scope.md) 默认值与任务卡同步。
- **看板**：新建 [T2-P1-009.md](../../agents/TASK_BOARD_002/tasks/T2-P1-009.md)（简卡 + 计划外链）；[README.md](../../agents/TASK_BOARD_002/README.md) 索引；[T2-P0-016.md](../../agents/TASK_BOARD_002/tasks/T2-P0-016.md) bash 子项链至 T2-P1-009。
- **测试**：`suite_test` 三条 `bash_ast_*` 仍显式 `.with_bash_ast(BashAstChecker::new(true, …))`，行为不变。

### 2026-05-07 | T2-P0-016 子项 `bash` PR-L（T3 AST allowlist + SandboxBackend 骨架）落地

- **范围冻结依据**：[bash-pr-l-scope.md](../architecture/tools/bash-pr-l-scope.md)（plan §六风险表「Phase-L AST/Sandbox 范围未定义」的关闭文）。
- **新模块** `src/core/permission/bash_ast.rs`：
  - `BashAstChecker { enabled, allowlist, denylist }`：手写切段（识别 `;` `&` `\n` `&&` `||` `|` 顶层操作符；引号 / 反引号 / `$(...)` / `(...)` 一律按字面量处理，**不**触发外层切段；MVP 拒 heredoc `<<` 与 `for/while/until/if/case/function/select/{` 流程控制）+ allowlist/denylist 命中判定（命中 deny → `AstReject::AstDeny`；命中 allow → `AstSegmentVerdict::AllowedSkipApproval`；其余 → `AstSegmentVerdict::Defer`，由调用方 fallback 旧 gate 三层）。
  - `SandboxBackend` trait + `NoopSandboxBackend`：占位接口，PR-L 内仅交付 `Noop`（直接 `cmd.spawn()`，与 PR-E.2 行为字节级等价）；后续 PR 挂 macOS Seatbelt / Linux Landlock 时仅替换 `Arc<dyn SandboxBackend>` 注入。
  - `PersistentShell` trait 占位：真 PTY 循环不在 PR-L 范围。
  - `ToolsBashAstConfig`：`{ enabled, allowlist, denylist, sandbox_backend }`；**默认 `enabled=false`**（与 executor 一致）。**当** `enabled=true` 且 allow/deny 皆空时，仅切段 + `detect_unsupported`、不做列表命中，除 unsupported 命中外与旧 gate-only 字节级等价（scope spec §4）。
- **wire**：`DefaultPrimitiveExecutor` 新增 `bash_ast: BashAstChecker` 字段 + `with_bash_ast` builder；`executor/bash.rs::execute_bash_impl` 在 `gate_check_bash` **之前**调 `executor.bash_ast.check(&audit_cmd)` —— 任何 `AstDeny` / `AstUnsupported` 早退 + 审计 `success=false`，不进入 gate / 不 spawn。
- **测试**：
  - `bash_ast` 模块 14 个 `#[cfg(test)]`：disabled 单段 Defer / 拆 ; && || | / deny 短路 / allow 跳 approval / 赋值前缀属性 / 子 shell 字面量 / 子 shell 内分隔符不切段 / 子 shell 未配对 / 引号未配对 / 流程控制拒 / heredoc 拒 / glob 前缀模式 / NoopSandboxBackend spawn echo。
  - `suite_test` 3 个端到端 `#[tokio::test]`：`bash_ast_allowlist_denies_compound_command_short_circuit`（`git --version && rm -rf <probe>` deny 命中早退 + 磁盘 fixture 文件未被删）/ `bash_ast_default_empty_lists_keeps_legacy_behavior`（空 list 行为等价回归）/ `bash_ast_heredoc_returns_unsupported_error`（heredoc 早退）。
  - `cargo test --lib` 765 全绿（748 → 762 PR-L.bash_ast 14 例 → 765 PR-L.suite 3 例），`cargo fmt --check` / `cargo clippy --all-targets -D warnings` 全绿；`agent_loop_tests` / `bash_assignment_deny` / `cli_tests` / `primitives_tools_tests` / `tool_catalog_doc` 集成测 100% 回归通过。
- **不变量**：`PrimitiveExecutor::execute_bash` trait 方法名 / dispatcher / 所有 mock / `wasmedge_e2e_tests` host_call 全部未动；**默认 `enabled=false`** 时 `bash_ast.check` 不切段，与无 AST 栈一致；**显式 `enabled=true` 且空 allow/deny** 时除 `detect_unsupported` 早退外 `gate_check_bash` 与 PR-E.2 一字不差（scope spec §4）。
- **遗留 / 后续 PR**：
  - `[tools.bash.ast]` config 段反序列化 + `api/chat` 装配注入：本 PR 仅暴露 builder（`with_bash_ast`），TOML 接入留给后续 PR；
  - `AllowedSkipApproval` 当前**未**在 `gate_check_bash` 上真正跳 approval（仅切段判定；deny 已早退）：跳 approval 的接线动会牵动 grant trace + 审计字段，独立 PR 处理更安全；
  - 真实 SandboxBackend / PersistentShell 实现：按 scope spec §3「显式不在 PR-L 内」。

### 🔌 INTERFACE (接口变更，T2-P0-016 bash 子项 PR-L)

- **新模块**：`crate::core::permission::{ BashAstChecker, AstReject, AstSegmentVerdict, BashSegment, SandboxBackend, NoopSandboxBackend, PersistentShell, ToolsBashAstConfig }`。
- **`bash` 工具运行时行为**：**默认 `executor.bash_ast.enabled=false`**，不跑 AST 前置；**`with_bash_ast(..., true, ...)` 或未来 TOML 打开后**：空 allow/deny 时与 PR-E.2 字节级等价（除 unsupported 早退）；命中 deny → `AppError::Primitive("AstDeny: ...")` + 审计 `success=false`；命中 unsupported（heredoc / 流程控制） → `AppError::Primitive("AstUnsupported: ...")`。
- **`DefaultPrimitiveExecutor` builder**：新增 `with_bash_ast(BashAstChecker)`；不调用时默认 `BashAstChecker::new(false, [], [])`（与无 AST 等价）。
- **配置**：`[tools.bash.ast].{ enabled, allowlist, denylist }` 与 `[tools.bash.sandbox].backend` 已在 `ToolsBashAstConfig` 定义；TOML 反序列化接入待后续 PR。

### 2026-05-07 | T2-P0-016 子项 `bash` PR-I（T2 后台三件套）落地

- **新模块** `src/core/tools/primitive/bash_task.rs`：`BashTaskRegistry` + `BashTask{ info, pid }` + `spawn / read_output / stop / list` 四个 API；锁分层避开「stop 等 wait」死锁——`Child` 句柄 move 进 wait 任务独占 `await`、stop 路径靠 `pid → libc::killpg(SIGKILL)` 不依赖句柄；wait 任务感知 `child.wait()` 返回时**不**回退覆盖 `Stopped`（人为 stop 不会被「自然 Finished」误判）。
- **schema 扩展**：`bash_parameters` 新增 `run_in_background?: bool`；catalog 新增 `task_output / task_stop / task_list` 三个工具，schema 分别为 `{ task_id, since? }` / `{ task_id }` / `{}`；`docs/tool-catalog.md` 重派生。
- **wire**：`tool_exec` `bash` 分支新增 `run_in_background=true → handle_bash_background` 走注册表立即返回 ticket；新增 `task_output / task_stop / task_list` 三分支；execute_tool 签名扩 `bash_task_registry: &Option<Arc<BashTaskRegistry>>`，未注入时四个新路径返回「未启用」错误（与 `config_get/set` 同形态），同步 bash 路径完全不变。
- **AgentLoop**：`types.rs` 加 `bash_task_registry: Option<Arc<BashTaskRegistry>>` 字段；`accessors.rs` 加 `with_bash_task_registry` builder + 两条构造器都初始化为 `None`；`tool_dispatcher` 透传 `&agent.bash_task_registry`。
- **api/chat 装配**：`ChatContext` 新增 `bash_task_registry: Arc<BashTaskRegistry>` 字段，`from_config` 用 `<agent_trail_dir>/tool-results/` 作 `persist_dir` 构造；turn 启动时 `agent_loop.with_bash_task_registry(ctx.bash_task_registry.clone())` 注入。
- **测试**：`bash_task` 模块 3 个 `#[cfg(test)]`（spawn→read→stop→list 全链 / 自然 Finished 携带 exit_code / 未知 task_id 错误）；`tool_exec` 4 个新 `#[tokio::test]`（未注入 registry 时 background bash / task_output / task_list 友好错误 + 真实 registry 跑通 background bash → task_output → task_stop → task_list 全 lifecycle，断言 ticket JSON / chunk JSON / list 中状态 = `stopped`）；`cargo test --lib` 748 全绿（738 → 741 PR-E.4 → 744 PR-I.bash_task → 748 PR-I.tool_exec），`cargo fmt --check` / `cargo clippy --all-targets -D warnings` 全绿；现有 `gate_suite_*` / `bash_assignment_deny` / `agent_loop_tests` / `cli_tests` / `primitives_tools_tests` / `tool_catalog_doc` 集成测全部 100% 回归通过。
- **不变量**：`PrimitiveExecutor::execute_bash` trait 方法名 / dispatcher `("primitive","executeBash")` / 所有 mock 全部未动；新增 `task_*` 三件套**不**走 PrimitiveExecutor（直接 tool_exec ↔ BashTaskRegistry）—— extension / dispatcher 路径完全不感知。

### 🔌 INTERFACE (接口变更，T2-P0-016 bash 子项 PR-I)

- **LLM 工具名**：新增 `task_output` / `task_stop` / `task_list`；`bash` 入参新增 `run_in_background?: bool`（默认 false）。
- **`bash run_in_background=true` 出参**：`{ taskId, logPath, startedAtUnixMs }`（camelCase JSON）；同步路径仍返回原 BashResult 文本格式。
- **`task_output` 出参**：`{ taskId, content, startOffset, nextOffset, finished, exitCode? }`；`finished=true` 时 `exitCode` 一定有值（`Stopped` → `-1`，`Finished{ code }` → 实际退出码）。
- **`task_list` 出参**：`[{ taskId, command, startedAtUnixMs, logPath, status: { state, exitCode? } }]`；按 startedAtUnixMs 升序。
- **持久化路径**：与 PR-E.3 共用 `<agent_trail_dir>/tool-results/`，文件名 `bash-<taskId>.log`；模型可用 `read` 工具按路径取尾部全文。

---


### 2026-05-06 | T2-P0-016 子项 `bash` PR-E（命名闸 + T1 超时 + 输出累积）全部 6 个 phase-e-* 子 todo 完成

- **PR-E.0 命名闸**：`catalog::execute_bash → bash`、`tool_exec` `match "bash"`、`system_prompt` 旧名移除；`session/manager/context.rs::warn_if_legacy_tool_name` 追加 `execute_bash → bash` OnceLock 节流 warn；`docs/tool-catalog.md` 重派生（schema 含 `args` + `timeout_ms`）。trait `PrimitiveExecutor::execute_bash` 方法名 / dispatcher `("primitive","executeBash")` / 所有 mock / `wasmedge_e2e_tests` 中的 `executeBash` host_call 名 全部未动（与 read/write/edit 同型）。
- **PR-E.1 Schema + Config**：`catalog.rs` 的 `bash_parameters` 补 `args: array<string>`（PR-A 尾扫）+ 新增 `timeout_ms: integer`（min=1, max=600_000）；`infra::config::types` 新增 `ToolsBashConfig { timeout_ms: u64 = 120_000, max_output_chars: usize = 30_000 }` + `DEFAULT_TOOLS_BASH_TIMEOUT_MS / MAX_TOOLS_BASH_TIMEOUT_MS / DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS / MAX_TOOLS_BASH_MAX_OUTPUT_CHARS` 常量；`infra/config/mod.rs` 与 `infra/mod.rs` 重导出；`tool_exec` `bash` 分支解析并 clamp `timeout_ms` 后透传。
- **PR-E.2 核心超时（spawn + timeout(wait) + kill）**：`executor/bash.rs` 从 `Command::output()` 重写为 `Command::spawn` + 并行 `tokio::task` 读 stdout/stderr 管道 + `tokio::time::timeout(_, child.wait())`；超时分支 `kill_process_tree` 在 Unix 下用 `Command::process_group(0)` 把子进程做新进程组 leader、再 `libc::killpg(pgid, SIGKILL)` 杀整组——单 PID kill 无法处理 `sh -c '...; sleep N'` 派生孙子进程的场景；Windows 退化为 `Child::kill`；不论平台 `child.wait()` 收尸防僵尸。新依赖 `[target.'cfg(unix)'.dependencies] libc = "0.2"`。
- **PR-E.3 输出累积 + 落盘**：新建 `src/core/tools/primitive/executor/output_accum.rs`，`accumulate_with_persist(text, max_chars, persist_dir, prefix)` 做「头尾保留 + 中段省略 hint」（多字节安全），超限且配置了 `bash_persist_dir` 时把完整原文写入 `<persist_dir>/<prefix>-<unix_ms>-<rand6>.txt`；`BashResult` 扩 `timed_out / truncated / persisted_output_path`（均 `#[serde(default)]`，`persisted_output_path` 加 `skip_serializing_if = Option::is_none`，`#[derive(Default)]` 让 mock `..Default::default()` 兼容）；`DefaultPrimitiveExecutor` 新增 `bash_timeout_ms / bash_max_output_chars / bash_persist_dir` 字段 + `with_*` builder。
- **PR-E.4 单测**：bash.md §10 T1 三例 `bash_wallclock_timeout_kills_process` / `bash_output_truncation_keeps_head_tail` / `bash_persists_full_output_when_truncated` 落地于 `core::tools::primitive::tests::suite_test`；`output_accum.rs` 内置 8 例 `#[cfg(test)]`（空输出、阈值边界、head+tail、持久化路径）；回归 `gate_suite_*` / permission gate_test 中所有 bash 相关用例（`cargo test --lib` 741 全绿，本卡 +11：3 §10 T1 + 8 output_accum）。
- **PR-E.5 集成测**：`tests/{agent_loop_tests, cli_tests, primitives_tools_tests, bash_assignment_deny, context_management_tests, wasmedge_e2e_tests}` rename 后全绿；未新增 integration 二进制 → `scripts/test-groups.sh` 不动；`E2E_SCENARIO_LIBRARY.md` **不适用**（BashResult 仅追加 optional 字段、向后兼容；现有 E2E 用例的 expected stdout 都是文本输出/文件存在等，不依赖结构字段名）。
- **门禁**：`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test --lib`（741） 全绿；3 个 §10 T1 集成场景在单测层即覆盖（spawn 真实 sh），不再额外起 integration 二进制。
- **不变量**：`PrimitiveExecutor::execute_bash` trait 方法名（含已加的 `timeout_ms: Option<u64>` 形参）/ dispatcher `("primitive","executeBash")` / 所有 mock 全部就地扩参，未改名；`wasmedge_e2e_tests` host_call `executeBash` 维持。

### 🔌 INTERFACE (接口变更，T2-P0-016 bash 子项 PR-E)

- **LLM 工具名**：`execute_bash → bash`（短名，无运行时别名）；transcript 旧名仅 `tracing::warn` 一次。
- **`bash` 入参**：`{ command, cwd?, args?, timeout_ms? }`（与冻结 [bash.md](../../docs/architecture/tools/bash.md) §4 一致）；`timeout_ms` 上界 600_000ms，`tool_exec` 与 trait 实现各 clamp 一次。
- **`bash` 语义**：`spawn` + 并行 reader + `timeout(wait)`；超时 `killpg(SIGKILL)` 杀整组（Unix），`exit_code = -1 + timed_out = true` 回执；输出超 `[tools.bash].max_output_chars`（默认 30_000）头尾截断 + 可选落盘。
- **新配置**：`[tools.bash].timeout_ms` / `[tools.bash].max_output_chars` / 环境变量 `PI_WASM__TOOLS__BASH__*`，默认 120_000ms / 30_000 chars。
- **新出参字段**：`BashResult.timedOut: bool` / `BashResult.truncated: bool` / `BashResult.persistedOutputPath?: string`（截断且配置 `bash_persist_dir` 时回填）。

### ⚠️ BLOCKED (阻塞/风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无（PR-E 完结） | bash T2 后台（PR-I）/ T3 AST/Sandbox（PR-L）按 plan §六 在 PR-E 合入后串行开 PR；先出 1 页 `phase-l-scope-spec` 再启 PR-L | 同 PR 或下卡 |

---


### 2026-05-06 | T2-P0-016 子项 `write` 全部 10 个 todo 完成（PENDING_INTEGRATION）

- **PR-命名**：`catalog::write_file → write`、`tool_exec` `match "write"`、`system_prompt` 旧名移除；`session/manager/context.rs` 的 `warn_if_legacy_tool_name` 追加 `write_file → write` 旧名 warn 分支（OnceLock 节流，无重定向；与 `read_file` / `edit_file` 同处汇总）；`docs/tool-catalog.md` 重派生（`write_file → write`，edit description 字面量同步）。
- **PR-C（T1）契约门禁**：`tool_exec` `write` 分支用 `normalize_path` 算 `resolved`（与 read 落 stamp 同形 key）；新增 `Exists` / `NoPriorRead` / `Stale` 三类策略拒 = `is_error: true` 早退；成功写盘后 `ReadFileState::invalidate(&resolved)`；`write_file_impl` 加 `exists && !overwrite → AppError::Primitive(Exists)` 二道防线，防止 trait 直调（dispatcher / extension）绕过。
- **NoPriorRead 与 edit 同 PR 强拒**：抽公共函数 `tool_exec::check_mutation_stamp(state, path, op_label)`（前身 `check_edit_staleness`），无 stamp → `NoPriorRead`；`edit` / `hashline_edit` / `write` 三分支统一调用；`tool_exec_dedup_test::edit_no_prior_read_does_not_block_phase1` 改造为 `edit_no_prior_read_rejects_after_t2_p0_016`（断言反转 + 改名 + 磁盘字节级未变断言）；其它 6 个 edit/hashline_edit/secrets 单测加 `prime_read_stamp` helper 先 `read` 再 `edit`；同步 [edit.md](../../docs/architecture/tools/edit.md) §2.4.2 表 5 / §2.4.3 / §9 表 / §10 测试矩阵 / §10.2「Phase1 策略」段。
- **PR-G（T2）LF + 回执**：`infra::config::types` 新增 `ToolsWriteConfig { normalize_crlf }`（默认 `true`，常量 `DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF`）；`infra/config/mod.rs` 与 `infra/mod.rs` 重导出；`DefaultPrimitiveExecutor` 增 `write_normalize_crlf` 字段 + `with_write_normalize_crlf` builder（与 `with_read_max_bytes` 模式一致）；`api/chat` 装配处注入；`write_file_impl` 在 `write_file_atomic` 之前做 `\r\n → \n`，并先 `read_file_utf8` 旧内容用于 `build_simple_diff`；`WriteFileResult` 扩字段 `bytes_written: u64` + `diff_hint: Option<String>`（`#[serde(default)]`，老调用方 `bytes_written=0/diff_hint=None` 兼容；mocks / e2e fixture 全部补齐）；`tool_exec` 回执文案：`已写入 / 已覆盖: <path> (N bytes)` + 可选 `--- diff` 块。
- **T3-K secrets**：`write_file_impl` 在 LF 规范化后、`.bak` / 落盘之前调 `scan_new_content_for_secrets(original_or_empty, final_text)`（与 edit 共用函数，仅扫**新引入**的命中，避免 false-positive）；命中走 `require_user_confirmation(PrimitiveOperation::Write, …)`，拒 → `AppError::Primitive("SecretsRejected: …")` + 审计 `success=false / user_approved=false` + 磁盘字节级未变（新建场景文件根本不会被创建）。
- **测试**：`cargo test --lib` 730 通过（674 → 704 → 714 → 730，本卡 +16：5 PR-C/PR-命名 + 3 PR-G + 3 T3-K + 5 config）；`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` 全绿。
- **不变量**：`PrimitiveExecutor::write_file` trait 方法名 / dispatcher `("fs"|"primitive","writeFile")` / 所有 mock / `wasmedge_e2e_tests` 中的 `writeFile` host_call 名 全部未动；`tests/context_management_tests.rs::test_build_context_preserves_order_with_mixed_turns` fixture 故意保留 `write_file` 旧名以验证 transcript 历史回放。
- **L313 / PR-A 四工具短名**：`read`/`write`/`edit`/`bash` 在 `catalog` / `tool_exec` / `system_prompt` / 测试与 transcript 单迭代内一致；旧名拒识 + `warn_if_legacy_tool_name` 节流；**PR-B～E** bash 余量仍见 `agents/TASK_BOARD_002/tasks/T2-P0-016.md`。

### 🔌 INTERFACE (接口变更，T2-P0-016 write 子项)

- **LLM 工具名**：`write_file → write`（短名，无运行时别名）；transcript 旧名仅 `tracing::warn` 一次。
- **`write` 入参**：`{ path, content, overwrite? }`（与冻结 [write.md](../../docs/architecture/tools/write.md) §4.1 一致；**不**新增 per-call `normalize_line_endings?`）。
- **`write` 语义**：`overwrite=false && exists → Exists`；`overwrite=true && exists` 必先 `read`（否则 `NoPriorRead`）+ stamp 与 `mtime/size` 一致（否则 `Stale`）；成功后 `ReadFileState::invalidate(&resolved)`；磁盘上 `\r\n` 默认折叠为 `\n`（`[tools.write] normalize_crlf=false` 关掉）；回执含 UTF-8 字节数 + 可选 diff 摘要。
- **共享门禁**：`edit` / `hashline_edit` 与 `write(overwrite=true)` 在无 prior read 时**统一强拒** `NoPriorRead`（同函数 `check_mutation_stamp`）。
- **新配置**：`[tools.write] normalize_crlf` / 环境变量 `PI_WASM__TOOLS__WRITE__NORMALIZE_CRLF`，默认 `true`。

### ⚠️ BLOCKED (阻塞/风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无（write + PR-A） | **L313 / PR-A 四工具短名** 已交付（与 write 子项同分支）；**PR-B～E** bash 余量仍见 `agents/TASK_BOARD_002/tasks/T2-P0-016.md`，不阻塞当前 PR-A 结论 | - |

---


### ✅ DONE (已完成/进行中)

- [✓] **[P0]** `core::tools` 按四层分包：`contract`（catalog / registry / confirmation）、`primitive`、`config_tool`、`pipeline`（edit_normalize / read_state）；删除根模块兼容 `pub use`，全仓 `use` 与 openspec / 任务板路径对齐。
- [✓] **[P0]** `config.rs` 拆为 `config_tool/{allowlist,get,set,mod}` + `tests_config_tool.rs`；补回迁移时丢失的设计注释；`toml_to_json` 收紧为模块内私有；`docs/tool-catalog.md` 由 `gen-tool-catalog` 与源路径一致。
- [✓] **[P0]** 验证：`cargo test -p pi_wasm` 全量（含集成与 wasmedge e2e）通过。

### 🔌 INTERFACE (接口变更)

- Rust 调用方须改用 `crate::core::tools::contract::*`、`config_tool`、`pipeline::*`；`crate::core` 对外 re-export 已指向新路径。

### ⚠️ BLOCKED (阻塞/风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

---

# feature/strengthen-four-core-tools 状态

| 字段 | 值 |
|---|---|
| Owner | Tom |
| State | PENDING_INTEGRATION |
| Branch | `feature/strengthen-four-core-tools` |
| Task | `T2-P0-017 | strengthen-edit-tool` |
| Update Time | 2026-05-05 23:30 |
| Cov% | - |

## Step-by-Step

### 2026-05-05 | T2-P0-017 全部 11 个 todo 完成；全量门禁通过

- **Phase1（PR-命名 + PR-D / T1）**：`catalog::edit` 短名 + `oneOf` schema；`tool_exec` `match "edit"` 分支 + `parse_edit_args` + `check_edit_staleness`；`session/manager/context.rs` 加 `edit_file → edit` 旧名 transcript warn（OnceLock 节流，无重定向）；`write_edit::edit_file_impl` 重写为「原文快照 → 字节索引 `match_indices` → `replace_all` / `Ambiguous` / `Overlap` 校验 → 一次性按起点降序 splice → `.bak` 仅在校验通过后写盘前建、写成功删、写失败回滚」；保留 `PrimitiveExecutor::edit_file` trait 方法名（决策 6 lock）；`replace_all` 信号通过 `EDIT_REPLACE_ALL_MARKER` (`\u0000…\u0000`) 编码到 `old_content` 前缀；`docs/tool-catalog.md` 重新派生。
- **Phase2（PR-H / T2）**：新建 `src/core/tools/edit_normalize.rs` —— `strip_bom` / `detect_line_ending` / `normalize_to_lf`（**字节级实现**修补 `as char` 多字节 bug）/ `restore_line_endings` / `fold_curly_quotes` / `desanitize` / `normalize_for_match` / `build_normalized_byte_map` 双轨（normalized → 原文字节偏移映射）；`apply_string_edits` 接入 `(disk_text, write_back)` 链路：模型 `“foo”` 命中磁盘 `"foo"`、NBSP / 零宽字符 desanitize、CRLF/BOM 文件改后行尾保留；`tool_exec` 增 `.ipynb` 拒绝 + Notebook 错误文案；E5 错误码（NotFound / Ambiguous / Overlap / Stale / Notebook / BinaryFile / Io）回执统一格式 + hint。
- **Phase3（PR-M + T3-K / T3）**：注册新工具 `hashline_edit`（`{ path, edits: [{ op, pos, end?, lines }] }`）；trait 方法 `PrimitiveExecutor::hashline_edit` 默认 `Unsupported`；`DefaultPrimitiveExecutor` 实现 `hashline_edit_impl` 复用 `read::compute_line_hash`（与 read.md §4 算法 byte-equal），校验每段锚点（`OutOfRange` / `HashMismatch`）+ 行号区间重叠 + 自下而上 splice + `.bak` 兜底；新建 `src/core/security/secrets.rs`（regex：openai_api_key / aws_access_key_id / slack_token / high_entropy_hex）；`scan_new_content_for_secrets` 仅扫「edit 新引入」的 secrets（避免 false-positive 反复打扰）；命中走 `require_user_confirmation`，拒 → `SecretsRejected` + 磁盘字节级未变 + 无 `.bak` 残留。
- **测试**：lib +30 例（674 → 704 → 714）覆盖 §10 测试矩阵 T1 / T2 / T3 + secrets + hashline；`scripts/run-integration-tests.sh all` 全量门禁 EXIT_CODE=0（release / clippy / lib 714 / integration parallel + serial 39 全绿，含 wasmedge_e2e 与 dispatcher 等）。
- **文档**：`docs/architecture/tools/edit.md` §2.4.3 追加「NoPriorRead 与 T2-P0-016 write 同 PR 锁同节奏」决策行；`docs/tool-catalog.md` 同步生成（新增 `edit` `oneOf` + `hashline_edit`）。
- **不变量**：`PrimitiveExecutor::edit_file` 方法名 / dispatcher `("fs"|"primitive","editFile")` / 所有 mock / 旧 `tests/primitives_tools_tests.rs::test_primitive_executor_edit_file_replaces_content` / `wasmedge_e2e_tests` 中的 `editFile` host_call 名 全部未动；改的只有 LLM 短名与底层语义。

### 🔌 INTERFACE (接口变更)

> 本卡 PENDING_INTEGRATION 引入的对外行为：
- **LLM 工具名**：`edit_file → edit`（短名）；transcript 旧 `edit_file` 不重定向，仅 `tracing::warn` 一次（OnceLock 节流，与 read PR-RA 同型）。
- **`edit` 入参**：`oneOf` 形状 A（`path, old_content, new_content, replace_all?`）/ B（`path, edits: [{ old_content, new_content, replace_all? }]`，`edits` 优先）。
- **`edit` 语义**：所有段对**原文快照**字节索引一次性匹配 + `replace_all` + 重叠检测 + 单次 `write_file_atomic`；BOM/CRLF 文件改后字节级保留行尾与 BOM；模型可用弯引号 / NBSP / 零宽字符命中直引号 / 普通空格；`.ipynb` 直接拒。
- **新工具 `hashline_edit`**：`{ path, edits: [{ op: replace|insert|delete, pos: "<line>#<2char>", end?, lines? }] }`；与 `read hashline=true` 闭环；锚点漂移 → `HashMismatch`。
- **写盘前 secrets 扫描**：`edit` / `hashline_edit` 在 `write_file_atomic` 之前对**新引入**的 OpenAI/AWS/Slack/高熵 hex 命中走 `require_user_confirmation`；拒 → `SecretsRejected` + 磁盘原样。

### 2026-05-05 | Phase1 — PR-命名 + PR-D（T1）启动

- **动机**：承接计划 `~/.cursor/plans/t2-p0-017_edit_工具_254e5a1e.plan.md` 与 `docs/architecture/tools/edit.md` §2.4 决策表，把 `edit_file → edit` 短名 + `oneOf` schema + `edits[]` 对原文快照一次应用 + `replace_all` + 重叠检测 + `edit` 前 staleness + `.bak` 写序修正一次合入；消除现状 `lines().join("\n")` 链式 + 校验前 `.bak` 残留两类潜伏 bug。
- **范围（本步）**：仅 LLM 短名 + 解析 + write_edit 重写 + staleness 注入 + 错误码集合（NotFound/Ambiguous/Overlap/Stale/BinaryFile/Io）+ T1 单测；`NoPriorRead` 与 T2-P0-016 write 同 PR 锁、normalize/ipynb/hashline_edit/secrets 留 Phase2/3。
- **决策（lock）**：`PrimitiveExecutor::edit_file` trait 方法名保留不改（与 read PR-RA 同型）；字节索引（`match_indices`）作为 span 单一坐标系。

### 2026-05-05 | 认领 T2-P0-017，建分支

- 看板状态：`agents/TASK_BOARD_002/tasks/T2-P0-017.md`：`TODO → DOING`，负责人 Tom。
- 分支：`feature/strengthen-four-core-tools`（与计划/看板一致），从 `develop@f9f9409` 切出。

### 🔌 INTERFACE (接口变更)

> 本卡完成后会引入的对外行为：
- **LLM 工具名**：`edit_file → edit`（短名）；transcript 旧 `edit_file` 不重定向，仅 `tracing::warn`。
- **`edit` 工具入参**：`oneOf` 形状 A（`path, old_content, new_content, replace_all?`）/ B（`path, edits: [{...}]`）。
- **执行语义**：多段对原文快照一次应用 + 重叠检测 + 单次 `write_file_atomic` + 校验阶段不写盘。

### ⚠️ BLOCKED (阻塞/风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
