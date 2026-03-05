| Owner | Update Time | State | Branch |
| :--- | :--- | :--- | :--- |
| llm_agent | 2025-03-05 18:48 | ACTIVE | feature/llm |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** T1-P0-004 LLM 统一接入模块：core/llm 目录与类型（ChatMessage/ChatRequest/ChatResponse/StreamEvent）、LlmProvider Trait、SessionTokenUsage @2025-03-05
- [✓] **[P0]** OpenAiProvider：非流式 chat、流式 chat_stream（SSE 解析）、model_override、LlmConfig 集成
- [✓] **[P0]** 限流（Semaphore 并发上限）、指数退避重试（仅非流式）、count_tokens 近似实现
- [✓] **[P0]** 单元测试：类型与序列化、provider new 失败、count_tokens、is_retriable、SSE 流解析；覆盖率满足要求

### 🔌 INTERFACE (接口变更)
- **LlmProvider**：`provider_name`、`chat`、`chat_stream`（返回 `Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>`）、`count_tokens`
- **ChatRequest**：`model_override: Option<String>` 用于会话级模型覆盖，与 SessionEntry 约定一致
- **LlmConfig**（infra）：新增 `max_concurrent_requests`、`retry_count`、`stream_timeout_sec`
- **lib**：re-export `core::*`（ChatMessage, ChatRequest, ChatResponse, LlmProvider, OpenAiProvider, SessionTokenUsage, StreamEvent）、infra 增加 `LlmConfig`

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
