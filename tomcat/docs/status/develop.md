| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-06 07:35 +0800 | ACTIVE | develop | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P1]** Add Models 架构方案：新增 `tomcat-vscode-ext/docs/architecture/model-management-add-models.md` 总览 + 5 篇子文档（术语/决策/协议/验收/UI ASCII 基线），对齐 Phase 2 文档组织方式 @2026-07-06

### 集成说明
- 面向用户文档已双语化（英文默认 + opencode 式切换栏）：根 README、扩展 README、`user-guide` 各新增 `.zh.md` 中文镜像；`tomcat-vscode-ext/.vscodeignore` 已放行 `README.zh.md` 入 VSIX。
- 根 README 已对齐双组件 monorepo 现状：补充 `tomcat/` + `tomcat-vscode-ext/` 组件索引、Agent Box/CLI 双入口架构图，并修正终端用户前提（`tomcat init` → `~/.tomcat/assets/.env`；Rust 1.70+ 仅源码构建需要）。
- 用户文档已切换主推 **Tomcat Agent Box**：根 README、扩展 README、`user-guide` 与 `package.json` 面板/命令文案同步更新，新增 `assets/tomcat-agent-box.png` 截图；`manifest_contract` 与 GUI 单测已复跑通过。
- 已在 `develop` 上以 `git merge --no-ff` 合入 `feature/tomcat-vscode-extension`（merge `2b04acd`），覆盖 `T2-P1-019` 与 `T2-P1-020`。
- Rust develop-side 门禁已通过：`./scripts/run-integration-tests.sh all` 全绿；`integration-openai-responses-wire` 在 LiteLLM 直连网关口径下复跑通过。集成期补修 `tomcat/src/ext/runtime/instance.rs`，把 QuickJS host bootstrap 排除到插件 timeout budget 之外，并补强对应断言，消除 `quickjs_e2e_tests::runaway_plugin_timeout_interrupts_when_budget_disabled` 的假红；同一提交还带入 `tomcat/src/core/tools/primitive/executor/write_edit.rs` 的备份提交路径收敛与回滚失败显式报错，并为 overwrite/rollback 边界补了配套测试。
- VSCode 扩展门禁已通过：`npm run build`、`npm run test:unit`、`npm run test:integration`、`npm --prefix gui run test`、`npm audit`（0 vulnerabilities）、`npm run package:vsix`、`npm run test:e2e:vscode-install`（26 passing）、`npm run verify:vsix`（4 passing，含截图裁剪产物）。
- 为消除 VSIX 安装 E2E 的 develop-side 假红，本轮补稳了 `hostE2eScenario.ts` / `e2eHostFixture.ts`：ownership 场景改为显式挂起 participant 问答后再切 webview，transcript UI 场景在常规安装套件允许直接落到最终折叠态，而 `verify:vsix` 继续强制捕获 docked todo/progress 视觉证据；同一提交也修正了 `App.tsx` / `Composer.tsx` / `provider.ts` / `handler.ts` 的 ownership 释放、interrupt 可用性与 history loading 收口，并补上单测与安装 E2E 覆盖。
- 4 件套 review 已覆盖 `serve`、`summary`、`plan_tool`、`primitive/executor` 与 VSCode 扩展热区；未发现未记录的规范违背。`integration-real-llm` 本轮无新增 target，按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` §4 跳过，不构成本次合并阻塞。
- 结论：`feature/tomcat-vscode-extension` 已在 `develop` 完成合并、复测与验收，`T2-P1-019` / `T2-P1-020` 可转 `DONE`；对应验收提交现已位于 `origin/develop`。
