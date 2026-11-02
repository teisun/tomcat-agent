# Agent Loop 设计

本文为 [Architecture](../openspec/specs/Architecture.md) 中「9. Agent Loop 设计」的详细设计，总览见主文档。

## 2026-05 PlanRuntime 启动挂接补充

agent/chat 进入主循环前，启动顺序新增一条 plan 恢复挂接：

1. 先调用 `init_context_state(...)` 组装上下文；
2. 从返回的 `ContextState.latest_plan_event` 读取最近的 `plan.create/build/update`；
3. 调用 `PlanRuntime::attach_from_event(...)` 回盘派生运行时状态。

当前实现约束：

- `attach_from_event(None)` 保持 `Chat`；
- `plan.create` 只恢复 `active_planning_plan_id`；
- `plan.build` / `plan.update` 会恢复 `active_plan_path`，并且仅在盘 state 为 `pending / executing` 时显式恢复 `PlanState`；
- 盘 state 为 `planning / completed` 时，agent loop 仍以 `Chat` 装配 prompt 和 catalog；
- `Completed` 是瞬时态，不作为稳定 prompt/cursor label 出现在用户面前。

说人话：agent loop 现在在装历史上下文时，顺便把 active plan 的恢复线索也带进来了；后面装 prompt 前再把 runtime 对齐一次。

---

## 正常收尾的输入消费与后台信号

```text
text-only/合法静默 finalize 成功
  → 工具预算耗尽：结束，队列留给下一轮
  → steering / follow-up：按此顺序用统一记账、持久化助手消费
  → steering / 非 Signal 追加输入：关闭静默许可；后台 Signal：许可不变
```

`run_inner` 收尾同时检查两种队列，复用 `inject_steering_messages/inject_follow_up_messages` 保留 msg_id、append 与上下文记账；错误/取消不作为正常收尾续跑。静默许可的开关与确认规则只在 [stream-terminal-errors.md](llm/stream-terminal-errors.md)「内部通知的正常空终态」维护。Stopped 的模型通知在 completion subscriber 被过滤，独立的 UI 生命周期通道照常更新；Serve 空闲自然通知仅排队、不启动模型。

**仍存在的风险（本期延期）**：`run_inner` 最后一次查队列之后，经过 chat 监听注销、上下文停车与收尾持久化，直到 Serve `mark_idle` 之前，仍可能接受一条 `queued:true` 的真实输入，却暂时没有消费者，需等下一次输入。admission 与 idle 未在同一临界区，此次不宣称已消除。后续单独设计防丢唤醒，并先用确定性 barrier 固定时序；Signal 空闲不唤醒、中断、维护 busy 的优先级应保留。

原生写入的 commit-aware 停止由 primitive/dispatcher 边界负责，不能取消丢弃整个 reasoning future；非写盘 compaction 请求在自身边界响应取消。已开始写盘的工具要等真正提交结果及备份登记完成，才能报告停止并恢复文件。

## 13.1 概述与设计目标

Agent Loop 编排模型请求、工具执行、Steering / FollowUp / Abort 和消息持久化。三层职责为 **Conversation → Reasoning → Request Retry**：重试只包住一次逻辑模型请求，不重新进入整个工具循环。

```text
接收输入
  └─ Conversation：输入、FollowUp、最终收尾
       └─ Reasoning：保留已接受正文、工具结果、工具轮次
            ├─ Request Retry：得到下一份可接受回复（有限尝试）
            └─ 提交回复 / 执行工具 → 下一请求获得新额度
```

工具、权限、持久化和用户停止仍由原有边界负责；请求恢复不代表任务结束，也不代表消息已经保存。

### 设计目标

1. **生命周期清晰**：对话管理 → 思考-行动 → 单请求容错重试，三层职责分离、边界明确。
2. **pi-mono 生态兼容**：事件名、Steering/FollowUp 语义、消息类型边界与 pi-mono 一致。
3. **可观测**：所有关键节点向事件总线发布 AgentEvent/ExtensionEvent，便于 UI 与插件订阅。
4. **可恢复**：Context Overflow、Rate Limit、网络超时等可重试错误由 Request Retry 统一处理。
5. **可扩展**：插件通过 Hook 点参与 Prompt 构建、工具调用前后等环节。

## 13.2 术语与次数

| 术语 | 含义 |
| --- | --- |
| **Agent Loop** | 智能体循环：接收用户输入 → 调用 LLM → 执行工具 → 结果回注 → 再调用 LLM → … → 最终回答。 |
| **Tool Call** | 工具调用：LLM 通过结构化请求委托宿主执行 read_file、write_file、edit_file、execute_bash、list_dir 等。 |
| **Context Overflow** | 上下文溢出：对话历史超过 LLM 的 context window 限制。 |
| **Tool Loop Detection** | 工具循环检测：防止同一工具+同参数或 A↔B 交替无进展地重复调用。 |
| **Turn / 逻辑模型请求** | 为得到下一份可接受回复而进行的一组尝试；成功后可能执行工具，再进入下一 Turn。响应头、部分正文、工具预览都不算成功。 |
| **Attempt** | 同一逻辑请求的一次尝试：重新组装请求、完整消费流及尾随 usage、检查空答等终态；不是整个思考行动循环。 |
| **Steering** | 转向/中断注入：用户于 Agent 工作中插话；当前工具执行完后跳过剩余工具，注入新消息并重新调用 LLM。 |
| **FollowUp** | 追加消息：Agent 刚结束时用户追加消息，在同一会话上下文中继续处理，不重新初始化。 |
| **Compaction / 附件降级** | 上下文压缩保留 System Prompt + 最近 N 条，对中间历史做摘要或截断，腾出空间；压缩或附件降级后仍在同一请求内再试，消耗原额度，不清零。 |
| **Abort** | 用户停止；不再发请求，保存已收到的部分内容及已完成工具结果，按原规则收尾。 |

- 交互模式：`max_attempts`（配置默认 **4**）；无人值守临时故障和空答：**10**。均**含首次**，所以10次意味着首次加最多9次重试。
- 无人值守的其他可恢复错误：当前尝试序号达到默认交互上限4时停止，不因换错误类型再送4次。
- 账户、鉴权、内容拒绝等明确错误仍立即结束本次执行。普通未知流错误、Parse 的范围不放宽。
- `max_tool_rounds` 属于 Reasoning；请求重试既不刷新它，也不额外消耗一个工具轮次。`final_text` 同样跨请求重试保留。
- provider 的 HTTP 重试不变：例如 Agent 2次 × 每次2次HTTP = 4次HTTP，不等于2次HTTP。标题、压缩、hostcall 不递归套入新循环。

### 历史来源

这次是**修正旧设计，不是恢复某个曾经正确的实现**。Git `eb2cd0a9`（2026-03-11）把 Attempt 定义为整段思考行动；`9f25ac4e`（2026-03-12）按该设计实现；`09333a6d`（2026-08-10）把无人值守额度增至10，但没有改计数范围。因此一次成功请求之后的工具执行不会刷新后续请求的额度。现在通过交换循环位置解决，而非给整个任务计数器添加清零回调。

## 13.3 控制流与安全边界

```mermaid
flowchart TD
    Input[输入 / FollowUp] --> Reasoning[开始或继续工具循环]
    Reasoning --> Budget[新逻辑请求：局部 attempt = 1]
    Budget --> Request[guard / 组装 / 完整消费流 / 检查终态]
    Request -->|可恢复失败且有额度| Repair[必要的压缩或降级；检查进展]
    Repair --> Backoff[auto_retry_start / 可取消退避 / attempt 加一]
    Backoff --> Request
    Request -->|可接受回复| EndRetry[若发生过重试：auto_retry_end success]
    Request -->|不可恢复 / 耗尽 / 用户取消| Exit[结束重试；原有失败或取消收尾]
    Repair -->|压缩无进展| Exit
    EndRetry --> Commit[持久化回复；若有工具则执行并保存结果]
    Commit -->|还有工具轮次| Budget
    Commit -->|正常收尾| FollowUp{有追加输入且未耗尽工具预算?}
    FollowUp -->|有| Reasoning
    FollowUp -->|无| End[任务完成]
    Commit -->|提交失败| Exit
```

### 控制流伪代码

```text
run_inner(messages):                         # run.rs：Conversation
    发布 agent_start
    注入 steering / follow-up
    run_reasoning_loop(messages)
    正常完成且工具预算未耗尽 → 按原顺序消费追加输入，否则收尾

run_reasoning_loop(messages):                # reasoning_loop.rs：Reasoning
    保留 turn_index、final_text
    对每个逻辑请求：
        outcome = run_request(messages, turn_index)
        取消 → 保存 partial，按原规则返回 Interrupted
        回复可接受 → 提交正文 / 调度工具并保存结果
        提交失败 → 报错，不在请求循环里重新询问模型
        有后续请求且工具预算未耗尽 → 继续；否则正常收尾

run_request(messages, turn_index):           # request_retry.rs：Request Retry
    按当前执行模式选择额度；attempt = 1
    每次尝试：
        非首次 → auto_retry_start；退避（可被取消）
        run_request_once：guard → 校验结构 → 组装 → 收完整个流 → 终态检查
        成功 / 取消 / Fatal → 退出
        Retryable → 沿用压缩、无进展停止、附件降级、错误种类与额度限制
    若发出过 auto_retry_start：只发一次 auto_retry_end
    返回结果，不执行工具
```

**取消与副作用：** 失败流中的工具预览不能执行；此前已完成工具不会被恢复入口重放。原生写入保持 commit-aware 停止，不能取消丢弃整个 reasoning future。不能承诺模型以后永远不再要求相同外部操作。

### 13.3.1 VS Code 的五分钟恢复是另一个阶段

```text
任务失败 → 原 Retry / Resume 按钮显示 05:00
            ├─ 点击：立即执行原动作
            ├─ 到期：自动执行同一动作；再次失败再等5分钟
            └─ 取消 / 提交新输入：停止本次等待，手动按钮保留
```

扩展宿主按会话计时，网页只显示 `{ errorId, dueAt }`，不修改消息增量协议。只有本窗口收到的现场失败、且有效历史应用后确有恢复动作，才能排程；打开旧失败历史不会自动启动。到期复核空闲、权限、问题待答及同一错误动作，再复用原 `retry` / `resume` RPC。

- 所有具备任务恢复按钮的错误都适用，包括鉴权和余额错误；这不改变请求内的立即失败规则。
- 五分钟冷却**不限轮数**，因此整个任务的累计等待和费用没有总上限。
- 取消只影响当前等待；提交新输入、编辑重发、关闭会话等接管操作同步撤销它。只改草稿、切换会话、隐藏侧栏、重建页面不会取消或重置截止时间，后台恢复不抢焦点。
- 关闭窗口、宿主销毁、工作区替换、服务断连/重启后清理，不跨重启持久化，不替 observer 会话执行，也不自动回答权限问题。执行已经开始后用原 Stop。
- RPC 拒绝或 ACK 不确定不盲目再发；已接受后仅刷新失败不重开已点击按钮。新的现场失败才允许新一轮等待，包括复用同一 errorId 的 Resume 再次失败。

实现及验证入口见扩展 `docs/testing-layers.md`。

> **Aborted 语义**（见 [interrupt-and-cancellation.md](interrupt-and-cancellation.md)）：
> - `LoopError::Aborted { partial_text, partial_messages }` 由 `make_aborted` 构造，`partial_messages = messages[start_idx..]`，天然包含本轮已完成的 `tool_result` 与（若存在）已作为 partial push 的 assistant 消息。
> - `AgentLoop::run` 外层把 `Err(Aborted)` 转成 `AgentRunOutcome::Interrupted(AgentRunResult { new_messages, .. })`，与 `Completed` **共享同一持久化路径**；这是 T-003/T-004/T-017 的实现锚点。
> - 事件层：新增 `AgentEvent::Interrupted { session_id, partial_text_len, tool_results_count }`（wire：`agent_interrupted`），同时保留 `AgentEnd { error: "interrupted" }` 兼容旧订阅者。

---

## 13.4 消息类型设计

> **[已重构]** 原 `AgentMessage` 中间层已删除（见 `feature/collapse-to-chatmsg`），统一使用 `ChatMessage`（OpenAI wire format）。不同语义通过 `MessageKind` 字段区分，无需转换层。

```
ChatMessage（统一表示，直接发给 LLM）
┌──────────────────────────────────────────┐
│ role: user      + kind: Normal           │  普通用户消息
│ role: assistant                          │  助手回复
│ role: tool                               │  工具结果
│ role: system                             │  系统提示
│ role: user      + kind: Steering         │  内部 Steering 指令
│ role: user      + kind: CompactionSummary│  压缩摘要（替换旧消息）
└──────────────────────────────────────────┘
附加 #[serde(skip)] 字段: msg_id, kind, timestamp（不影响 wire format）
```

- **Steering**（`kind: MessageKind::Steering`）：标记内部指令，role 为 user 但不计入 turn 边界。
- **CompactionSummary**（`kind: MessageKind::CompactionSummary`）：压缩后的历史摘要，替换被压缩的消息段。
- **`ChatMessage::user_with_parts(parts)`**：仍然是普通 `role=user, kind=Normal`，只是 `content` 不是纯文本而是 `Parts` 数组。当前主要用于 `read` 命中图片 / PDF 后，把真正的 `InputImage` / `InputFile` 作为**紧跟 tool_result 的下一条 user 消息**注入；它不是 `follow_up_queue`，也不是 `steering_queue`。
- 由于内存表示与 wire format 统一，更换 LLM Provider 无需修改转换逻辑。

---

## 13.5 System Prompt 构建流水线

每次 LLM 调用前，System Prompt 按以下顺序拼装：

1. **基础身份**（固定）：如「你是一个 AI 编程助手，运行在 tomcat 环境中…」
2. **可用工具描述**（动态）：从工具注册中心生成，如 read_file、write_file、edit_file、execute_bash、list_dir 等。
3. **插件注入**：通过 `before_prompt_build` 等 Hook，插件注入额外 system 指令或上下文。
4. **会话级配置**：模型、温度、会话自定义 system prompt 追加。

---

## 13.6 事件发布时序

Agent Loop 与 [事件系统](events.md) 的对应关系：每个 AgentEvent 在 Loop 中的**精确发布时机**如下。

```text
agent_start
  ├─ turn_start                       ← 每个逻辑请求一次
  │    ├─ message_start/update/end    ← 一次流尝试
  │    ├─ [context_overflow_trim_*]   ← 必要时修复当前请求上下文
  │    ├─ [auto_retry_start → 再尝试]  ← 可重复，含本次退避信息
  │    └─ [auto_retry_end]            ← 该请求重试的统一出口
  ├─ 持久化可接受回复 / tool_execution_*、tool_call/tool_result
  ├─ turn_end
  ├─ 下一逻辑请求（新额度）……
  └─ agent_end                       ← 真正任务收尾，不跟随 auto_retry_end 提前结束
```

`auto_retry_end.success=true` 只表示本次模型请求恢复；提交或工具后续仍可能失败。只有发布过 `auto_retry_start` 才发布一次结束事件；取消、Fatal、额度耗尽和压缩无进展同样收口，保持事件与 transcript 口径一致。

方括号 `[]` 表示条件性发布（仅在对应场景触发）。

---

## 13.7 Steering、FollowUp 与 Abort

### 13.7.1 先看总图

```text
真实用户输入
  │
  ├─► ChatMessage::user("...")
  │    └─► 作为 initial_messages 进入 AgentLoop::run()
  │
  ├─► AgentLoop::follow_up("...")
  │    └─► follow_up_queue.push(ChatMessage::user(...))
  │         └─► turn 入口 / reasoning batch boundary 会 drain
  │              └─► 作为下一次 LLM 请求前的补充 user 消息
  │
  ├─► AgentLoop::steer("...")
  │    └─► steering_queue.push(ChatMessage::steering(...))
  │         ├─► run() 开头先用 inject_steering_messages(...) 注一次
  │         └─► 每个单独 tool 执行完后再检查一次
  │              └─► 若非空：记账 + append/persist + push，然后 break
  │                   跳过本轮剩余 tool_calls，直接进入下一轮 LLM
  │
  └─► read 图片 / PDF
       └─► tool_exec 返回 follow_up_parts
            └─► tool_dispatcher 先 push tool_result
                 再立刻 push ChatMessage::user_with_parts(parts)
                 （同一轮、同一条工具结果后面，不走 follow_up_queue）
```

**说人话**：这里其实有三条完全不同的“加消息”路径。`follow_up_queue` 是**回合与回合之间**加一条普通 user 消息；`steering_queue` 是**本轮工具列表中途插话并打断剩余工具**；`user_with_parts` 则是某些工具（现在主要是 `read` 图片 / PDF）为了把“真正的附件内容”补回对话，在**tool_result 后面立刻补一条普通 user 多模态消息**。

### 13.7.2 三类入口对照

| 入口 | 触发时机 | 写入载体 | 谁消费 | 会不会打断当前 tool list | 典型来源 |
|------|----------|----------|--------|---------------------------|----------|
| `steer(msg)` | Agent 工作中 | `steering_queue.push(ChatMessage::steering(msg))` | `inject_steering_messages(...)` | **会**。当前单个 tool 做完后就 break，剩余 tool_calls 不再执行 | 用户中途改方向、宿主主动插话 |
| `follow_up(msg)` | 一个 turn 正常收口后，或宿主补进新事实时 | `follow_up_queue.push(ChatMessage::user(msg))` | turn 入口 / reasoning batch boundary drain | **不会直接打断当前单个 tool**。它只影响“下一次发给 LLM 的消息集”，不回头改已执行完的 tool_calls | 用户追加一句话、后台任务完成的 synthetic 通知 |
| `user_with_parts(parts)` | 单个 tool_result 已生成之后 | 直接 `push_message(..., ChatMessage::user_with_parts(parts))` | 同一轮 `messages` | **不会单独打断**；但它必须出现在 tool_result 之后、steering break 之前 | `read` 命中图片 / PDF 后的 `follow_up_parts` |
| `abort()` / Ctrl+C | 任何 await 点 | `cancel_token.cancel()` | stream / tool await 的 `tokio::select!` | 相当于整个 turn 终止，不只是跳过剩余工具 | 用户中止 |

### 13.7.3 `user_with_parts` / `follow_up_parts` 是什么

- `ChatMessage::user_with_parts(parts)` 是一个构造函数：生成 **`role=user, kind=Normal`** 的多模态 user 消息，`content` 不是纯文本，而是 `Parts` 数组。
- `follow_up_parts` 不是队列，它只是工具执行返回值里带出来的一组 `ChatMessageContentPart`。当前主要由 `read` 工具在读到图片 / PDF 时产出：
  - 图片：生成 `InputImage`（可能是 file_id，也可能是 inline base64）。
  - PDF：生成 `InputFile`（可能是 file_id，也可能是 inline base64）。
- `tool_dispatcher` 的时序是固定的：**先 tool_result，占位说明先入消息；再 user_with_parts，把真正的图片 / PDF 实物补进去；最后才检查 steering_queue**。这样即使这次 tool round 被 steering 提前打断，下一轮 LLM 也仍能看到完整的“tool 占位句 + 实物附件”。

```text
execute_tool(read image/pdf)
  │
  ├─► ToolResultMessage("已读取附件，详见后续 parts")
  ├─► ChatMessage::user_with_parts([InputImage/InputFile ...])
  └─► steering_queue?
       ├─ yes -> inject steering + break
       └─ no  -> 继续执行剩余工具
```

### 13.7.4 队列与信号的当前实现口径

- `steering_queue`：`Arc<Mutex<Vec<ChatMessage>>>`。入口是 `AgentLoop::steer(&self, msg)`；消费口径统一走 `inject_steering_messages(...)`，也就是**先做 `ctx_state.on_message_appended(...)` 记账，再走 `push_message(...)` 分配 / 持久化 `msg_id`**，不再直接 `extend(q.drain(..))`。
- `follow_up_queue`：同样是 `Arc<Mutex<Vec<ChatMessage>>>`。`AgentLoop` 自带一份局部队列；`ChatContext` 还能通过 `with_shared_follow_up_queue(...)` 注入一份 session 级共享队列，用于后台 shell 完成通知等 auto-feed 场景。当前有两个消费点：**between-turns drain**，以及**非 steered 的 tool batch 收口后、下一次发起 LLM 请求前**的 reasoning batch-boundary drain。它仍不会打断正在执行的单个 tool，也不会回头改写已写入历史的 tool_result。
- `cancel_token`：`tokio_util::sync::CancellationToken`。每个 user turn 都会重建，避免上一轮的 cancel 污染下一轮。

### 13.7.5 真实输入优先于 queued follow-up

`follow_up_queue` 里的消息常常是 runtime 自己补进来的“后来发生的新事实”，典型例子就是后台 shell 完成通知。它们应该被模型看到，但**不应该压过用户刚刚真正输入的新 prompt**。

chat host 在同一轮里可能同时拿到两样东西：

- `readline()` 刚读到的真实用户输入
- 前面排队但还没消费的 synthetic follow-up

这时组装下一次请求的顺序必须是：**先 drain queued follow-up，再把真实用户输入放最后**。

```text
错误顺序
[real user input, queued follow-up]
                    │
                    └─ LLM 看到“最后一条 user 消息”成了后台通知
                       容易把“任务刚跑完”误当成当前主问题

正确顺序
[queued follow-up, real user input]
                  │
                  └─ LLM 先知道后台发生了什么，
                     但最后落点仍是用户刚刚真正想问的话
```

**说人话**：后台通知只是补充上下文，不是抢方向盘。它可以插进下一次请求里，但不能盖过用户刚刚敲下来的那句话。

### 13.7.6 `max_tool_rounds` 不能被 follow-up 绕过

一个很隐蔽的边界是：`max_tool_rounds` 限的是**单次 reasoning loop 内部**还能再做几轮 LLM/tool 往返，而 `follow_up_queue` 的消费点在**外层 conversation loop**。如果不把“这次是因为预算触顶才结束”的原因显式传出去，外层会把一次“预算耗尽返回”误判成“正常收尾”，随后又因为队列非空而再开一个 reasoning loop。

```text
错误路径
reasoning loop 达到 max_tool_rounds
        │
        └─ return Ok(...)
             │
             └─ conversation loop 看到 follow_up_queue 非空
                  └─ 再开一个 reasoning loop
                       -> 预算名义上结束，实际上继续跑

修正后
reasoning loop 达到 max_tool_rounds
        │
        └─ 标记 reasoning_turn_budget_exhausted = true
             │
             └─ conversation loop 先检查这个标记
                  ├─ true  -> 直接 Completed，不 drain follow_up_queue
                  └─ false -> 按正常 follow-up 逻辑续跑
```

**说人话**：`follow_up_queue` 只能帮 agent 接上“同一次预算里还没来得及看的新事实”，不能当成“再送一条免费命”的后门。否则 reviewer/verifier 这类本来该停下来的流程，会因为一条 follow-up 又偷偷多跑一轮。

### 13.7.7 与 current-tail guard 的关系

Steering / FollowUp / `user_with_parts` 都会改变“下一次发给 LLM 的 messages”。current-tail guard 在每次请求尝试前检查这一工作集，包括压缩或附件降级后的重发：

```text
单个 tool 执行完成
  -> push tool_result
  -> (可选) push user_with_parts
  -> (可选) inject steering 并 break
  -> emit TurnEnd
  -> maybe_reduce_before_next_llm(...)
  -> 下一次 llm.chat_stream(...)
```

也就是说，guard 看到的是**已经把 tool_result、附件 follow-up、steering 注入结果都算进去之后**的真实工作集。

---

## 13.8 工具循环检测（ToolLoopGuard）

防止 Agent 陷入无效重复调用的三道防线：

**第一道：Generic Repeat（通用重复）**

- 维护最近 HISTORY_SIZE（如 30）条工具调用的滑动窗口。
- 同一工具名 + 同参数 hash 重复超过 WARNING_THRESHOLD（如 10）→ 向 LLM 注入警告。
- 超过 CRITICAL_THRESHOLD（如 20）→ 强制终止当前工具循环。

**第二道：Ping Pong（乒乓检测）**

- 检测 A → B → A → B → … 的交替模式。
- 两工具交替超过一定次数且结果无实质变化 → 注入警告。

**第三道：Global Circuit Breaker（全局熔断）**

- 单次工具循环内工具调用总次数超过 GLOBAL_BREAKER（如 30）→ 强制终止。

处理策略：

- **Warning**：向 messages 注入 system 提醒，不中断执行。
- **Critical**：跳过工具执行，向 messages 注入错误说明，让 LLM 换方式。

---

## 13.9 上下文压缩（Compaction）

详见 [上下文管理技术方案](context-management.md)。以下为与 Agent Loop 交互的概要：

上下文管理采用四层防护（L0 tool_result 清理 → L1 异步预热 → L2 检查与应用 → L3 物理截断），由 ratio 水位线驱动。除既有三处交互外，阶段二还新增了一条 **mid-turn current-tail guard** 钩子，位置固定在 `tool_dispatcher` 返回、`TurnEnd` 发完、下一次 `llm.chat_stream(...)` 之前：

- **本轮 tool round 刚结束时（阶段二新增）**：`current_tail_guard::maybe_reduce_before_next_llm(...)`。顺序是 **precheck → apply 已完成历史 → history recompact → tail reduction waves → 必要时 single branch_summary collapse**；collapse 后会记录 `afterCollapse` 诊断，但**不会**在这里二次 fail-closed。详见 [`current-tail-aggregate-guard.md`](./current-tail-aggregate-guard.md)。

既有三处交互时机保持不变：

- **⑤ LLM 回复后**（user turn 完成，绝不阻塞 UI），顺序为：`preheat.try_restart_if_pending(...)`（ExhaustedPending → Running）→ `check_after_reply`（ratio >= 0.85，非阻塞 poll；成功 apply boundary 时其内部紧接执行 L0）→ `preheat.try_start()`（Idle → Running，条件满足时）→ `emit_context_metrics`
- **② 发起下一次 LLM 请求前**（下一个 user turn 进入时）：`preheat.try_restart_if_pending(...)` → `check_before_request`：`L2` 检查 CompactionSummary → 完成则 Boundary 切换并在内部执行 L0（ratio >= 0.98 时可化异步为同步等待）→ 成功时 `messages` 从更新后的 `ctx_state.messages` 重新拍平，并接回已持久化的本轮输入
- **③ 单请求内 API 返回 Context Overflow**：L3物理截断 + 重试（Request Retry）；发布 `ContextOverflowTrimStart` / `ContextOverflowTrimEnd`，不能靠压缩重置额度。

**context_metrics_update 发射节奏**（单次 `run_reasoning_loop`）：

- **首次**向 LLM 发起 `chat_stream` **之前**（`turn_index == 1`）发一条估算；每一次 provider `Usage` 到达时，`stream_handler` 立即发一条实测 metrics，发生在工具调用被调度之前；assistant **无** `tool_calls` 时 timing ⑤ 再发最终估算，若因 **`max_tool_rounds`** 耗尽退出则在该轮 `TurnEnd` 后再发估算。
- tool round 之间只有 current-tail guard 实际改写上下文时才追加一条估算 metrics；正常工具执行不会为每一轮额外刷事件。`providerUsageMeasured=true` 仅标记直接来自当前 `Usage` 的事件，所有其余快照均为 `false`。
- 水位消费者对 cold slot 的 `false` 保持未知；一旦已有 provider 实测基线，`false` 是压缩、提示词变化或预算重算后的刷新，必须更新显示而不能清空。

> 步骤编号（②③⑤）对应 [context-management.md §5.6](context-management.md) 的完整链路图。

---

## 13.10 错误分类与处理策略

| 类别 | 处理方 | 示例 | 行为 |
|------|--------|------|------|
| **可重试** | Request Retry | ContextOverflow、RateLimit、NetworkTimeout、ServerError(5xx)，以及 `server_error`、`upstream_http2_stream_error`、`upstream_stream_read_error` | 沿用分类、L3压缩及退避，消耗同一请求额度；明确账户/内容拒绝优先 |
| **致命** | 直接终止 | 401、非上下文类 400（如 invalid model）、ModelNotFound、MaxAttemptsExceeded | 发布 agent_end { error } |
| **工具错误** | 不终止 Loop | 文件不存在、权限拒绝、命令失败 | 作为 ToolResult 返回 LLM，由 LLM 调整策略 |

- **单一事实来源**：LLM 错误分类不再靠散落的裸字符串 `contains("503")` / `contains("timeout")` 猜测；统一由 `infra/error/llm.rs` 基于 `LlmErrorStage + http_status` 提供 `is_retryable_llm_error`、`llm_connect_or_network`、`is_context_overflow` 三个判定入口。
- **provider 重试边界**：`openai` / `openai-responses` 的 provider 层仅在**首个 delta 产出前**对流式建连故障做自动重试；一旦已经开始消费正文，后续 `BodyRead` / `Parse` 等错误必须原样上抛给 Request Retry，避免重复出字。
- **请求重试退避**：`[llm].agent_retry_base_delay_ms` 默认500ms；指数退避叠加±20% jitter，交互封顶8s、无人值守封顶30s；已有 `Retry-After` 优先。请求额度见13.2，扩展的五分钟冷却见13.3.1。

---

## 13.11 插件 Hook 点

Loop 在以下位置发布 ExtensionEvent，插件通过 `agent.on(event_name, callback)` 订阅：

- **before_prompt_build**：System Prompt 构建前，可注入 system 指令或上下文。
- **tool_execution_start** / **tool_execution_end**（见 [events.md](events.md)）：Agent 观察向，流式/UI。
- **tool_call** / **tool_result**（ExtensionEvent，见 [events.md](events.md)）：扩展钩子，执行前/后；与观察向事件名不同。
- **input**（见 [events.md](events.md)）：用户输入进入 Loop 前，可预处理。

约束（与 events.md 一致）：按注册顺序执行；单次回调错误不影响其他回调与主流程；Hook 可有超时，超时则跳过；异常通过 ExtensionError 记录。

---

## 13.12 与现有架构模块的关系

| Architecture 章节 | 与 Agent Loop 的关系 |
|------------------|----------------------|
| 2. 宿主核心能力层 | Loop 调用 Session、LLM、4 原语、工具注册中心 |
| 2.1 会话管理 | Loop 读写 Transcript，组装上下文 |
| 2.2 LLM 接入 | Loop 调用 chat_stream() |
| 2.3 4 原语 / 2.4 工具注册中心 | Loop 通过工具注册中心执行工具 |
| 8. 事件系统 | Loop 是 AgentEvent/ExtensionEvent 的主要发布者，发布时机见 13.6、13.11 |
| 9. 会话存储 | 合法消息边界即时append；请求重试不包住消息提交 |
| 11. 异步 Hostcall | 插件通过 Hostcall 调用 LLM/exec 时，底层走异步 submit/poll |
| 12. JS API 对齐 | 插件注册的自定义工具经工具注册中心进入 Loop 的 tool_definitions |

代码映射：

- `src/core/agent_loop/run.rs`：Conversation生命周期。
- `src/core/agent_loop/reasoning_loop.rs`：模型与工具循环、回复提交及工具轮次。
- `src/core/agent_loop/request_retry.rs`：单次请求组装与响应判定、有限重试与统一结束事件。
- `src/core/agent_loop/error_classifier.rs`：复用公共分类、上下文与附件恢复。
- `src/infra/error/llm.rs`：公共LLM故障语义。
- `src/api/chat/run_loop/mod.rs`：沿用现有配置装配，不另建预算来源。
