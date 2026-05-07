| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-05-07 17:18 | ACTIVE | feature/stream-timeout | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 认领 `T2-P0-003`：任务卡状态改为 `PENDING_INTEGRATION`、负责人改为 Jerry，并同步看板索引状态与负责人字段。
- [✓] **[P0]** `openai.rs` 在 bytes 层接入 `tokio_stream::StreamExt::timeout`；`stream_timeout_sec==0` 语义为关闭逐事件超时。
- [✓] **[P0]** `openai_responses/mod.rs` 同口径接入 bytes 层空闲超时，统一超时错误文案 `流式空闲超时: stream_timeout_sec=<n>s`。
- [✓] **[P0]** 新增单测：`openai_stream_test` 与 `openai_responses_test` 覆盖「无字节超时」与「keepalive 不误超时」。
- [✓] **[P0]** `docs/TODOS.md` 关闭 `#T-132`，补充 2026-05-07 核对说明。
- [✓] **[P0]** 分支侧门禁：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`、`cargo test --test cli_tests -- --test-threads=1`、`RUST_LOG=pi_wasm=debug,info ./scripts/run-integration-tests.sh all`（第二次复跑全绿）。

### 🔌 INTERFACE (接口变更)
- `OpenAiProvider::chat_stream`：流式字节读取新增空闲超时控制（`stream_timeout_sec`）。
- `OpenAiResponsesProvider::chat_stream`：与 Completions 保持同口径的空闲超时控制。
- 统一超时错误语义：`AppError::Llm("流式空闲超时: stream_timeout_sec=<n>s")`，便于上层按 Retryable 分类。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
