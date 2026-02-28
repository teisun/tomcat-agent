# Memory 记忆模块

**设计思想**：Memory 基于 MEMORY.md 与会话转录（sessions/*.jsonl）构建向量与关键词索引，支持混合检索（vector + BM25）。Agent 通过 memory-tool 与 memory-search 使用记忆，auto-reply 回合后可触发 memory-flush 做 compaction，session-memory 钩子在 /new 时保存会话到 memory。

---

## 一、职责概览

- **索引**：MEMORY.md、workspace/memory/*.md、sessions 转录文件，经 chunk、embedding 后写入 sqlite-vec。
- **检索**：searchVector（向量）、searchKeyword（BM25）、mergeHybridResults 合并。
- **Embedding**：OpenAI、Gemini 批处理（batch-openai、batch-gemini）。
- **同步**：sync-memory-files、sync-session-files，watch 文件变化做增量索引。

---

## 二、入口与 API

- **getMemorySearchManager**：`openclaw/src/memory/search-manager.js`（或 index），返回 MemorySearchManagerResult。
- **manager.ts**：MemoryIndexManager，负责索引、搜索、同步。
- **memory-search.ts**：Agent 侧 `resolveMemorySearchConfig`、调用 manager 搜索。
- **memory-tool.ts**：Pi 工具，封装 memory 检索。

---

## 三、与上下游衔接

| 消费者 | 使用方式 |
|--------|----------|
| Agent | memory-tool、memory-search |
| auto-reply | memory-flush（回合后 compaction） |
| hooks | session-memory（/new 时保存） |
| CLI | memory status、index、search |

---

## 四、子文档索引

- [索引与检索](06-Memory/索引与检索.md)：sqlite-vec、BM25、hybrid
- [Embedding与同步](06-Memory/Embedding与同步.md)：batch、sync、watch
