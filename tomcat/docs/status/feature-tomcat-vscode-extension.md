| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-28 07:10 +0800 | DOING | feature/tomcat-vscode-extension | — |

### ✅ DONE
- [x] **[P1]** 认领 `T2-P1-020`，任务卡 / 看板索引已切到 `DOING / Tom`；依赖例外已按用户显式要求记录。
- [x] **[P1]** 建立 `feature/tomcat-vscode-extension` 分支并初始化本分支 status 文件。
- [x] **[P1]** 按 `tomcat-vscode-extension-phase2.md` 与 `02-stage-a-slash-and-serve.md` / `03-stage-b-webview.md` 完成 Phase 2 主体实现：Stage A 的 `/plan` / `/model` slash + serve 协议/状态/事件扩展已落地，Stage B 的 sidebar webview / React GUI / shared pool / ownership / diff bridge 已接通。
- [x] **[P1]** Rust `serve --print-schema` fixture 已随 Phase 2 协议扩展刷新，`tests/fixtures/serve/serve.schema.json` 与 `tests/fixtures/serve/serve.d.ts` 不再漂移。
- [x] **[P1]** 纠正此前把内部 `__testing` / host harness 误表述为“真实桌面 UI 验收”的问题；本轮重新以真实 VS Code 桌面 UI 作为最终口径。
- [x] **[P1]** 侧栏 webview 已收敛为聊天式布局：时间线 transcript（消息 / thinking / tool / approval / plan 卡片）、底部内嵌 composer（`+` / `Chat|Plan` / `Model` / `Ctx%` / 圆形发送）、会话选择栏、停靠 Todo widget、附件 chips 与合并后的 Plan/Build 卡片全部落地。
- [x] **[P1]** 真实桌面 UI 验收已完成：打开 Tomcat 侧栏、真实发送消息、切模型 `fake-model -> gpt-5.4`、`Chat -> Plan -> Build -> Chat`、打开 `.plan.md` 文件、添加附件并发送、观察 `Ctx 42% -> 58%`，全程不依赖内部注入。
- [x] **[P1]** 更新交付文档：status、`T2-P1-020` 任务卡、看板索引与 Stage B webview 架构文档已同步到最新实现事实。
- [x] **[P1]** 按 `/commit-with-status` 完成本地合规提交（聊天式 webview 重构 + 文档同步）。
- [x] **[P2]** 修复 macOS login bash 下 `tomcat init` / `install.sh` 写入 PATH 后新终端仍找不到 `tomcat`：`auto_add_to_path` 改为 PATH 写 `.bashrc` 并确保 `.bash_profile` source `.bashrc`，`install.sh` 同步补齐。
- [x] **[P2]** 补全 bash login shell 链路：`.bash_profile` 同时 source `.profile` 与 `.bashrc`，避免截断 `~/.profile` 中已有 Rust/cargo 等通用环境；新增 bash 场景 init 回归测试。
- [x] **[P1]** 2026-06-26 误删事故后恢复可执行计划：`.cursor/plans/transcript-ui-restore.plan.md`（仿 VSCode Chat 重做 transcript，52 todo，含 utility-flash 默认模型配置）。
- [x] **[P1]** 新增 Agent 安全规则 `tomcat/.cursor/rules/no-rm-rf.mdc`（禁止 `rm -rf "$VAR"` 跨命令边界等事故形态，alwaysApply）。
- [x] **[P1]** Transcript UI 仿 VSCode Chat（`transcript-ui-restore.plan.md`）：后端 `core/summary` utility 摘要、`TurnEnd.summaryTitle`、`session.title_updated` / `plan.todos` / `session.todos` wire；前端 ThinkingGroup / ToolRow / FileChip / ProgressRow / user pill / assistant 无框；`utility-flash` 默认 title 模型。
- [x] **[P1]** 后端单测：`cargo test --lib` 1935 passed（含 `summary::` 6 项、`models_toml` 5 项）。
- [x] **[P1]** 前端单测：GUI 74 项全绿；webview helper 测试（state / protocol / provider / dual_channel）18 项全绿。
- [x] **[P1]** §2 Rust 集成测试 `transcript_summary_integration_tests`（575 行 / 7 用例，mock LlmProvider 黑盒）：`TurnEnd.summaryTitle` 三路径（Some/None/utility 失败回退）、`plan.todos`/`session.todos` emit、`get_state` `planTodos`/`sessionTodos`、session title 异步覆盖；登记 `test-groups.sh` PARALLEL；`cargo test --test` → 5 passed; 2 ignored。
- [x] **[P1]** §3 VSCode 扩展 E2E：`npm run test:e2e:vscode-install` → **15 passing (49s), exit 0**，含 `assertTranscriptUiFlow`（user pill / assistant 无框 / thinking+tool 折叠摘要 / tool 扁平行+竖线+可展开 / FileChip / transcript 内联 progress / 停靠 Todo widget / 合并 Plan 卡 / `session.title_updated`）。
- [x] **[P1]** §4 verify-vsix 视觉验收：新增 `run-vscode-verify-vsix.ts` + `crop-screenshot.py` + `verify:vsix` script + 截图路径 env 化（`TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR`）；`npm run verify:vsix` → 1 passing + 裁剪图视觉确认 10 项 checklist（user pill / assistant 无框 / 折叠摘要 / tool 扁平行+竖线 / FileChip 图标 / bash / transcript 内联 progress / 停靠 Todo widget 折叠态 / Todo widget 展开态 / 合并 Plan 卡）。
- [x] **[P1]** §4 verify-integration：`./scripts/run-integration-tests.sh integration` → **exit 0，parallel + serial 全绿**；期间发现并修复 `serve_schema_fixture` 漂移（重生成 `tests/fixtures/serve/serve.schema.json` / `serve.d.ts`）；`transcript_summary_integration_tests` 5 passed/2 ignored、`cli_tests` 108 passed/0 failed、`serve_print_schema_matches_fixture` ok。
- [x] **[P1]** Wave 3 只读 Review：reviewer 结论 §2/§3/§4-vsix 测试有效性基本成立、无明确 bug；唯一实质性缺口为 §2 test 7 异步 `session.title_updated` emit 无活跃覆盖（已记为 stretch），低风险项见 `.cursor/plans/transcript-ui-restore-progress.md`。
- [x] **[P1]** 2026-06-27 第二轮自查：修复 bash/web_search 变"卡片"根因 bug（`state.ts` tool 事件误调 `clearStreaming` 清掉 `activeAssistantId` → 同轮第 2+ 工具 `assistantMessageId` 丢失 → 孤立 → `ToolCallCard`；改用 `clearThinkingStreaming` 只清 `activeThinkingId`）+ 孤立 tool 兜底由 `ToolCallCard` 改 `ToolRow`；E2E `assertTranscriptUiFlow` 加 `toolRowCount>=3` + `toolCardCount===0` 回归断言；补全 3 项 🟡 缺口（ThinkingGroup 折叠头 check/loading 图标 + spin、shimmer 改 `summaryTitle===null && isStreaming`、ToolRow grep `N results`/search_workspace 分支/web_search 结构化/edit 图标 + bash `<code>cmd</code>`）；gui 67 项单测全绿、ext tsc 干净、`verify:vsix` E2E ✔（read/bash/web_search 三工具均扁平行、零卡片）。详见 `.cursor/plans/transcript-ui-restore-progress.md`"卡片根因修复"节。
- [x] **[P1]** 2026-06-27 第三轮 transcript UI polish：webview 正式引入 `@vscode/codicons`（`vite base:"./"` 打包 `dist/codicon.ttf`，FileChip / tool / progress / todo 图标恢复）、`FileChip` 基线对齐修正、底部新增仿 VSCode 的停靠 Todo widget（折叠 `Render the transcript UI (2/4)`、展开 `Todos (2/4)` + radio icon 列表）、`PlanFileCard` 与原 `Build` strip 合并为 Cursor 风卡片（标题/描述来自 `.plan.md` frontmatter，footer `View Plan` + `Build`），并新增 provider frontmatter 缓存读取、`TodoListWidget`/`PlanFileCard`/`provider` 单测；`npm --prefix gui run test` 72 passing、`npm run lint` 通过、`npm run verify:vsix` 1 passing，新增裁剪图 `progress` / `todo-expanded` / `collapsed` / `expanded` 四张。
- [x] **[P1]** 2026-06-27 第四轮 transcript UI polish follow-up：收敛折叠态口径（`collapsed` 在 turn 结束后无 todo widget / 无内联 progress；`progress` 保留忙碌态内联 progress + 停靠 todo widget）、Composer 去掉 `Tomcat is responding...` 并将 `Plan: planning` 下移到 footer、Plan 卡 footer 同行与 `4 todos` 轻量 pill、`verify:vsix` 启动前清空旧 visual 产物并新增 `file-chip` 近景（`scrollIntoView` + 可视区断言）、`ToolRow` 扁平行 inline 组与固定图标列对齐（Read/chip、Ran/code、Searched 文本）；`npm --prefix gui run test` 74 passing、`npm run lint` 通过、`npm run verify:vsix` 1 passing，裁剪图 `collapsed` / `progress` / `todo-expanded` / `expanded` / `file-chip` 五张。
- [x] **[P1]** 2026-06-27 内置工具 icon 验收：`ToolRow` 补齐 19 个 built-in 工具的 codicon 与可读 label（`load_skill` / `list_dir` / `config_*` / `create_plan` / `update_plan` / `todos` / `ask_question` / task 系列等）；E2E fixture 新增 `tool icon showcase` 场景并断言 `toolRowCount>=19`，`verify:vsix` 产出 `tool-icons` / `tool-icons-bottom` 两张裁剪图便于一次性目视验收。
- [x] **[P1]** 2026-06-27 Transcript UI 事件接缝整改（planPath / contextRatio / custom `plan.*` history replay / plan transition wire）已落地：后端补 `plan.enter` / `plan.exit` / `plan.pending` / `plan.complete` 事件与 `get_state.contextUtilizationRatio` / `planPath`，前端改为按 `plan.path` 全局单卡收敛并在终态事件后回读 `get_state` 真相；期间发现并修复 cross-owner 观察态 bug（`setOwnership()` 错误清空 `conflictMessage`，导致只读会话误显示可 `Build`）。
- [x] **[P1]** 2026-06-27 剩余全量 gate 已补跑完成：提权后 `cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --nocapture`、`./scripts/run-integration-tests.sh integration`、`npm run test:unit`、`npm run test:integration`、`npm run test:e2e:vscode-install`、`TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR=/tmp/tomcat-vsix-verify-artifacts npm run verify:vsix` 全部通过；期间同步修正 1 个受新 `planPath` 语义影响的旧后端测试 `create_plan_multiple_times_overrides_active_planning_id_and_binding_path`。
- [x] **[P1]** 2026-06-27 `verify:vsix` 进一步补齐计划验收口径：默认增跑 `switch restore` / `reload replay` / `cross-owner` / `transcript UI` 四条安装态用例，并新增三张场景裁剪图 `switch-restore` / `reload-replay` / `cross-owner`，用于人工目视确认 `planCard + Ctx + footer planState` 与自动断言中的 `planCardCount===1` 一致。
- [x] **[P2]** 2026-06-28 清理 `tomcat-vscode-ext` / `gui` 的 `npm audit` 告警：根包通过 `overrides` 升级 `mocha` 传递依赖 `diff` / `serialize-javascript`，GUI 显式锁定 `esbuild@0.28.1` 并刷新 `vite`，两侧 `npm audit` 均为 0 vulnerabilities。
- [x] **[P2]** 2026-06-28 修复 serve 集成测试套跑偶发超时：新增 `warmTomcatBinaryForSuite()` 在 `beforeAll` 预热 `cargo build --bin tomcat`，避免首个真实 serve 用例把编译冷启动挤进 30s 测试预算；`npm run test:unit` → 主包 94 + GUI 82 全绿。

### 🔄 IN PROGRESS
- [ ] **[P1]** 推送 `feature/tomcat-vscode-extension` 远端后，将 `T2-P1-020` 前移到 `PENDING_INTEGRATION` 并走集成合并流程。

### 🔌 INTERFACE (当前口径)
- 当前唯一真相以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension-phase2.md` 为主，Phase 1 基线继续以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension.md` 为事实起点。
- 当前分支已具备 Phase 1 基线：`@tomcat` 原生 Chat Participant、`ask_question` 审批回环、`vscode.diff` 预览 / `WorkspaceEdit` 应用、多会话 `sessionId` 路由、serve failed/restart/backpressure UI 降级。
- 本轮已在同一分支增量落地：Stage A 的 `/plan` / `/model` + serve 协议扩展，以及 Stage B 的 React + Vite webview、timeline 状态模型、`get_messages` 历史补齐、共享项目 scope 会话池、单活跃归属、plan 文件打开、附件透传、context budget 展示与真实 UI 验收入口。
- 本轮事件接缝增量口径：`get_state` 明确返回 `planPath` / `contextUtilizationRatio`；serve/live/history 统一补齐 `plan.enter` / `plan.exit` / `plan.pending` / `plan.complete`；webview DOM 测试快照新增 `ctxLabel` / `planNoticeReplayed` / `planStateText` / `planCardCount`，用于切会话恢复、reload 历史重放与 cross-owner 观察一致性验收。

### 🧪 ACCEPTANCE
- Rust：
  - `cargo build --release` → **exit 0**
  - `cargo clippy --all-targets -- -D warnings` → **exit 0**
  - `cargo test --lib -- --nocapture` → **1943 passed / 0 failed / 1 ignored**
  - `./scripts/run-integration-tests.sh integration`（以 crate `.env` 运行，`NO_PROXY=127.0.0.1,localhost`，`OPENAI_API_KEY` 设本地 mock 占位以驱动 `test_user_chat_skill_list_reload_use` 的本地 mock OpenAI server）→ **exit 0，parallel + serial 全绿**；`serve_schema_fixture` 漂移已通过重生成 `tests/fixtures/serve/serve.schema.json` / `serve.d.ts` 修复；`real_mimo_web_search` 与 `test_user_background_bash_multiple_timeout_slices_real_llm_cli` 为 flaky 真 网络 / 真 LLM 用例（本次通过，非本次改动引入）。
  - §4 verify-vsix 视觉验收：`TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR=<dir> npm run verify:vsix` → **4 passing / exit 0**，默认覆盖 `switch restore` / `reload replay` / `cross-owner` / `transcript UI`；脚本启动前清空 `<dir>/tomcat-vsix-visual-*.png` 旧产物；E2E 含 `toolRowCount>=3` + `toolCardCount===0`、`todoWidgetVisible`（progress 态）/ `!todoWidgetVisible`（collapsed 态）、`todoWidgetExpanded`、`todoWidgetItemCount>=4`、`planCardCount>=1`、`planCardTodoCountText==="4 todos"`、`composerFooterPlanStatus==="Plan: planning"`、`ctxLabel`、`planNoticeReplayed`、`planStateText` 与 `planCardCount===1` 等断言，并产出 `<dir>/tomcat-vsix-visual-{switch-restore,reload-replay,cross-owner,collapsed,progress,todo-expanded,expanded,file-chip,tool-icons,tool-icons-bottom}-cropped.png`（`screencapture` 需 VSCode 窗口可见，若被其它全屏 app 遮挡会捕获到遮挡窗口，此时以 E2E DOM 断言为功能验收铁证）。
  - `./scripts/run-integration-tests.sh integration-openai-responses-wire`
  - `./scripts/run-integration-tests.sh integration-real-llm`
- Extension：
  - `npm audit`（`tomcat-vscode-ext` + `gui`）→ **0 vulnerabilities**
  - `npm run test:unit` → **exit 0**（主包 `31` files / `94` tests + GUI `14` files / `82` tests 全绿）
  - `npm run test:integration` → **exit 0**
  - `npm --prefix gui run test`
  - `npm run build`
  - 本轮针对性回归：`tsc -p tsconfig.json --noEmit`、`tsc -p e2e-harness/tsconfig.json`、`vitest run tests/webview_provider_flow.test.ts src/ui/webview/tests/state.test.ts src/ui/participant/tests/planState.test.ts` → **27 passing**
- 真实桌面 UI：
  - 隔离 VS Code profile + 安装打包 VSIX 后，以真实侧栏 webview 完成：打开 Tomcat、发送 prompt、切模型、`Chat <-> Plan`、`Build`、打开 plan 文件、添加附件并发送、观察 `Ctx%` 变化。
  - 本轮安装态补验：`TOMCAT_E2E_GREP="restores plan cards and Ctx after switching sessions|replays plan history after a webview reload|keeps cross-owner plan state in sync in the webview" node --import ./node_modules/tsx/dist/loader.mjs ./scripts/run-vscode-install-e2e.ts`（sandbox 外）→ **3 passing / 0 failing**
  - 本轮完整安装态回归：`npm run test:e2e:vscode-install`（sandbox 外）→ **18 passing / 0 failing / exit 0**，含 `restores plan cards and Ctx after switching sessions`、`replays plan history after a webview reload`、`keeps cross-owner plan state in sync in the webview` 三条新增场景。
- Host / UI coverage（当前口径）：
  - 已完成 host/devhost/install harness 覆盖 participant happy-path、approval、diff/apply、interrupt/restart、多会话路由，以及 Phase 2 的 `/plan`、`/model`、webview streaming、webview diff/apply、webview multi-session、webview ownership、切会话恢复 plan 卡与 `Ctx%`、reload 历史重放、cross-owner plan enter/build/exit 观察一致。
  - 上述 harness 现统一降级表述为“host integration / internal UI harness”，不再作为“真实桌面 UI 验收”口径。
- Rust 补测：`cargo test serve_get_state_includes_active_plan_path_and_context_ratio`、`serve_event_pump_streams_plan_transition_events`、`enter_and_exit_write_transition_events`、`completed_and_pending_modes_emit_transition_events`、`finalize_completed_to_chat_does_not_emit_event`、`update_plan_reopen_completed_to_pending_and_emits_plan_pending` → **全部通过**
- 例外说明：OpenAI Files live 组在 `integration-openai-responses-wire` 中按设计保持 opt-in，因未设置 `PI_LIVE_OPENAI_FILES=1` 而自跳过；`T2-P1-020` 的开发与本地验收已完成，但因尚未按用户要求执行提交/推送流程，状态暂留 `DOING`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
