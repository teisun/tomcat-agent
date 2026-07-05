| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Cursor | 2026-06-20 21:05 +0800 | ACTIVE | develop | — |

### 2026-06-20 | chore: 发布版本号 0.1.7

- **变更**：`tomcat/Cargo.toml` 与 `Cargo.lock` 将 crate 版本从 `0.1.6` 升至 `0.1.7`。
- **原因**：`feature/agent-server-ui-gateway` 已合入 `develop` 并完成验收，主线版本号与当前集成状态对齐，便于后续 release / install 脚本引用。

### 2026-06-20 | merge `feature/agent-server-ui-gateway` → develop（T2-P1-018 集成验收）

- **合并范围**：已将 `feature/agent-server-ui-gateway` 快进合入 `develop`（feature 头提交 `99a720f`），带入 `tomcat serve` 多会话 stdio gateway、共享 `AgentRegistry` + `FanoutEventBus`、writer backpressure 改造、`ask_question` camelCase wire、`init/models.toml` 迁移、schema / `.d.ts` 工件、多模态附件 roundtrip，以及对应 tests / docs / VSCode 扩展架构文档同步。
- **develop 侧验收**：clean env `./scripts/run-integration-tests.sh all` 全绿；live 组按 `set -a && source .env && set +a && NO_PROXY=aigateway.sunmi.com no_proxy=aigateway.sunmi.com ...` 口径复跑 `integration-real-llm` 与 `integration-openai-responses-wire`，均通过。
- **网络口径**：本机已接入内网时，`aigateway.sunmi.com` 应走 `NO_PROXY` 直连，不再经本地 `127.0.0.1:7890` 代理；否则 `responses_inline_image_describe_roundtrip` 可能返回 HTML `403 Forbidden`。同口径下 `responses_inline_pdf_input_file_summarize_roundtrip` 记录过 1 次瞬时“未看到附件”的 live 抖动，但原文件级复跑与脚本重试均转绿，已同步到验收文档。
- **状态台账**：`T2-P1-018` 已从 `PENDING_INTEGRATION` 更新为 `DONE`；`v0.1.7` 已同步至 `main` / `master` 并推送远端。

### 2026-06-17 | merge `feature/host-functions-point-override` → develop（Nibbles 集成验收）

- **合并范围**：已将 `feature/host-functions-point-override` 合入 `develop`，把 host-facing `functions[]` 的 layer+point override 语义、`web_search.backend` 单赢家消费模型、插件系统架构文档补图，以及 develop 侧验收所需的 VM 会话清理、test lock / Tokio handle 修复、`cli_tests` 路由断言收口一起带入主线。
- **develop 侧验收**：`cargo build --release`、`cargo test --lib -- --nocapture`、`cargo clippy --all-targets -- -D warnings` 全部通过；串行 E2E 复跑完成，`cli_tests` **108 passed**、`quickjs_e2e_tests` **15 passed**。其中 `cli_tests` 的 develop-side 收尾包含两处预期修正：shadowed `web_search.backend` 不再跨插件兜底，以及 503 exhausted 后按同进程 `context_state` 保留失败轮进度。
- **integration 口径**：`run-integration-tests.sh integration` 触发 `openai_responses_integration_tests` 的 401；经用户确认，本机 `OPENAI_API_KEY` 已过期，属于外部环境阻塞而非代码回归。本轮据此以本地 build / lib / clippy / E2E 门禁作为 develop 合并验收记录，其余非 OpenAI 本地回归路径均已复核通过。
- **状态台账**：`TASK_BOARD_002` 中未找到与 `feature/host-functions-point-override` 对应的 `PENDING_INTEGRATION` 任务卡，因此本轮仅更新 `docs/status/develop.md`；未推送远端。

### 2026-06-16 | merge `feature/plugin-function-surface` → develop（T2-P1-016 / T2-P1-017 集成验收）

- **合并范围**：将 `feature/plugin-function-surface` 合并到 `develop`（merge `0496fc2`），覆盖 rquickjs 插件函数面与 PackageManager 主体实现，并带入功能分支验收收口：`pi_bridge.js` 的 handler 错误隔离与 async hostcall budget reset、layered host function catalog 发现、PackageManager registry / force-install 回滚清理、web_search plugin fallback / normalizeCount 修正、builtin plugin 运行时产物刷新，以及对应 OpenSpec / 场景库 / 状态台账同步。
- **develop 侧复核**：继续按编码规范 4 件套复扫 `src/ext/`、`src/core/package/`、`src/api/chat/context.rs`、`src/api/cli/*`、`src/core/tools/web_search/*` 及相关 tests/docs；额外修复两处 develop-side 门禁问题：`search_bash_contract_test` 的后台 stdout 断言窗口在并发满载下过紧，改为 300ms；`dispatch_with_extension_test` 的 `CapturingLlm::new()` 返回类型触发 `clippy::type-complexity`，已抽成命名 type alias。
- **§4 全量验收（develop 侧复跑）**：`OPENAI_API_KEY` 先用 `./scripts/verify-openai-apis.sh 1` 确认为外部 401 无效 key；按用户确认，本轮以与功能分支相同的口径复跑 `cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`，并依据 `scripts/test-groups.sh` 跑完 `integration-parallel` / `integration-serial`，仅显式排除 `openai_responses_integration_tests`。其余门禁全绿：`lib` **1802 passed, 1 ignored**；并发 / 串行 integration 全部通过，DeepSeek 相关 `cli_tests` / `llm_tests`、QuickJS / plugin / package-manager 路径均复验通过。
- **结论**：`T2-P1-016` / `T2-P1-017` 已在 `develop` 完成合并、review 与复验，任务卡从 `PENDING_INTEGRATION` 更新为 `DONE`。唯一未补跑项是受本机无效 OpenAI 凭证影响的 `openai_responses_integration_tests`；该项已明确为外部环境阻塞，不视为本次代码合并阻塞。未推送远端。

### 2026-06-11 | fix(install): 改用 release metadata 下载资产

- **动机**：本机执行 `curl .../install.sh | bash` 时，直连 `github.com/.../releases/download/...` 获取 `SHA256SUMS` / tar.gz 出现 `HTTP2 framing layer` 与超时，导致一键安装不稳定。
- **实现**：`install.sh` 改为先读取 GitHub release 元数据，再解析目标 asset 的 API 下载地址与 `sha256` digest；下载阶段优先走 `python3 + urllib`，失败后回退到 `curl --http1.1 --retry-all-errors`，不再依赖单独下载 `SHA256SUMS` 直链。
- **验证**：已在本机用临时 `HOME` 完整执行 `bash tomcat/scripts/install.sh -y -v v0.1.4`，成功安装并输出 `tomcat 0.1.4`。

