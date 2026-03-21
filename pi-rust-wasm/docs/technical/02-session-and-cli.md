# 会话管理与 CLI 说明 (Session & CLI)

## 1. 概述 (Overview)

- **职责**：会话元数据 store（sessions.json）、对话 transcript（pi 系 JSONL）的读写与 CRUD；CLI 子命令 init/doctor/config/session/plugin/audit，无参默认 chat。
- **所在层级**：宿主核心能力层（core/session）、交互层（api/cli）。
- **核心文件**：
  - `src/core/session/store.rs` — sessions.json 元数据 load/save、原子写
  - `src/core/session/transcript.rs` — SessionHeader、TranscriptEntry、流式读/追加写、get_entry/get_children/get_branch 等
  - `src/core/session/manager.rs` — SessionManager：CRUD、上下文组装、会话级配置
  - `src/api/cli.rs` — clap 子命令定义与 run_init/run_doctor/run_config/run_session/run_plugin/run_audit/run_chat
  - `src/main.rs` — 二进制入口，调用 `run_cli()`

设计约束：禁止全量加载 transcript；sessions.json 写必用「临时文件 + 重命名」；并发写通过 Mutex 序列化。

### 1.1 CLI 与会话存储关系（ASCII）

```text
  pi <subcommand> / pi (默认 chat)
            |
            v
     +------+-------+
     | src/api/cli  |
     +------+-------+
            |
     +------v-------------+----------------------+
     | SessionManager      |  其他子命令 init/   |
     | store (HashMap)     |  doctor/config/...  |
     | + transcript JSONL  |                      |
     +---------------------+----------------------+
            |
            v
     sessions.json  +  <session_id>.jsonl
     (原子写)         (仅追加，禁止整文件解析)
```

- **边界**：`SessionManager` 不直接调用 LLM；对话路径经 `chat` 组装上下文后再交给 `AgentLoop` + `LlmProvider`（见 [模块技术文档索引](./README.md)「图 2」）。
- **代码入口**：`run_cli()` → 各 `run_*` handler（`src/api/cli.rs`）。

---

## 2. 会话存储 (Session Storage)

### 2.1 元数据 store（sessions.json）

- **类型**：`SessionStore = HashMap<String, SessionEntry>`，key 为 sessionKey（MVP 固定 `agent:default:main`）。
- **SessionEntry 字段**：session_id、updated_at、session_file、cwd、thinking_level、model_override、input_tokens、output_tokens、compaction_count（camelCase 序列化）。
- **路径**：由 `AppConfig::storage.sessions_dir` 决定，默认 `~/.pi/agent/sessions`；文件名为 `sessions.json`。
- **原子写**：`save_store` 使用 infra 的 `write_file_atomic`（写临时文件后 rename）。

### 2.2 对话 transcript（pi 系 JSONL）

- **首行**：SessionHeader（type、version、id、timestamp、cwd）。
- **后续行**：TranscriptEntry 枚举（message、model_change、thinking_level_change、session_info、compaction、branch_summary、label、custom），与 session-pi-mono-format.jsonl 兼容。
- **读写约定**：禁止整文件 `from_str`；使用 BufReader 逐行读；上下文组装仅保留最近 N 条（默认 10）；append 仅追加不修改历史。

---

## 3. SessionManager API

- **构造**：`SessionManager::new(sessions_dir: PathBuf)` 或 `SessionManager::from_sessions_dir(sessions_dir: &str)`（内部 normalize_path）。
- **CRUD**：`create_session`、`get_session`、`list_sessions`、`update_session`、`delete_session`、`archive_session`。
- **消息**：`append_message`、`append_thinking_level_change`、`append_model_change`、`append_compaction`、`append_session_info`、`append_label_change`；`get_entries`、`get_entry`、`get_children`、`get_leaf_entry`、`get_branch`。
- **上下文**：`build_context_messages(recent_n)` 返回最近 N 条 Message 的 message 列表，供 LLM 使用。

---

## 4. CLI 子命令

| 子命令 | 说明 |
|--------|------|
| `pi-wasm`（无参） | 默认执行 chat（占位） |
| `init` | 生成默认配置文件（默认路径 ~/.pi/agent/config.toml） |
| `doctor` | 检查配置文件存在与合法性；WasmEdge/QuickJS 检测占位 |
| `config get/set/edit/export/import` | 配置管理，session 依赖 load_config(None) 与默认路径 |
| `session list/new/switch/delete/archive/search` | 依赖 SessionManager，空会话列表时提示 |
| `plugin list/load/unload/enable/disable/info` | 占位，待 T1-P0-009 对接 |
| `audit list/show/export` | 占位，待 T1-P1-001 对接 |
| `chat [--resume]` | 进入交互式对话模式（流式渲染、多轮上下文、工具调用） |

---

## 5. 对话模式 (Chat Mode, T1-P0-011)

### 5.1 概述

`pi-wasm chat`（或无参默认）进入交互式对话模式。核心文件：
- `src/api/chat.rs` — ChatContext、chat_loop、工具调用执行、CliConfirmation
- `src/api/render.rs` — MarkdownRenderer（流式代码块高亮，基于 syntect）

### 5.2 架构

`run_chat` → `ChatContext::from_config` → `tokio::Runtime::block_on(chat_loop)`。主循环：rustyline 读输入 → `build_context_messages` 组装历史 → `chat_stream` 流式输出（MarkdownRenderer 高亮）→ 工具调用循环（最多 10 轮）→ 写 transcript。

### 5.3 关键设计

- **流式渲染**：消费 `StreamEvent::ContentDelta`，代码块闭合后 syntect 高亮。
- **多轮上下文**：`build_context_messages(context_cap())` 取最近 N 条。
- **工具调用**：`ToolCallDelta` 累积后执行 read_file/write_file/edit_file/execute_bash/list_dir；结果以 `role=tool` 回传 LLM。
- **用户确认**：`CliConfirmation` 实现 `UserConfirmationProvider`（stdin y/N）。
- **会话隔离**：`effective_model` 优先用 `SessionEntry.model_override`。
- **快捷键**：Ctrl+C 中断生成，Ctrl+D 退出，↑↓ 历史。

### 5.4 类型变更

- `ChatMessage.content` → `Option<ChatMessageContent>`；新增 `tool_calls`、`tool_call_id`。
- `ChatMessageRole` 新增 `Tool`。
- `ChatRequest` 新增 `tools`。
- `StreamEvent` 新增 `ToolCallDelta`。

---

## 6. 依赖与验收

- **依赖**：T1-P0-001~006、TASK-01、TASK-02。
- **验收**：`cargo test -j 1 --lib -- --test-threads=1` 通过；clippy/rustfmt 通过；chat 可流式对话、多轮上下文、工具调用。
