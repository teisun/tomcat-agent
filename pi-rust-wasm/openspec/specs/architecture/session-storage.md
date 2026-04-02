本文为 [Architecture](../Architecture.md) 中「会话存储数据结构设计」的详细设计，总览见主文档。会话与存储根目录、多 agent 布局见 [工作目录与数据布局](work-dir-and-data-layout.md)。

## 会话存储数据结构设计

### 元数据 store（sessions.json）

单文件 JSON：`sessionKey -> SessionEntry`。列表与路由由此提供，不另建 SQLite 索引。

```rust
/// 会话根目录：~/.pi_/agents/<agentId>/sessions/
/// sessionKey 格式：agent:<agentId>:<channelKey>，MVP 单入口用 agent:main:main
pub type SessionStore = std::collections::HashMap<String, SessionEntry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub session_id: String,           // 当前 transcript 文件 id，对应 <sessionId>.jsonl 或 pi 系 <timestamp>_<uuid>.jsonl
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
    // 预留：channel/agent 相关字段供三期多 channel 使用
}
```

### 对话 transcript（pi 系 JSONL）

每会话一个 `.jsonl` 文件：**每行一个 JSON 对象**（非管道分隔）；首行 session header，后续每行一条 entry，树形 id/parentId。内存中为结构化类型（pi-mono 为 `SessionEntry` 联合类型），落盘时每行 `JSON.stringify(entry)`。与 pi-mono 格式兼容。

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
    Compaction(CompactionEntry),
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

/// Compaction entry：Layer 2 LLM 摘要产物，替换一批已压缩的旧 turns。
/// 详见 [上下文管理技术方案](context-management.md) §5.4 / §6.3。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
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
    /// init_context_state 遇到 is_boundary=true 时，丢弃其前已暂存的所有 entry，
    /// 使跨重启重建结果与运行时内存状态一致，防止已摘要的旧 turns 与 summary 重复加载。
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_boundary: bool,
}
```

**会话路径与会话标识**
- **会话根目录** `~/.pi_/agents/<agentId>/sessions/`；MVP 阶段 agentId 固定为 `main`。
- **sessionKey** (路由键，预留多channel)：`agent:<agentId>:<channelKey>`，MVP 用 `agent:main:main`，后续 channelKey 可扩展如: `agent:mybot:telegram:group:123`
- **sessionId** 当前对话对应的 transcript 唯一 id(sessionId=<timestamp>_<uuid>)，对应文件名'<sessionId>.jsonl'; SessionEntry中'sessionId'指向改文件

**Source of truth**：transcript 内容以 JSONL 文件为准；sessions.json 为元数据与路由的权威，写入时覆盖该文件。
