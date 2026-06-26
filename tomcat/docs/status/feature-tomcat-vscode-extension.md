| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-27 00:15 +0800 | DOING | feature/tomcat-vscode-extension | — |

### ✅ DONE
- [x] **[P1]** 认领 `T2-P1-020`，任务卡 / 看板索引已切到 `DOING / Tom`；依赖例外已按用户显式要求记录。
- [x] **[P1]** 建立 `feature/tomcat-vscode-extension` 分支并初始化本分支 status 文件。
- [x] **[P1]** 按 `tomcat-vscode-extension-phase2.md` 与 `02-stage-a-slash-and-serve.md` / `03-stage-b-webview.md` 完成 Phase 2 主体实现：Stage A 的 `/plan` / `/model` slash + serve 协议/状态/事件扩展已落地，Stage B 的 sidebar webview / React GUI / shared pool / ownership / diff bridge 已接通。
- [x] **[P1]** Rust `serve --print-schema` fixture 已随 Phase 2 协议扩展刷新，`tests/fixtures/serve/serve.schema.json` 与 `tests/fixtures/serve/serve.d.ts` 不再漂移。
- [x] **[P1]** 纠正此前把内部 `__testing` / host harness 误表述为“真实桌面 UI 验收”的问题；本轮重新以真实 VS Code 桌面 UI 作为最终口径。
- [x] **[P1]** 侧栏 webview 已收敛为聊天式布局：时间线 transcript（消息 / thinking / tool / approval / plan 卡片）、底部内嵌 composer（`+` / `Chat|Plan` / `Model` / `Ctx%` / 圆形发送）、会话选择栏、活动 plan strip 与附件 chips 全部落地。
- [x] **[P1]** 真实桌面 UI 验收已完成：打开 Tomcat 侧栏、真实发送消息、切模型 `fake-model -> gpt-5.4`、`Chat -> Plan -> Build -> Chat`、打开 `.plan.md` 文件、添加附件并发送、观察 `Ctx 42% -> 58%`，全程不依赖内部注入。
- [x] **[P1]** 更新交付文档：status、`T2-P1-020` 任务卡、看板索引与 Stage B webview 架构文档已同步到最新实现事实。
- [x] **[P1]** 按 `/commit-with-status` 完成本地合规提交（聊天式 webview 重构 + 文档同步）。
- [x] **[P2]** 修复 macOS login bash 下 `tomcat init` / `install.sh` 写入 PATH 后新终端仍找不到 `tomcat`：`auto_add_to_path` 改为 PATH 写 `.bashrc` 并确保 `.bash_profile` source `.bashrc`，`install.sh` 同步补齐。
- [x] **[P1]** 2026-06-26 误删事故后恢复可执行计划：`.cursor/plans/transcript-ui-restore.plan.md`（仿 VSCode Chat 重做 transcript，52 todo，含 utility-flash 默认模型配置）。
- [x] **[P1]** 新增 Agent 安全规则 `tomcat/.cursor/rules/no-rm-rf.mdc`（禁止 `rm -rf "$VAR"` 跨命令边界等事故形态，alwaysApply）。

### 🔄 IN PROGRESS
- [ ] **[P1]** 按 `transcript-ui-restore.plan.md` 执行 transcript UI 仿 VSCode Chat 重做（上午未提交工作已丢失，从干净基线重建）。
- [ ] **[P1]** 推送 `feature/tomcat-vscode-extension` 远端后，将 `T2-P1-020` 前移到 `PENDING_INTEGRATION` 并走集成合并流程。

### 🔌 INTERFACE (当前口径)
- 当前唯一真相以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension-phase2.md` 为主，Phase 1 基线继续以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension.md` 为事实起点。
- 当前分支已具备 Phase 1 基线：`@tomcat` 原生 Chat Participant、`ask_question` 审批回环、`vscode.diff` 预览 / `WorkspaceEdit` 应用、多会话 `sessionId` 路由、serve failed/restart/backpressure UI 降级。
- 本轮已在同一分支增量落地：Stage A 的 `/plan` / `/model` + serve 协议扩展，以及 Stage B 的 React + Vite webview、timeline 状态模型、`get_messages` 历史补齐、共享项目 scope 会话池、单活跃归属、plan 文件打开、附件透传、context budget 展示与真实 UI 验收入口。

### 🧪 ACCEPTANCE
- Rust：
  - `cargo build --release`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test --lib -- --nocapture`
  - `./scripts/run-integration-tests.sh integration`（以 crate `.env` 运行，并显式设置 `NO_PROXY=127.0.0.1,localhost`；期间发现 `serve_schema_fixture` 漂移，刷新 fixture 后补跑 `integration-serial` 全绿）
  - `./scripts/run-integration-tests.sh integration-openai-responses-wire`
  - `./scripts/run-integration-tests.sh integration-real-llm`
- Extension：
  - `npm run test:unit`
  - `npm run test:integration`
  - `npm --prefix gui run test`
  - `npm run build`
- 真实桌面 UI：
  - 隔离 VS Code profile + 安装打包 VSIX 后，以真实侧栏 webview 完成：打开 Tomcat、发送 prompt、切模型、`Chat <-> Plan`、`Build`、打开 plan 文件、添加附件并发送、观察 `Ctx%` 变化。
- Host / UI coverage（当前口径）：
  - 已完成 host/devhost/install harness 覆盖 participant happy-path、approval、diff/apply、interrupt/restart、多会话路由，以及 Phase 2 的 `/plan`、`/model`、webview streaming、webview diff/apply、webview multi-session、webview ownership。
  - 上述 harness 现统一降级表述为“host integration / internal UI harness”，不再作为“真实桌面 UI 验收”口径。
- 例外说明：OpenAI Files live 组在 `integration-openai-responses-wire` 中按设计保持 opt-in，因未设置 `PI_LIVE_OPENAI_FILES=1` 而自跳过；`T2-P1-020` 的开发与本地验收已完成，但因尚未按用户要求执行提交/推送流程，状态暂留 `DOING`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
