# 14. 多 Agent 架构设计

本文为 [Architecture](../Architecture.md) 中第 14 节的详细设计，总览见主文档。

---

## 14.0 设计参考与竞品对比

在确定本项目的多 Agent 架构之前，对主流 coding agent 与 multi-agent 框架进行了系统调研，聚焦「多 Agent 编排」这一具体能力点。

### 14.0.1 调研对象与关键结论

| 项目 | 类型 | 多 Agent 触发机制 | 上下文隔离 | 完成通知 | 深度限制 | 关键设计亮点 |
|------|------|-------------------|------------|----------|----------|--------------|
| **openclaw**（本地） | coding agent | `sessions_spawn` 工具调用 → Gateway 异步排队 | 完全隔离（仅 system prompt 注入任务） | 双轨：`agent.wait` 长轮询 + 进程内 `onAgentEvent` push | `spawnDepth` 写入 session store，`maxChildrenPerAgent` 并发上限 | SubagentRegistry（进程级 Map + 磁盘持久化）；`runId` 精确路由事件；`spawnedBy` 字段链式记录父子关系 |
| **claude-code** | Anthropic 官方 CLI | `Agent` 工具（旧称 `Task`），LLM 自主决定调用 | 完全隔离（全新 context window，只返回 final message） | 同步阻塞 await | **硬性限制 2 层**（子不可再派发） | 子 Agent 有独立 agentId + sessionId，支持 `resume: sessionId` 跨次恢复；transcript 独立不受父 Agent compaction 影响 |
| **aider** | 交互式 coding agent | **不支持** | — | — | — | 单 Agent 结构，社区提议未被采纳 |
| **SWE-agent** | 自动化软件工程 | **无父子派发**，通过 SWE-ReX 并行启动独立实例 | 完全隔离（各自独立 Docker shell + HistoryProcessor） | 无相互通知，外部调度层汇总 | 无递归结构 | 强调水平扩展（数百并行实例），而非树形编排 |
| **AutoGen v0.4** | 多 Agent 框架 | 事件驱动 pub/sub（GroupChatManager 发布 `RequestToSpeak`） | **默认共享**（GroupChat 广播全体） | 异步事件推送（非阻塞） | 无硬限，靠 termination condition 终止 | `CancellationToken` 显式级联取消；`SingleThreadedAgentRuntime` 进程级注册表 |
| **LangGraph** | 有向图 agent 编排 | 子图（subgraph）作为父图节点，到节点时 `invoke(state)` | 可配置（同/异 schema 决定隔离程度） | 同步 invoke + streaming；支持 `interrupt()` 暂停人工介入 | 软限 `recursion_limit`（默认 25，可调），超限抛 `GraphRecursionError` | Checkpointer 系统（`thread_id` 键）支持状态持久化与跨次恢复；子图以节点名为 namespace 隔离状态 |
| **CrewAI** | 角色团队 | 角色委派（manager LLM 决策 → `allow_delegation=True`） | 默认共享 Crew Memory（可 scoped 隔离） | 同步串行（前一任务输出为后一任务 context） | `max_iterations`（默认 15 次/agent）| Crew/Agent/Task 三层模型；A2A 协议支持委派远程 Agent |
| **bolt.diy** | 全栈生成 agent | **尚未实现**（标注 `In Progress`） | — | — | — | 当前单 Agent，MCP 工具扩展在先 |

### 14.0.2 三大编排模式对比

```
工具调用派发             有向图 (DAG)              角色团队 (GroupChat)
   (Tool Dispatch)      (StateGraph)             (Role Team)

  父 Agent                 ┌──┐                  Agent A
     │                     │  │→ SubGraph         ↕  ↕  ↕
  tool_call ──→ 子 Agent   └──┘                  Agent B  Agent C
     ↑                                            (共享消息广播)
  ToolResult
```

| 维度 | 工具调用派发 | 有向图 | 角色团队 |
|------|-------------|--------|---------|
| **代表项目** | claude-code, openclaw | LangGraph | AutoGen GroupChat, CrewAI |
| **触发方式** | LLM 自主决定工具调用 | 确定性图节点跳转 | Manager 或轮转算法选择发言者 |
| **上下文** | 强隔离（fresh context window） | 可配置（隔离/共享） | 默认共享（广播全体） |
| **并发能力** | 天然支持多子 Agent 并发 | 图拓扑决定并发度 | 顺序轮转（一次一个发言者） |
| **完成通知** | 同步阻塞 await（或 push 增强） | 同步 invoke + streaming | 异步事件推送 |
| **abort 传播** | 无自动级联（openclaw 需显式调用） | 异常链传播 | CancellationToken 显式级联 |
| **实现复杂度** | 低（声明式工具定义） | 中（State + Edges） | 高（角色/选择器/终止条件） |
| **适合场景** | 独立子任务并行，不需互通 | 复杂工作流，有明确前后依赖 | 多角色协作讨论、审查迭代 |

### 14.0.3 本项目选型理由

**采用「工具调用派发」作为主干，参考 LangGraph 的深度限制思路，参考 openclaw 的 Registry + 双轨通知机制。**

理由如下：

1. **Rust 类型安全天然契合工具调用派发**：每个子 Agent 的输入/输出建模为 `AgentRunResult`（strongly-typed struct），枚举可自然表达 `Success(String) | Interrupted | Timeout`，比动态 pub/sub 消息体更安全。

2. **与现有 AgentLoop 架构零冲突**：`dispatch_agent` 作为普通工具注入 `tool_definitions`，AgentLoop 自身无需修改，符合开闭原则。

3. **避免 GroupChat 的顺序瓶颈**：coding 任务的并发子任务（多文件并行修改、并行测试）需要 `tokio::spawn` 并发多子 Agent，GroupChat 的单发言者轮转模型不适合。

4. **深度软限 + 并发数上限双保险**：openclaw 用 `spawnDepth` + `maxChildrenPerAgent` 双重保护的方案经过生产验证，本项目直接复用该思路。

5. **升级路径清晰**：Phase 2 可以在工具调用派发的基础上叠加有向图（将 `dispatch_agent` 调用序列建模为 DAG），Phase 3 若需要协作审查可引入 GroupChat 作为可选编排策略，无需推翻现有设计。

---

## 14.1 概述与设计目标

多 Agent 能力覆盖两个相互独立的维度：

- **维度 A — 多会话并发（Session-Level Concurrency）**：多个不同 session 各自对应一个独立的 `AgentLoop` 实例，共享同一进程中的 `LlmProvider`、`PrimitiveExecutor`、`EventBus` 等基础设施，彼此上下文完全隔离，各自持有独立的 `abort_signal` / `steering_queue`。
- **维度 B — 主-子 Agent 编排（Agent Hierarchy / Orchestration）**：主 Agent 通过注册一个 LLM 可调用的 `dispatch_agent` 工具，将子任务委托给独立的子 `AgentLoop` 实例执行；主 Agent 同步等待子任务完成后得到结果，作为 `ToolResult` 继续工作。

### 设计目标

1. **最小侵入性**：`AgentLoop` 本身不修改；多 Agent 能力以新增组件（Registry、工具）的方式叠加。
2. **强上下文隔离**：子 Agent 不继承父 Agent 的 messages 历史，仅通过任务描述传递意图。
3. **安全防护**：嵌套深度限制 + 并发实例数上限，防止 LLM 幻觉导致无限递归或资源耗尽。
4. **级联中止**：父 Agent 中止时，自动传播到所有子 Agent（参考 AutoGen `CancellationToken` 思路）。
5. **可观测**：子 Agent 的启动与结束均发布专用 AgentEvent，父子关系可通过 session_id 前缀追踪。
6. **分阶段落地**：Phase 1（现状）→ Phase 2（多会话并发）→ Phase 3（主-子编排），每阶段均可独立上线。

---

## 14.2 术语

| 术语 | 说明 |
|------|------|
| **AgentInstance** | 一个 `AgentLoop` + 关联的 `session_id` + `abort_signal` 的逻辑单元。 |
| **AgentRegistry** | 进程级注册表，维护所有活跃 `AgentInstance`，以 `session_id` 为 key，参考 openclaw `subagentRuns: Map<runId, SubagentRunRecord>`。 |
| **AgentHandle** | 注册表中单个实例的元数据记录（`session_id`、`abort_signal`、`spawn_depth`、`parent_session_id`）。 |
| **RootAgent** | 由用户/API 直接发起的 `AgentLoop`，`spawn_depth=0`，`parent_session_id=None`。 |
| **SubAgent / ChildAgent** | 由主 Agent 通过 `dispatch_agent` 工具创建的 `AgentLoop`，`spawn_depth=parent+1`。 |
| **dispatch_agent** | 注册到 `tool_definitions` 中的 LLM 可调用工具，触发子 Agent 创建与执行。 |
| **spawn_depth** | 当前 `AgentInstance` 距根 Agent 的嵌套层数，防止无限递归（参考 openclaw `spawnDepth`、LangGraph `recursion_limit`）。 |
| **MAX_SPAWN_DEPTH** | 全局可配置的最大嵌套深度，默认值 `3`；超限时 `dispatch_agent` 返回错误 ToolResult，不终止主 Agent。 |
| **MAX_CONCURRENT_AGENTS** | 进程级最大并发 `AgentInstance` 数，默认值 `16`。 |
| **CascadeAbort** | 父 Agent 中止时通过 Registry 遍历所有子 Agent 并触发其 `abort_signal`（参考 AutoGen CancellationToken）。 |

---

## 14.3 维度 A：多会话并发

### 14.3.1 设计原则

- `AgentLoop` 无全局单例，可按 `session_id` 独立构造多个，天然支持并发。
- `AgentLoopConfig.session_id` 已存在，`AgentEvent` 均携带此字段；多实例的事件在同一 `EventBus` 上以 `session_id` 区分，订阅方按需过滤。
- 共享资源（`LlmProvider`、`PrimitiveExecutor`、`EventBus`）均以 `Arc<dyn ...>` 注入，内部按需持有线程安全结构，多实例并发安全。
- 各实例的 `abort_signal: Arc<AtomicBool>` 独立，互不影响。

### 14.3.2 AgentRegistry（进程级）

新增 `src/core/agent_registry.rs`（Phase 2 实现）：

```rust
/// 进程级 AgentLoop 注册表，以 session_id 为 key。
/// 参考 openclaw subagent-registry.ts 中 subagentRuns: Map<runId, SubagentRunRecord>。
pub struct AgentRegistry {
    agents: DashMap<String, Arc<AgentHandle>>,   // session_id -> handle
}

pub struct AgentHandle {
    pub session_id:        String,
    pub abort_signal:      Arc<AtomicBool>,
    pub spawn_depth:       u32,
    pub parent_session_id: Option<String>,
}
```

核心接口：

| 方法 | 说明 |
|------|------|
| `register(session_id, handle) -> Result<()>` | 注册新实例；同一 `session_id` 重复注册返回 `Err`（幂等保护）。 |
| `unregister(session_id)` | 实例结束后注销。 |
| `abort(session_id)` | 定向中止某个实例，设置其 `abort_signal`。 |
| `abort_children(parent_session_id)` | 级联中止指定父 session 下的所有子 Agent（CascadeAbort）。 |
| `abort_all()` | 进程退出时全部中止。 |
| `active_count() -> usize` | 当前活跃实例数，供 `MAX_CONCURRENT_AGENTS` 上限检查。 |
| `get(session_id) -> Option<Arc<AgentHandle>>` | 查询指定实例元数据。 |

### 14.3.3 AgentLoopConfig 扩展

在现有 [`src/core/agent_loop.rs`](../../../src/core/agent_loop.rs) 的 `AgentLoopConfig` 中新增两个字段（Phase 2 实现时补充）：

```rust
pub struct AgentLoopConfig {
    // ... 已有字段 ...
    /// 父 session；RootAgent 为 None。
    pub parent_session_id: Option<String>,
    /// 当前嵌套层数；RootAgent 为 0。递归检查上限为 MAX_SPAWN_DEPTH。
    pub spawn_depth: u32,
}
```

### 14.3.4 并发约束

- 同一 `session_id` 不可重复注册（`register` 返回 `Err(AlreadyRegistered)`）。
- 进程级最大并发数上限 `MAX_CONCURRENT_AGENTS`（默认 `16`），超限时拒绝创建并返回明确错误。
- 每个父 session 的最大活跃子 Agent 数 `MAX_CHILDREN_PER_AGENT`（默认 `5`），参考 openclaw `maxChildrenPerAgent` 设计。

---

## 14.4 维度 B：主-子 Agent 编排

### 14.4.1 触发机制

主 Agent 的 LLM 通过调用 `dispatch_agent` 工具触发子 Agent 创建。该工具以 `tool_definition` JSON 的形式在构造主 AgentLoop 时注入 `config.tool_definitions`。

**工具 JSON Schema：**

```json
{
  "name": "dispatch_agent",
  "description": "派发子任务给一个独立的 SubAgent 执行。SubAgent 继承父 Agent 的工作目录与权限配置，拥有完全独立的 LLM 上下文（不继承对话历史），完成后将最终回答作为工具结果返回。适用于可并行或可委托的独立子任务。",
  "parameters": {
    "type": "object",
    "properties": {
      "task": {
        "type": "string",
        "description": "子任务的完整描述（含必要上下文），子 Agent 将以此为初始消息开始工作"
      },
      "model": {
        "type": "string",
        "description": "子 Agent 使用的模型，不填则继承父 Agent 的模型配置"
      },
      "max_turns": {
        "type": "integer",
        "description": "子 Agent 最大 reasoning 轮次，默认 20"
      }
    },
    "required": ["task"]
  }
}
```

> **设计决策**：与 claude-code 的 `Agent` 工具对齐（只需 `task` 字符串，子 Agent 独立运行），相比 openclaw 的 `sessions_spawn` 参数更简洁；不传入父 Agent messages 历史，避免 token 重复消耗。

### 14.4.2 子 Agent 创建与执行流

```
主 AgentLoop (session_id="S1", spawn_depth=0)
│
│   LLM 返回 tool_call: dispatch_agent { task: "分析并修复 src/foo.rs 中的 panic" }
│
▼
execute_tool("dispatch_agent", args)
│
├─ [Guard 1] 检查 spawn_depth >= MAX_SPAWN_DEPTH (默认 3)
│   → 超限：返回错误 ToolResult，说明原因，不终止主 Agent
│
├─ [Guard 2] 检查 registry.active_count() >= MAX_CONCURRENT_AGENTS
│   → 超限：返回错误 ToolResult
│
├─ [Guard 3] 检查当前父 session 下活跃子数 >= MAX_CHILDREN_PER_AGENT
│   → 超限：返回错误 ToolResult
│
├─ 生成子 session_id = "{parent_session_id}:sub:{uuid_v4}"
│
├─ 构造子 AgentLoopConfig {
│     session_id:        "S1:sub:<uuid>",
│     parent_session_id: Some("S1"),
│     spawn_depth:       self.config.spawn_depth + 1,    // = 1
│     model:             args.model.unwrap_or(self.config.model.clone()),
│     max_turns:         args.max_turns.unwrap_or(20),
│     tool_definitions:  父 Agent 工具集（排除 dispatch_agent，防止无限递归）,
│   }
│
├─ 构造子 AgentLoop，注入相同的 Arc<dyn LlmProvider> / Arc<dyn PrimitiveExecutor> / Arc<EventBus>
│
├─ registry.register(child_session_id, child_handle)
│
├─ 发布 AgentEvent::SubAgentStart { session_id, parent_session_id, task, spawn_depth }
│
├─ child_loop.run(vec![ AgentMessage::User { text: args.task } ]).await
│
│   ... 子 Agent 独立运行，所有 AgentEvent 以 session_id="S1:sub:<uuid>" 在 EventBus 发布 ...
│
├─ registry.unregister(child_session_id)
│
├─ 发布 AgentEvent::SubAgentEnd { session_id, parent_session_id, result, is_error }
│
└─ 返回 ToolResult { content: child_result.final_text, is_error: false }
```

### 14.4.3 上下文隔离原则

参考 claude-code 的强隔离设计（只返回 final message）和 openclaw 的 system prompt 注入方式：

- **不继承 messages 历史**：子 Agent 只接收 `args.task` 字符串，外加宿主注入的系统 prompt（含子 Agent 身份描述、`spawn_depth`、`parent_session_id`）。
- **独立 Transcript**：子 Agent session 的 transcript 写入 `agents/default/sessions/{child_session_id}.jsonl`，或通过配置设为不落盘（ephemeral）。
- **工具集独立**：子 Agent 默认继承父 Agent 的工具集，但**排除** `dispatch_agent` 工具（防止无限递归）。如需允许孙 Agent，显式在 `args` 中传入 `allow_sub_dispatch: true` 并受 `spawn_depth` 检查约束。

### 14.4.4 嵌套深度限制

综合 openclaw `spawnDepth + maxSpawnDepth` 与 LangGraph `recursion_limit` 的设计：

- `AgentLoopConfig.spawn_depth` 在构造子 Agent 时以 `parent.spawn_depth + 1` 传入。
- 执行 `dispatch_agent` 时，若 `self.config.spawn_depth >= MAX_SPAWN_DEPTH`（默认 `3`），拒绝并向 LLM 返回错误 ToolResult，附带说明文本，不终止主 Agent（LLM 可自行选择其他工具继续）。
- `MAX_SPAWN_DEPTH` 可通过 `AgentLoopConfig.max_spawn_depth` 字段覆盖（字段级），也可通过全局 config 配置（进程级）。

### 14.4.5 CascadeAbort（级联中止）

参考 AutoGen 的 `CancellationToken` 机制，对 openclaw 「不自动级联」的缺陷做改进：

- 每个 `AgentHandle` 持有独立的 `abort_signal: Arc<AtomicBool>`。
- 当父 Agent 收到 Abort（用户 Ctrl+C 或 API 调用）时，在设置自身 `abort_signal` 的同时，调用 `registry.abort_children(parent_session_id)`，该方法遍历所有 `parent_session_id` 匹配的子 Agent 并设置其 `abort_signal`。
- 子 Agent 在 reasoning loop 的工具间隙检查 `abort_signal`，发现置位后按 Abort 语义终止并发布 `agent_end(interrupted)`。
- 级联传播是**深度优先**的：子 Agent abort 时同样触发 `registry.abort_children(child_session_id)`，确保孙 Agent 也被中止。

---

## 14.5 事件系统扩展

在现有 [`src/infra/events.rs`](../../../src/infra/events.rs) 的 `AgentEvent` 枚举上新增两个变体（Phase 3 实现时补充）：

```rust
/// 子 Agent 启动时发布，用于 UI 展示嵌套任务树与进度追踪。
SubAgentStart {
    session_id:        String,   // 子 Agent 的 session_id
    parent_session_id: String,   // 父 Agent 的 session_id
    task:              String,   // 子任务描述（截断到合理长度）
    spawn_depth:       u32,      // 嵌套层数
},

/// 子 Agent 结束时发布（无论成功、失败、中止）。
SubAgentEnd {
    session_id:        String,
    parent_session_id: String,
    result:            String,   // final_text 或错误描述
    is_error:          bool,
    elapsed_ms:        u64,      // 子 Agent 执行耗时
},
```

**父子事件追踪**：

- `EventBus` 上所有事件均携带 `session_id`（已有），订阅方通过 `session_id.starts_with(parent_session_id)` 即可过滤整棵子树的事件。
- 子 Agent 的 `ThinkingStart`、`ToolCallStart`、`ToolCallEnd` 等流式事件以子 `session_id` 发布，UI 层可按 `session_id` 分组展示嵌套进度树。

---

## 14.6 与现有设计的关系

| 章节 | 关系 |
|------|------|
| **第 8 节（事件系统）** | 新增 `SubAgentStart` / `SubAgentEnd` 两个 `AgentEvent` 变体；EventBus 共享实例，以 `session_id` 区分事件来源；订阅方通过前缀过滤追踪父子关系。 |
| **第 9 节（会话存储）** | `SessionEntry` 预留注释「channel/agent 相关字段供三期多 channel 使用」；本节给出 `parent_session_id` 的具体语义与写入时机（子 Agent 创建时 patch）。 |
| **第 10 节（工作目录）** | 子 Agent session 的 transcript 路径沿用 `agents/<agentId>/sessions/` 布局，以 `child_session_id`（含冒号，需 URL encode 或替换为下划线）作为文件名。 |
| **第 13 节（Agent Loop）** | `AgentLoop` 本身不修改；`dispatch_agent` 工具作为普通工具注入 `tool_definitions`；`AgentRegistry` 是新增的进程级管理层；`AgentLoopConfig` 新增 `parent_session_id` / `spawn_depth` 两个字段。 |

---

## 14.7 数据流图

### 14.7.1 多会话并发（维度 A）

```
                 ┌─────────────────────────────────────────────────────────────┐
                 │                       进程                                   │
  用户/API        │                                                               │
   Session A ───▶│  AgentLoop(S-A)        AgentLoop(S-B)        AgentLoop(S-C)  │
   Session B ───▶│  abort_signal_a        abort_signal_b        abort_signal_c  │
   Session C ───▶│       │                     │                     │          │
                 │       └─────────┬───────────┘─────────────────────┘          │
                 │                 ▼                                              │
                 │         AgentRegistry                                          │
                 │    { S-A: handle_a, S-B: handle_b, S-C: handle_c }           │
                 │                 │                                              │
                 │     ┌───────────┼───────────┐                                │
                 │     ▼           ▼           ▼                                │
                 │  LlmProvider  EventBus  PrimitiveExecutor    ← 共享 Arc<>    │
                 └─────────────────────────────────────────────────────────────┘
```

### 14.7.2 主-子 Agent 编排（维度 B）

```
  主 AgentLoop (S1, depth=0)
  ┌─────────────────────────────────────────┐
  │  [thinking]  LLM 决定调用 dispatch_agent  │
  │      │                                   │
  │  execute_tool("dispatch_agent", ...)     │
  │      │                                   │
  │      │  Guard: depth / concurrent check  │
  │      │                                   │
  │      ▼                                   │
  │  ┌───────────────────────────────────┐   │
  │  │  子 AgentLoop (S1:sub:uuid, depth=1)│   │
  │  │  [thinking→tool→result→...→done]  │   │
  │  │  events: S1:sub:uuid              │   │
  │  └───────────────────────────────────┘   │
  │      │                                   │
  │      ▼  ToolResult { final_text }        │
  │  [thinking]  LLM 继续基于结果工作         │
  └─────────────────────────────────────────┘

  EventBus 事件流（按 session_id 区分）：
  S1       → AgentStart, ThinkingStart, ToolCallStart(dispatch_agent), SubAgentStart, SubAgentEnd, ToolCallEnd, ...
  S1:sub:* → AgentStart, ThinkingStart, ToolCallStart(...), ToolCallEnd, AgentEnd
```

---

## 14.8 MVP 降级与实施顺序

多 Agent 能力分三阶段落地，每阶段均可独立上线：

### Phase 1（当前，已完成）

- 单 Agent 运行，`AgentLoopConfig` 已含 `session_id`。
- 技术上可通过外部代码多次构造 `AgentLoop` 实例，但无注册表管理，无安全防护。

### Phase 2（多会话并发）

**新增文件**：`src/core/agent_registry.rs`

**修改文件**：
- `src/core/agent_loop.rs`：`AgentLoopConfig` 新增 `parent_session_id: Option<String>` 和 `spawn_depth: u32`（默认 0）。
- `src/core/mod.rs`：导出 `AgentRegistry`、`AgentHandle`。
- `src/lib.rs`：导出公共接口。

**能力**：CLI/API 层支持多 session 并发，事件订阅可按 `session_id` 过滤，`AgentRegistry` 提供全局实例管理与 abort 接口。

### Phase 3（主-子 Agent 编排）

**新增文件**：`src/core/dispatch_agent_tool.rs`（实现 `dispatch_agent` 工具的执行逻辑）

**修改文件**：
- `src/infra/events.rs`：新增 `SubAgentStart` / `SubAgentEnd` 两个 `AgentEvent` 变体。
- `src/core/agent_loop.rs`：`execute_tool` 方法增加 `dispatch_agent` 分支，调用 `dispatch_agent_tool::run()`。

**能力**：LLM 可通过 `dispatch_agent` 工具委托子任务，支持嵌套深度检查、并发数上限、级联 Abort、SubAgent 事件发布。

---

## 14.9 安全性与资源防护

| 风险 | 防护措施 |
|------|---------|
| LLM 幻觉导致无限派发子 Agent | `MAX_SPAWN_DEPTH`（深度限制） + `MAX_CHILDREN_PER_AGENT`（单父并发限制） |
| 大量子 Agent 耗尽内存/CPU | `MAX_CONCURRENT_AGENTS`（进程级并发上限），超限返回错误 ToolResult |
| 父 Agent abort 后子 Agent 仍在消耗资源 | `CascadeAbort`：深度优先遍历 Registry，逐一设置 `abort_signal` |
| 子 session_id 文件名非法字符（含冒号） | transcript 路径使用 `child_session_id.replace(':', "_")` 生成安全文件名 |
| 子 Agent 意外 panic 影响父 Agent | `child_loop.run().await` 包裹在 `catch_unwind` 或 `tokio::spawn + JoinHandle` 中，panic 转化为 `AgentRunResult::Err`，不传播到父 Agent |
