# 模块技术文档（`docs/technical/`）

本目录存放与 **`src/` 代码目录一一对应** 的模块说明，便于开发与 Code Review 时快速对照实现。宏观设计与契约仍以 [openspec/specs/architecture](../../openspec/specs/architecture) 为准。

---

## 文件名编号规则（为何有多个 `02-`？）

前缀表示**架构层级**，不是严格递增的「第几章」：

| 前缀 | 含义 | 文档 |
|------|------|------|
| `01-` | 最底层：基础设施 | [01-infrastructure.md](./01-infrastructure.md) |
| `02-*` | **同层并列**的宿主核心 / 扩展能力（无先后顺序） | [02-llm-module.md](./02-llm-module.md)、[02-session-and-cli.md](./02-session-and-cli.md)、[02-wasm-runtime-and-plugin.md](./02-wasm-runtime-and-plugin.md) |
| `03-` | 更上层编排（依赖下层能力） | [03-agent-loop.md](./03-agent-loop.md) |

---

## 图 1：全局分层与模块落位

```text
                         +------------------+
                         |   api/ (CLI)     |
                         |  chat / 子命令    |
                         +--------+---------+
                                  |
                         +--------v---------+
                         | core/agent_loop   |  ◄── 03-agent-loop
                         +--------+---------+
                                  |
         +------------------------+------------------------+
         |                        |                        |
+--------v---------+    +---------v---------+    +---------v------------------+
| core/llm         |    | core/session      |    | ext/ (WasmEngine,          |
| LlmProvider      |    | SessionManager    |    |  Dispatcher, PluginMgr)     |
| ◄── 02-llm       |    | ◄── 02-session    |    | ◄── 02-wasm                 |
+--------+---------+    +---------+---------+    +---------+-------------------+
         \___________________|_______________________/
                               |
                    +----------v-----------+
                    |      src/infra       |
                    | AppError, AppConfig |
                    | EventBus, logging   |
                    | ◄── 01-infra        |
                    +---------------------+
```

- **依赖方向**：自上而下依赖 `infra`；`ext` 与 `core` 内各模块通过 Trait / 注入协作，避免环依赖。
- **延伸阅读**：[Architecture.md](../../openspec/specs/Architecture.md) 分层说明与资源模式。

---

## 模块索引

| 文档 | 职责摘要 |
|------|----------|
| [01-infrastructure.md](./01-infrastructure.md) | 错误、配置、日志、平台 IO、事件总线 |
| [02-llm-module.md](./02-llm-module.md) | LLM 统一接入、流式、限流与重试 |
| [02-session-and-cli.md](./02-session-and-cli.md) | 会话存储、JSONL transcript、CLI |
| [02-wasm-runtime-and-plugin.md](./02-wasm-runtime-and-plugin.md) | WasmEdge、Hostcall、插件生命周期 |
| [03-agent-loop.md](./03-agent-loop.md) | 三层 Agent 循环与工具编排 |

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
                      |   build_context_messages  |
                      v                           v
               +------+------+            (最近 N 条 -> ChatMessage)
               | AgentLoop   |<-------------------+
               +------+------+
                      |
                      v
               +------+------+
               | LlmProvider |  ----HTTP---->  模型 API
               +-------------+
```

- **边界**：持久化格式与路径约定见 [工作目录与数据布局](../../openspec/specs/architecture/work-dir-and-data-layout.md)；各模块细节见对应 `02-*` 文档。
- **02-llm**、**02-session** 内另有针对本模块的精简 ASCII，可与本图对照阅读。
