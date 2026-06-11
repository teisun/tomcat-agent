| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-06-11 22:49 +0800 | DONE | feature/optimize01 | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P1]** T2-P1-015 follow-up：将 `sessionId` 收敛到 `ScopedEventEmitter` 事件信封，统一把同一 `session_id` 写入 wire `payload.sessionId` 与 `EventContext.session_id`；同步收口 `infra/event_bus`、`infra/events`、`agent_loop`、`api/chat` 与 CLI 渲染 / ask_question / preflight 路径，并补齐 `session_envelope_test`、`event_bus` / `events` / `agent_registry` / `cli_turn_renderer` / `preflight` 等回归测试 @2026-06-11
- [✓] **[P1]** 集成验收补漏：会话级 stderr listener 改为按 `session_id` 严格 demux；`EventContext::with_session_id()` 与 emitter 统一 trim / blank 规范；同步修正 `plugin-system/events.md` 的顶层 envelope `sessionId` 口径，并补 `chat_git_preflight_tests` / `context_management_tests` 的 `ScopedEventEmitter` 签名适配 @2026-06-11
- [✓] **[P1]** 本分支全量验收通过：`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all` 全绿（含 `cargo build --release`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`、`integration-parallel`、`integration-serial` / `cli_tests`）；`.integration_test_output.log` 末尾 `EXIT_CODE=0` @2026-06-11

### 🔌 INTERFACE (接口变更)
- `ScopedEventEmitter`：事件唯一出口统一注入 `payload.sessionId` 与 `EventContext.session_id`，领域事件本身不再手工散落重复 `sessionId` 字段。
- `EventContext.session_id`：成为消费侧 demux 标签，CLI / agent registry / preflight / ask_question / stream handler 等绑定会话的消费者按该字段过滤。
- 子 Agent 生命周期事件：事件信封上的 `sessionId` 固定绑定 child session，父子关系继续由 `parentSessionId` / `childSessionId` 表达。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
