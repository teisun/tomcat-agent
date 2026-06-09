| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-09 10:20 | ACTIVE | feature/optimize | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 新增 `TodosRuntime`，todos 落盘路径改为 `~/.tomcat/agents/<id>/todos/<session_id>.todo.md` @2026-06-09
- [✓] **[P0]** 照 `WebFetchRuntime` 范式经 `ChatContext → AgentLoop → ToolExecCtx` 注入，PlanRuntime 不再参与持久化 @2026-06-09
- [✓] **[P0]** 删除 `list_session_todos_files` / `purge_inactive` / `todos_persist_base` 多文件遗留机制 @2026-06-09
- [✓] **[P0]** 重写 `todo_runtime` / `todos` 单测与相关集成测；`ensure_work_dir_structure` 增加 `todos` 目录 @2026-06-09
- [✓] **[P0]** 同步 architecture / catalog / INTEGRATION 文档口径 @2026-06-09

### 🔌 INTERFACE (接口变更)
- `TodosRuntime::new(base_dir, session_id)` + `persist(&TodoFile)`：todos 持久化唯一入口。
- `todos::execute(runtime, todos_runtime, args)`：新增 `todos_runtime` 参数；未注入时仅内存推进。
- `AgentLoop::with_todos_runtime(...)`：会话级 todos runtime 注入 builder。
- `TodoFile` frontmatter：`session_key` 改为 `session_id`（由 `TodosRuntime` 写入）；文件名不再含 `todos_id`。
- 删除 `PlanRuntime::set_todos_persist_base` / `todos_persist_base`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
