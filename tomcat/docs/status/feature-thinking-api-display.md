| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-05-07 20:30 | PENDING_INTEGRATION | feature/thinking-api-display | - |

### ✅ DONE (已完成)
- [✓] **[P0]** 架构方案落稿：`docs/architecture/llm-stream-events-cli-pipeline.md`，与 `ARCHITECTURE_SPEC §4.1/§4.2` 对齐。
- [✓] **[P0]** 任务卡 `agents/TASK_BOARD_002/tasks/T2-P0-006.md` 改 DOING/Tom，链接架构方案与 P0–P7 实施顺序。
- [✓] **[P0]** Phase 1：`StreamEvent::Thinking`（types + serde）→ Completions reasoning 三路解析 + 请求体 reasoning_effort/thinking → stream_handler `kind=thinking_delta` 透传 → `CliTurnRenderer` 单订阅者 + folded/expanded → `/thinking on/off/toggle` 与 `PI_CHAT_SHOW_THINKING`。
- [✓] **[P0]** Phase 2：Responses reasoning 三事件显式映射 + 未知事件 trace；`thinking_policy` 引入 `ThinkingLevel`/`ThinkingFormat`/`resolve_request_fields`；`should_strip_on_resend` / `should_persist_thinking` / `strip_anthropic_thinking_blocks` helpers。
- [✓] **[P0]** 分支侧门禁：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`（826 passed）、`cargo test --test cli_tests -- --test-threads=1`（77 passed）、`RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh all`（全绿）。

### 🔌 INTERFACE (接口变更)
- 新增 `StreamEvent::Thinking { delta, source, signature }` 内部事件（`source=summary|raw`；signature None 时 skip）。
- `assistantMessageEvent` payload 扩展 `kind: "content_delta" | "thinking_delta"` + `source: "summary" | "raw"`；老消费者读 `delta` 不破坏正文路径。
- 新增本地命令 `/thinking on|off|toggle` 与 `PI_CHAT_SHOW_THINKING` 环境变量；`ChatContext.show_thinking: Arc<AtomicBool>` 由 `CliTurnRenderer` 共享。
- `LlmConfig.thinking: ThinkingConfig` 新增（enabled/level/format/max_tokens/show/persist/strip_on_resend/print_to_stderr）；当前默认 `enabled=true, show=false`，即折叠 raw 但仍显示 summary。
- `OpenAiRequestBody` 新增可选 `reasoning_effort`/`thinking` 字段（None 时不入 wire）；Responses build_request_body 启用时写 `reasoning: {effort: ...}` 对象。
- 新模块 `core::llm::thinking_policy`：`ThinkingLevel`/`ThinkingFormat`/`resolve_request_fields`/`should_strip_on_resend`/`should_persist_thinking`/`strip_anthropic_thinking_blocks`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
