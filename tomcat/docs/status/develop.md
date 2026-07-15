| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-15 17:40 +0800 | ACTIVE | develop | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P1]** 云化服务技术方案已落盘：`tomcat/docs/architecture/cloud-scale-serving/` 新增 `01-overview` + Phase A/B/C（`02/03/04`），并在架构 README 登记「扩展性与云化」阅读顺序；纠正「全站单 EventBus」误诊，明确 A 期单机 mailbox/热温冷为先落地、本地 `stdio` 零回退；尚未进入 Phase A 编码 @2026-07-15
- [✓] **[P0]** `feature/transcript-ui-and-checkpoints` 已 fast-forward 合入 `develop`（`866752c -> 62c8811`）；develop-side review 发现并修复跨 session restore 漏洞：`restore_checkpoint` / `/restore` 现拒绝不属于当前会话的 checkpoint，避免 `revertFiles=true` 把共享工作区回滚到别的会话快照；新增 `serve_restore_checkpoint_rejects_foreign_session_checkpoint` 回归测试，`cargo test -p tomcat serve_restore_checkpoint -- --nocapture` 4/4 通过 @2026-07-15
- [✓] **[P0]** develop-side 扩展快门禁与真实宿主高风险复核完成：`npm run gate:fast` 全绿；`TOMCAT_E2E_GREP='renders the \\.plan\\.md custom editor|renders transcript action rows and context groups|keeps sticky user prompts aligned with historical turns in the Tomcat webview|restores plan cards and Ctx after switching sessions' npm run test:e2e:vscode-devhost` 4/4 通过 @2026-07-15
- [✓] **[P0]** develop-side Rust web-search 门禁已收口：把 `tomcat/tests/cli_tests.rs` 与 `tomcat/tests/web_search_tool_tests.rs` 中真实 HTTPS mock + plugin runtime 成功路径的外层 wall-clock budget 与 timeout 失败 teardown 对齐，消除 nextest 满并发下的假红 / slow / hung；focused 复核全绿，`./scripts/run-integration-tests.sh integration-parallel` 364/364 通过（仅 `real_mimo_web_search` 被标 slow 但 PASS）@2026-07-15
- [✓] **[P0]** `feature/add-model-form-redesign` 已快进合入 `develop`（HEAD `68a4f43`）：覆盖 Add Model 双模式 / `ModelView.source` / `ThinkingFormat::Auto` 收口、Composer `@` 上下文搜索，以及 Transcript diff / plan / sticky UX 修复；develop-side review 中顺手补齐 Webview live diff snapshot 复用、`hashline_edit.rs` clippy 收口与对应 E2E 断言对齐 @2026-07-12
- [✓] **[P1]** 新增根目录 Cursor command `/release-cli-ext`：固化 CLI→EXT 发版顺序（patch+1、develop 推送、main/master fast-forward、cli tag 等资产、ext tag 等资产），减少手工漏步 @2026-07-06
- [✓] **[P1]** Add Models 架构方案：新增 `tomcat-vscode-ext/docs/architecture/model-management-add-models.md` 总览 + 5 篇子文档（术语/决策/协议/验收/UI ASCII 基线），对齐 Phase 2 文档组织方式 @2026-07-06
- [✓] **[P1]** Add Models 实现已合入 `develop`：以 `git merge --no-ff` 合并 `feature/add-models`（merge `68ff90a`），并在分支侧与 `develop` 侧完成 Rust/CLI/serve 全量门禁、VS Code 扩展 build/unit/integration/GUI/VSIX install/verify 复跑，验收通过 @2026-07-09
- [✓] **[P1]** 会话标题修复已落地：后端新增 `ChatMessage::first_text()` + `extract_user_text_from_content()`，让 `content` 为 `Parts`/`input_text` 的首条 user 消息也能正确派生标题；`run_loop` 叠加 L0 即时 `session.title_updated` 与 title scene 失败时降级到主模型；扩展状态机/E2E fake host 同步补齐。验证：`cargo test --lib first_text`、`cargo test --lib extract_user_text_from_content`、`cargo test --lib append_user_message_with_structured_parts_derives_title_from_input_text`、`cargo test --lib read_first_user_message_text_supports_structured_input_text_parts`、`cargo test --test transcript_summary_integration_tests session_title_updated_`、`npm run lint`、`npx vitest run src/ui/webview/tests/state.test.ts`、`TOMCAT_E2E_GREP='derives non-placeholder session titles from first webview prompt segments' npm run test:e2e:vscode-devhost` 全绿。

### 🔌 INTERFACE (接口变更)
- 无代码接口变更。新增架构文档入口：`tomcat/docs/architecture/cloud-scale-serving/01-overview.md`（父导航）及 Phase A/B/C 子文档；不改变现有 `serve` / `stdio` 对外契约。
- `restore_checkpoint` / `/restore` 现强制 checkpoint 的 `session_id` 必须等于当前会话；跨 session restore 返回错误 `checkpoint 不属于当前会话，不能跨会话 restore`，不再允许误回滚共享工作区。

### ⚠️ BLOCKED (阻塞/风险)
- 当前无 develop-side 阻塞项。`real_mimo_web_search` 在 nextest 中 86.99s 被标记为 slow，但结果为 PASS，仅作为性能观察项保留，不构成门禁阻塞。
- Phase A 编码尚未启动；待评审确认后再按 `02-phase-a-session-mailbox-hot-cold.md` 落地。

### 集成说明
- 本轮仅文档：云化技术方案四篇 + 架构 README 地图；无 Rust/扩展代码变更，不触发集成门禁。
- `feature/transcript-ui-and-checkpoints` 已本地 fast-forward 合入 `develop`（HEAD `62c8811`），覆盖 checkpoint restore / transcript UI / `.plan.md` custom editor / bash summary upgrade；develop-side acceptance 现已 **通过**，结论更新为 **GO**（尚未推送远端）。
- Rust develop-side 复核确认：先前 web-search 红点并非产品逻辑回归，而是测试自身的预算/清理脆弱。真实 HTTPS mock + plugin runtime 成功路径在 nextest 满并发下会逼近 5s/10s 外层 timeout；部分用例又在 `end_session` 前 panic，导致大盘表现为 slow/hung。现已在 `tomcat/tests/cli_tests.rs` 与 `tomcat/tests/web_search_tool_tests.rs` 放宽 success-path wall-clock budget，并保证 timeout/result error 时先 teardown 再失败；focused `cargo test --test web_search_tool_tests works_from_public_api -- --nocapture`、`cargo test --test web_search_tool_tests runtime_auto_routes_to_plugin_backends_after_retryable_failures -- --nocapture`、`cargo test --test web_search_tool_tests runtime_session_vm_survives_idle_beyond_call_timeout -- --nocapture`、`cargo test --test cli_tests test_chat_path_executes_web_search_tool_with_mock_server -- --nocapture` 全绿。
- Rust 默认快门禁已重新闭环：`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`、`cargo test --doc` 与 `./scripts/run-integration-tests.sh integration-parallel` 全绿；其中 integration-parallel 为 `364/364 passed`，仅 `real_mimo_web_search` 被 nextest 标 slow 但正常 PASS。
- 此分支无独立 `PENDING_INTEGRATION` 任务卡映射；develop-side 仅记录合并与验收事实，分支侧证据以 `docs/status/feature-transcript-ui-and-checkpoints.md` 为准，不额外伪造 TASK_BOARD 状态迁移。
- `feature/add-model-form-redesign` 已在 `develop` 合并验收：扩展 `npm run check:wire`、`npm run gate:fast`、`npx vitest run src/ui/webview/tests/provider.test.ts src/ide/tests/diff_apply_edit.test.ts` 全绿；Devhost 全量先暴露 `applies edits from the Tomcat webview` 回归，补齐 live prepared snapshot 语义后，`TOMCAT_E2E_GREP='applies edits from the Tomcat webview' npm run test:e2e:vscode-devhost` 复绿。
- 真 LLM 复核本轮未追加 `integration-real-llm`：本机 `DEEPSEEK_API_KEY` / `MIMO_API_KEY` 在 `.env` 中存在，但缺 `OPENAI_API_KEY`，不足以按当前高风险清单完整复跑 OpenAI 直连口径；该空白已显式记录，不冒充“全绿”。
- 面向用户文档已双语化（英文默认 + opencode 式切换栏）：根 README、扩展 README、`user-guide` 各新增 `.zh.md` 中文镜像；`tomcat-vscode-ext/.vscodeignore` 已放行 `README.zh.md` 入 VSIX。
- 根 README 已对齐双组件 monorepo 现状：补充 `tomcat/` + `tomcat-vscode-ext/` 组件索引、Agent Box/CLI 双入口架构图，并修正终端用户前提（`tomcat init` → `~/.tomcat/assets/.env`；Rust 1.70+ 仅源码构建需要）。
- 用户文档已切换主推 **Tomcat Agent Box**：根 README、扩展 README、`user-guide` 与 `package.json` 面板/命令文案同步更新，新增 `assets/tomcat-agent-box.png` 截图；`manifest_contract` 与 GUI 单测已复跑通过。
- 本次 Add Models 按例外流程验收：以 `tomcat-vscode-ext/docs/architecture/model-management-add-models.md` 及其 5 篇子文档作为验收 SSoT，未回填 `TASK_BOARD_002` 任务卡，也未补建 `tomcat/docs/status/feature-add-models.md`；合并与验收口径均按用户确认流程执行。
- 已在 `develop` 上以 `git merge --no-ff` 合入 `feature/add-models`（merge `68ff90a`），并再次复跑 `tomcat/` 的 `./scripts/run-integration-tests.sh all` 与 `tomcat-vscode-ext/` 的 `npm run build`、`npm run test:unit`、`npm run test:integration`、`npm --prefix gui run test`、`npm run test:e2e:vscode-install`、`npm run verify:vsix`，全部通过。
- 已在 `develop` 上以 `git merge --no-ff` 合入 `feature/tomcat-vscode-extension`（merge `2b04acd`），覆盖 `T2-P1-019` 与 `T2-P1-020`。
- Rust develop-side 门禁已通过：`./scripts/run-integration-tests.sh all` 全绿；`integration-openai-responses-wire` 在 LiteLLM 直连网关口径下复跑通过。集成期补修 `tomcat/src/ext/runtime/instance.rs`，把 QuickJS host bootstrap 排除到插件 timeout budget 之外，并补强对应断言，消除 `quickjs_e2e_tests::runaway_plugin_timeout_interrupts_when_budget_disabled` 的假红；同一提交还带入 `tomcat/src/core/tools/primitive/executor/write_edit.rs` 的备份提交路径收敛与回滚失败显式报错，并为 overwrite/rollback 边界补了配套测试。
- VSCode 扩展门禁已通过：`npm run build`、`npm run test:unit`、`npm run test:integration`、`npm --prefix gui run test`、`npm audit`（0 vulnerabilities）、`npm run package:vsix`、`npm run test:e2e:vscode-install`（26 passing）、`npm run verify:vsix`（4 passing，含截图裁剪产物）。
- 为消除 VSIX 安装 E2E 的 develop-side 假红，本轮补稳了 `hostE2eScenario.ts` / `e2eHostFixture.ts`：ownership 场景改为显式挂起 participant 问答后再切 webview，transcript UI 场景在常规安装套件允许直接落到最终折叠态，而 `verify:vsix` 继续强制捕获 docked todo/progress 视觉证据；同一提交也修正了 `App.tsx` / `Composer.tsx` / `provider.ts` / `handler.ts` 的 ownership 释放、interrupt 可用性与 history loading 收口，并补上单测与安装 E2E 覆盖。
- 4 件套 review 已覆盖 `serve`、`summary`、`plan_tool`、`primitive/executor` 与 VSCode 扩展热区；未发现未记录的规范违背。`integration-real-llm` 本轮无新增 target，按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` §4 跳过，不构成本次合并阻塞。
- 结论：`feature/tomcat-vscode-extension` 已在 `develop` 完成合并、复测与验收，`T2-P1-019` / `T2-P1-020` 可转 `DONE`；对应验收提交现已位于 `origin/develop`。
- 已知残留（本期刻意不补）：插件 `ext/dispatcher/session_ops.rs -> try_append_message_to_session` 的注入路径仍不推送活跃 `session.title_updated`，disk 刷新后的标题正确；本轮仅修复 Prompt → `run_loop` 主链路，即计划中的“放法一”。
