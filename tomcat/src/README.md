# 模块技术文档（`src/`）

本目录下的 **`README.md` 与 `src/` 代码子目录一一对应**，便于开发与 Code Review 时快速对照实现。宏观设计与契约仍以 [docs/architecture](../docs/architecture) 为准。

---

## 文件名编号规则（为何有多个 `02-`？）

前缀表示**架构层级**，不是严格递增的「第几章」：

| 前缀 | 含义 | 文档 |
|------|------|------|
| `01-` | 最底层：基础设施 | [infra/README.md](./infra/README.md) |
| `02-*` | **同层并列**的宿主核心 / 扩展能力（无先后顺序） | [core/llm/README.md](./core/llm/README.md)、[core/session/README.md](./core/session/README.md)、[ext/README.md](./ext/README.md) |
| `03-` | 更上层编排（依赖下层能力） | [core/README.md](./core/README.md) |
| — | 交互入口（CLI / chat） | [api/README.md](./api/README.md) |

---

## 图 1：全局分层与模块落位

```text
                         +------------------+
                         |   api/ (CLI)     |
                         |  chat / 子命令    |
                         +--------+---------+
                                  |
                         +--------v---------+
                         | core/agent_loop   |  ◄── core/README（Agent 循环）
                         +--------+---------+
                                  |
         +------------------------+------------------------+
         |                        |                        |
+--------v---------+    +---------v---------+    +---------v------------------+
| core/llm         |    | core/session      |    | ext/ (WasmEngine,          |
| LlmProvider      |    | SessionManager    |    |  Dispatcher, PluginMgr)     |
| ◄── core/llm     |    | ◄── core/session  |    | ◄── ext/README              |
+--------+---------+    +---------+---------+    +---------+-------------------+
         \___________________|_______________________/
                               |
                    +----------v-----------+
                    |      src/infra       |
                    | AppError, AppConfig |
                    | EventBus, logging   |
                    | ◄── infra/README    |
                    +---------------------+
```

- **依赖方向**：自上而下依赖 `infra`；`ext` 与 `core` 内各模块通过 Trait / 注入协作，避免环依赖。
- **延伸阅读**：[Architecture.md](../openspec/specs/Architecture.md) 分层说明与资源模式。

---

## 模块索引

| 文档 | 职责摘要 |
|------|----------|
| [api/README.md](./api/README.md) | CLI 入口、子命令、`chat`/`render` 与 core 的衔接 |
| [infra/README.md](./infra/README.md) | 错误、配置、日志、平台 IO、事件总线、审计 |
| [core/llm/README.md](./core/llm/README.md) | LLM 统一接入、流式、限流与重试 |
| [core/session/README.md](./core/session/README.md) | 会话存储、JSONL transcript |
| [ext/README.md](./ext/README.md) | WasmEdge、Hostcall、插件生命周期 |
| [core/README.md](./core/README.md) | Agent 循环、Compaction、core 层其它子模块索引 |

运行时工作区目录树（非 `src` 模块）见 [docs/architecture/directory-structure.md](../docs/architecture/directory-structure.md)。

---

## 图 2：核心数据面（LLM / 会话 / CLI / Transcript）

多轮对话主路径上，**会话与 LLM** 通过 `SessionManager` 与 `LlmProvider` 衔接；CLI 仅负责入口与参数。

```text
  用户 / TTY
      |
      v
 +----+----+     +------------------+     +------------------+
 | main.rs |---->| api/cli + chat  |---->| SessionManager   |
 +---------+     +--------+---------+     | sessions.json    |
                      |                  | *.jsonl transcript
                      |                  +--------+---------+
                      |                           |
                      |   init_context_state      |
                      v                           v
               +------+------+            (当天+补齐 -> Vec<ChatMessage>)
               | AgentLoop   |<-------------------+
               +------+------+
                      |
                      v
               +------+------+
               | LlmProvider |  ----HTTP---->  模型 API
               +-------------+
```

- **边界**：持久化格式与路径约定见 [工作目录与数据布局](../docs/architecture/work-dir-and-data-layout.md)；各模块细节见上表。
- **core/llm**、**core/session** 内另有针对本模块的精简 ASCII，可与本图对照阅读。
