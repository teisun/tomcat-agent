# 多会话协议对比：pi-rust-wasm / pi-mono / OpenClaw

> 状态：**调研完成**
>
> 目的：梳理三个项目的会话存储与多会话机制，为 pi-rust-wasm 实现「真·多会话」提供设计参考。

---

## 1. 摘要对比表

| 维度 | pi-rust-wasm（当前 MVP） | pi-mono（coding-agent） | OpenClaw（Web UI） |
|------|------------------------|----------------------|-------------------|
| 索引文件 | `sessions.json`（HashMap: key → entry） | **无**；每个会话一个 JSONL 文件 | `localStorage`（`UiSettings` 含 `sessionsByGateway`） |
| 多会话表示 | 仅一个固定 key `agent:main:main`，`session new` 覆盖同 key | 同一 cwd 编码子目录下多个 `*.jsonl` 文件并存 | 每个 Gateway 独立维护 `sessionKey` + `lastActiveSessionKey` |
| 默认打开哪个 | 固定返回 `agent:main:main` | `continueRecent`：按文件 mtime 选最近 | `lastActiveSessionKey`：上次交互的会话，持久化到 localStorage |
| 切换会话 | `session switch` 为占位，实际不生效 | `setSessionFile` / TUI `/resume` 选择器 | URL `?session=` 参数 + `setLastActiveSessionKey` 写 localStorage |
| Transcript 格式 | JSONL，`SessionHeader`（v3）+ `TranscriptEntry` 树 | JSONL，`SessionHeader`（v3）+ `SessionEntry` 树，兼容 | 不直接管理 transcript（由内嵌 pi-mono agent 管理） |
| 会话删除 | `session delete`：从 store 移除 + 删文件 | 删除 `.jsonl` 文件；TUI 中 `Ctrl+D` 确认删除 | 由 Gateway 管理，UI 不直接删文件 |

---

## 2. pi-rust-wasm 当前实现与本机磁盘状态

### 2.1 磁盘样例

`~/.pi_/agents/main/sessions/sessions.json`（当前内容）：

```json
{
  "agent:main:main": {
    "sessionId": "1774419565065_322f934f1b9d3bbc",
    "updatedAt": 1774419565065,
    "sessionFile": "/Users/yankeben/.pi_/agents/main/sessions/1774419565065_322f934f1b9d3bbc.jsonl",
    "cwd": "/Users/yankeben/workspace/Tomcat"
  }
}
```

只有一条记录。对应的 transcript 是同目录下 `<sessionId>.jsonl`，首行为 `SessionHeader`。

### 2.2 代码要点

**固定 key**：`store.rs` 定义 `DEFAULT_SESSION_KEY = "agent:main:main"`。`manager.rs` 的 `current_session_key()` 始终返回该常量。

来源：`src/core/session/store.rs:14`、`src/core/session/manager.rs:72-74`

**`session new` 覆盖**：`cli.rs` 中 `SessionSub::New` 调用 `create_session(key, None)`，始终使用固定 key。由于 `create_session` 内部 `store.insert(session_key, entry)`，新会话**覆盖**旧条目，无法共存。

来源：`src/api/cli.rs:682-685`、`src/core/session/manager.rs:82-113`

**`session switch` 占位**：输出 `"当前会话 key 固定为 agent:main:main，切换逻辑占位。"`，不修改任何状态。

来源：`src/api/cli.rs:687-693`

**`chat` 入口**：`ensure_session` 检查 `agent:main:main` 是否存在，不存在则创建；存在则直接续用。无「选最近」或「用户选择」逻辑。

来源：`src/api/chat.rs:380-388`

### 2.3 问题总结

1. **无法创建第二个会话**：`session new` 始终覆盖同一 key。
2. **无法切换**：`current_session_key()` 硬编码，`switch` 为空操作。
3. **无法恢复最近会话**：无 `continueRecent` 或 `lastActiveSessionKey` 机制。
4. **`session list` 始终最多一条**：HashMap 中只有一个 key。

---

## 3. pi-mono（coding-agent）-- 主参考

来源：`pi-mono/packages/coding-agent/src/core/session-manager.ts`、`pi-mono/packages/coding-agent/docs/session.md`

### 3.1 存储模型：纯文件系统，无索引

pi-mono **不使用** `sessions.json` 索引文件。多会话 = 同一目录下多个 JSONL 文件：

```
~/.pi/agent/sessions/--<cwd-encoded>--/<timestamp>_<uuid>.jsonl
```

其中 `<cwd-encoded>` 为工作目录去掉前导 `/`，`/` 替换为 `-`，两端包裹 `--`。例如 cwd 为 `/Users/alice/project` 时，目录名为 `--Users-alice-project--`。

来源：`session-manager.ts` 函数 `getDefaultSessionDir`（约 420 行）

### 3.2 核心 API

| 静态方法 | 功能 | 关键逻辑 |
|---------|------|---------|
| `create(cwd)` | 创建新会话 | 生成新 JSONL 文件，文件名 `<timestamp>_<uuid>.jsonl` |
| `continueRecent(cwd)` | 续接最近会话 | `findMostRecentSession` 按文件 **mtime** 倒序选第一个；无文件则创建新会话 |
| `open(path)` | 打开指定会话文件 | 直接加载并建立索引 |
| `list(cwd)` | 列出当前 cwd 的会话 | 遍历目录，解析每个 JSONL 首行得到 `SessionInfo` |
| `listAll()` | 列出所有 cwd 的会话 | 遍历 `~/.pi/agent/sessions/` 下所有子目录 |
| `forkFrom(source, target)` | 从另一个会话 fork | 复制全部 entry 到新文件，`parentSession` 指向源 |
| `inMemory(cwd)` | 创建内存会话 | 不持久化，用于测试/SDK |

来源：`session-manager.ts:1255-1410`

### 3.3 「默认打开哪个」

pi-mono 的 CLI 使用 `continueRecent`：

```typescript
static continueRecent(cwd: string, sessionDir?: string): SessionManager {
    const dir = sessionDir ?? getDefaultSessionDir(cwd);
    const mostRecent = findMostRecentSession(dir);
    if (mostRecent) {
        return new SessionManager(cwd, dir, mostRecent, true);
    }
    return new SessionManager(cwd, dir, undefined, true);
}
```

`findMostRecentSession` 扫描目录中所有 `.jsonl`，按 `statSync(path).mtime` 倒序排列，返回第一个。

来源：`session-manager.ts:474-484`、`session-manager.ts:1280-1287`

### 3.4 JSONL 格式

首行为 `SessionHeader`：

```json
{"type":"session","version":3,"id":"<uuid>","timestamp":"<ISO>","cwd":"<path>","parentSession":"<optional>"}
```

后续行为 `SessionEntry`，每行含 `id`/`parentId` 形成树结构（支持分支），类型包括：`message`、`thinking_level_change`、`model_change`、`compaction`、`branch_summary`、`custom`、`custom_message`、`label`、`session_info`。

pi-rust-wasm 的 transcript 格式已与此兼容（v3 header、相同的 entry 类型枚举）。

### 3.5 SessionInfo（列表元数据）

`list` / `listAll` 返回 `SessionInfo`，从 JSONL 文件解析而来：

```typescript
interface SessionInfo {
    path: string;
    id: string;
    cwd: string;
    name?: string;              // 用户自定义名称（session_info entry）
    parentSessionPath?: string; // fork 来源
    created: Date;
    modified: Date;             // 最后一条 user/assistant 消息时间 > header 时间 > 文件 mtime
    messageCount: number;
    firstMessage: string;
    allMessagesText: string;    // 用于搜索
}
```

来源：`session-manager.ts:165-179`

---

## 4. OpenClaw -- 辅助参考

OpenClaw 的「会话」主要指 Gateway 多通道聊天会话（与 coding-agent transcript 不同），但其 **「记住上次选中的会话」** 和 **LRU 缓存** 机制对 pi-wasm 有参考价值。

### 4.1 `sessionKey` / `lastActiveSessionKey` 持久化

OpenClaw Web UI 通过 `UiSettings` 持久化两个字段：

- **`sessionKey`**：当前会话标识（默认 `"main"`）。
- **`lastActiveSessionKey`**：上次活跃的会话，重启后用于恢复。

`applySettings` 中始终保证 `lastActiveSessionKey` 有值：

```typescript
lastActiveSessionKey: next.lastActiveSessionKey?.trim() || next.sessionKey.trim() || "main"
```

启动时 `loadSettings` 从 localStorage 读取，恢复到 `host.applySessionKey = host.settings.lastActiveSessionKey`。

此外，`sessionsByGateway` 为 `Record<string, ScopedSessionSelection>`，按 Gateway URL 分桶存储每个 Gateway 的 `sessionKey` + `lastActiveSessionKey`，支持多 Gateway 场景。

来源：`openclaw/ui/src/ui/app-settings.ts:64-87`、`openclaw/ui/src/ui/storage.ts:11-20、192-240、285-334`

**对 pi-wasm 的启示**：可在 `sessions.json` 或独立 `state.json` 中持久化 `current_session_key`，启动时读取恢复，而非硬编码。

### 4.2 LRU 会话缓存

```typescript
export const MAX_CACHED_CHAT_SESSIONS = 20;
```

`getOrCreateSessionCacheValue` 使用 `Map` 的插入顺序特性实现 LRU：访问时先 `delete` 再 `set` 刷新顺序，超过上限时淘汰 `keys().next()`（最老的）。

来源：`openclaw/ui/src/ui/chat/session-cache.ts`

**对 pi-wasm 的启示**：pi-wasm 当前场景为 CLI 单进程，暂不需要 LRU 内存缓存。但若未来支持长驻进程（daemon / Web UI），可参考此模式限制内存中加载的会话数量。

---

## 5. 差异总结与演进路线

### 核心差距

| 能力 | pi-mono | pi-rust-wasm 现状 |
|------|---------|-----------------|
| 多会话共存 | 自然支持（多文件） | 不支持（单 key 覆盖） |
| 默认恢复 | `continueRecent`（mtime） | 固定 key |
| 切换 | `setSessionFile` / TUI `/resume` | 占位 |
| 新建 | 生成新文件，不影响已有 | 覆盖同 key |
| 列表 | 遍历目录 + 解析 header | 仅读 HashMap（最多一条） |
| Fork/分支 | `forkFrom` + 树状 entry | 不支持 |
| 用户命名 | `session_info` entry 中 `name` | 不支持 |

### 路线 A：pi-mono 式（纯文件系统，去掉 sessions.json）

**做法**：

- 去掉 `sessions.json` 索引。每个会话一个 JSONL 文件，按 cwd 编码子目录存放。
- `session new`：生成 `<timestamp>_<uuid>.jsonl`，不影响已有文件。
- `pi chat`（默认）：`continueRecent` 按文件 mtime 选最近。
- `pi chat --resume`：列出目录中所有会话，用户选择。
- `session list`：遍历目录，解析 JSONL 首行得到元数据。
- `session switch`：`open` 指定文件路径。

**对 transcript JSONL 的影响**：无。格式已兼容 pi-mono v3。

**对 CLI 的影响**：

| 子命令 | 改动 |
|-------|------|
| `new` | 不再传 key，直接创建文件 |
| `list` | 遍历目录，输出 `SessionInfo` |
| `switch` | 接受 session id 或文件路径 |
| `delete` | 接受 session id，删除对应文件 |
| `archive` | 移动文件到 archive 子目录 |
| `search` | 遍历目录 + `allMessagesText` 搜索 |

**优点**：与 pi-mono 协议完全对齐；无索引维护负担；新增会话零成本。

**缺点**：列表需遍历文件系统 + 解析 JSONL 首行（会话数多时有 I/O 开销）；无 O(1) 查询。

### 路线 B：增强现有 sessions.json

**做法**：

- 保留 `sessions.json`（HashMap 索引），允许多个 key 共存。
- `session new`：生成新的唯一 key（如 `agent:main:<timestamp>`），写入新条目。
- 新增 `current_key` 字段：持久化到 `sessions.json` 顶层或独立 `state.json`。
- `current_session_key()` 从持久化状态读取，不再硬编码。
- `session switch <key>`：更新 `current_key` 并持久化。
- `pi chat`（默认）：读取 `current_key`，若无则选 `updated_at` 最大的。

**对 transcript JSONL 的影响**：无。索引与 transcript 解耦。

**对 CLI 的影响**：

| 子命令 | 改动 |
|-------|------|
| `new` | 生成新 key，写入 HashMap |
| `list` | 直接读 HashMap，O(1) |
| `switch` | 更新 `current_key` 持久化 |
| `delete` | 从 HashMap 移除 + 删文件 |
| `archive` | 从 HashMap 移除（不删文件） |
| `search` | 遍历 HashMap（元数据有限，深度搜索仍需读 JSONL） |

**优点**：列表/查询 O(1)；与现有 `store.rs` / `manager.rs` 改动最小；元数据集中。

**缺点**：需维护索引一致性（文件删除/损坏时索引可能过期）；与 pi-mono 协议有分歧（pi-mono 无索引）。

### 路线 C：混合（索引 + cwd 分桶）

**做法**：

- 保留 `sessions.json` 做快速查询/列表的缓存索引。
- 文件按 pi-mono 的 cwd 编码子目录存放。
- `session new`：创建文件 + 写入索引。
- 启动时可选校验索引与文件系统一致性。

**对 transcript JSONL 的影响**：无。

**优点**：兼顾 pi-mono 目录兼容性与查询效率。

**缺点**：两套机制并存（索引 + 文件系统），一致性维护复杂度最高。

---

## 相关源码与文档链接

| 项目 | 文件 | 说明 |
|------|------|------|
| pi-rust-wasm | `src/core/session/store.rs` | `SessionStore` 类型、`DEFAULT_SESSION_KEY`、load/save |
| pi-rust-wasm | `src/core/session/manager.rs` | `SessionManager`：CRUD、`current_session_key()`、transcript 读写 |
| pi-rust-wasm | `src/core/session/transcript.rs` | `SessionHeader`、`TranscriptEntry`、append/read |
| pi-rust-wasm | `src/api/cli.rs` | `run_session`：`new`/`list`/`switch`/`delete`/`archive`/`search` |
| pi-rust-wasm | `src/api/chat.rs` | `ensure_session`、`chat_loop` |
| pi-rust-wasm | `docs/technical/02-session-and-cli.md` | 会话管理与 CLI 技术文档 |
| pi-mono | `packages/coding-agent/src/core/session-manager.ts` | `SessionManager` 完整实现 |
| pi-mono | `packages/coding-agent/docs/session.md` | JSONL 格式文档 |
| OpenClaw | `ui/src/ui/app-settings.ts` | `sessionKey`/`lastActiveSessionKey` 持久化 |
| OpenClaw | `ui/src/ui/storage.ts` | `UiSettings`、`sessionsByGateway`、`loadSettings`/`saveSettings` |
| OpenClaw | `ui/src/ui/chat/session-cache.ts` | LRU 会话缓存 |
