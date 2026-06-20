| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-21 00:05 +0800 | PENDING_INTEGRATION | feature/tomcat-vscode-extension | — |

### ✅ DONE
- [x] **[P1]** 认领 `T2-P1-019`，任务卡已切到 `DOING / Tom`，看板索引已同步当前状态与负责人。
- [x] **[P1]** 建立 `feature/tomcat-vscode-extension` 分支并初始化本分支 status 文件。
- [x] **[P1]** 按 SSoT 落地 `P1/P2/P3`：桥接核心、原生 participant MVP、多会话与生命周期。
- [x] **[P1]** 补齐单元/集成/真实宿主 E2E 与文档收口。
- [x] **[P1]** 真实宿主门禁已覆盖 VSCode Dev Host、VSCode 安装版、Cursor 兼容运行，并补上多会话路由与 interrupt/restart 验收。
- [x] **[P1]** 分支侧门禁已跑完：`cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --nocapture`、`npm run build`、`npm run lint`、`npm run test:unit`、`npm run test:integration`、`npm run test:e2e:vscode-devhost`、`npm run test:e2e:vscode-install`、`npm run test:e2e:cursor`。

### 🔌 INTERFACE (当前口径)
- 唯一真相仍是 `tomcat-vscode-ext/docs/architecture/tomcat-vscode-extension.md`。
- 扩展侧已落地：`@tomcat` 原生 Chat Participant、`ask_question` 审批回环、`vscode.diff` 预览 / `WorkspaceEdit` 应用、多会话 `sessionId` 路由、serve failed/restart/backpressure UI 降级。
- 真实宿主验收入口已落地：`npm run test:e2e:vscode-devhost`、`npm run test:e2e:vscode-install`、`npm run test:e2e:cursor`。

### 🧪 ACCEPTANCE
- Rust：`cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --nocapture` 通过（`1900 passed; 0 failed; 1 ignored`）。
- Extension：`npm run build`、`npm run lint`、`npm run test:unit`、`npm run test:integration` 通过。
- Host E2E：VSCode Dev Host / VSCode 安装版 / Cursor 兼容运行三条链路均通过，且覆盖一问一答、审批、diff/apply、interrupt/restart、多会话路由。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
