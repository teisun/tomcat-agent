| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Cursor | 2026-06-16 11:55 +0800 | ACTIVE | feature/host-functions-point-override | — |

### 2026-06-16 | host-facing `functions[]` point override 语义收口

- **分支与范围**：从 `develop` 拉出 `feature/host-functions-point-override`，收口 host-facing `functions[]` 的分层发现 / 注册面语义，并同步更新主架构文档、PackageManager、User Stories、E2E 场景库、集成交付说明、`web_search` 架构文档与用户指南。
- **语义结论**：`functions[]` 与普通 plugin 一样复用 `scope > agent > global` 三层发现 / 安装链；真正特殊的地方只在 `FunctionRegistry` 物化阶段按 `point` 执行 override。高层声明覆盖低层；同层冲突 stable first-wins + warning；高层移除后低层可在 refresh 后重新成为赢家；`web_search.backend` 的 `unsupported_backend` 不再跨插件兜底。
- **代码与测试收口**：`materialize_host_functions_from_catalog()` 改为按 layer+point 选赢家并产出 `function_point_conflict` / `function_point_shadowed` warning；`ExtPluginSearchInvoker` 只消费当前赢家 provider；`runtime_split_test` / `plugin_function_invoker_test` / `function_registry_test` / `catalog_test` 已补齐 point override、tools 不受影响、registry contract 与官方单插件 `auto` 路由守护。
- **本轮验证**：已通过 `cargo test -p tomcat --lib runtime_split_test -- --nocapture`、`cargo test -p tomcat --lib plugin_function_invoker_test -- --nocapture`、`cargo test -p tomcat --lib function_registry_test -- --nocapture`、`cargo test -p tomcat --lib catalog_test -- --nocapture`、`cargo test -p tomcat --lib plugin_backend_ -- --nocapture`、`cargo clippy --all-targets -- -D warnings`。
- **门禁决策**：本轮**未**额外复跑“无 OpenAI 全量 integration gate”。原因是改动面集中在 host function 分层发现、point override 物化、`web_search.backend` 调用收口与文档/单测；未改 CLI 外部协议、真实 LLM/OpenAI 路径或新的 integration binary。focused suites 已覆盖本次语义风险面，故先以 focused + clippy 作为本轮 gate 记录。

### 2026-06-16 | merge `feature/plugin-function-surface` → develop（T2-P1-016 / T2-P1-017 集成验收）

- **合并范围**：将 `feature/plugin-function-surface` 合并到 `develop`（merge `0496fc2`），覆盖 rquickjs 插件函数面与 PackageManager 主体实现，并带入功能分支验收收口：`pi_bridge.js` 的 handler 错误隔离与 async hostcall budget reset、layered host function catalog 发现、PackageManager registry / force-install 回滚清理、web_search plugin fallback / normalizeCount 修正、builtin plugin 运行时产物刷新，以及对应 OpenSpec / 场景库 / 状态台账同步。
- **develop 侧复核**：继续按编码规范 4 件套复扫 `src/ext/`、`src/core/package/`、`src/api/chat/context.rs`、`src/api/cli/*`、`src/core/tools/web_search/*` 及相关 tests/docs；额外修复两处 develop-side 门禁问题：`search_bash_contract_test` 的后台 stdout 断言窗口在并发满载下过紧，改为 300ms；`dispatch_with_extension_test` 的 `CapturingLlm::new()` 返回类型触发 `clippy::type-complexity`，已抽成命名 type alias。
- **§4 全量验收（develop 侧复跑）**：`OPENAI_API_KEY` 先用 `./scripts/verify-openai-apis.sh 1` 确认为外部 401 无效 key；按用户确认，本轮以与功能分支相同的口径复跑 `cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`，并依据 `scripts/test-groups.sh` 跑完 `integration-parallel` / `integration-serial`，仅显式排除 `openai_responses_integration_tests`。其余门禁全绿：`lib` **1802 passed, 1 ignored**；并发 / 串行 integration 全部通过，DeepSeek 相关 `cli_tests` / `llm_tests`、QuickJS / plugin / package-manager 路径均复验通过。
- **结论**：`T2-P1-016` / `T2-P1-017` 已在 `develop` 完成合并、review 与复验，任务卡从 `PENDING_INTEGRATION` 更新为 `DONE`。唯一未补跑项是受本机无效 OpenAI 凭证影响的 `openai_responses_integration_tests`；该项已明确为外部环境阻塞，不视为本次代码合并阻塞。未推送远端。

### 2026-06-11 | fix(install): 改用 release metadata 下载资产

- **动机**：本机执行 `curl .../install.sh | bash` 时，直连 `github.com/.../releases/download/...` 获取 `SHA256SUMS` / tar.gz 出现 `HTTP2 framing layer` 与超时，导致一键安装不稳定。
- **实现**：`install.sh` 改为先读取 GitHub release 元数据，再解析目标 asset 的 API 下载地址与 `sha256` digest；下载阶段优先走 `python3 + urllib`，失败后回退到 `curl --http1.1 --retry-all-errors`，不再依赖单独下载 `SHA256SUMS` 直链。
- **验证**：已在本机用临时 `HOME` 完整执行 `bash tomcat/scripts/install.sh -y -v v0.1.4`，成功安装并输出 `tomcat 0.1.4`。

