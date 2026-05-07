| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-05-07 19:30 | ACTIVE | feature/thinking-api-display | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 架构方案落稿：`docs/architecture/llm-stream-events-cli-pipeline.md`（与 `ARCHITECTURE_SPEC §4.1/§4.2` 对齐）。
- [✓] **[P0]** 任务卡 `agents/TASK_BOARD_002/tasks/T2-P0-006.md` 改 DOING/Tom，链接架构方案与 P0–P7 实施顺序。
- [ ] **[P0]** Phase 1：`StreamEvent::Thinking` + Completions reasoning + stream_handler 透传 + `CliTurnRenderer` + `/thinking` 折叠命令。
- [ ] **[P0]** Phase 2：Responses reasoning 显式映射 + `ThinkingLevel`/`thinking_format` + `strip_on_resend` + `persist` + Anthropic/Qwen 占位。
- [ ] **[P0]** 分支侧门禁：fmt、clippy、单测、`./scripts/run-integration-tests.sh all`、E2E。

### 🔌 INTERFACE (接口变更)
- 新增 `StreamEvent::Thinking { delta, signature }` 内部事件（公开 API，serde 兼容）。
- `assistantMessageEvent` payload 新增 `kind` 字段：`content_delta` / `thinking_delta`，旧消费者继续读 `delta` 不破坏行为。
- 新增本地命令 `/thinking on|off|toggle` 与 `PI_CHAT_SHOW_THINKING` 环境变量（默认关）。
- `LlmConfig` 新增 `thinking` 配置块：`enabled` / `level` / `format` / `max_tokens` / `show` / `persist` / `strip_on_resend` / `print_to_stderr`（默认 `enabled=false` 不改变现有用户行为）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
