| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-06-19 18:49 +0800 | PENDING_INTEGRATION | feature/agent-server-ui-gateway | — |

### ✅ DONE (本轮整改已落地)
- [x] **[R13]** `AgentEvent` 补 `JsonSchema`，新增 `WireEvent` schema 根节点，并把 `OutFrame::Event` 从“任意 JSON”收紧为事件 wire schema 视角。
- [x] **[M1]** `tomcat serve --print-schema` 不再输出 `unknown` 空壳；`.d.ts` 现由 JSON Schema 经 in-crate emitter 生成，fixture 已刷新。
- [x] **[R14]** `prompt` / `follow_up` 附件入口接通：`params.attachments` 解析、校验、组装为 `ChatMessage::user_with_parts(...)`，busy follow_up 队列也保留多模态内容。
- [x] **[M3/L2]** `interrupt` 命中未知会话返回标准错误；stdio E2E 已锁住 `agent_interrupted` 下行与 stdout-only-NDJSON 契约。
- [x] **[L1]** 惰性/延期字段收口：`get_messages.upToSeq` 保留 `null` 占位并写清是 Phase 2、`OutFrame::is_lossless()` 保留 TODO 但不接 writer、`serve.session_idle_unload_ms` 在代码/文档中都明确为预留未接线。
- [x] **[Docs]** `04-runtime-reference.md`、`02-implementation-details.md`、状态文档已同步真实用例名与当前实现状态。

### 2026-06-19 | remediation acceptance

- **验证**：
  - `cargo test serve_ --lib`
  - `cargo test --test serve_schema_fixture`
  - `cargo test --test serve_stdio_e2e`
  - `cargo test --test serve_multi_session`
  - `cargo test --test serve_ask_question_tests`
  - `cargo test --test serve_robustness_tests`
- **关键新增锁点**：
  - schema / TS：`serve_print_schema_matches_fixture`、`serve_dts_preserves_wire_event_session_id`、`serve_emitted_event_validates_against_generated_schema`
  - 附件：`serve_prompt_with_image_attachment_builds_multimodal_message`、`serve_follow_up_with_attachment_queues_multimodal_message_when_busy`、`serve_prompt_invalid_attachment_returns_error`、`serve_prompt_without_attachments_falls_back_to_user_text`、`serve_steer_ignores_attachments`、`serve_prompt_with_attachment_roundtrip`
  - 中断 / 传输：`serve_interrupt_emits_agent_interrupted_e2e`、`serve_interrupt_emits_agent_interrupted_and_tool_execution_end`、`serve_stdout_only_emits_ndjson_frames`、`serve_unknown_command_returns_error_response`
- **说明**：Phase 2 的 visible/hidden、`upToSeq`/`seq` 重建、`session_idle_unload_ms` 自动回收仍保留设计，不在本轮实现范围。

### 🔌 INTERFACE (当前口径)
- `tomcat serve --stdio`：Phase 1 stdio server，支持多会话、控制帧、ask_question 回环。
- `tomcat serve --print-schema`：导出命令/控制/响应/事件的 JSON Schema 与 TypeScript `.d.ts`。
- `params.attachments`：本期接通 `prompt` / `follow_up`；`steer` 仍按文本处理，不消费附件。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| visible/hidden、`seq/upToSeq` 快照续接 | 设计保留到 Phase 2，本期故意不做 | 后续 UI 真的引入后台会话降级时再实现 |
