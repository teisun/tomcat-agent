# `todos`：每个 Agent 会话一本工作本

```text
主 Agent ── TodosRuntime ── todos/<session_id>.todo.md ── 主侧边栏

plan reviewer ── 自己的 TodosRuntime ── todos/<child_session_id>.todo.md
                 不改主清单，也不通知主侧边栏

正式计划：PlanRuntime + update_plan → .plan.md（与 Todos 独立）
```

`todos` 管理标题、Markdown 正文、待办与完成证据。它不推进正式 Plan，不触发评审或验收；所有模式均可使用，角色白名单仍生效。完整工具 Schema 以 [生成目录](../../tool-catalog.md) 和 `src/core/tools/contract/catalog.rs` 为准。

## 1. 怎么用

```json
{
  "title": "修复登录",
  "draft": "## 处理记录\n问题出在旧令牌缓存。",
  "ops": [{"kind": "upsert", "id": "fix-login", "content": "修复并回归"}]
}
```

```json
{
  "ops": [{
    "kind": "set_status", "id": "fix-login", "status": "completed",
    "evidence": ["file:src/login.rs", "test:login_regression"]
  }]
}
```

| 参数 | 行为 |
|---|---|
| `draft` | 不传则保留；传字符串整体替换；`""` 清空。Markdown 原样保存，不剥标题或代码块。 |
| `title` | 不传则保留；传字符串改名；`""` 清除标题。不限于新建时使用。 |
| `ops` | 顺序应用 `upsert` / `set_status` / `remove`。只改正文或标题时可以省略。 |
| `new_todos=true` | 重建文档身份与创建时间，清空标题、正文、条目和证据，再应用本次参数；文件路径不变。 |
| `replace=true` | 仅重建条目，不改正文、标题、身份或创建时间。必须提供非空且全为 `upsert` 的列表；旧证据不继承。 |

`{}`、`{"ops":[]}` 与空 `replace` 会报错；清空整本使用 `new_todos=true`。最多三个独立条目同时 `in_progress`；未知 ID 或非法操作导致整批拒绝。

**证据规则与 `update_plan` 共用同一段操作代码，不调用 Plan 工具：**仅 `set_status(status="completed")` 可以带 `evidence`；追加、去重、忽略空白项；空数组不清除已有证据。普通更新保留证据，删除条目、整表重建和新建工作本会清除对应旧证据。

## 2. 每次成功都返回文件地址

```json
{
  "scope": "session", "applied": 1, "replace": false, "new_todos": false,
  "active_in_progress": null, "active_todos_id": "td_...", "title": "修复登录",
  "items": [{"id": "fix-login", "content": "修复并回归", "status": "completed", "evidence": ["test:login_regression"]}],
  "todos_path": "/.../.tomcat/agents/main/todos/<session_id>.todo.md"
}
```

- `items` 是完整当前清单，包含已有证据；标题已清除时不返回 `title`。
- `todos_path` 是绝对路径，每次成功保存后都返回。正文不自动回传，按需 `read(todos_path)`。
- `active_todos_id` 对应文档内的 `todos_id`，不再由 PlanRuntime 持有。
- 旧字段 `persisted_path` 改为 `todos_path`；移除依赖 Plan 的 `mode` 和与面板重复的 `panel_snapshot_id`。面板仍使用其自带的快照 ID。

## 3. 文件格式 v2 与冷恢复

固定地址：`~/.tomcat/agents/<agentId>/todos/<session_id>.todo.md`。普通更新与 `new_todos` 都覆盖这一份，不保留版本历史，不放进项目或 worktree。

```text
---
schema_version: 2
session_id: ...
todos_id: td_...
title: 修复登录
created_at: ...
items:
- id: fix-login
  content: 修复并回归
  status: completed
  evidence: ["test:login_regression"]
  kind: work
---
<draft 原文>
```

YAML 存结构化条目与元数据；正文只放 `draft`，不再自动附加 `## Todos` 勾选清单。`TodosRuntime` 是完整文档及路径的唯一持有者，`open()` 直接读盘，不依赖历史事件或压缩摘要。`GetState` 从它查询条目；三个 Plan 开工入口把 `has_unfinished()` 传给 `build_plan`，仍只是提醒，不阻止开工。

| 打开时遇到什么 | 怎么处理 |
|---|---|
| 无文件 | 暂无文档，不为闲置会话创建空文件。 |
| v2 且 session_id 匹配 | 恢复标题、正文、条目、证据与创建信息。 |
| v1 | 不迁移、不恢复；下一次成功更新才覆盖为 v2。旧版原本就是只写不读的投影。 |
| 损坏、未知版本、归属不符、IO 错误 | 告警，聊天继续；Todos 更新（包括 new）拒绝，原件不动，也不提供恢复入口；每次更新前重试读取，修复后可继续。 |

```text
commit：锁内克隆 → 应用并校验 → 原子保存 → 替换内存 → 解锁 → 发布
                     失败：文件、内存、通知均不提交
```

**保存失败就是失败，不再降级成“内存成功、磁盘没跟上”。**沿用 `write_file_atomic` 的唯一临时名、同步与改名流程。同一 AgentLoop 顺序写自己的工作本，不加第二把更新门闩或事件队列。发布回调在锁外运行，可安全查询已提交快照。

## 4. Agent 隔离与界面

- 主 Agent：由 ChatContext 注入运行时；保存后通知现有 TodosPanel 并写 `session.todos` 条目事件。正文不进入事件或进度投影。
- Plan reviewer：可以借用正式 PlanRuntime，但挂载自己的 TodosRuntime，发布回调为空；工具结果留在它自己的子会话记录中。
- Explorer、verifier、code reviewer 不提供 `todos`。工具执行只依赖本 Agent 的 TodosRuntime，未挂载时报不可用错误。
- 子会话 ID：`<parent>-child-<role>-<16位唯一后缀>`；后缀复用主会话生成器，不再用随 Registry 重建归零的序号。
- 关闭会话保留文件；删除父会话同时清理其 `.todo.md` 及 `<session_id>-child-*.todo.md` 子孙文档，其他会话不动。
- 不新增编辑器或改变现有面板协议。未来 worker、子 Agent 压缩和跨重启续接不属于本次交付，见 [多 Agent](../multi-agent.md)。

## 5. 压缩后怎么继续

```text
ControlSnapshot::capture(本 loop 的 Plan?, 本 loop 的 Todos?)
    未完成 Plan → Plan 路径、时间、待办
    已完成 Plan → 保留 Plan 路径和时间，有 Todos 也记其路径和时间
                   Todos 严格更新才列其待办，否则显示完成提示，不列待办
    无可读 Plan → 已保存/恢复的 Todos 路径、时间、待办（空文档也算）
```

上述判断只影响摘要，不改变计划绑定或侧栏。正文与完整证据不放进摘要；模型按入口读文件，使用 `todos` 更新工作本。预热快照采用启动时状态，不在后台追随之后的修改；详见 [上下文管理 §7.5](../context-management.md#75-compaction-v2-修订t2-p0-002)。

## 6. 回归覆盖

| 范围 | 测试文件 |
|---|---|
| v2 保真、旧版忽略、损坏保护与重试 | `src/core/plan_runtime/tests/todo_runtime_test.rs` |
| 正文/标题/reset/replace、证据、保存失败与锁外发布 | `src/core/tools/plan_tool/tests/todos_test.rs` |
| 真实 reviewer 隔离、独立工具分发、ID 唯一、子孙清理 | `subagent_provider_binding_test.rs`、`reviewer_guards_test.rs`、`agent_registry_test.rs`、`usage_guard_test.rs` |
| 冷恢复、重复 `/compact`、切换 Plan、预热与 Collapse | `cmd_compact_test.rs`、`current_tail_guard_test.rs`、`lifecycle_test.rs`、`machine_block_test.rs` |
| Schema 与生成目录 | `src/core/tools/contract/tests/catalog_test.rs`、`tests/tool_catalog_doc.rs` |
