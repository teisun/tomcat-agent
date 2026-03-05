# LLM 统一接入模块说明 (LLM Module)

## 1. 概述 (Overview)

- **职责**：为宿主 API 与 chat 提供统一的 LLM 能力：LlmProvider Trait、OpenAI 格式适配器、流式/非流式调用、限流与指数退避重试、Token 统计、会话级模型配置（model_override）。
- **所在层级**：宿主核心能力层（`src/core/llm`），依赖基础设施层（AppError、LlmConfig、load_config 等）。
- **核心文件**：
  - `src/core/mod.rs` — core 层聚合，re-export llm 对外类型
  - `src/core/llm/mod.rs` — LLM 模块聚合与 re-export
  - `src/core/llm/types.rs` — ChatMessage、ChatRequest、ChatResponse、StreamEvent、TokenUsage
  - `src/core/llm/provider.rs` — LlmProvider Trait 定义
  - `src/core/llm/openai.rs` — OpenAiProvider 实现（非流式/流式、限流、重试）
  - `src/core/llm/token_usage.rs` — SessionTokenUsage 会话级汇总结构

## 2. 使用方式

- **构造 OpenAiProvider**：`OpenAiProvider::new(&config)`，其中 `config` 为 `LlmConfig`（含 api_base、api_key_env、default_model、max_concurrent_requests、retry_count、stream_timeout_sec）。api_key 从 `api_key_env` 指定环境变量读取，未设置则返回错误。
- **非流式调用**：`provider.chat(request).await`，请求中 `model_override` 优先于 `request.model` 选模型；支持限流（Semaphore）与可重试错误的指数退避重试。
- **流式调用**：`provider.chat_stream(request).await` 返回 `Box<dyn Stream<Item = Result<StreamEvent, AppError>>>`，消费端可通过 drop 提前结束以释放连接。
- **Token 统计**：`ChatResponse.usage` / `StreamEvent::Usage` 提供单次 usage；会话级汇总由调用方使用 `SessionTokenUsage` 累加，并写入 SessionEntry（当 003 可用时）。

## 3. 会话级模型配置

- `ChatRequest.model_override: Option<String>` 与 SessionEntry.model_override 约定一致；为 None 时使用请求的 model 字段（通常由上层从 LlmConfig.default_model 或 SessionEntry 填入）。

## 4. 扩展

- 新增其他厂商适配器：实现 `LlmProvider` Trait，在 core/llm 内注册或通过配置选择即可。
