# Memory 记忆模块

## 零、先用大白话

Memory 像 **便利贴墙 + 档案柜**。  
**人写的话**多在 workspace 的 **`MEMORY.md`**、**`memory/*.md`**。  
**聊天记录**在 **`sessions/*.jsonl`**。  
要找旧话，系统先把这些纸 **扫进索引**（像给每段话做指纹）。  
搜的时候：**按意思**（向量）和 **按关键字**（BM25）一起用。

**这一节你会学到**：索引在哪；Agent 怎么搜；别和「模型权重记忆」搞混。

---

**设计思想**：Memory 基于 MEMORY.md 与会话转录（sessions/*.jsonl）构建向量与关键词索引，支持混合检索（vector + BM25）。Agent 通过 memory-tool 与 memory-search 使用记忆，auto-reply 回合后可触发 memory-flush 做 compaction，session-memory 钩子在 /new 时保存会话到 memory。

---

## ASCII 核心四图

### 1) 结构图

```text
MEMORY.md / workspace/memory/*.md
        +
sessions/*.jsonl
        |
        v
索引层（sqlite-vec + FTS）
        |
        v
memory_search / memory_get（工具面）
```

### 2) 调用流图

```text
文件变更 watch
  -> chunk -> embed -> upsert index
      -> Agent 调用 memory_search
          -> mergeHybridResults
              -> 片段注入 prompt
```

### 3) 时序图

```text
Agent        memory tool        Index store       Provider embed
  |                |                  |                |
  | search query   |                  |                |
  |--------------->| BM25+vector     |                |
  |                |--------------------------------->|
  |<---------------| hits             |                |
```

### 4) 数据闭环图

```text
对话增长 -> compaction / flush
        |
        v
提炼写入 memory/*.md
        |
        v
索引增量更新
        |
        v
后续检索读到新记忆
```

---

## 一、职责概览

- **索引**：MEMORY.md、workspace/memory/*.md、sessions 转录文件，经 chunk、embedding 后写入 sqlite-vec。
- **检索**：searchVector（向量）、searchKeyword（BM25）、mergeHybridResults 合并。
- **Embedding**：OpenAI、Gemini 批处理（batch-openai、batch-gemini）。
- **同步**：sync-memory-files、sync-session-files，watch 文件变化做增量索引。

---

## 二、入口与 API（随插件拆分，路径以仓库为准）

- **宿主侧门面**：**`getMemorySearchManager`**、**`MemoryIndexManager`** 等由 **`src/plugin-sdk/memory-core.ts`** 与 **`src/plugins/memory-runtime.ts`** 组合暴露；底层 sqlite-vec / 索引实现在 bundled 的 **`extensions/memory-core/`** 中（不要假设仍存在顶层 **`src/memory/`** 目录）。
- **Agent 侧编排**：**`src/agents/memory-search.ts`** — `resolveMemorySearchConfig`、与 Gateway/插件运行时交互。
- **工具名**：如 **`memory_get`**、**`memory_search`** 在 **`src/agents/tool-catalog.ts`** 等与 Pi 工具策略中登记；具体执行经嵌入式 Pi 订阅与插件运行时转发。

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

---

## 常见误会

- **误会**：Memory = 模型自带长期记忆。**正解**：这是 **你磁盘上的文件 + 索引**；换模型只要路径还在都还在。  
- **误会**：删了 `memory/*.sqlite` 就丢了所有记忆。**正解**：**源文件**在 Markdown / jsonl；sqlite 多是 **可重建缓存**（仍先备份再删）。  
- **误会**：`memory_get` 能读整台电脑。**正解**：路径有 **白名单/边界**（安全更新见 [00-主PRD.md](00-主PRD.md) 摘要）。
