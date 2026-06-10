本文为 [Architecture](../Architecture.md) 中「会话存储数据结构设计」的详细设计，总览见主文档。会话与存储根目录、多 agent 布局见 [工作目录与数据布局](work-dir-and-data-layout.md)。

## 会话存储数据结构设计

### 元数据 store（sessions.json）

单文件 JSON，根结构为 **`SessionStore { sessions{id→entry}, current{key→id} }`**。`sessions` 存所有会话档案，`current` 只存“每个 scope 当前指向哪一个 sessionId”。列表与路由由此提供，不另建 SQLite 索引。

```rust
/// 会话根目录：~/.tomcat/agents/<agentId>/sessions/
/// sessionKey 格式：agent:<agentId>:<channelKey>
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStore {
    #[serde(default)]
    pub sessions: std::collections::HashMap<String, SessionEntry>,
    #[serde(default)]
    pub current: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    #[serde(default)]
    pub session_key: String,
    pub session_id: String,           // 当前 transcript 文件 id，对应 <sessionId>.jsonl
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>, // 可选显式 transcript 路径
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_tokens_freed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_chars_persisted: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_id: Option<String>,
}
```

**开发阶段兼容口径**：当前不做旧 `sessions.json` 迁移。`tomcat init` 会直接写新结构；若用户未先 `init` 就直接使用，而本地文件是旧结构、空文件或反序列化失败，运行时也会直接重建为新结构。

### 对话 transcript（pi-mono 相容 JSONL）

每会话一个 `.jsonl` 文件：**每行一个 JSON 对象**（非管道分隔）；首行 session header，后续每行一条 entry，树形 id/parentId。内存中为结构化类型（pi-mono 为 `SessionEntry` 联合类型），落盘时每行 `JSON.stringify(entry)`。与 pi-mono 格式兼容。

> 启动恢复的物理读取方案已经从“单靠 transcript 尾扫”升级为“transcript + sibling resume sidecar”。sidecar 的字段、更新时机、冷热路径与 trace 见 [`chat-resume-hydration.md`](./chat-resume-hydration.md)；本文件只保留存储布局与数据结构本身。

```rust
/// 首行：session header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub r#type: String, // "session"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>, // 3
    pub id: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// 后续每行：一条 SessionEntry。内存中为 enum 联合类型，落盘时每行序列化一个变体。
/// JSON 通过 type 字段区分（snake_case），与 pi-mono / pi_agent_rust 一致。
/// 参考：[session-pi-mono-format.jsonl](../guides/examples/session-pi-mono-format.jsonl)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    BranchSummary(BranchSummaryEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    Custom(CustomEntry),
}

/// 各 entry 变体均包含或 flatten 公共基座：id、parent_id、timestamp，组成树形结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryBase {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// `branch_summary` entry：JSONL `type: branch_summary`，Layer 1/2 上下文压缩摘要行（原 compaction 语义）。
/// 详见 [上下文管理技术方案](context-management.md) §5.4 / §5.7 / §6.3。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_start_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_end_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_boundary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preheat_compaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_covered_tokens_before: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_summary_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_saved: Option<usize>,
}
```

**会话路径与会话标识**
- **会话根目录** `~/.tomcat/agents/<agentId>/sessions/`；当前 agentId 固定为 `main`。
- **sessionKey**（作用域键）：`agent:<agentId>:<channelKey>`；当前主要有 `agent:main:main`（`claw`）与 `agent:main:proj:<hash>`（`code`）。
- **sessionId**：单次对话对应的 transcript 唯一 id（`<timestamp>_<uuid>`），对应文件名 `<sessionId>.jsonl`；`SessionEntry.session_id` 指向该文件。
- **resume sidecar**：同目录 sibling 文件 `<sessionId>.resume-index.json`，仅保存启动恢复定位所需的 metadata，不保存完整消息正文；缺失/损坏可重建，详见 [`chat-resume-hydration.md`](./chat-resume-hydration.md)。

**Source of truth**：transcript 内容以 JSONL 文件为准；sessions.json 为元数据与路由的权威，写入时覆盖该文件。

**主 chat 的消息级持久化矩阵（2026-05）**：
- `user` / 纯文本 `assistant` final / 每条 `tool_result`：形成合法 message 边界后立即 append 到 transcript，并在返回前 `flush + sync_data`。
- `assistant + tool_calls`：形成合法块后立即 append，但只做 `flush`；紧随其后的首条 `tool_result` 的 `sync_data` 会把该块一并落稳。
- `thinking_trace` / `event` / `custom` / `branch_summary` / `sessions.json` observability / checkpoint：仍保持原有异步或 turn-end 路径，不承担消息正确性。
- stream delta / thinking delta / CLI 中间渲染态：不直接写入主 transcript。

**失败与恢复语义（2026-05）**：
- `Failed` 不再把本轮视为“从未发生”：凡是已经即时落盘的 `user`、完整 `assistant`、`assistant + tool_calls`、已完成 `tool_result` 都会保留在 transcript。
- 流式中途尚未形成合法 message 边界的半截 delta 仍不写主 transcript；用户下一条输入会作为新的 `user` 继续追加，而不是覆盖上一轮。
- `chat --resume` hydrate 会扫描 transcript 尾部最后一个 `assistant.tool_calls` block，并按原顺序补齐所有缺失的 `role=tool, content="[interrupted]"` 结果；若尾部工具序列中间穿插了 `user/assistant/system` 等非 `tool` role，则拒绝猜测、不做自愈。
- 若主 chat 进程在即时落盘阶段命中 `append_message_chain` invariant（例如磁盘 transcript 已被其他写入者改变），当前进程会立即重新 `init_context_state`，以磁盘 JSONL 为准重建内存 `context_state`；若重建失败则退回空 `messages` fallback，避免继续携带 dangling `assistant.tool_calls` 并在下一轮触发 `No tool output found ...` 死循环。
- background follow-up / synthetic user message 统一在 **drain 时** 走与普通 `user` 相同的即时 append 路径；仅 enqueue 不落盘，避免第二套持久化窗口。

**上下文可观测累计（方案 B）**：`compaction_count` / `compaction_tokens_freed` / `tool_result_chars_persisted` 在进程内由 `ContextState` 更新，**每个 user turn 结束**（成功路径与可恢复错误路径）由 `SessionManager::persist_context_observability` 刷入 `sessions.json`；`init_context_state` 启动时读回填入 `ContextState`，实现重启后累计不无故归零。该累计**不以 transcript 重放重建**；与 transcript 手工编辑可能不一致。

**BranchSummaryEntry（JSONL `type: branch_summary`）可选 token 估算字段**（camelCase，旧行可缺省）：`estimatedCoveredTokensBefore`、`estimatedSummaryTokens`、`estimatedTokensSaved` — L1 预热写入，供 L2 apply 计入 `session_obs.compaction_tokens_freed` 而无需再次用 `estimated_token_count` 前后差计算。

**开发阶段说明（不向前兼容）**：运行时联合类型仅含 **`branch_summary`** 等当前变体，**不**再识别历史 JSONL 别名 **`type: compaction`**。若文件中仍存在该 `type`，反序列化将失败，读 tail 实现为 **跳过该行并 `warn` 日志**（不崩溃、不提供自动迁移）。本地旧文件需手工改为 `branch_summary` 或重新生成会话文件。
