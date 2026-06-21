| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-21 21:01 +0800 | DOING | feature/tomcat-vscode-extension | — |

### ✅ DONE
- [x] **[P1]** 认领 `T2-P1-020`，任务卡 / 看板索引已切到 `DOING / Tom`；依赖例外已按用户显式要求记录。
- [x] **[P1]** 建立 `feature/tomcat-vscode-extension` 分支并初始化本分支 status 文件。
- [x] **[P1]** 按 `tomcat-vscode-extension-phase2.md` 与 `02-stage-a-slash-and-serve.md` / `03-stage-b-webview.md` 完成 Phase 2 主体实现：Stage A 的 `/plan` / `/model` slash + serve 协议/状态/事件扩展已落地，Stage B 的 sidebar webview / React GUI / shared pool / ownership / diff bridge 已接通。
- [x] **[P1]** Rust `serve --print-schema` fixture 已随 Phase 2 协议扩展刷新，`tests/fixtures/serve/serve.schema.json` 与 `tests/fixtures/serve/serve.d.ts` 不再漂移。

### 🔄 IN PROGRESS
- [ ] **[P1]** 把 `T2-P1-020` 从 `PENDING_INTEGRATION` 回退到 `DOING`，纠正此前把内部 `__testing` / host harness 误表述为“真实桌面 UI 验收”的问题。
- [ ] **[P1]** 按用户要求收敛 sidebar webview 体验，使其与 VS Code Chat 一致：具备可输入对话、真实会话消息流、模型切换、plan 模式切换与更接近 chat 的 composer / stream 布局。
- [ ] **[P1]** 用真实 VS Code 桌面 UI 重新验收打开 sidebar、可见 webview、实际输入/发送、模型切换与 plan 操作，不再以内部注入作为最终口径。

### 🔌 INTERFACE (当前口径)
- 当前唯一真相以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension-phase2.md` 为主，Phase 1 基线继续以 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension.md` 为事实起点。
- 当前分支已具备 Phase 1 基线：`@tomcat` 原生 Chat Participant、`ask_question` 审批回环、`vscode.diff` 预览 / `WorkspaceEdit` 应用、多会话 `sessionId` 路由、serve failed/restart/backpressure UI 降级。
- 本轮已在同一分支增量落地：Stage A 的 `/plan` / `/model` + serve 协议扩展，以及 Stage B 的 React + Vite webview、共享项目 scope 会话池、单活跃归属、diff/apply 回桥与真实 UI 验收入口。

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
- Host / UI coverage（当前口径）：
  - 已完成 host/devhost/install harness 覆盖 participant happy-path、approval、diff/apply、interrupt/restart、多会话路由，以及 Phase 2 的 `/plan`、`/model`、webview streaming、webview diff/apply、webview multi-session、webview ownership。
  - 上述 harness 现统一降级表述为“host integration / internal UI harness”，不再作为“真实桌面 UI 验收”口径。
- 例外说明：OpenAI Files live 组在 `integration-openai-responses-wire` 中按设计保持 opt-in，因未设置 `PI_LIVE_OPENAI_FILES=1` 而自跳过；同时 `T2-P1-020` 当前仍处 `DOING`，需等真实桌面 UI 验收完成后再重提 `PENDING_INTEGRATION`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 依赖例外 | `T2-P1-020` 默认依赖 `T2-P1-019 DONE` 后再认领；本次按用户显式要求先认领并在本分支继续推进 | 继续保留，待真实桌面 UI 验收完成后再按正常集成流程收口 |
| UI 验收口径纠偏 | 之前的 `__testing`/host harness 不能替代用户要求的桌面 UI 操作式验收 | 已回退状态为 `DOING`，当前正在补真实 UI 验收 |
