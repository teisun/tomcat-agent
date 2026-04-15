# Memory Embedding 与同步

## 零、先用大白话

Embedding 像 **把一段话压成一串数字指纹**。  
以后找「意思相近」就靠指纹比远近。  
**同步**像 **档案柜一动就更新卡片**：你改了 Markdown，索引跟着长。

**这一节你会学到**：批处理为啥存在；watch 从哪触发。

---

**设计思想**：Memory 使用 OpenAI、Gemini 等 provider 生成 embedding，通过批处理（batch-openai、batch-gemini）提高效率。文件变化通过 chokidar watch 或定时同步检测，增量更新索引。

---

## ASCII 核心四图

### 1) 结构图

```text
文本 chunk
        |
        v
createEmbeddingProvider（多厂商）
        |
        v
batch 队列 -> HTTP embed API
        |
        v
sqlite-vec 列更新
```

### 2) 调用流图

```text
新增/修改文件
  -> debounce
      -> 计算差分 chunk
          -> batch embed
              -> upsert vectors
```

### 3) 时序图

```text
Watcher      sync job          embed provider      sqlite
  |              |                    |               |
  | change       |                    |               |
  |------------->| batch request      |               |
  |              |------------------->|               |
  |              | vectors            |               |
  |              |----------------------------------->|
```

### 4) 数据闭环图

```text
embedding 模型切换
        |
        v
重算或渐进回填
        |
        v
检索质量监控（延迟/错误率）
        |
        v
回退 provider 或调 batch 大小
```

---

## 一、Embedding

- **embeddings.ts**：**`extensions/memory-core/src/memory/embeddings.ts`** — `createEmbeddingProvider` 等，抽象多提供商。
- **批处理实现**：**`runOpenAiEmbeddingBatches`** / **`runGeminiEmbeddingBatches`** 定义在 **`packages/memory-host-sdk/src/host/batch-openai.ts`**、**`batch-gemini.ts`**；由 **`extensions/memory-core/src/memory/provider-adapters.ts`** 等组装进索引管线。

---

## 二、同步

- **sync-memory-files**：同步 MEMORY.md、memory/*.md。  
- **sync-session-files**：同步 sessions 转录。  
- **onSessionTranscriptUpdate**：sessions 的 transcript-events 触发增量索引。

---

## 常见误会

- **误会**：换 embedding 模型不用重建索引。**正解**：旧指纹和新指纹 **不可比**；通常要 **重算或迁移计划**。  
- **误会**：watch 一定实时。**正解**：常有 **debounce**；大文件还会分批。  
- **误会**：embed API 失败就静默跳过。**正解**：看日志；有时会 **降级** 或 **重试**。
