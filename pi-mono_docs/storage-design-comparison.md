# 存储设计对比：openclaw / pi-mono / pi_agent_rust

| 维度 | openclaw (Clawdbot) | pi-mono | pi_agent_rust |
|------|--------------------|---------|----------------|
| **会话持久化层数** | 两层：元数据 store + 对话 transcript | 单层：仅 JSONL 会话文件 | 单层主存 + 可选索引/可选 SQLite 后端 |
| **元数据 / 路由存储** | **sessions.json**（每 Agent 一个）：`sessionKey -> SessionEntry`；JSON5；可编辑；存 sessionId、updatedAt、toggles、token 计数、model 覆盖、compactionCount、memoryFlushAt 等 | 无独立元数据文件；会话列表通过扫描会话目录得到 | 无独立「路由」文件；可选 **session-index.sqlite** 作派生索引（path、id、cwd、message_count、last_modified、size、name），便于 resume/list |
| **对话内容存储** | **\<sessionId\>.jsonl**：append-only，树形（id/parentId）；复用 pi-coding-agent SessionManager 格式（首行 session header，其余为 entry）；Telegram topic 为 `<sessionId>-topic-<threadId>.jsonl` | **单文件 JSONL**：`~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`；首行 session header，其余为 SessionEntry；树形 id/parentId | **主存**：JSONL（与 pi-mono v3 兼容），路径 `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`。**可选**：开启 `sqlite-sessions` 时可用 SQLite 存会话内容（替代 JSONL） |
| **会话目录 / 路径** | `~/.clawdbot/agents/<agentId>/sessions/`；store：`sessions.json`；transcript：`<sessionId>.jsonl` | `~/.pi/agent/sessions/--<cwd_encoded>--/`；文件名 `<timestamp>_<uuid>.jsonl` | 与 pi-mono 相同：`~/.pi/agent/sessions/--<path>--/`；文件名 `YYYY-MM-DDTHH-MM-SS.sssZ_id.jsonl` |
| **会话标识与路由** | **sessionKey**（如 agent:id:main、group、channel、cron、webhook）→ 当前 **sessionId**；reset/每日/空闲过期会建新 sessionId | 按 **cwd** 分目录；每个文件即一会话；无 sessionKey 概念，仅 CLI 的 continue/resume | 与 pi-mono 类似，按 cwd 分目录；resume 时用 **session-index** 查 `cwd + last_modified` 等，不做 sessionKey 路由 |
| **Transcript 格式** | 与 pi-mono 一致：type: session 头 + message/custom_message/custom/compaction/branch_summary 等 entry | JSONL v3：session header（version, id, cwd, timestamp）+ message/model_change/thinking_level_change/compaction/branch_summary/session_info/label/custom | 与 pi-mono v3 一致；可选 SQLite 时表结构存 header + entries（json 列） |
| **索引 / 加速** | 无单独索引；列表与当前会话由 sessions.json 提供 | 无；通过遍历目录找会话 | **session-index.sqlite**：常驻索引，在 session 保存时增量更新；锁文件 `session-index.lock`；支持全量 reindex |
| **Source of truth** | Gateway 为权威；sessions.json 可手改但会被 Gateway 覆盖/补全；transcript 由 Pi SessionManager 读写 | JSONL 文件即唯一真相源 | **JSONL 为 primary**；SQLite 为派生索引或可选后端；冲突时以 JSONL 为准（SYNC_STRATEGY.md） |
| **大会话 / 扩展存储** | 未描述分段或 sidecar | 未做；单文件线性增长 | **Session Store V2（规划/Phase 2）**：分段 append log + sidecar 偏移索引 + checkpoint，便于大会话 resume 与恢复 |
| **记忆 / 长期记忆** | **独立于会话**：工作区 Markdown（memory/YYYY-MM-DD.md、MEMORY.md）；pre-compaction 时触发 memory flush 写入 | 核心仅会话；无内置“记忆”存储（web-ui 另有 IndexedDB 缓存） | 核心仅会话；无内置“记忆”存储 |
| **并发与锁** | 未强调；Gateway 单进程为主 | 未强调 | SQLite 写用 `session-index.lock` + busy timeout 5s；多实例写索引串行化 |
| **配置 / 特性开关** | 无会话后端选择 | 无 | `session_store=jsonl|sqlite`（sqlite 需 feature `sqlite-sessions`）；autosave/checkpoint 等可调 |

---

## 小结

- **openclaw**：面向多 Agent、多 channel（Telegram/Discord/Slack 等），用 **sessions.json 做路由与元数据**，transcript 仍为 **pi 系 JSONL**，记忆用 **工作区 Markdown**，与 pi-mono 会话格式兼容。
- **pi-mono**：**纯 JSONL**，按 cwd 分目录，无元数据文件、无索引、无 SQLite；简单、可读、Git 友好。
- **pi_agent_rust**：**JSONL 为主**，与 pi-mono v3 兼容；增加 **session-index.sqlite** 做列表/resume 加速；可选 **SQLite 会话后端**；规划 **V2 sidecar** 应对大会话；冲突时以 JSONL 为权威。

pi-rust-wasm 若要对齐 pi 生态，会话存储应以 **pi-mono 的 JSONL 格式与目录约定** 为准；索引或可选后端可参考 pi_agent_rust 的 session-index 与可选 SQLite，而不引入 openclaw 的 sessionKey/sessions.json 路由层（除非要做多 channel 网关）。
