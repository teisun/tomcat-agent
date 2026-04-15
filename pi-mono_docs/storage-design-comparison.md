# 存储设计对比：openclaw / pi-mono / pi_agent_rust

## 先用大白话

三样东西都要**把聊天记录记下来**，但记法不一样：有的像**带索引的笔记本**（多一层 JSON 做目录），有的像**一本写到黑的日记本**（一个 JSONL 文件从头追加），有的在日记本外再挂**小卡片索引**（SQLite）。下面表格是给要对齐存储行为的人看的；**pi-mono 这一列以 `SessionManager` 源码为准**。

---

## ASCII：谁有几层「壳」

```
  openclaw:   [ sessions.json 路由层 ] ---> [ 某 sessionId.jsonl 对话 ]
  pi-mono:    （无单独路由文件）        ---> [ 每会话一个 .jsonl ]
  pi_rust:    （可选 session-index）   ---> [ JSONL 或可选 SQLite 正文 ]
```

---

## 对比表（维护时请对照各仓库实现）

| 维度 | openclaw (Clawdbot) | pi-mono | pi_agent_rust |
|------|--------------------|---------|----------------|
| **会话持久化层数** | 两层：元数据 store + 对话 transcript | 单层：仅 JSONL 会话文件 | 单层主存 + 可选索引/可选 SQLite 后端 |
| **元数据 / 路由存储** | **sessions.json**（每 Agent 一个）：`sessionKey -> SessionEntry`；JSON5；可编辑；存 sessionId、updatedAt、toggles、token 计数、model 覆盖、compactionCount、memoryFlushAt 等 | 无独立元数据文件；会话列表通过**扫描** `sessions/--编码cwd--/` 下 `.jsonl` 得到 | 无独立「路由」文件；可选 **session-index.sqlite** 作派生索引（path、id、cwd、message_count、last_modified、size、name），便于 resume/list |
| **对话内容存储** | **\<sessionId\>.jsonl**：append-only，树形（id/parentId）；首行 session header；Telegram topic 为 `<sessionId>-topic-<threadId>.jsonl` | **单文件 JSONL**：默认目录 **`~/.pi/agent/sessions/--<cwd编码>--/`**；文件名 **`{ISO时间戳中的 : 和 . 换成 -}_{sessionId}.jsonl`**（`sessionId` 为 UUID v7）；首行 session header，其余为 SessionEntry；树形 id/parentId | **主存**：JSONL（与 pi-mono v3 兼容），路径约定与 pi-mono 对齐。**可选**：`sqlite-sessions` 时用 SQLite 存正文 |
| **会话目录 / 路径** | `~/.clawdbot/agents/<agentId>/sessions/`；store：`sessions.json`；transcript：`<sessionId>.jsonl` | `getDefaultSessionDir`：`join(agentDir, "sessions", "--" + cwd去掉首字符后把 / \ : 换成 - + "--")`；默认 **agentDir = ~/.pi/agent**（可用环境变量覆盖，见 coding-agent `config.ts`） | 与 pi-mono 对齐；文件名模式以该仓库文档为准 |
| **会话标识与路由** | **sessionKey** → **sessionId**；可策略性换新 sessionId | 按 **cwd** 分子目录；**每个 jsonl 文件即一会话**；CLI **continue / resume** 选文件 | 类似 cwd 分目录；可用 **session-index** 加速 |
| **Transcript 格式** | 与 pi-mono 对齐的 JSONL 语义 | **CURRENT_SESSION_VERSION = 3**：header + message / model_change / thinking_level_change / compaction / branch_summary / session_info / label / custom / custom_message 等 | 与 v3 兼容；SQLite 模式则表结构存条目 |
| **索引 / 加速** | sessions.json | 无单独索引文件 | session-index.sqlite 等 |
| **Source of truth** | Gateway 与 store 协同 | **JSONL 文件即真相** | JSONL 为主时 JSONL 为准 |
| **大会话 / 扩展存储** | 未在此表展开 | 单文件线性增长 | 可能有分段等规划（以 pi_agent_rust 文档为准） |
| **记忆 / 长期记忆** | 工作区 Markdown 等 | 核心会话无内置「记忆文件」；**web-ui** 另有 IndexedDB | 以该仓库为准 |
| **并发与锁** | Gateway 单进程为主 | 源码未强调文件锁（单机 CLI 为主场景） | SQLite 时有锁策略 |
| **配置 / 特性开关** | 有路由层 | 无后端切换 | `session_store=jsonl|sqlite` 等 |

---

## 小结

- **openclaw**：多 Agent、多 channel，用 **sessions.json** 做路由，transcript 仍兼容 **pi 系 JSONL**。
- **pi-mono**：**纯 JSONL**；目录名编码 cwd；**无** sessions.json、**无** 会话索引文件。
- **pi_agent_rust**：JSONL 兼容 + 可选索引/SQLite；细节以该仓库为准。

若 **pi-rust-wasm** 等实现要对齐 pi 生态，会话文件格式与目录约定建议以 **pi-mono `SessionManager`** 为权威；需要索引或可选后端时再参考 pi_agent_rust，而不是照搬 openclaw 的 sessionKey 层（除非你做网关型产品）。
