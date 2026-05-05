# feature/tool-system-cleanup 状态

| 字段 | 值 |
|---|---|
| Owner | Spike |
| State | PENDING_INTEGRATION |
| Branch | `feature/tool-system-cleanup` |
| Task | `T2-P0-005 | tool-system-cleanup` + `T2-P1-007 | tool-system-deferred-followups / #T-152 search_files` + `T2-P0-005 子项「多 LLM 层改造 + OpenAI Responses」` |
| Update Time | 2026-05-05（read 工具加强 PR-RA/RB/RF/RJ/RM 6 PR 实施 + 集成测试登记） |
| Cov% | - |

## Step-by-Step

### 2026-05-05（同日追加 #4）| read 工具加强：PR-RA/RB/RF/RJ/RM 全部落地 + 集成测试登记

- **动机**：承接计划 `~/.cursor/plans/strengthen-read-tool_92f396c7.plan.md`，按 `openspec/specs/architecture/tools/read.md` §0–§4 决策表把 `read_file → read` 命名 + 分页 + 二进制结构化提示 + 行号 + dedup/staleness + image/PDF 多模态 + hashline 一次性补齐到 v3.0。
- **代码**：
  - **PR-RA 命名切换**：`src/core/tools/catalog.rs` `read_file → read`（`description` 改 cc-fork 风格短句）；`src/core/agent_loop/tool_exec.rs` 仅留 `"read"` 分支，`"read_file"` 按未知工具回错；`src/core/llm/system_prompt.rs` 全量字面量切换；`src/core/session/manager/context.rs` 用 `OnceLock` 守 `tracing::warn!("legacy tool name: read_file → read")`，**不重定向**历史回放。
  - **PR-RB T1 read（offset/limit + 二进制 hint + 分块流式 + 25 MiB 上限）**：`src/core/tools/primitive/executor.rs` 新增 `read_window_blocking`（`memchr` 单循环抽窗，跳过段不建行级 `String`）；`src/infra/config/types.rs` 新增 `[tools.read].max_bytes`（默认 `DEFAULT_TOOLS_READ_MAX_BYTES = 25 MiB`），metadata 阶段无 `offset/limit` 时拒大文件；二进制返回结构化 `AppError` 含首字节 hex（不污染上下文）。
  - **PR-RF T2（行号 + dedup/staleness + FILE_UNCHANGED stub）**：`format_with_line_numbers`（`{:>6}\t{content}`，默认 `line_numbers=true`）；新建 `src/core/tools/read_state.rs`（`ReadStamp{mtime,size,content_hash,offset,limit,is_partial_view}` + `ReadFileState` `RwLock<HashMap<PathBuf,ReadStamp>>` + `FILE_UNCHANGED_STUB`）；`AgentLoopConfig` / `ChatContext` 跨轮持有 `Arc<ReadFileState>`；`tool_exec` dedup 短路命中时返回 stub 字面量。
  - **PR-RJ T3（多模态 + ChatMessageContentPart 重构）**：
    - PR-RJ-0：`src/core/llm/types.rs` 重构 `ChatMessageContentPart::image_b64` / `file_b64` 为 `(mime, &Path)`，集中 `metadata` 二次校验 + `read` + base64 编码；`decode_b64_len` 标 `#[allow(dead_code)]` 留作测试 helper。
    - PR-RJ T3-a：`src/core/tools/primitive/types.rs` 升级 `read` 输出 schema 为 `ReadResult` 4 态枚举（`Text`/`Image`/`Pdf`/`FileUnchanged`）；`PrimitiveExecutor::read` 默认实现回退到 `read_file` → 包成 `Text`，旧 mock **零改动** 升级。
    - PR-RJ T3-b：`executor.rs` 新增 `detect_inline_mime`（PNG/JPEG/GIF/WebP/PDF magic + ext）+ metadata 阶段 `IMAGE_MAX_BYTES`/`FILE_MAX_BYTES` 预检，命中 → `ReadResult::Image|Pdf`（**只**带元信息，不读字节）。
    - PR-RJ T3-c：`tool_exec` 返回签名升 `(String, bool, Vec<ChatMessageContentPart>)`；`tool_dispatcher` 在 tool message 之后**注入下一条 user message**承载 image/file part（OpenAI tool→user 注入边界，spec §4.2）。
  - **PR-RM T3 hashline**：`Cargo.toml` 新增 `xxhash-rust = { version = "0.8.15", features = ["xxh32"] }`；`executor.rs` 新增 `compute_line_hash`（whitespace-stripped + nibble→XX）+ `format_with_hashlines`（`{:>6}#XX:{content}`）；`hashline:bool` 优先于 `line_numbers`，schema 与 system prompt 同步。
- **测试**：
  - **lib 单测 +33**（674 全量绿）：T1 read window 6 例、T2 行号/状态 13 例、T3-a/b 路由 4 例、T3-c 多模态注入 + dedup 5 例、PR-RM hashline 2 例、`read_state` 8 例、helper 重构修订 ~10 例。
  - **集成测试 `tests/read_tool_tests.rs` ＋ 6 例**：`read_text_offset_limit_window_with_line_numbers` / `read_binary_returns_structured_hint` / `read_hashline_renders_two_char_hash_prefix` / `read_png_routes_to_image_and_can_build_input_image_part` / `read_pdf_routes_to_pdf_and_can_build_input_file_part` / `read_oversize_image_rejected_before_loading_bytes`。**全 6 例满足 INTEGRATION_TEST_SPEC §9.0 强制门禁**：入口 `common::setup_logging()`，每用例 `info_span!`，AAA 三阶段各落 `tracing::info!`。
  - **test-groups 登记**：`scripts/test-groups.sh` `PI_WASM_INTEGRATION_PARALLEL_TESTS` 追加 `read_tool_tests`；`openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md` §7.2 并发组清单同步。
  - **`docs/tool-catalog.md`** 用 `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` 重新派生（`read_file` → `read`，5 个新参数 schema 全量覆盖）。
- **文档**：
  - `openspec/specs/architecture/tools/read.md` v2 → v3，§0/§1/§2/§3/§4 全量补齐 12 项落地点 + One-Glance Map；`openspec/specs/User_Stories.md` 把单条「`read_file` 二进制错误」扩成「`read` 分页 + 行号 + hashline + dedup + 多模态 + 二进制结构化错误」两条；`E2E_SCENARIO_LIBRARY.md` E2E-CLI-021 拆出 021/021a/021b/021c/021d/021e 6 条 + 「已实现」段引用 `tests/read_tool_tests.rs`。
  - `pi-rust-wasm/src/lib.rs` / `core/mod.rs` 公开 re-export `ReadResult` / `ReadTextResult` / `ReadBinaryResult`，给集成测试与未来插件层使用。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --test-threads=1`（674 PASS）、`cargo test --test read_tool_tests`（6 PASS）、`cargo test --test agent_loop_tests`（11 PASS）：PASS。

### 2026-05-05（同日追加 #3）| Session：`estimate_msg_chars` 覆盖 Parts，fallback 与多模态权重对齐

- **动机**：`ContextState::estimated_token_count` 在首轮 stream 完成、`last_api_usage` 仍为 `None` 时走 `estimate_context_chars / 4`；原先 `estimate_msg_chars` 仅等价于纯文本或误调用不存在方法，导致含 `InputImage` / `InputFile` 的 `Parts` 消息在 fallback 路径被当成 0，`usage_ratio()` 与 Compaction 触发严重低估。
- **代码**：[`src/core/session/manager/types.rs`](../../src/core/session/manager/types.rs) — `estimate_msg_chars` 对 `ChatMessageContent::Text` 用 `len()`、`Parts` 对各 part 调用 `ChatMessageContentPart::estimated_chars()`（与 `IMAGE_CHAR_ESTIMATE` / `FILE_CHAR_ESTIMATE` 一致），并保留 `tool_calls` 序列化长度累加。
- **测试**：[`src/core/session/manager/tests/context_state_test.rs`](../../src/core/session/manager/tests/context_state_test.rs) — 新增 `estimate_msg_chars_text_only_returns_string_len`、`estimate_msg_chars_with_image_part_uses_image_estimate`、`estimate_msg_chars_with_file_part_uses_file_estimate`；测试 import 改为 `crate::core::llm::` 公开 re-export（满足 clippy `--all-targets`）。
- **文档**：[`src/core/llm/openai.rs`](../../src/core/llm/openai.rs) / [`openai_responses.rs`](../../src/core/llm/openai_responses.rs) — `count_tokens` 上补充 doc-comment：trait 启发式为 `chars/3`，业务预算以 `ContextState::estimated_token_count`（优先 API usage，否则 `chars/4`）为准，二者分母不同为有意设计。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --test-threads=1`：PASS。

### 2026-05-05（同日追加 #2）| LLM 多模态 wire（T2-P0-012）：types 三态枚举 + Responses input_image / input_file + Completions 拒绝

- **动机**：用户希望「先支持图片、文件、PDF 附件」，wire 期不做 OpenAI Files API 上传管理（拆 T2-P0-015）。`/v1/responses` 主线已通；只需补 types + 翻译层 + 单测/集成测试。
- **代码**：
  - [`src/core/llm/types.rs`](../../src/core/llm/types.rs) — `ChatMessageContentPart` 升级为 `#[serde(tag = "type", rename_all = "snake_case")]` 三态枚举（`InputText` / `InputImage` / `InputFile`），加 `text` / `image_b64` / `file_b64` / `image_file_id` / `file_file_id` helper 与 `ChatMessage::user_with_parts`；`IMAGE_MAX_BYTES = 4_718_592` (4.5 MB) / `FILE_MAX_BYTES = 25 MB` 硬限 + image MIME 白名单 + base64 合法性校验；`count_tokens` 启发式：`InputImage = 3600` chars / `InputFile = 8000` chars
  - [`src/core/llm/openai_responses.rs`](../../src/core/llm/openai_responses.rs) — 新增 `part_to_responses_value` 显式按变体写 wire（`image_url=data:..` / `file_data=data:..` / `file_id` 三选一，`file_id` 优先）；`extract_text` 仅取 `InputText`；`build_responses_input` 角色规则：仅 `User` 透传非 text part，其它角色 `tracing::warn!` 并丢弃非 text 部分
  - [`src/core/llm/openai.rs`](../../src/core/llm/openai.rs) — 新增 `reject_multimodal_parts`，`chat` / `chat_stream` 入口扫描 messages，含非 `InputText` part 时返回结构化非可重试错误「`provider=openai 不支持多模态附件，请改用 provider=openai-responses`」
  - [`Cargo.toml`](../../Cargo.toml) — 新增 `base64 = "0.22"` 依赖（helper 解码长度校验）；不动 `reqwest` 的 `multipart` feature（留给 T2-P0-015）
- **测试**：
  - 5 单测（wire / 角色降级）+ 1 拒绝测试（Completions）+ 5 helper 失败用例（`base64` 非法 / 超 IMAGE_MAX_BYTES / 非白名单 mime / 超 FILE_MAX_BYTES / file_id 空字串）
  - 3 集成测试（[`tests/openai_responses_integration_tests.rs`](../../tests/openai_responses_integration_tests.rs)）：
    1. `responses_inline_image_describe_roundtrip` — 一张 46 KB 小狗 PNG → `/v1/responses` input_image data URL → 断言文本至少命中通用词（`dog/puppy/animal/...`）或常见品种词（`beagle/labrador/...`）之一
    2. `responses_inline_pdf_input_file_summarize_roundtrip` — reportlab 生成的 1.8 KB 单页 PDF → input_file data URL → 断言文本至少命中 `[hello/pdf/summary/summarize/test]`
    3. `responses_inline_image_b64_helper_rejects_oversize` — 本地 helper 校验，构造 `IMAGE_MAX_BYTES + 1` 字节，断言返回结构化错误，不打 OpenAI、不带 `#[ignore]`
  - 测试 fixtures：[`tests/fixtures/llm_multimodal/`](../../tests/fixtures/llm_multimodal/) 含 `sample_image.png` / `sample_image_b64.txt` / `sample_pdf_b64.txt` / `gen_sample_pdf.py` / `README.md`（图片来源 Unsplash CC0；PDF 由 reportlab 一次性生成，测试本身只 `include_str!` 读 `.txt`）
- **文档**：
  - `openspec/specs/architecture/llm-multiprovider-integration.md` §1.3 / §2.1 / §6.5.3（PDF / input_file 改为「已实现 wire」）/ §6.6（仅 vision 改为「与 §6.5 互斥落地，Completions 路径结构化拒绝」）/ §8 修订记录追加
  - `src/core/llm/README.md` 新增 §3.5「多模态 parts」小节（双通道 helper 表 + 最小调用示例 + 角色与 wire 规则）
  - `agents/TASK_BOARD_002.md` §T2-P0-012 子项 4 项 `[x]` 勾选；新增 `T2-P0-015 | llm-files-upload-manager` 任务卡（**ID 选 015 而非原计划 013**：因 013 / 014 已被占用，顺位取 015）；§5 拓扑增加 `T2-P0-012 → T2-P0-015` 边
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --test-threads=1`、`cargo test --test openai_responses_integration_tests -- --test-threads=1`：PASS（image roundtrip 真实命中 "a happy beagle ..."；PDF roundtrip 真实命中 "the pdf contains a brief test message: hello pdf ..."）。

### 2026-05-05（同日追加）| LLM：`registry` 接管 `mod` + 测试按 §A.9 内挂 + 对外只暴露 `resolve_llm`

- **动机**：消除「为单测放宽 `pub(crate)` / `pub(super)`」与「每加 Provider 改 `core/llm/mod.rs`」两类扩散；上层与集成测试统一通过 `resolve_llm` 拿 `Arc<dyn LlmProvider>`。
- **代码**：`registry.rs` 内 `#[path] mod openai` / `openai_responses`；`openai.rs` / `openai_responses.rs` 私有 wire/SSE 辅助全部私有，`#[cfg(test)] #[path]` 挂载 `tests/openai_*`；`mocks::load_dotenv` 升为 `pub(crate)` 供内挂测试复用；`lib.rs` / `core/mod.rs` 去掉 `OpenAiProvider` / `OpenAiResponsesProvider` 重导出，改导出 `resolve_llm`；`tests/llm_tests` 与 `openai_responses_integration_tests` 改走 registry。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --test-threads=1`、`./scripts/run-integration-tests.sh integration`：PASS。

### 2026-05-05 | 子项「多 LLM + OpenAI Responses」实现与文档同步

- **代码**：`src/core/llm/registry.rs` + `resolve_llm`；`openai_responses.rs`；`ChatContext::from_config` 经 registry 装配；`default_llm_provider = "openai-responses"`。
- **测试**：`core/llm/tests` 下 registry / wire / stream 单测；`tests/openai_responses_integration_tests.rs` 与 `llm_tests` 同口径（需 `OPENAI_API_KEY`）；`scripts/test-groups.sh` 已加入 `openai_responses_integration_tests` 并发组。
- **文档**：`openspec/.../llm-multiprovider-integration.md` §1.3 / §2 / §4 / §5.1 / §8；`src/core/llm/README.md`；`pi.config.toml.example`；`docs/user-guide.md` 示例块。
- **Phase G（2026-05-05）**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -j 1 --lib -- --test-threads=1`、`./scripts/run-integration-tests.sh integration`：PASS。看板 `T2-P0-005` 已收 `PENDING_INTEGRATION`；实现已落于本分支 tip（待 push `origin/feature/tool-system-cleanup`）。

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
- **2026-05-05（多 LLM + Responses 收口）**：同上全量门禁 PASS；`openai_responses_integration_tests` 依赖 `OPENAI_API_KEY`（与 `llm_tests` 同口径）；请求体 `max_output_tokens` 与上游最小值（16）对齐，避免 400。

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
- **架构文档**：新增 `openspec/specs/architecture/tools/search_files.md`（含 One-Glance Map + 行为对照 + 预检策略 + 测试映射）。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`：PASS；`search_files_tests`（10 passed）、分类集成全量：PASS（口径见 [INTEGRATION_TEST_SPEC §7](../../openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)、[`scripts/test-groups.sh`](../../scripts/test-groups.sh)）。
- **看板**：`agents/TASK_BOARD_002.md` T2-P0-005 子项追加「search_files 兜底与预检」，状态 `PENDING_INTEGRATION`，并写明与 T2-P1-008 的口径关系。

### 2026-05-03 14:45 | 集成验收口径与门禁文档对齐提交

- 门禁与流程：`Dispatcher` 全量前 `test-groups`、`TASK_BOARD` / `develop` status / `UNIT_TEST_SPEC` 验收引用统一为 **INTEGRATION_TEST_SPEC §7**（不再从 specs 链到 `agents/INTEGRATION_MERGE_AND_ACCEPTANCE`）；§7.2 正文列举与 `scripts/test-groups.sh` 对齐（含 `search_files_tests`、`tool_catalog_doc`）。
- 仓库：`Tomcat/.gitignore` 与 `pi-rust-wasm/.gitignore`  scratch 目录 **`workspace-temp/`**；本地已将目录 **`workspace` → `workspace-temp`**（若其他克隆仍有旧名请自行 `mv`）。
- 代码要点：`truncation` 空 `work_dir` 不落盘 `tool-results`；测试用 `agent_definition_dir` 子目录名 **`workspace-temp`**；`test-groups` 已含 `search_files_tests`。

### 2026-05-03 13:40 | 文档编写规范拆分与引用对齐

- `openspec/specs/guides/workflow/DOCUMENTATION_GUIDE.md` 精简为索引页；新增 `MODULE_README_SPEC.md`、`ARCHITECTURE_SPEC.md`；`openspec/specs/Architecture.md`、`agents/plan/PLAN_SPEC.md`、`PLAN_SKELETON.md` 中架构方案与 One-Glance Map 硬约束改为指向 `ARCHITECTURE_SPEC.md`（标杆：`architecture/tools/search_files.md`）。
- `openspec/specs/architecture/tools/search_files.md` 扩充协议表、竞品分析、时序与状态机 ASCII 图。
- 仓库根 `pi-rust-wasm/.gitignore` 忽略本地 `tool-results/` 与 `workspace-temp/`（研发 scratch 约定见 `UNIT_TEST_SPEC.md` §1.2），避免误提交。

### 2026-05-03（同日追加）| Unix：退出 chat 后 Tier1 安装可继续（nohup detached）

- **`preflight.rs`**：`cfg(unix)` 路径用 `/bin/sh -c 'nohup … >> log 2>&1 &'` + `spawn`，不 `output()` 等待；Homebrew 并发用 `pgrep -f` 窄匹配；可选 `preflight-detached-log.marker` 仅记日志路径（UX）；Windows 仍阻塞 `output()`，源码内 TODO：PowerShell detached。
- **`stderr.rs`**：处理 wire `detached` / `already_installing`（灰字 + `logPath` + `tail -f` 提示）。
- **`search_files.md`**：§4 / §7 / §8 / §9 / §12 与上述行为对齐。
- **测试**：`api/chat/tests/preflight_test.rs` + `preflight.rs` `#[path]` 挂载（`RUST_FILE_LINES_SPEC` §A.9）；用例含 `nohup_shell_quotes_log_path_with_spaces`。**未**新增 integration 二进制（无需改 `test-groups.sh`）。
- **门禁**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -p pi_wasm --lib api::chat::preflight::tests`：提交前执行。

### 2026-05-03 19:16 | search_tools 预检可观测性与 CLI 展示

- **预检**：包管理器每次安装尝试将完整 stdout/stderr 落盘 `~/.pi_/agents/main/logs/preflight-file-log-<ts>.log`；`tracing` target `pi_wasm_preflight`（`RUST_LOG=debug`）；成功/失败事件 `extra` 带 `logPath`；文档明确 `Command::output` 无 pi 侧超时，勿与 `PI_SEARCH_TIER2_DEADLINE_MS` 混淆；`PreflightConfig` 注释同步。
- **stderr 监听**：`WIRE_SEARCH_TOOLS_PREFLIGHT` 优先经 `rustyline::ExternalPrinter` 输出，避免 `readline` 阻塞时 `[tools]` 与输入行错位；失败时附加截断后的 stderr / error / log 路径摘要。
- **架构**：`search_files.md` One-Glance Map 补充上述行为。

### 2026-05-05 | 承接子项「多 LLM 层改造 + OpenAI Responses」

- **状态**：T2-P0-005 由 `PENDING_INTEGRATION` 改回 `DOING`；本子项与已有 6 个 ahead commits（origin 未 push）一并延后到本子项完成后再 push 交集成。
- **范围**（plan `~/.cursor/plans/llm-multiprovider-and-responses_d469e7f0.plan.md`）：
  - Phase A 新建 `src/core/llm/registry.rs` + `resolve_llm`，`src/api/chat/mod.rs:164` 一行替换为 registry 调用；`default_llm_provider()` 翻为 `"openai-responses"`（D3）。
  - Phase B 新建 `src/core/llm/openai_responses.rs`：`build_responses_input`（system → instructions、user/assistant/tool 翻 `input` items）+ `convert_tools_to_responses` + `count_tokens`。
  - Phase C `ResponsesStream`：默认 SSE 解析，第一帧无 `data:` 前缀且能 parse 整行 JSON 时切 NDJSON 兜底；retry / fallback / proxy / 信号量复用 `LlmConfig`。
  - Phase D Compaction 联动：`generate_summary` 已走 `LlmProvider::chat`，主对话翻 Responses 后摘要随之，无代码改动；仅回归 preheat 测试。
  - Phase E 单测（registry / wire / stream）+ 集成 `tests/openai_responses_integration_tests.rs`（httptest 或 wiremock，登记 `scripts/test-groups.sh` 并发组）+ 全量门禁。
  - Phase F 文档：spec `llm-multiprovider-integration.md` §4 / §2.4 / §7 / §8、`README.md`、`pi.config.toml.example`、`user-guide.md`、看板子项勾选。
  - Phase G commit-guard 分阶段提交 + 6 ahead commits + 本子项 commits 一起 push + 看板回 PENDING_INTEGRATION。
- **决策摘要**：D1–D7（plan §2 / §2.1）；架构走 spec 岔路 A，Agent Loop 仍组一份 `ChatRequest`，wire 翻译封装在 Provider 内部。

### 2026-05-04 | macOS Homebrew 预检仅 bottle、stderr 提示 Tier2

- **`preflight.rs`**：`brew install` 使用 `--force-bottle`；`build_nohup_shell_command` 在 `program == "brew"` 时前缀 `HOMEBREW_NO_BUILD_FROM_SOURCE=1`，避免缺 bottle 时长时间源码编译；`detached` 事件文案区分 Homebrew 与通用路径。
- **`stderr.rs`**：`already_installing` 分支追加灰字说明 Tier1 未就绪时仍可用进程内 Tier2 搜索。
- **文档**：`search_files.md`、`TASK_BOARD_002` 与上述行为对齐；`docs/reports/agent-tools-comparison.md` 为五项目 Agent 工具对比调研归档。
- **测试**：`preflight_test` 覆盖 brew 命令前缀与「非 brew 不注入 `HOMEBREW_NO_BUILD_FROM_SOURCE`」。
