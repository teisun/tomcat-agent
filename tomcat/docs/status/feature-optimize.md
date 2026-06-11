| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-06-10 16:35 | ACTIVE | feature/optimize | - |

### ✅ DONE (已完成/进行中)
- [✓] **[P1]** 修复多 provider 场景下 timing ⑤ `preheat` 误用主 provider 的错配问题：`turn_finalize` / `current_tail_guard` 统一改走 `AgentLoop::compaction_provider()`，`AgentLoopConfig.compaction_llm` 重命名为 `compaction_provider`，并为 accessor / timing⑤ start+restart / current-tail collapse / child-agent compaction pair 补齐路由矩阵测试 @2026-06-11
- [✓] **[P0]** 新增 `TodosRuntime`，todos 落盘路径改为 `~/.tomcat/agents/<id>/todos/<session_id>.todo.md` @2026-06-09
- [✓] **[P0]** 照 `WebFetchRuntime` 范式经 `ChatContext → AgentLoop → ToolExecCtx` 注入，PlanRuntime 不再参与持久化 @2026-06-09
- [✓] **[P0]** 删除 `list_session_todos_files` / `purge_inactive` / `todos_persist_base` 多文件遗留机制 @2026-06-09
- [✓] **[P0]** 重写 `todo_runtime` / `todos` 单测与相关集成测；`ensure_work_dir_structure` 增加 `todos` 目录 @2026-06-09
- [✓] **[P0]** 同步 architecture / catalog / INTEGRATION 文档口径 @2026-06-09
- [✓] **[P1]** Review 整改：合并 `execute_tool_full_with_policy` 统一签名，删除 `purge_inactive_on_new_todos` 孤儿配置，对齐 create-plan/update-plan 文档路径 @2026-06-09
- [✓] **[P1]** T2-P1-015：会话模式与多会话并发（`claw/code`、隐藏兼容别名 `chat -> code`、开发阶段旧 `sessions.json` 直接重建、`ChatContext` 三层迁移完成、checkpoint/restore 护栏与并发回归）已完成开发与验收回归，进入 `PENDING_INTEGRATION` @2026-06-10

### 🔌 INTERFACE (接口变更)
- `AgentLoopConfig.compaction_provider`：运行时 compaction / preheat provider 注入入口（原 `compaction_llm` 更名）；`AgentLoop::compaction_provider()` 成为统一访问器。
- `TodosRuntime::new(base_dir, session_id)` + `persist(&TodoFile)`：todos 持久化唯一入口。
- `todos::execute(runtime, todos_runtime, args)`：新增 `todos_runtime` 参数；未注入时仅内存推进。
- `AgentLoop::with_todos_runtime(...)`：会话级 todos runtime 注入 builder。
- `execute_tool_full_with_policy(..., todos_runtime, ...)`：`todos_runtime` 并入统一工具执行入口；删除 `execute_tool_full_with_todos_runtime_and_policy` 中间层。
- `TodoFile` frontmatter：`session_key` 改为 `session_id`（由 `TodosRuntime` 写入）；文件名不再含 `todos_id`。
- 删除 `PlanRuntime::set_todos_persist_base` / `todos_persist_base`。
- 删除 `TodosConfig.purge_inactive_on_new_todos`（从未接线）。
- `SessionMode` / `session_key_for()`：会话 scope 解析统一入口；默认模式由 `[session].default_mode` / `TOMCAT_SESSION_MODE` 驱动。
- `tomcat claw` / `tomcat code`：分别绑定全局与项目 scope；保留隐藏兼容别名 `tomcat chat -> tomcat code`。
- `sessions.json`：`SessionStore { sessions{id→entry}, current{key→id} }`；开发阶段不做旧结构兼容，`init` 与直接使用路径遇旧结构/反序列化失败时统一重建新文件。
- `ChatContext { global_services, scope_services, session_runtime }`：本轮已完成彻底去单例化；per-session 状态全部下沉，checkpoint 按 work_tree 复用，`read_file_state` / todos / cancel token 按会话隔离。
- checkpoint restore：checkpoint notes 自动记录 `changedPaths`，`/restore` 未显式给路径时默认按当前会话历史改动集收窄。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
