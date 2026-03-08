| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| llm_agent | 2026-03-05 23:54 | ACTIVE | feature/llm | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** T1-P0-004 LLM 统一接入模块：core/llm 目录与类型（ChatMessage/ChatRequest/ChatResponse/StreamEvent）、LlmProvider Trait、SessionTokenUsage @2025-03-05
- [✓] **[P0]** OpenAiProvider：非流式 chat、流式 chat_stream（SSE 解析）、model_override、LlmConfig 集成
- [✓] **[P0]** 限流（Semaphore 并发上限）、指数退避重试（仅非流式）、count_tokens 近似实现
- [✓] **[P0]** 单元测试：类型与序列化、provider new 失败、count_tokens、is_retriable、SSE 流解析；覆盖率满足要求
- [✓] **[P0]** LLM 代理与降级：LlmConfig 增加 `proxy`、`api_base_fallback`；OpenAiProvider 构建 Client 支持 proxy，chat/chat_stream 主 base 连接失败时自动用 fallback 重试；UNIT_TEST_SPEC 融合 Gemini 版；文档更新

### 🔌 INTERFACE (接口变更)
- **LlmProvider**：`provider_name`、`chat`、`chat_stream`（返回 `Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>`）、`count_tokens`
- **ChatRequest**：`model_override: Option<String>` 用于会话级模型覆盖，与 SessionEntry 约定一致
- **LlmConfig**（infra）：`max_concurrent_requests`、`retry_count`、`stream_timeout_sec`；新增可选 `proxy`（显式 HTTP 代理 URL）、`api_base_fallback`（主 API 不通时自动重试的备用 base）
- **lib**：re-export `core::*`（ChatMessage, ChatRequest, ChatResponse, LlmProvider, OpenAiProvider, SessionTokenUsage, StreamEvent）、infra 增加 `LlmConfig`

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
