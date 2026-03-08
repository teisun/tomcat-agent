| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| session_cli_agent | 2025-03-05 14:00 | DONE | feature/session-cli | - |

### ✅ DONE (已完成)
- [✓] **[P0]** T1-P0-003 存储层与会话管理：SessionStore、SessionEntry、sessions.json 原子写、load_store/save_store
- [✓] **[P0]** T1-P0-003 transcript：SessionHeader、TranscriptEntry、流式读/追加写、get_entry/get_entries_tail/get_children/get_leaf_entry/get_branch
- [✓] **[P0]** T1-P0-003 SessionManager：CRUD、当前会话、上下文组装（最近 N 条）、append_message 等、会话级配置隔离
- [✓] **[P0]** T1-P0-003 单测：store/transcript/manager 边界与覆盖率
- [✓] **[P0]** T1-P0-010 CLI 骨架：clap 子命令 init/doctor/config/session/plugin/audit/chat，无参默认 chat
- [✓] **[P0]** T1-P0-010 init：生成默认配置文件
- [✓] **[P0]** T1-P0-010 doctor：配置存在与合法性、WasmEdge/QuickJS 占位
- [✓] **[P0]** T1-P0-010 config：get/set/edit/export/import 骨架
- [✓] **[P0]** T1-P0-010 session：list/new/switch/delete/archive/search，依赖 SessionManager，空列表提示
- [✓] **[P0]** T1-P0-010 plugin/audit：占位（待 009/P1-001 对接）

### 🔌 INTERFACE (接口变更)
- **SessionManager**：`from_sessions_dir`、`create_session`、`list_sessions`、`get_session`、`update_session`、`delete_session`、`archive_session`、`append_message`、`get_entries`、`build_context_messages`、`get_entry`/`get_children`/`get_leaf_entry`/`get_branch`
- **lib 导出**：`SessionManager`、`SessionStore`、`SessionEntry`、`TranscriptEntry`、`SessionHeader`、`DEFAULT_SESSION_KEY`、`run_cli`
- **api**：`run_cli()` 入口，子命令由 main 调用

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
