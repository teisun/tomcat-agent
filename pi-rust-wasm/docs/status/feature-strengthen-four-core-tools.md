| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-05-06 16:35 | ACTIVE | feature/strengthen-four-core-tools | - |

### 2026-05-06 | T2-P0-016 子项 `write` 全部 10 个 todo 完成（PENDING_INTEGRATION）

- **PR-命名**：`catalog::write_file → write`、`tool_exec` `match "write"`、`system_prompt` 旧名移除；`session/manager/context.rs` 的 `warn_if_legacy_tool_name` 追加 `write_file → write` 旧名 warn 分支（OnceLock 节流，无重定向；与 `read_file` / `edit_file` 同处汇总）；`docs/tool-catalog.md` 重派生（`write_file → write`，edit description 字面量同步）。
- **PR-C（T1）契约门禁**：`tool_exec` `write` 分支用 `normalize_path` 算 `resolved`（与 read 落 stamp 同形 key）；新增 `Exists` / `NoPriorRead` / `Stale` 三类策略拒 = `is_error: true` 早退；成功写盘后 `ReadFileState::invalidate(&resolved)`；`write_file_impl` 加 `exists && !overwrite → AppError::Primitive(Exists)` 二道防线，防止 trait 直调（dispatcher / extension）绕过。
- **NoPriorRead 与 edit 同 PR 强拒**：抽公共函数 `tool_exec::check_mutation_stamp(state, path, op_label)`（前身 `check_edit_staleness`），无 stamp → `NoPriorRead`；`edit` / `hashline_edit` / `write` 三分支统一调用；`tool_exec_dedup_test::edit_no_prior_read_does_not_block_phase1` 改造为 `edit_no_prior_read_rejects_after_t2_p0_016`（断言反转 + 改名 + 磁盘字节级未变断言）；其它 6 个 edit/hashline_edit/secrets 单测加 `prime_read_stamp` helper 先 `read` 再 `edit`；同步 [edit.md](../../openspec/specs/architecture/tools/edit.md) §2.4.2 表 5 / §2.4.3 / §9 表 / §10 测试矩阵 / §10.2「Phase1 策略」段。
- **PR-G（T2）LF + 回执**：`infra::config::types` 新增 `ToolsWriteConfig { normalize_crlf }`（默认 `true`，常量 `DEFAULT_TOOLS_WRITE_NORMALIZE_CRLF`）；`infra/config/mod.rs` 与 `infra/mod.rs` 重导出；`DefaultPrimitiveExecutor` 增 `write_normalize_crlf` 字段 + `with_write_normalize_crlf` builder（与 `with_read_max_bytes` 模式一致）；`api/chat` 装配处注入；`write_file_impl` 在 `write_file_atomic` 之前做 `\r\n → \n`，并先 `read_file_utf8` 旧内容用于 `build_simple_diff`；`WriteFileResult` 扩字段 `bytes_written: u64` + `diff_hint: Option<String>`（`#[serde(default)]`，老调用方 `bytes_written=0/diff_hint=None` 兼容；mocks / e2e fixture 全部补齐）；`tool_exec` 回执文案：`已写入 / 已覆盖: <path> (N bytes)` + 可选 `--- diff` 块。
- **T3-K secrets**：`write_file_impl` 在 LF 规范化后、`.bak` / 落盘之前调 `scan_new_content_for_secrets(original_or_empty, final_text)`（与 edit 共用函数，仅扫**新引入**的命中，避免 false-positive）；命中走 `require_user_confirmation(PrimitiveOperation::Write, …)`，拒 → `AppError::Primitive("SecretsRejected: …")` + 审计 `success=false / user_approved=false` + 磁盘字节级未变（新建场景文件根本不会被创建）。
- **测试**：`cargo test --lib` 730 通过（674 → 704 → 714 → 730，本卡 +16：5 PR-C/PR-命名 + 3 PR-G + 3 T3-K + 5 config）；`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` 全绿。
- **不变量**：`PrimitiveExecutor::write_file` trait 方法名 / dispatcher `("fs"|"primitive","writeFile")` / 所有 mock / `wasmedge_e2e_tests` 中的 `writeFile` host_call 名 全部未动；`tests/context_management_tests.rs::test_build_context_preserves_order_with_mixed_turns` fixture 故意保留 `write_file` 旧名以验证 transcript 历史回放。

### 🔌 INTERFACE (接口变更，T2-P0-016 write 子项)

- **LLM 工具名**：`write_file → write`（短名，无运行时别名）；transcript 旧名仅 `tracing::warn` 一次。
- **`write` 入参**：`{ path, content, overwrite? }`（与冻结 [write.md](../../openspec/specs/architecture/tools/write.md) §4.1 一致；**不**新增 per-call `normalize_line_endings?`）。
- **`write` 语义**：`overwrite=false && exists → Exists`；`overwrite=true && exists` 必先 `read`（否则 `NoPriorRead`）+ stamp 与 `mtime/size` 一致（否则 `Stale`）；成功后 `ReadFileState::invalidate(&resolved)`；磁盘上 `\r\n` 默认折叠为 `\n`（`[tools.write] normalize_crlf=false` 关掉）；回执含 UTF-8 字节数 + 可选 diff 摘要。
- **共享门禁**：`edit` / `hashline_edit` 与 `write(overwrite=true)` 在无 prior read 时**统一强拒** `NoPriorRead`（同函数 `check_mutation_stamp`）。
- **新配置**：`[tools.write] normalize_crlf` / 环境变量 `PI_WASM__TOOLS__WRITE__NORMALIZE_CRLF`，默认 `true`。

### ⚠️ BLOCKED (阻塞/风险)

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无（write 子项） | T2-P0-016 卡内 L313「bash + 短名总闸」仍 TODO，由另认领推进，不在本 PR 范围 | - |

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
- **文档**：`openspec/specs/architecture/tools/edit.md` §2.4.3 追加「NoPriorRead 与 T2-P0-016 write 同 PR 锁同节奏」决策行；`docs/tool-catalog.md` 同步生成（新增 `edit` `oneOf` + `hashline_edit`）。
- **不变量**：`PrimitiveExecutor::edit_file` 方法名 / dispatcher `("fs"|"primitive","editFile")` / 所有 mock / 旧 `tests/primitives_tools_tests.rs::test_primitive_executor_edit_file_replaces_content` / `wasmedge_e2e_tests` 中的 `editFile` host_call 名 全部未动；改的只有 LLM 短名与底层语义。

### 🔌 INTERFACE (接口变更)

> 本卡 PENDING_INTEGRATION 引入的对外行为：
- **LLM 工具名**：`edit_file → edit`（短名）；transcript 旧 `edit_file` 不重定向，仅 `tracing::warn` 一次（OnceLock 节流，与 read PR-RA 同型）。
- **`edit` 入参**：`oneOf` 形状 A（`path, old_content, new_content, replace_all?`）/ B（`path, edits: [{ old_content, new_content, replace_all? }]`，`edits` 优先）。
- **`edit` 语义**：所有段对**原文快照**字节索引一次性匹配 + `replace_all` + 重叠检测 + 单次 `write_file_atomic`；BOM/CRLF 文件改后字节级保留行尾与 BOM；模型可用弯引号 / NBSP / 零宽字符命中直引号 / 普通空格；`.ipynb` 直接拒。
- **新工具 `hashline_edit`**：`{ path, edits: [{ op: replace|insert|delete, pos: "<line>#<2char>", end?, lines? }] }`；与 `read hashline=true` 闭环；锚点漂移 → `HashMismatch`。
- **写盘前 secrets 扫描**：`edit` / `hashline_edit` 在 `write_file_atomic` 之前对**新引入**的 OpenAI/AWS/Slack/高熵 hex 命中走 `require_user_confirmation`；拒 → `SecretsRejected` + 磁盘原样。

### 2026-05-05 | Phase1 — PR-命名 + PR-D（T1）启动

- **动机**：承接计划 `~/.cursor/plans/t2-p0-017_edit_工具_254e5a1e.plan.md` 与 `openspec/specs/architecture/tools/edit.md` §2.4 决策表，把 `edit_file → edit` 短名 + `oneOf` schema + `edits[]` 对原文快照一次应用 + `replace_all` + 重叠检测 + `edit` 前 staleness + `.bak` 写序修正一次合入；消除现状 `lines().join("\n")` 链式 + 校验前 `.bak` 残留两类潜伏 bug。
- **范围（本步）**：仅 LLM 短名 + 解析 + write_edit 重写 + staleness 注入 + 错误码集合（NotFound/Ambiguous/Overlap/Stale/BinaryFile/Io）+ T1 单测；`NoPriorRead` 与 T2-P0-016 write 同 PR 锁、normalize/ipynb/hashline_edit/secrets 留 Phase2/3。
- **决策（lock）**：`PrimitiveExecutor::edit_file` trait 方法名保留不改（与 read PR-RA 同型）；字节索引（`match_indices`）作为 span 单一坐标系。

### 2026-05-05 | 认领 T2-P0-017，建分支

- 看板状态：TASK_BOARD_002 §「T2-P0-017」 `TODO → DOING`，负责人 Tom。
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
