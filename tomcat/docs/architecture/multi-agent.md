# 多 Agent：子会话的执行、交付与观察

范围：父会话派发 Explorer、计划流程调用 Plan Reviewer。两者运行真正的 `AgentLoop`，拥有自己的工作本和可压缩历史。Code Reviewer、Verifier 保留各自的内部入口、提示词与权限；不是可供 `dispatch_agent` 选择的类型。本文不定义 Worker、跨重启续接或无人值守的控制者接管。

## 先看总图

**A.1 职责**：子会话拥有工作过程；父会话拿到结论，不接收子会话的全部上下文。

```text
父任务 → 整批准入 → 独立子会话 ── 过程 ──→ 只读旁看
                         ├── 状态 ──→ 父任务卡 / Dock / 模型查询
                         └── 报告 ──┬─ 前台：工具结果
                                    └─ 后台：票据先返回，报告进父队列
```

**A.2 实现**：前后台复用同一套子会话和监督任务。不同的是结果送给谁，不是怎样执行工具。

```text
dispatch_agent → PlanRuntime → ProdExplorerDispatcher → AgentRegistry.start_described_batch
create_plan + 审查同意       → ProdPlanReviewerDispatcher → spawn_subagent_internal
                                   ↓
                  ChildSession → AgentLoop + ContextState + TodosRuntime
                                   ↓
          私有 shell / 压缩预热收尾 → subagent.finished → 注销 → 发布终态
                    ┌─ 同步等待者（Explorer 整批一次交付）
          Explorer ─┤
                    └─ ReportBatch → follow_up_queue → 忙父消费 / 空闲父开轮
```

**B. 状态**：`stopping` 表示请求已收到，不等于任务已退出。`interrupted` 是历史证据不足的降级，不是 Stop 的回执。

```text
running ────────→ completed / failed
   └─ 定向停止 → stopping ── 清理完成 → stopped
历史开始指针 + 不在跑 + 无结束记录 → interrupted
```

## 1. 术语

| 名称 | 载体与约束 | 说人话 |
|---|---|---|
| 子会话 | `AgentHandle.session_id`：`<parent>-child-<role>-<唯一后缀>`。同名任务也有不同 ID。 | 用 ID 找孩子，不用标题猜。 |
| 任务行 | 子 Agent 的 `ChildRow` 或 Bash registry 的 shell 行，按 `(kind,id)` 和递增 `revision` 投影。不是报告。 | 进度在哪看都应一样。 |
| 报告 / 唤醒 | `MessageKind::Signal` 中的报告才是数据；`Notify` 只提醒宿主重新检查队列。 | 响铃不是信件；信已读就不开新轮。 |
| 控制 / 旁观 | 父 slot 的控制连接提供执行环境和交互策略。只读 child view 不是新 slot。 | 退出旁看不是叫停任务。 |

## 2. 选型依据

Hermes 在 `hermes-agent/tools/delegate_tool_toolsets.py` 从父工具减去不可委派项；opencode 在 `opencode/packages/core/src/plugin/agent.ts` 限制递归。Tomcat 的专用角色采用白名单，让新工具默认不可用；`child_tools.rs` 统一排除调度和交互工具，并让模型目录与执行守卫使用同一份定义。

工具派发适合独立的代码勘察：只需要父子通信，不需要共享聊天历史。原设计调研中的两种基线是 Claude Code 的同步委派和 openclaw 的异步投递；Tomcat 保留两种等待体验，但对一次派发只用一条报告交付路径。定向停止沿用角色自己的执行器，不把 shell 进程和 Agent 合并为一种任务。

## 3. 落地决策与实施点

`ChatContextRegistry` 管理 Serve 的父会话槽、准入和订阅；进程级 `AgentRegistry` 登记 root 与 child、限制配额并传播取消。不同父会话可并行，同一个父会话的模型回合串行；旁看 child 不另占父会话槽。原文维度 A/MA2、§14.3 的多会话设计现对应[服务端会话槽与执行注册表分工](agent-server-and-ui-gateway/02-implementation-details.md#332-chatcontextregistry-与现有-agentregistry-分工对齐-ma1ma2)。

### 3.1 路线比较

| 关注点 | 采用 | 放弃及原因 |
|---|---|---|
| 执行隔离 | 重用 AgentLoop；角色选择提示词、权限和结果格式。 | 复制父 transcript 或父待办会污染压缩和进度。 |
| 配额与取消 | 整批校验、整批登记；监督任务持有 registration guard。 | 调用者超时或退出时就释放配额，会出现「任务还在清理，名额已复用」。 |
| 两种交付 | 默认并行等待；后台先回票据，随后投递 Signal。 | 前台也入队会让主 Agent 收到两次。 |
| 旁看 | 父子 ID 校验 + 公共消息投影 + 打开的 child 订阅。 | 切换控制会话会改变输入和草稿的归属；靠路径或标题查找无法证明归属。 |
| 任务列表 | 子行属于 handle，shell 行属于 Bash registry；两者共用 UI 行协议。 | 通用执行 registry 会迫使进程、Agent 共享错误的回收语义。 |

### 3.2 实施点

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点 | 说人话 |
|---|---|---|---|---|
| 装配与运行 | 独立预算、工作本、记录、工具与读状态 | `child_session.rs`、`child_tools.rs`、`prod_reviewer.rs` | §8 隔离/压缩 | 主清单不会显示子任务自己的便签。 |
| 派发与完成 | 整批准入、唯一终态、单路交付 | `agent_registry/mod.rs`、`rows.rs`、`child_reports.rs` | §8 前后台/停止 | 父回合结束不等于子任务结束。 |
| 宿主续跑 | Serve 自动报告回合、CLI 首键保护、ACP 原请求结算 | `child_wake.rs`、`chat/run_loop/mod.rs`、`acp/adapter.rs` | §8 CLI/ACP | 正常完成不用再发一句「继续」。 |
| 观察和设置 | 定向停止、历史降级、只读视图、下一次派发取新模型 | `serve/{commands,child_view}.rs`、扩展 `BackgroundTasks.tsx`、`SubagentSettingsView.tsx` | §8 视图/模型 | 标题看执行，角色名改设置，Stop 只停这一行。 |

## 4. 协议与行为

工具定义以 [`catalog.rs`](../../src/core/tools/contract/catalog.rs) 和[生成目录](../tool-catalog.md)为准；Serve 命令与事件以 [`types.rs`](../../src/api/serve/types.rs) 为准，扩展由 `npm run gen:wire` 生成声明。以下样例只展示业务载荷。

| `dispatch_agent` 字段 | 类型 / 默认 | 约束 |
|---|---|---|
| `tasks` | 必填非空数组 | 每项校验通过且配额足够才启动；没有工具层的 6 项上限。注册表默认每父 8、总数 16，仍检查深度。 |
| `tasks[].type` | 必填字符串 | 只接受 `explorer`。 |
| `tasks[].description`、`prompt` | 必填非空字符串 | 标题单行、最多 120 字符；完整指令单独交给 child。 |
| `tasks[].id` | 预留 | 不向模型声明；运行时如收到则整批拒绝，包括显式 null。 |
| `run_in_background` | 可选布尔，省略为 false | 不接受 null。false 返回整批报告；true 返回票据数组。 |

```jsonc
// 请求
{"tasks":[{"type":"explorer","description":"查找路由入口","prompt":"列出请求进入路由器的位置"}],"run_in_background":true}
// 工具结果示例；同一 ID 用于旁看、查结果、停止
[{"id":"S-child-explorer-abc","type":"explorer","description":"查找路由入口"}]
```

**交付与宿主。** 子任务将完整报告和记录写入器直接返回监督任务；监督任务写入结束记录后再交付，不按字符数截断。前台不发布后台 Signal。后台每个未被取消的 child 把自己的报告进队，忙父可在正常消费点先读；同批最后一个结束后，只有仍有报告的批次才有独立唤醒资格。Serve 在准入锁内重查控制者、busy、中断状态和自动回合预算，通过现有 `start_turn` 启动报告回合。维护任务或执行回合变空闲时也重查。不把报告当作新的用户授权。CLI 空编辑器可自动续跑，已经开始输入则保留输入；ACP 把输出映射到原 `session/prompt`，以内部 `session_settled` 而非首次 `agent_idle` 结算。父上下文实际接收报告时发 `subagent_reports_received {count}`，不带报告正文；ACP 把子进度和接收数量映射为工具通知，不混入最终答复。

**任务行与历史。** `get_state.backgroundTasks` 给出当前父会话快照；`background.updated` 推送整行。`toolCallId` 关联派发卡，内部计划审查用其明确的 `childSessionId`。完成行留在父 handle；仅活跃行进 Dock。历史分页只补当前页引用的 child，优先用注册表，再校验子记录末尾；不扫描目录。在有开始但没有结束证据时显示 Interrupted。没有可靠 ID 的旧审查仍可读摘要，不凭文件名生成链接。

```text
subagent_list()       → {agents:[{id,type,description,state,activity,startedAt,endedAt?,revision,...}]}
subagent_output({id}) → 活跃：状态/活动；完成或失败：子记录末尾的报告；停止：没有报告
subagent_stop({id})   → stop_child(parent,id)；与 stop_background(kind=subagent) 同源
```

**Stop 和断线。** 控制者的 `stop_background(sessionId,kind,taskId)` 校验所属父会话；`kind=shell` 使用该父会话的 Bash registry。重复停止不重写终态。控制者的停止流程即使收尾超时，也会作废旧报告批次并清掉待处理输入；超时仍报错，不把尚未退出的任务标为已停。主 Stop 取消此父会话所有仍登记的 child，包括上一回合派出的后台 child；**不停父会话已后台化的 shell**。控制 EOF、会话删除、回滚重发按对应边界使旧批次失效。控制连接断开时同时收掉前台、子任务、后台命令和待确认事项，等待完成再清临时队列。旁观连接 EOF 只退订；不能按客户端总数判断是否取消。

**只读视图。** `view_child(sessionId,childId,cursor?)` 不接受路径，要求当前可读父会话、合法 ID、匹配的 JSONL 头与 parent/child meta。省略或 null cursor 取最新页并订阅，非空 cursor 向前翻页。返回公共消息、可展示思考、工具卡和边界，不回送 system prompt 或 provider 签名。原父事件泵按 child ID 分发，重放当前消息并仅向订阅者转发增量。关闭、断开或终态撤销订阅；无 child 输入队列。父会话继续接收输出，返回不丢草稿。

## 5. 文件职责图

```text
api/chat/context.rs：装配会话服务、role dispatcher、事件投影
 ├─ core/agent_loop/tool_exec/{mod,branches/dispatch_agent}.rs：解析与分派
 │   → core/plan_runtime/{mod,explorer,prod_reviewer}.rs：调度与角色结果契约
 │   → core/agent_registry/{mod,rows}.rs：准入、监督、取消、终态
 │   → core/plan_runtime/{child_session,child_tools,child_activity}.rs：装配、能力、活动摘要
 │      ├─ core/session/subagent_transcript.rs：种子 ID、归属校验、子 JSONL
 │      └─ core/compaction/preheat.rs：退出前 abort_and_join
 ├─ core/plan_runtime/{child_reports,child_queries}.rs：排队、取回和定向停止
 │   → api/serve/{registry,child_wake,commands}.rs：准入、开轮、结算
 │   → api/acp/adapter.rs | api/chat/run_loop/mod.rs + infra/platform/stdin.rs
 ├─ core/tools/primitive/bash_task.rs：shell 独立的注册、停止与输出排空
 ├─ api/serve/{event_pump,child_view,ownership,connection}.rs：可见范围与订阅
 │   → 扩展 sessionRouter.ts → webview/{provider,state}.ts → BackgroundTasks / ChildConversation
 └─ infra/config/{subagents,default_model,load}.rs → core/llm/resolver.rs：角色模型热更新
```

查「为什么停不掉」先看 handle 和监督任务；查「为什么没续跑」看批次资格、父队列和控制连接；只有服务行正确但界面不对时才看扩展投影。

## 6. 配置

`[subagents.explorer].model`、`[subagents.plan_reviewer].model` 分表保存。没有角色选择时继承派发时父会话的有效模型。Plan Reviewer 兼容旧 `[reviewer].model_override`，**仅当新角色表不存在**时使用；空表意味着明确继承，不让旧值复活。Code Reviewer 继续读取旧配置。适用进程环境覆盖时 `TOMCAT_REVIEWER_MODEL` 优先，Plan Reviewer 的设置只读；共享服务不引入启动者的任意环境作为个人设置。

`update_subagent_settings({role,model})` 的 role 必填，model 缺省不动，null 清除，字符串须能在目录中解析。原子落盘后复用 `refresh_all_model_catalogs` / resolver 刷新；运行中的 child 不改 provider、预算或模型。删除模型时一并清理角色引用；手改无效模型在派发时报告角色和模型，不偷偷改用其他 provider。

## 7. 降级

| 条件 | 结果 |
|---|---|
| 参数、角色、配额或前置模型解析无效 | 整批零启动，工具结果说明原因。 |
| 创建子 JSONL 失败 | 继续使用独立内存压缩；无法回看或事后取报告，明确告知。 |
| 取消或 panic | 等待该 child 的私有 shell、预热和 loop 退出；只写一个终态，取消不作为成功报告。 |
| 服务重启 | 可重开已保存的记录，但不恢复执行、不继续旧 ACP 请求。 |

## 8. 测试矩阵

| 维度 | 实际用例 | 状态 |
|---|---|---|
| 私有待办与压缩 | `concurrent_reviewer_and_explorer_own_separate_todos_and_compress_only_child_progress`、`dispatched_child_compacts_using_its_model_budget_and_own_transcript` | ✅ 2026-11-23 |
| 批次、配额、模型查询与停止 | `real_explorer_batch_has_one_delivery_path_and_keeps_quotas` | ✅ 2026-11-23 |
| 读边界、订阅与取消 | `child_view_is_scoped_filters_private_payloads_and_detaches`、`eof_stops_owned_background_processes_before_control_can_transfer` | ✅ 2026-11-23 |
| 宿主续跑 | `background_explorer_finishes_inside_original_acp_prompt`、`tty_background_explorer_wakes_only_before_the_first_key` | ✅ 2026-11-23 |
| 热刷新与删除 | `existing_plan_reviewer_reads_new_role_then_inherits_after_clear`、`open_parent_dispatch_reads_refreshed_role_selection`、`serve_provider_commands_batch_save_point_utility_and_remove_the_card` | ✅ 2026-11-23 |
| 原生窗口 | `host.e2e.test.ts` 的 `real subagent:`，使用临时 HOME、真实 Serve 和受控模型，验证卡片停止、Dock 单停、只读旁看、返回保留草稿和 Explorer 设置 | ✅ 2026-11-23 |

## 9. 风险与边界

并发：先校验整批再登记；以 `kind + id` 和 revision 防止迟到活动覆盖完成行。资源释放：监督任务拥有配额，等待者销毁不抢先注销；shell 停止必须等进程及输出排空。历史：子执行记录单独保存在 `agents/<agentId>/subagent-sessions/<childId>.jsonl`，工作本单独保存在 `todos/<childId>.todo.md`；根会话列表不列出 child。旧版本缺少角色配置能力时选择器只读。报告唤醒不解决真实用户输入与回合收尾的所有竞态，见 [AgentLoop](agent-loop.md)。

## 10. 历史决策

早期草案的通用 `general/shell/orchestrator` schema、仅同步等待、只取消根 token 的方案已被上述实现取代；不能用作当前协议。相邻契约见[工作本](tools/todos.md)、[共享 Serve 与 ACP](shared-serve-and-acp-host-integration.md)和[工具目录](../tool-catalog.md)。
