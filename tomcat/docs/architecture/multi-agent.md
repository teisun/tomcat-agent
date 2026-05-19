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
| **dispatch_agent** | 注册到 `tool_definitions` 中的 LLM 可调用工具，触发子 Agent 创建与执行；schema 见 §14.4.1，含 `task` / `subagent_type` / `role` / `allowed_tools` / `model` / `max_turns`。 |
| **subagent_type** | `dispatch_agent` 与 `AgentLoopConfig` 上的子 Agent 画像枚举（`general` / `explore` / `shell` / `cursor-guide`），决定 system prompt 模板与默认 `allowed_tools` 集；与 cc-fork-01 `Task.subagent_type` 同名。 |
| **role** | 子 Agent 派生角色（`leaf` / `orchestrator`），决定子 Agent 自身能否再调用 `dispatch_agent`；对标 hermes `delegate_task.role`；取代旧的 `allow_sub_dispatch: bool`。 |
| **allowed_tools** | 子 Agent 工具白名单（数组），与父 catalog 取交集后生效；省略时取 `subagent_type` 默认集；与 [`plan-runtime.md`](./plan-runtime.md) reviewer 的 `allowed_tools` 字段同名同义。 |
| **internal subagent dispatch** | 内部 Rust API 形态的子 Agent 派发入口（不进 catalog），与 LLM-facing `dispatch_agent` 互补，复用 §14 基础设施；对标 codex `run_codex_thread_one_shot`；reviewer 即此路径消费方，详见 §14.6.1 与 [`tools/reviewer.md`](./tools/reviewer.md)。 |
| **spawn_depth** | 当前 `AgentInstance` 距根 Agent 的嵌套层数，防止无限递归（参考 openclaw `spawnDepth`、LangGraph `recursion_limit`）。 |
| **MAX_SPAWN_DEPTH** | 全局可配置的最大嵌套深度，默认值 `2`（与 `agent_registry` 常量一致）；超限时 `dispatch_agent` 返回错误 ToolResult，不终止主 Agent。 |
| **MAX_CONCURRENT_AGENTS** | 进程级最大并发 `AgentInstance` 数，默认值 `16`。 |
| **CascadeAbort** | 父 Agent 中止时通过 Registry 遍历所有子 Agent 并触发其 `abort_signal`（参考 AutoGen CancellationToken）。 |

---

## 14.3 维度 A：多会话并发

### 14.3.0 落地选型决策表（MA1–MA12）

> 与 [`tools/reviewer.md` §4.1](./tools/reviewer.md#41-落地选型决策表) 同格式（**维度 / 取舍 / 拒因 / 说人话**）。覆盖 §14.3（多会话并发）+ §14.4（主-子编排）的全局选型；§14.6.1 internal subagent dispatch 与 §14.7 数据流图都是这张表的展开。

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| **MA1 注册表层级** | 进程级 `AgentRegistry`（`session_id` 为 key） | 塞进 `ChatContext` → 跨会话限流 / `CascadeAbort` 都做不了；与文档 §14.3.2 / §14.7.1 既有图示一致 | 全楼一张登记表。 |
| **MA2 会话壳层级** | `ChatContextRegistry`（`session_key` → `Arc<ChatContext>`）管 [`TodoRuntime`](./plan-runtime.md#62-todoruntimechat-路径) / [`PlanRuntime`](./plan-runtime.md#63-planruntimeplanexec-路径) / mode | 与 Agent 执行树正交；混在一起会让 Phase 2 的多会话路由失语义 | 每间聊天室一个管家。 |
| **MA3 Handle 内容** | 只存 `session_id / parent_session_id / spawn_depth / abort_signal`（详见 §14.3.2 `AgentHandle`），**不存** `AgentLoop` | `AgentLoop` 每轮新建，进 `DashMap` 会泄漏且析构难；与「子 loop 跑完即 drop」语义冲突 | 登记工牌不登记工人本体。 |
| **MA4 父子关系登记** | 子 `AgentHandle.parent_session_id = Some(parent)`；`abort_children(parent_id)` **扫表反查**（对标 openclaw `spawnedBy`） | 维护 `children: Vec` 要 register/unregister 双写一致，遇到 panic / abort 半步极易漂移 | 只记上级是谁，查孩子靠扫表。 |
| **MA5 子 loop 启动点** | **两个唯一入口**：①  `dispatch_agent_tool::run`（LLM 工具，§14.4）；② `AgentRegistry::spawn_subagent_internal`（内部 Rust API，§14.6.1） | 在 LLM 回调或工具内部散落 `AgentLoop::new` 难审计、难做 Guard | 统一两个入口函数，其它地方禁止 new。 |
| **MA6 子 loop 运行方式** | 调用方 **同步 `await child_loop.run(...)`**；并发体现在多 session 各自有父 loop | 异步 fire-and-forget 会让父 Agent 拿不到 `ToolResult` / `ReviewSummary` 盲继续 | 派出去就要等结果。 |
| **MA7 共享基础设施** | `Arc<dyn LlmProvider>` / `Arc<dyn PrimitiveExecutor>` / `Arc<EventBus>` **进程级共享**（与 §14.3.1、§14.7.1 一致） | 每 loop 一套：重复 HTTP/MCP 连接、内存/FD 线性涨、全局限流难统一；与 [`ChatContext`](../../src/api/chat/mod.rs) 注入模型冲突；隔离应靠 `session_id` + 独立 `ContextState` + 独立 `abort_signal` | 水电共用，房间隔开。 |
| **MA8 上下文隔离** | 子 loop **不**继承父 messages；仅 `task` + subagent system prompt（§14.4.3） | 继承历史会爆窗 / 串味，与 claude-code / codex 共识相悖 | 子 Agent 白纸进场。 |
| **MA9 深度 / 并发上限** | `spawn_depth + MAX_SPAWN_DEPTH` + `MAX_CHILDREN_PER_AGENT` + `MAX_CONCURRENT_AGENTS`（§14.3.4 / §14.4.4） | 任何一个缺位都会被 LLM 幻觉打穿 | 楼层与人数双限。 |
| **MA10 CascadeAbort** | 父 `abort` → `registry.abort_children(parent_session_id)` 深度优先（§14.4.5） | openclaw 不自动级联踩过坑 | 父停子必停。 |
| **MA11 internal vs LLM dispatch** | 共用 `AgentRegistry` / `spawn_depth` / `CascadeAbort` / `SubAgentStart/End` 事件；**不**共用 schema / catalog（详见 §14.6.1） | reviewer 权限不能挤进 `dispatch_agent` 的 `subagent_type` 枚举 | 一条登记处，两种进门。 |
| **MA12 transcript** | 子 `session_id` 独立 `agents/<agentId>/sessions/<child>.jsonl`；含 `:` 时文件名 `replace(':', '_')`（§14.6 / §14.9） | 子事件混进父 transcript → 回放与 compaction 全乱 | 各记各的账。 |

**说人话（§14.3.0 总览）**：先把「谁登记、登记什么、子 loop 在哪起、共享什么、隔离什么、怎么停」一次性钉死，§14.3 起的所有结构图、§14.4 起的所有流程图都只是把这些选项画出来。

---

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

#### 14.3.2.1 `ChatContextRegistry` vs `AgentRegistry` 分工

两张表都是「进程级 `HashMap`」，但 key、value、生命周期与责任**完全不同**：

| 维度 | `ChatContextRegistry`（详见 [`plan-runtime.md` §6.4](./plan-runtime.md#64-chatcontext-持有关系)） | `AgentRegistry`（§14.3.2） |
|------|---------------------------------|---------------------------|
| Key | `session_key`（持久 chat 会话身份，如 `"agent:main:main"`） | `session_id`（运行时实例 id，含 `:sub:<uuid>` 前缀） |
| Value | `Arc<ChatContext>`（含 `TodoRuntime` / `PlanRuntime` / 共享 `Arc` 服务 / `root_session_id`） | `Arc<AgentHandle>`（仅控制面元数据 + `abort_signal`） |
| 生命周期 | 与 chat session 同寿（启动 → 退出） | **跑时注册，结束注销**（与 `AgentLoop::run` 同寿） |
| 关心的事 | mode 切换、PlanFile/TodoFile IO、面板投影、`/plan` 命令 | 并发上限、`spawn_depth`、`CascadeAbort`、`SubAgentStart/End` 路由 |
| 是否持有 `AgentLoop` | **否**（每轮新建） | **否**（仅持 Handle，Loop 在 `dispatch_agent_tool::run` / `spawn_subagent_internal` 栈帧内拥有） |

**关键不变量**：

- 一个 `session_key` 对应**唯一** `ChatContext`；该 ChatContext 一辈子 0..n 次产生父 `AgentLoop`，每个父 loop 在 `AgentRegistry` 各登记一条 `AgentHandle`（含其下属子 loop 的 handle）。
- `AgentHandle` 字段**严格收敛**为：`session_id`、`parent_session_id`、`spawn_depth`、`abort_signal`，（可选 Phase 3+ `subagent_type` / `role` 镜像供 `tool_exec` 守卫读）。**不**加入 `AgentLoop`、`messages`、`tool_state` 等执行面字段——这些数据要么在 `AgentLoop` 栈上、要么在 `ContextState` 里。
- 子 loop 在 `AgentRegistry` 里登记的 `abort_signal` **与子 loop 内部使用的是同一个 `Arc<AtomicBool>`**——`CascadeAbort` 在表上一次 `store(true)`，reasoning 间隙的子 loop 就能看到。

**说人话**：`ChatContextRegistry` 是「楼层档案」（哪间聊天室是谁的）；`AgentRegistry` 是「访客登记处」（谁正在干活、急停按钮在哪）。两张表互不掺和，但要靠 `ChatContext.root_session_id` 把彼此串起来——见 [`plan-runtime.md` §6.4](./plan-runtime.md#64-chatcontext-持有关系)。

### 14.3.3 AgentLoopConfig 扩展

在现有 [`src/core/agent_loop.rs`](../../../src/core/agent_loop.rs) 的 `AgentLoopConfig` 中新增两个字段（Phase 2 实现时补充）：

```rust
pub struct AgentLoopConfig {
    // ... 已有字段 ...
    /// 父 session；RootAgent 为 None。
    pub parent_session_id: Option<String>,
    /// 当前嵌套层数；RootAgent 为 0。递归检查上限为 MAX_SPAWN_DEPTH。
    pub spawn_depth: u32,
    /// 预设子 Agent 画像（决定 system prompt 模板与默认 allowed_tools）；
    /// RootAgent 取 SubagentType::None；由父 Agent 通过 dispatch_agent.subagent_type
    /// 入参或 internal subagent dispatch 调用方决定。
    pub subagent_type: SubagentType,
    /// 子 Agent 派生角色；leaf 不可再派发，orchestrator 可再派发；
    /// 取代旧的 allow_sub_dispatch: bool 字段。RootAgent 取 Role::Orchestrator。
    pub role: Role,
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
      "subagent_type": {
        "type": "string",
        "enum": ["general", "explore", "shell", "cursor-guide"],
        "description": "预设子 Agent 画像；决定 system prompt 模板与默认 allowed_tools 集；省略时取 \"general\""
      },
      "role": {
        "type": "string",
        "enum": ["leaf", "orchestrator"],
        "description": "leaf：子 Agent 不可再调用 dispatch_agent（防递归，对标 hermes role='leaf'）；orchestrator：可继续派发但仍受 spawn_depth 与 MAX_SPAWN_DEPTH 约束（对标 hermes role='orchestrator'）；省略时取 leaf"
      },
      "allowed_tools": {
        "type": "array",
        "items": { "type": "string" },
        "description": "显式工具白名单。省略时 = subagent_type 预设默认集 ∩ 父 Agent 当前 catalog；传入时 = 入参列表 ∩ 父 catalog（双保险）。runtime 始终先剔除 dispatch_agent，role=orchestrator 时再重新注入"
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

> **设计决策**：保持「单工具 + 多形态参数化」（与 hermes-agent `delegate_task`、cc-fork-01 `Task` 同构），不拆成多个 `dispatch_*` 工具——多形态间只在 `subagent_type` / `role` / `allowed_tools` 三个参数上分化，避免工具目录爆炸。`task` 字符串只描述任务，**不再**承担工具约束与递归权限，权限边界由 `role` + `allowed_tools` + `subagent_type` 三参数共同决定。
>
> **历史决策（已替换）**：早期方案使用 `allow_sub_dispatch: bool` 单一布尔开关控制递归权限，已被 `role` 枚举取代（`leaf` 等价旧的 `false`，`orchestrator` 等价旧的 `true`）。

### 14.4.2 子 Agent 创建与执行流

```
主 AgentLoop (session_id="S1", spawn_depth=0)
│
│   LLM 返回 tool_call: dispatch_agent { task: "分析并修复 src/foo.rs 中的 panic" }
│
▼
execute_tool("dispatch_agent", args)
│
├─ [Guard 1] 检查 spawn_depth >= MAX_SPAWN_DEPTH (默认 2)
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
│     subagent_type:     args.subagent_type.unwrap_or("general"),
│     role:              args.role.unwrap_or("leaf"),
│     model:             args.model.unwrap_or(self.config.model.clone()),
│     max_turns:         args.max_turns.unwrap_or(20),
│     tool_definitions:  resolve_child_tools(
│                            parent_catalog,
│                            subagent_type,        // 预设默认集
│                            args.allowed_tools,   // 可选覆盖/收紧
│                            role,                 // orchestrator 时重新注入 dispatch_agent
│                        ),
│   }
│
├─ 构造子 AgentLoop，注入相同的 Arc<dyn LlmProvider> / Arc<dyn PrimitiveExecutor> / Arc<EventBus>
│
├─ registry.register(child_session_id, child_handle)
│
├─ 发布 AgentEvent::SubAgentStart { session_id, parent_session_id, task, spawn_depth }
│
├─ child_loop.run(vec![ ChatMessage::user(args.task) ]).await
│
│   ... 子 Agent 独立运行，所有 AgentEvent 以 session_id="S1:sub:<uuid>" 在 EventBus 发布 ...
│
├─ registry.unregister(child_session_id)
│
├─ 发布 AgentEvent::SubAgentEnd { session_id, parent_session_id, result, is_error }
│
└─ 返回 ToolResult { content: child_result.final_text, is_error: false }
```

> **图侧注**：`registry.register` 写入的是 `AgentHandle`（仅元数据 + `abort_signal`，§14.3.2.1），**不是** `AgentLoop` 本体；`child_loop` 在 `execute_tool` 的栈帧里 `new` 出来，`run().await` 返回后立即 drop。详见 §14.4.2.2「子 AgentLoop 的所有权与生命周期」。

### 14.4.2.1 调用栈与代码落点

子 `AgentLoop` 有且仅有**两条**启动路径（与 §14.6.1 互补），它们在 `spawn_subagent_internal` 之后汇合：

```text
路径 A — LLM-facing dispatch_agent（Phase 3，对应 §14.4.1 工具 schema）
  父 AgentLoop::run
    │
    ├─ reasoning 产出 tool_call("dispatch_agent", args)
    │
    ├─ AgentLoop::execute_tool（src/core/agent_loop.rs 现有分支扩展）
    │
    └─ dispatch_agent_tool::run(parent_handle, args)        ← 新文件 src/core/dispatch_agent_tool.rs
         │
         ├─ Guard 1/2/3（spawn_depth / 全局 / 父并发）
         ├─ 构造 child AgentLoopConfig（session_id / parent / depth / subagent_type / role）
         │
         └─ AgentRegistry::spawn_subagent_internal(deps, parent, cfg, initial_messages)   ↘
                                                                                          │
路径 B — internal subagent dispatch（reviewer 等，§14.6.1）                                │
  父 AgentLoop::execute_tool("create_plan", args)                                          │
    │                                                                                       │
    ├─ create_plan::write_plan（先释放 plan advisory lock，见 reviewer.md RV14）              │
    │                                                                                       │
    └─ PlanRuntime::dispatch_reviewer(allow_review_edit)                                    │
         │                                                                                  │
         └─ AgentRegistry::spawn_subagent_internal(deps, parent, cfg, initial_messages)   ──┘
                │
                │   ──── 共用后半段 ────
                │
                ├─ registry.register(child_session_id, AgentHandle { … })
                ├─ child_loop = AgentLoop::new(Arc::clone(llm), Arc::clone(primitive),
                │                              Arc::clone(event_bus), cfg, child_cancel)
                ├─ event_bus.publish(SubAgentStart { … })
                ├─ result = child_loop.run(initial_messages).await   ★ 子 loop 在此真正运行
                ├─ event_bus.publish(SubAgentEnd { … })
                ├─ registry.unregister(child_session_id)
                └─ return Ok(result)                                  // child_loop 在此 drop
```

**与现状代码的差距（Phase 1 → Phase 2/3 PENDING）**：

| 半段 | 现状（Phase 1） | 拟定（Phase 2/3） |
|------|----------------|-------------------|
| 父 `chat_loop` → 父 `AgentLoop::new` → `run` | ✅ 已存在（[`src/api/chat/mod.rs`](../../../src/api/chat/mod.rs)） | 增加 `AgentRegistry::register` 自身 handle |
| `dispatch_agent_tool` 模块 | ❌ 不存在 | Phase 3 新增 `src/core/dispatch_agent_tool.rs` |
| `AgentRegistry::spawn_subagent_internal` | ❌ 不存在 | Phase 2 新增（`src/core/agent_registry.rs`） |
| `PlanRuntime::dispatch_reviewer` | ❌ 不存在 | Phase 3，路径 B 入口，见 [`tools/reviewer.md` §4.3](./tools/reviewer.md#43-派发入口api-形态) |

**说人话**：子 Agent 不是 `AgentRegistry` 自动跑起来的，而是某个**调用方**（`dispatch_agent_tool::run` 或 `PlanRuntime::dispatch_reviewer`）`new` 一个 `AgentLoop` 并 `await run`；`AgentRegistry` 只是在跑之前/之后记一笔，方便限流与父停子停。

### 14.4.2.2 子 AgentLoop 的所有权与生命周期

> 本节是 §14.4.2.1 的「所有权与寿命」侧写：聚焦「**谁 new 了 child_loop / 谁拿着它的引用 / 它什么时候 drop**」。`reviewer.md` 等下游文档只引用本节，不重复展开。

#### 1) 现状 vs 拟定

| 角色 | Phase 1（现状代码） | Phase 2 / 3（拟定，PENDING） |
|------|---------------------|------------------------------|
| 父 `AgentLoop` | [`chat_loop`](../../../src/api/chat/mod.rs) 每用户输入 `AgentLoop::new(...)` 然后 `run(messages)` | 同左 + `AgentRegistry::register(parent_handle)` |
| 子 `AgentLoop` | **不存在**（无 dispatch 通路） | 仅在 `dispatch_agent_tool::run` 或 `spawn_subagent_internal` **栈帧**内 `new` |
| `AgentRegistry` | 仅文档 | `DashMap<session_id, Arc<AgentHandle>>`（不含 `AgentLoop`） |
| `ChatContext` | 持 `TodoRuntime` / `PlanRuntime` / 共享 `Arc` 服务 / `root_session_id` | 同左；**绝不**加 `subagents[]` 或 `Map<SubagentType, Agent>` |

#### 2) 唯一 `new` 点

子 `AgentLoop` **有且仅有**以下两处构造点（**禁止在 `ChatContext` / `PlanRuntime` / `AgentRegistry` / 工具 handler 散落 `AgentLoop::new`**）：

1. `dispatch_agent_tool::run(...)` 函数体内 → 委托给 `spawn_subagent_internal(...)`。
2. `AgentRegistry::spawn_subagent_internal(...)` 函数体内 → **真正**调 `AgentLoop::new(...)`。

最终落到 `spawn_subagent_internal` 一处构造，减小审计面。

#### 3) 引用关系（每行都是一个所有权事实）

| 引用方 | 引用什么 | 类型 | 持续多久 |
|--------|----------|------|----------|
| `spawn_subagent_internal` 栈帧 | `child_loop: AgentLoop` | **本体（局部变量，stack-owned）** | `AgentLoop::new` → `child_loop.run(...).await` 返回 → **drop** |
| `child_loop` 内部 | `llm` / `primitive` / `event_bus` | `Arc<dyn …>` clone（来自 `ChatContext` 或 `SpawnDeps`） | 进程级共享，子 loop drop 后 Arc 计数 -1 |
| `AgentRegistry` | `AgentHandle { abort_signal, parent_session_id, … }` | `Arc<AgentHandle>` | `register(...)` … `unregister(...)` 一窗口 |
| 父 `AgentLoop` | **不持有**子 loop 任何句柄 | — | 仅在 `execute_tool` 内 `await` `dispatch_agent_tool::run`，await 期间阻塞栈 |
| `ChatContext` | **不**持子 `AgentLoop`，仅提供共享 `Arc<dyn …>` | — | 与 chat session 同寿 |
| `PlanRuntime::dispatch_reviewer` | **不**持子 `AgentLoop`，只 `await spawn_subagent_internal` 拿 `AgentRunResult` | — | 单次 reviewer 调用栈帧 |

> 关键事实：`AgentRegistry` **没有** `agents: DashMap<session_id, AgentLoop>` 这样的字段——它只持 `AgentHandle`；子 `AgentLoop` 的真正所有权永远在**栈上**。

#### 4) 拟定伪代码（与实现对齐的契约）

```rust
// src/core/agent_registry.rs（Phase 2 新增；reviewer / dispatch_agent 共用）
pub async fn spawn_subagent_internal(
    deps: &SpawnDeps,             // Arc<dyn LlmProvider> / PrimitiveExecutor / EventBus
    parent: &ParentSpawnCtx,      // parent_session_id / spawn_depth / abort_signal
    cfg: AgentLoopConfig,         // 已带 child session_id / parent / depth / subagent_type / role
    initial_messages: Vec<ChatMessage>,
) -> Result<AgentRunResult> {
    // ① 共用 abort_signal：表里和子 loop 看到同一个 Arc<AtomicBool>
    let child_abort = Arc::new(AtomicBool::new(false));
    let child_cancel = cfg.cancel_token.clone();

    // ② 登记 Handle（仅元数据，绝不放 AgentLoop）
    deps.registry.register(
        cfg.session_id.clone(),
        Arc::new(AgentHandle {
            session_id:        cfg.session_id.clone(),
            parent_session_id: Some(parent.session_id.clone()),
            spawn_depth:       parent.spawn_depth + 1,
            abort_signal:      Arc::clone(&child_abort),
        }),
    )?;

    // ③ 真正 new：唯一一处，且 stack-owned
    let mut child_loop = AgentLoop::new(
        Arc::clone(&deps.llm),
        Arc::clone(&deps.primitive),
        Arc::clone(&deps.event_bus),
        cfg.clone(),
        child_cancel,
    );

    // ④ 父在此阻塞 await，子 loop 在自身栈帧内推进 reasoning / tool
    deps.event_bus.publish(AgentEvent::SubAgentStart { /* … */ });
    let result = child_loop.run(initial_messages).await;
    deps.event_bus.publish(AgentEvent::SubAgentEnd { /* … */ });

    // ⑤ 注销 Handle；本帧返回时 child_loop 自动 drop（Arc 服务 refcount -1）
    deps.registry.unregister(&cfg.session_id);
    result
}
```

#### 5) 父 vs 子对比

| 维度 | 父 `AgentLoop` | 子 `AgentLoop`（reviewer / dispatch_agent） |
|------|----------------|---------------------------------------------|
| 触发 | 用户输入 / `/plan build` | 父 `execute_tool` 内派发 |
| `new` 位置 | [`chat_loop`](../../../src/api/chat/mod.rs)（Phase 1） | `spawn_subagent_internal`（Phase 2/3） |
| 引用方 | `chat_loop` 栈帧（每用户输入一次） | `spawn_subagent_internal` 栈帧（每 spawn 一次） |
| `Registry` 是否登记 | Phase 2 起登记（自身 handle） | 始终登记 |
| `abort_signal` 来源 | 用户/CLI/SDK 注入 | `spawn_subagent_internal` 内新建，**与 Handle 共用同一 Arc** |
| 寿命 | 单 user turn | 单 spawn（一次 reviewer / 一次 dispatch_agent） |
| 与 `ChatContext` 关系 | 借共享 `Arc` 服务，不被持有 | 同左 |

#### 6) `AgentRegistry` 里到底有什么（ASCII）

```text
AgentRegistry
└── agents: DashMap<session_id, Arc<AgentHandle>>
      │
      ├─ "S1"                  → AgentHandle { parent=None,   depth=0, abort_signal_a }
      ├─ "S1:sub:abc"          → AgentHandle { parent=S1,     depth=1, abort_signal_b }   (子 loop 跑期间)
      └─ "S1:sub:abc:sub:def"  → AgentHandle { parent=S1:sub:abc, depth=2, abort_signal_c }

                  ▲                 ▲
                  │                 │
            AgentLoop 本体       AgentLoop 本体
            （父 chat_loop      （子 spawn_subagent_internal
             栈帧拥有）            栈帧拥有，run 完即 drop）
```

#### 7) 两条触发路径汇总

```text
ChatContext (session_key=K, root_session_id=S1)
    │
    │  user input / /plan build
    ▼
chat_loop ──── new ────▶ 父 AgentLoop(S1)
                            │
                            │  execute_tool(...)
                            │
              ┌─────────────┴──────────────┐
              │                            │
        路径 A: dispatch_agent          路径 B: create_plan → PlanRuntime::dispatch_reviewer
              │                            │
              └────────────┬───────────────┘
                           │
                           ▼
                AgentRegistry::spawn_subagent_internal
                   │  ① register(Handle)
                   │  ② new child AgentLoop
                   │  ③ child_loop.run().await
                   │  ④ unregister(Handle)
                   ▼
               child_loop drop
```

#### 8) 历史决策（本节末记）

- ❌ ~~`AgentRegistry` 字段含 `agents: DashMap<session_id, AgentLoop>`~~ → 否；只持 `AgentHandle`。
- ❌ ~~`ChatContext.subagents: Vec<Agent>` / `HashMap<SubagentType, Agent>`~~ → 否；与「每 spawn 新建 + 跑完即 drop」语义冲突。
- ❌ ~~每个 `AgentLoop` 自带 `LlmProvider`/`PrimitiveExecutor` 实例~~ → 否；进程级 `Arc<dyn …>` 共享（MA7）。
- ✅ 子 loop **唯一** `new` 点 = `spawn_subagent_internal`；`dispatch_agent_tool::run` 通过它落地。

**说人话**：子工人只在派遣公司（`spawn_subagent_internal`）里雇一次，干完就散；登记处只记工牌（`AgentHandle`）和急停按钮（`abort_signal`），不把工人养在抽屉里。

---

### 14.4.3 上下文隔离原则

参考 claude-code 的强隔离设计（只返回 final message）和 openclaw 的 system prompt 注入方式：

- **不继承 messages 历史**：子 Agent 只接收 `args.task` 字符串，外加宿主注入的系统 prompt（由 `subagent_type` 决定模板，含子 Agent 身份描述、`spawn_depth`、`parent_session_id`）。
- **独立 Transcript**：子 Agent session 的 transcript 写入 `agents/main/sessions/{child_session_id}.jsonl`，或通过配置设为不落盘（ephemeral）。
- **工具集由三参数共同决定**：子 Agent 工具集 = `subagent_type` 预设默认集 ∩ 父 Agent 当前 catalog ∩ （可选）`args.allowed_tools`，再按 `role` 决定是否重新注入 `dispatch_agent`：
  - `role = "leaf"`（默认）：剔除 `dispatch_agent`，子 Agent 不可再派发（对标 hermes `role='leaf'`、claude-code 子 Agent 不再可派发）。
  - `role = "orchestrator"`：重新注入 `dispatch_agent`，但仍受 `spawn_depth` / `MAX_SPAWN_DEPTH` 约束（对标 hermes `role='orchestrator'`）。
- **双保险**：即使 LLM 在 `allowed_tools` 中传入了父 catalog 不存在的工具名，runtime 端也会过滤；与 [`plan-runtime.md`](./plan-runtime.md) §4.2.3 reviewer `allowed_tools` 「白名单 ∩ 父 catalog」原则一致。

### 14.4.4 嵌套深度限制

综合 openclaw `spawnDepth + maxSpawnDepth` 与 LangGraph `recursion_limit` 的设计：

- `AgentLoopConfig.spawn_depth` 在构造子 Agent 时以 `parent.spawn_depth + 1` 传入。
- 执行 `dispatch_agent` 时，若 `self.config.spawn_depth >= MAX_SPAWN_DEPTH`（默认 `2`），拒绝并向 LLM 返回错误 ToolResult，附带说明文本，不终止主 Agent（LLM 可自行选择其他工具继续）。
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
| **第 13 节（Agent Loop）** | `AgentLoop` 本身不修改；`dispatch_agent` 工具作为普通工具注入 `tool_definitions`；`AgentRegistry` 是新增的进程级管理层；`AgentLoopConfig` 新增 `parent_session_id` / `spawn_depth` / `subagent_type` / `role` 四个字段。 |

### 14.6.1 internal subagent dispatch（reviewer 消费方）

[`plan-runtime.md`](./plan-runtime.md) 与 [`tools/reviewer.md`](./tools/reviewer.md) 中描述的 **reviewer** 子 Agent 走的是「**internal subagent dispatch**」路径，与本节 §14.4 的 LLM-facing `dispatch_agent` 工具互补：

| 维度 | LLM-facing `dispatch_agent`（§14.4） | internal subagent dispatch（reviewer） |
|------|--------------------------------------|----------------------------------------|
| 入口 | LLM 自主决定的 tool_call | 内部 Rust API（`AgentRegistry::spawn_subagent_internal(...)` 拟定），由 `CreatePlan` 工具内部同步 await |
| 是否进 catalog | 是 | 否 |
| `subagent_type` | 由 LLM 在 schema 内传入 | 不走 schema，调用方硬编码（reviewer 固定模板） |
| `allowed_tools` | 由 LLM 传入或继承父 catalog | 调用方硬编码（默认 `{read, grep, find}`；runtime 内部参数 `allow_review_edit=true` 时附加 `edit` / `update_plan`，且 `tool_exec` 守卫强制 `edit` 仅能作用于 `~/.tomcat/plans/*.plan.md` 且不得 raw 改 frontmatter） |
| 复用 §14 基础设施 | 全部（`AgentRegistry` / `spawn_depth` / `CascadeAbort` / `SubAgentStart`/`End` 事件） | 全部（同左） |
| 对标项目 | hermes `delegate_task`、claude-code `Task` / `Agent` | codex [`codex-rs/core/src/codex_delegate.rs::run_codex_thread_one_shot`](https://example/codex_delegate) |

**关键边界**：reviewer 的 `allowed_tools` **不**占用 `dispatch_agent` schema 的 `subagent_type` 枚举位；两条路径共享同一套 `AgentRegistry` / `spawn_depth` / `CascadeAbort`，但各自独立鉴权与构图。详细派生契约见 [`tools/reviewer.md`](./tools/reviewer.md)。

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
