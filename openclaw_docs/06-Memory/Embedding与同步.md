# Memory Embedding 与同步

**设计思想**：Memory 使用 OpenAI、Gemini 等 provider 生成 embedding，通过批处理（batch-openai、batch-gemini）提高效率。文件变化通过 chokidar watch 或定时同步检测，增量更新索引。

---

## 一、Embedding

- **embeddings.ts**：`createEmbeddingProvider`，抽象 OpenAI、Gemini 等。
- **batch-openai.ts**：`runOpenAiEmbeddingBatches`，批量调用 OpenAI embedding API。
- **batch-gemini.ts**：`runGeminiEmbeddingBatches`，批量调用 Gemini。

---

## 二、同步

- **sync-memory-files**：同步 MEMORY.md、memory/*.md。
- **sync-session-files**：同步 sessions 转录。
- **onSessionTranscriptUpdate**：sessions 的 transcript-events 触发增量索引。
