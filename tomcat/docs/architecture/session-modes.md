# 会话模式与多会话并发：claw（全局）/ code（按项目）+ 单进程多 AgentLoop

> 适用范围：`tomcat claw` / `tomcat code` 两个入口的会话组织、以及"单进程内多会话并发各跑各的 AgentLoop"（像 Cursor 多个聊天 tab）的运行时架构。跨 `src/api/cli`、`src/api/chat`、`src/core/session`、`src/core/agent_loop`、`src/core/agent_registry`、`src/infra/config` 多个一级子目录。
>
> 本文是 [会话存储数据结构](session-storage.md) 与 [工作目录与数据布局](work-dir-and-data-layout.md) 的上层方案。两件事在这里被统一：(1) **会话怎么按作用域分组**（claw 全局 / code 按项目）；(2) **多个会话怎么在一个进程里并发跑**。本文取代计划 `session-cwd-binding` 中的单模式与 legacy 迁移设计（见 §13）。
>
> 命名约定：原 `tomcat chat` 拆分为 **`tomcat claw`（全局，不绑定 cwd）** 与 **`tomcat code`（绑定项目目录）** 两个入口；对外文档与验收口径均以 `claw/code` 为准，代码里仅保留一个隐藏兼容别名 `chat -> code` 供旧习惯平滑过渡。章节编号对应 [`ARCHITECTURE_SPEC.md`](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 §1–§13。

## 先看总图

这套方案有两条正交的轴，分开看才不晕：**轴一 = 会话怎么按作用域分组（claw/code）**，**轴二 = 多个会话怎么在一个进程里并发**。下面两张图分别讲这两轴。

### 图 A：会话组织（一个作用域 scope → 多个会话 session）

专业描述：入口命令决定 `SessionMode`；模式 + 当前目录算出 **sessionKey（作用域键，scope）**；一个 scope 下可以有**多个** `sessionId`（各对应一份 `.jsonl`）；启动时默认续上该 scope 最近活跃的那个，也可新建。

```text
入口命令            作用域键 sessionKey(scope)              该 scope 下的会话(1 : N，每份 = 1 条 SessionEntry + 1 个 .jsonl)
                                                       ┌─ sessionId G-1  (.jsonl)
 tomcat claw ─────▶ "agent:main:main"        ─────────▶├─ sessionId G-2  (.jsonl)   ← 全局所有会话挤在一个 scope
                    (全局唯一，与 cwd 无关)              └─ ... 默认续 current[scope]

 tomcat code ─────▶ "agent:main:proj:<hash>"           ┌─ sessionId X-1  (.jsonl)
 (在项目 X 内)       hash = git仓库根(回退cwd)  ────────▶├─ sessionId X-2  (.jsonl)   ← 每个项目一个 scope，
                                                       └─ ...                          各装该项目的多个会话
```

说人话：把 sessionKey 想成"文件夹名"，sessionId 想成"文件夹里的一份份对话"。claw 只有一个大文件夹（`agent:main:main`），你所有全局对话都丢这；code 给每个 git 项目一个文件夹（名字是项目根的 hash），该项目的多个对话都在自己的文件夹里。**一个文件夹里可以有很多份对话**——这就是"一个 sessionKey 对应多个 sessionId（等价地，也就是多条 SessionEntry，因为 sessionId ↔ SessionEntry ↔ `.jsonl` 一一对应）"。换项目=换文件夹，所以 code 跨项目天然隔离；claw 永远同一个文件夹，所以全局连续。

### 图 B：并发运行时（单进程 + 多 AgentLoop，对标 codex）

专业描述：分**会话层**与 **turn 层**两层，别混。**会话层**——用户"打开/激活"一个会话，`SessionRuntimeRegistry`（`HashMap<sessionId, Arc<SessionRuntime>>`）里就有一个**常驻**的 `SessionRuntime`，持有该会话的 `cancel_token` / `context_state` / `plan_runtime` / `todos_runtime` / `read_file_state` / per-session bash 任务，**跨多轮存活**。**turn 层**——用户输入提示词时，对该 `SessionRuntime` 发起一个**单轮**的 `tokio::spawn(AgentLoop)` 与 LLM 交互；turn 结束/中断后该 `AgentLoop` task 终止，`SessionRuntime` **回到 Idle 仍留在 registry**（"热"着、保住已 hydrate 的上下文，等下一句），不随 turn 结束而移除（移除只发生在显式 close 或空闲驱逐，见 §8/§9）。共享层按"生命周期"再分两级（D16）：**GlobalServices**（进程级、无 per-session/per-project 状态：LLM、工具、event_bus、SessionManager、gate）与 **ScopeServices**（按 work_tree/项目缓存、同项目多会话共用一份：checkpoint、project skill 层）。

```text
                         单个 tomcat 进程（Tokio 多线程 runtime）
 ┌───────────────────────────────────────────────────────────────────────────┐
 │  GlobalServices(进程级共享，无 per-session/per-project 状态)                  │
 │   llm(+并发准入 D17) · model_catalog · llm_resolver · tool_registry ·        │
 │   event_bus(事件带 sessionId,D15) · SessionManager(sessions.json,RMW持锁 D10)│
 │   gate(授权=全局意图,保留 Global D16) · audit · web_fetch · web_search       │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  ScopeServices(按 work_tree/项目缓存，同项目多会话共用一份 D16)               │
 │   checkpoint_store(每 work_tree 一个实例,保串行 D18) · project skill 层       │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  SessionRuntimeRegistry : HashMap<sessionId, Arc<SessionRuntime>>           │
 │   ┌───────────────┐   ┌───────────────┐   ┌───────────────┐                 │
 │   │ SessionRuntime│   │ SessionRuntime│   │ SessionRuntime│                 │
 │   │  sessId X-1   │   │  sessId X-2   │   │  sessId G-1   │  ← 各自独立状态  │
 │   │  cancel_token │   │  cancel_token │   │  cancel_token │                 │
 │   │  context_state│   │  context_state│   │  context_state│                 │
 │   │  plan/todos   │   │  plan/todos   │   │  plan/todos   │                 │
 │   │  read_state   │   │  read_state   │   │  read_state   │  ← 先读后写,D16  │
 │   │  bash tasks   │   │  bash tasks   │   │  bash tasks   │  ← per-session   │
 │   │ turn 运行中:  │   │ turn 运行中:  │   │ Idle: 无 turn │                 │
 │   │ spawn AgentLp▶│   │ spawn AgentLp▶│   │ (热,留 registry)               │
 │   └───────┬───────┘   └───────┬───────┘   └───────────────┘                 │
 │           │ 事件(带 sessionId) │          ↑ 常驻：turn 结束回 Idle，不移除   │
 │           ▼                    ▼                                            │
 │     EventBus → 前端按 sessionId demux（CLI 当前 tab / 协议多连接）           │
 └───────────────────────────────────────────────────────────────────────────┘
```

说人话：这张图回答"多个会话怎么同时跑"。中枢是一张内存表 `SessionRuntimeRegistry`，键是 sessionId。每个被打开的会话都拎出一个常驻的"运行时小盒子"`SessionRuntime`（自己的取消开关、上下文、plan/todos、"先读后写"指纹表、后台 bash 任务），**只要会话还开着就一直留在表里**。关键区分：**盒子（SessionRuntime）是常驻的，干活的 `AgentLoop` 是按一轮一轮临时起的**——你发一句提示词才 `spawn` 一个 AgentLoop 去和 LLM 交互，这一轮结束 AgentLoop 就没了，盒子回到 Idle 继续待命（上下文还热着，不用重读历史）。图里 X-1、X-2 正各跑一轮（所以能并行），G-1 是打开着但这会儿没在跑（Idle）。共享层按生命周期分两级：**GlobalServices** 是进程里只有一份、谁都能用的（LLM、工具、存储管理器、权限闸门）；**ScopeServices** 是"按项目"才共用一份的（最典型是 checkpoint——它绑定一棵工作树，所以同一个项目的多个会话共用同一个，换项目才换一个，详见 D16/D18）。事件都带 sessionId，前端按 sessionId 分发到对应 tab。这跟 codex 的 `ThreadManager`（`HashMap<ThreadId, Arc<CodexThread>>`，thread 常驻、turn 按需跑）是同一套思路。盒子什么时候才从表里拿走？只有你显式关掉它，或太久不用被驱逐（见 §8/§9）——绝不是"一轮跑完就拿走"。

阅读顺序（说人话）：先看图 A 建立"作用域→多会话"的存储心智，再看图 B 建立"内存 registry→多 loop 并发"的运行时心智。§1 术语把两图的名词钉死；§4 决策表解释为什么这么选（尤其 D7 为什么单进程不走多进程、D8 为什么数据模型要 1:N）。

---

## 1. 术语统一（MUST）

本节钉死两轴的核心名词。轴一（组织）：`SessionMode` / `sessionKey(作用域)` / `sessionId`。轴二（并发）：`SessionRuntime` / `SessionRuntimeRegistry` / `AgentLoop`。

说人话：最关键的认知升级有两点——(1) **sessionKey 不再 1:1 指向一个会话**，它现在是"作用域/分组键"，一个 key 下挂多个 sessionId；(2) **sessionId 升格为一等公民**，每个 sessionId 有独立 transcript、独立运行时盒子，能独立并发跑。

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `SessionMode` | 会话作用域策略：`Code`(按项目)/`Claw`(全局) | `enum SessionMode`（`core/session/scope.rs`），由 CLI 子命令决定 | 进程级；只影响"算 sessionKey"一步 | 你进的是 code 门还是 claw 门 |
| `sessionKey`（作用域键 / scope） | 会话的**分组键**，1:N 指向多个 sessionId | `String`，语法 `agent:<agentId>:<channelKey>` | claw=`main`；code=`proj:<hash>`；用于"列出/归属"，不再 1:1 | 文件夹名：claw 一个、code 每项目一个 |
| `sessionId` | 单次对话的唯一身份，**一等公民** | `String`(`<ts>_<uuid>`)，1:1 对应 `<sessionId>.jsonl` 与一个 `SessionRuntime` | 创建后不变；可被列出/激活/并发跑/归档 | 文件夹里的一份对话 |
| `SessionEntry` | 某 sessionId 的元数据记录（**沿用现有结构**，新增 `sessionKey` 字段） | `sessions.json` 内 `sessions[sessionId]`，含 `sessionId/sessionKey/cwd/updatedAt/...` | 与 sessionId、`.jsonl` 一一对应（1:1:1） | 这份对话的卡片信息 |
| `current[sessionKey]` | 某 scope 最近活跃的 sessionId | `sessions.json` 内 `current` 映射 | 用于启动时"自动续上该 scope 最近一次" | 文件夹默认打开的那份 |
| `SessionRuntime` | 一个**已打开**会话的进程内运行时盒子（常驻，非按 turn） | `struct SessionRuntime`（新增），持 `context_state/cancel_token/plan_runtime/todos_runtime/...` | 每 sessionId 至多一个；**跨多轮存活**，turn 结束不移除；仅 close/驱逐时出表 | 一份对话打开后的"独立工位"（人走工位还在） |
| `SessionRuntimeRegistry` | 进程内活跃会话表 | `HashMap<sessionId, Arc<SessionRuntime>>`（新增，替代单 `ChatContext` 的 per-session 字段） | 增删查；并发 spawn/abort | 工位总表，管着同时跑哪些会话 |
| `SharedServices` | 跨会话共享、无 per-session 状态的服务 | LLM/工具/EventBus/checkpoint/SessionManager 等（由现 `ChatContext` 拆分而来） | 所有 SessionRuntime 复用 | 公共工具间，大家共用 |
| `AgentLoop` | **单轮** turn 的三层推理循环 | `core/agent_loop`（现有） | 发提示词时按 turn `tokio::spawn`；用本会话 `cancel_token`；**turn 结束即终止**（SessionRuntime 不随之移除） | 真正"干活"的那台机器，一轮一开 |

模糊词钉死："激活/打开会话"指把某 sessionId 装进 `SessionRuntimeRegistry` 并（可）`spawn` 其 `AgentLoop`；"后台会话"指 registry 中非当前前台、但其 `AgentLoop` task 仍在跑的会话。

## 2. 背景与竞品调研（Why）

### 2.1 问题陈述（说人话）

现状两个痛点叠加：

1. **跨项目串台**：`tomcat chat` 用固定 `DEFAULT_SESSION_KEY="agent:main:main"`（`core/session/store.rs`），`current_session_key()` 永远返回它（`session_impl.rs`）。换到新项目目录后，`ensure_current_session` 仍命中同一个 key、续上上个项目的历史，于是"记得上一个项目、以为自己还在上一个项目里"。
2. **只能一个会话**：主循环是 `block_on(chat_loop)` + 阻塞 `rustyline::readline`，同一时刻只有一个 root `AgentLoop` 串行跑（`run_loop/mod.rs`、`chat_cmd.rs`）。想"像 Cursor 同时开几个会话各跑各的"做不到。

本方案同时解决这两件事：轴一（claw/code 作用域）治串台，轴二（registry + 多 loop）治并发。

### 2.2 竞品调研一：会话怎么按目录组织（轴一）

| Agent | 与 cwd 的关系 | 关键证据 |
|------|--------------|----------|
| Cursor | 每工作区独立会话列表 | 产品行为（按 workspace 分组） |
| cc-fork-01 | transcript 落 `~/.claude/projects/<sanitized_cwd>/<sessionId>.jsonl`，**按 cwd 分目录、每目录多份** | `src/utils/sessionStorage.ts` |
| codex | SQLite `threads` 表每行带 `cwd`，列表按 cwd 过滤 | `thread-store/src/local`、`state/src/runtime.rs` |
| hermes / openclaw | 会话不绑 cwd，绑 `sessionKey=agent:<id>:<channel...>`（平台/频道维度），cwd 仅作 per-session 覆盖 | `gateway/session.py`、`routing/session-key.ts` |
| pi 系 / GenericAgent / Qevos | 会话绑独立文件/run 目录，与 cwd 解耦 | 见 §2.3 表 |

结论（轴一）：业界两种主流——**按 cwd 分组（cc-fork/codex/Cursor）** 与 **按逻辑频道 key 分组（hermes/openclaw）**。tomcat 的 `agent:<agentId>:<channelKey>` 语法天生能两者兼容：**code 把 cwd 编码进 channelKey（`proj:<hash>`），claw 用固定 `main`**，无需新存储引擎。

### 2.3 竞品调研二：多会话怎么并发（轴二，本次重点）

| Agent | 并发形态 | 多 session 承载 | 同 session 串行 | 跨 session 隔离 | 存储写协调 |
|------|---------|----------------|-----------------|-----------------|------------|
| **codex** ⭐ | **单进程 Tokio + 常驻 app-server** | `ThreadManager`：`HashMap<ThreadId, Arc<CodexThread>>`，每 thread 一个 `tokio::spawn(submission_loop)` | submission_loop 顺序消费 `Op` | 每 thread 独立 `Session` + 每 turn 独立 `CancellationToken` | rollout 每 thread 分文件 + SQLite UPSERT + per-thread RPC 串行队列 |
| **hermes** | 单进程 asyncio + 线程池 | `_sessions` dict / `_running_agents`，每 turn 一线程 | per-session `running` 锁 | ContextVar + `copy_context()`（task-local cwd/路由） | SQLite WAL + `BEGIN IMMEDIATE` + jitter 重试 |
| **openclaw** | 单 gateway 进程 + 多 lane async 队列 | `chatAbortControllers`/`ACTIVE_EMBEDDED_RUNS` Map | `session:{key}` lane 并发=1 + `ReplyRunRegistry` | run 级 `AbortController`；workspace per-agent | `sessions.json`/transcript 各按 path FIFO 队列 |
| **GenericAgent** | 单进程多线程 | TUI v2 `dict[int, AgentSession]`，每会话一守护线程 | 每会话单 `run()` 循环 | 独立 history/log/`task_dir`（`_intervene` 文件） | log 文件名带微秒戳；`temp/`、memory 全局共享 |
| **cc-fork-01** | **多进程**（每 CLI 实例）+ 进程内子 agent | PID 注册表 `~/.claude/sessions/{pid}.json`；进程内 `AppState.tasks` + AsyncLocalStorage | 每 `query()` 独立 | 多进程=OS 隔离；同进程=ALS | transcript 分文件**无跨进程锁、假设单写者**；mailbox/task 才 `proper-lockfile` |
| **pi_agent_rust** | 单进程单活跃 turn；ACP 例外 | ACP `HashMap<sessionId, AcpSessionState>`；SDK 多 handle/多 RPC 子进程 | 每 session 单 prompt（`is_streaming` 门控） | `AbortHandle` | `session-index.lock` + `{file}.jsonl.lock`（fs4 跨进程锁） |
| **pi-mono** | 单进程单活跃会话；多会话=多进程 | `AgentSessionRuntime` 单 `_session`，切换=teardown 重建 | `Agent.activeRun` 至多一个 | 每 run `AbortController` | jsonl 唯一文件名 + `wx` 创建，**无文件锁** |
| **QevosAgent** | 单 dashboard 单槽位 + 每 run 一子进程 | 全局仅一个 `agentProc` | — | 进程 + `run_dir` 隔离 | 全局 memory **无锁**，靠"同时只一个进程"约束 |

⭐ = 最值得 tomcat 对标。

结论（轴二）：

- **流派 A（单进程多任务 + 内存 registry）**：codex / hermes / openclaw / GenericAgent。中枢都是一张 `Map<sessionId, runtime>`，每会话一个 task/线程。**这是"像 Cursor 多 tab"的主流做法**，codex 最成熟（registry + 每 thread 一个 loop task + 协议多连接）。
- **流派 B（多进程，process-per-session）**：cc-fork / pi 系 / Qevos。OS 级隔离最强，但：跨进程写 transcript **多数无锁、靠'假设单写者'**（cc-fork、pi-mono 明说），要 PID 注册表 + IPC + 文件锁，资源开销大、IDE 集成复杂；Qevos 甚至根本不能真并发。

### 2.4 tomcat 现状评估（已具备 vs 待建）

已具备（可复用）：tokio(full) runtime；`AgentRegistry` 已能并发 `tokio::spawn` 子 agent（限流 `MAX_CONCURRENT_AGENTS=16`、级联 `abort_signal`）；`AgentLoop` 实例化 + `CancellationToken`；`BashTaskRegistry` 后台任务 + lifecycle→follow-up；`SessionManager` 的 `sessions.json` 原子写（`write_mutex`）。

待建（本方案核心改造）：

1. `sessions.json` 当前是 `HashMap<sessionKey, SessionEntry>`（**1:1，无法一个 key 多 session**）。
2. `current_session_key()` 写死 `DEFAULT_SESSION_KEY`。
3. transcript `append_message` 有 `// TODO: 并发 append TOCTOU 竞态，假设单线程串行`。
4. `ChatContext` 是**单会话装配根**：`cancel_token`/`plan_runtime`/`todos_runtime`/`context_state`/`follow_up_queue`/`completion_routes`/`openai_files_runtime`/`read_file_state`/`agent_registry` root 等全混在一层，且 `checkpoint`/`gate`/`skill_set`/`bash_task_registry` 也未按"进程级 / 项目级 / 会话级"分层（D16 待拆）。
5. 主循环阻塞 `readline` + 单 `block_on`，无多 loop 调度，无输出 demux。
6. `sessions.json` 的 `write_mutex` **只罩 `save_store`**，`load→改→save` 整段未持锁（并发会丢更新，D10 待修）；`checkpoint` 的 `ShadowGitStore` 按 `work_tree` 哈希定 `git_dir`、内部已有总锁串行，但当前跟单 `ChatContext` 走，未做"按 work_tree 缓存共用 + 按会话 paths 收窄 restore"（D18 待建）。

## 3. 目标与非目标（What）

### 3.1 目标

| 编号 | 目标 | 验收 | 说人话 |
|------|------|------|--------|
| G1 | code 跨项目隔离 | 在项目 A 起会话→`cd` 到 B 起 `tomcat code`，B 看不到 A 的历史，自我认知是 B | 换项目不串台 |
| G2 | claw 全局连续 | `tomcat claw` 在任意目录续同一组全局会话（默认续 current） | 全局助手不挑目录 |
| G3 | 一个 scope 多会话 | claw/code 都能在同一 scope 下 `new` 出多个 sessionId 并分别 `switch`/`list` | 一个文件夹放多份对话 |
| G4 | 单进程多会话并发 | 同一进程内 ≥2 个会话各跑各的 `AgentLoop`，互不阻塞；前台切走后后台会话仍能跑完一轮 | 像 Cursor 多 tab |
| G5 | 并发安全 | 多会话并发写 `sessions.json`/各自 transcript 不丢更新、不串文件 | 不打架 |
| G6 | 开发期旧索引直接重建 | `init` 直接写新结构；未先 `init` 直接使用时，遇到旧 `sessions.json` 或反序列化失败也自动重建 | 开发阶段不背旧结构包袱 |
| G7 | 可分阶段 | P1 数据模型即可单独上线（仍单前台会话），P2/P3 再加并发，不要求一次到位 | 小步快跑 |

### 3.2 非目标

| 非目标 | 推给 / 何时做 | 说人话 |
|--------|--------------|--------|
| **本期不改 CLI 交互范式**：保持现状 `block_on(chat_loop)` + 阻塞 `rustyline::readline`（`api/cli/chat_cmd.rs`），仍单前台会话 | TUI 期（TODO-1） | 现在这套"一根输入线"的 CLI 只能服务一个前台会话，本期不动它。 |
| **多会话能力的"用户暴露"**（`session open/ps/attach`、前台并发切换）推迟 | TUI 期（TODO-1）；依赖 P3 输入解耦 | 进程内能并发了，但让用户同时看多个 tab 要等 TUI 做出来再开放。 |
| 不引入 SQLite | — | 保持 `sessions.json + *.jsonl` 轻量；并发安全用 per-key/per-file 串行化，不为并发换存储引擎。 |
| 不在 P1/P2 强制做网络 daemon / IDE 协议；app-server（JSON-RPC over stdio，对标 codex）列 P3 可选 | P3 可选 | 先不做常驻服务/IDE 协议，等真要多 tab 体验再说。 |
| 不做多进程模型（见 D7） | — | 跨进程抢文件、要 IPC+锁，太重，不走。 |
| 不改 `agent:<agentId>:<channelKey>` 既有语法（只新增 channelKey 取值） | — | 沿用老 key 语法，只多几种取值，向后兼容。 |

> **TODO-1（本期不做）**：CLI 多会话的用户级暴露需要 TUI 框架（ratatui 等）重写渲染/输入层——多 pane/tab、后台会话事件缓冲、前台 demux（即 §P3 "输入解耦 + 输出 demux"）。本期 P1/P2 只做存储与运行时地基，**不触碰 CLI 交互**；待 TUI 立项后再把 §5.1 中标 `[P3·TUI]` 的子命令与并发前台切换开放给用户。**结论：本期 CLI 保持现状不动。**

## 4. 设计决策与实施（How）

### 4.1 落地选型决策表（已定稿）

> 七列遵循 [`ARCHITECTURE_SPEC §4.1`](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)：**维度**（审计轴）/ **关切**（可辩驳分叉）/ **决策**（一句裁决）/ **取自**（≥1 本仓 + ≥1 外部 agent 文件级证据）/ **入选理由**（设计＋为什么）/ **未入选 + 拒因** / **说人话**（背景＋解决方案）。D 编号被 §5/§7/§8/图 A·B 回指，新增只追加（D15–D18）不重排。本表覆盖两图意图：图 A（作用域 1:N）→ D1/D2/D8/D13；图 B（registry＋常驻 runtime/per-turn loop＋事件按 sessionId 路由）→ D7/D9/D14/D15；§5.2 职责拆分 → D13；**共享服务三层分层与并发安全（GlobalServices/ScopeServices/SessionRuntime、LLM 准入、checkpoint 作用域）→ D16/D17/D18，并修订 D9/D10**。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| **D1 作用域键·code** | code 模式按什么算 key 才能既隔离项目又稳定？ | 采用 `agent:main:proj:<hash(git根，回退cwd)>`；拒绝裸路径 / 项目名 | 本仓 `core/session/store.rs`(channelKey 语法)、`core/session/manager/session_impl.rs::current_session_key`；cc-fork `src/utils/sessionStorage.ts`(按 sanitized cwd 分目录)、codex `codex-rs/state/src/runtime.rs`(threads 行带 cwd) | 设计：取 git 仓库根的 hash 作 channelKey；理由：仓库根稳定→子目录/不同终端进同一会话，复用既有 `agent:<id>:<channel>` 语法、零新存储 | 裸路径当 key（cc-fork 用完整 cwd 路径）：子目录/软链不稳、含特殊字符；项目名：重名冲突 | 背景：`cd` 到新项目还续上个项目历史、串台。解决：给每个 git 项目按"仓库根指纹"算一个独立文件夹名——同项目同一组会话，换项目自动换一组。 |
| **D2 作用域键·claw** | claw 全局模式 key 怎么定，才能保持全局连续？ | 采用固定 `agent:main:main`；拒绝按 cwd 分 | 本仓 `core/session/store.rs::DEFAULT_SESSION_KEY="agent:main:main"`；hermes `gateway/session.py::build_session_key`(固定频道维度)、openclaw `src/routing/session-key.ts`(`agent:{id}:{channel}`) | 设计：channelKey 恒为 `main`；理由：全局模式天然不该受 cwd 影响，任意目录都续同一组全局会话 | 给 claw 也按 cwd 分(同 code)：违背"全局"语义，换目录就断上下文 | 背景：要一个不挑目录的全局助手。解决：claw 永远用固定 key，所以你在哪个目录打开，都会落到同一组全局会话。 |
| **D3 命令形态** | 用一个命令带 flag，还是拆两个命令暴露作用域？ | 采用拆分 `tomcat claw`/`tomcat code`，并保留隐藏兼容别名 `chat -> code`；拒绝单命令 + `--scope` | 本仓 `api/cli/mod.rs::Commands`(现 `Chat{resume}`)、`api/cli/chat_cmd.rs`；codex `codex-rs/cli`(按子命令分形态) | 设计：两个子命令各自固定 `SessionMode`，旧 `chat` 只作为通往 `code` 的兼容入口；理由：用户进门即显式选定隔离策略，又不至于把旧脚本悄悄引到全局会话 | 单命令 + `--scope` flag：易忘带 flag、默认仍串台(即现状痛点) | 背景：现在只有 `chat` 一个口、默认就串台。解决：正式入口分成 `claw`/`code` 两个口；保留下来的 `chat` 只做兼容跳转，而且直接跳到项目态 `code`。 |
| **D4 hash 算法** | 项目根→key 的 hash 用什么算法？ | 采用 FNV-1a 十六进制；拒绝 SHA-256 / `std DefaultHasher` | 本仓(新增 `core/session/scope.rs::fnv1a_hex`)；cc-fork `src/utils/sessionStorage.ts`(把 cwd sanitize 成目录名，我们改为 hash) | 设计：FNV-1a 纯函数取短 hex；理由：跨平台/跨版本稳定、key 短可读、无需加密强度 | SHA-256：key 过长无必要；`std DefaultHasher`：不保证跨版本稳定，升级可能换 key 致串/丢会话 | 背景：要把项目路径压成又短又稳的文件夹名。解决：用简单稳定的 FNV-1a 取指纹，长度短、不同机器/版本结果一致。 |
| **D5 key 计算封装** | "算 key"逻辑放哪、怎么可测？ | 采用集中到 `core/session/scope.rs::session_key_for(mode,cwd)` 纯函数；拒绝在 `context.rs` 内联 | 本仓 `api/chat/context.rs`(现 cwd→session 装配点)；codex `codex-rs/core/src/thread_manager.rs`(cwd→thread 解析集中在 manager) | 设计：纯函数单点产出 key；理由：单点可单测、上下游不感知模式分叉 | 在 `context.rs` 内联拼 key：难单测、模式分叉逻辑扩散进装配代码 | 背景：算 key 的分支若散落各处，难维护难测。解决：抽一个纯函数集中算（输入模式+目录、输出 key），单独测它即可。 |
| **D6 续接策略** | 启动时续旧会话、必新建、还是每次问？ | 采用默认续 `current[sessionKey]`、无则新建；拒绝每次新建 / 每次询问 | 本仓 `session_impl.rs::ensure_current_session`；cc-fork `src/utils/conversationRecovery.ts`(`--continue`)、codex `codex-rs/app-server`(`thread/resume`) | 设计：复用每 scope 的 `current` 指针；理由：命中"自动续上"心智、体验零意外，与现状默认行为一致 | 每次必新建：丢上下文；每次弹问：打断、增负担 | 背景：进来既想接着上次聊、又能开新的。解决：默认续这个作用域上次那份，想要新的再显式 `new`。 |
| **D7 并发模型** | 多会话并发用单进程多任务还是多进程？ | 采用单进程 Tokio + 内存 `SessionRuntimeRegistry`；拒绝多进程(process-per-session) | 本仓 `core/agent_registry/mod.rs`(已 `tokio::spawn` 子 agent，`MAX_CONCURRENT_AGENTS=16`)、`api/chat/context.rs`；codex `codex-rs/core/src/thread_manager.rs`(`HashMap<ThreadId,Arc<CodexThread>>`)+`session/mod.rs`(`tokio::spawn(submission_loop)`) | 设计：进程内 registry + 每会话常驻 `SessionRuntime`，turn 到来时再 `spawn` `AgentLoop`，共享无状态服务；理由：tomcat 已是 tokio+AgentRegistry 单进程多任务底子、增量最小，隔离用 per-session token/state，最贴合 IDE 多 tab | 多进程(cc-fork `src/utils/concurrentSessions.ts` PID 注册表 / Qevos `dashboard/server.js` 每 run 子进程)：跨进程 transcript 多数无锁、靠"假设单写者"(cc-fork `sessionStorage.ts`、pi-mono `session-manager.ts` 明说)，需 IPC+文件锁、开销大、集成复杂 | 背景：要像 Cursor 同时开多个会话各跑各的。解决：在一个进程里保留多个会话盒子，谁收到提示词就给谁起一轮 `AgentLoop`，底层服务大家共用，不开多进程去抢文件。 |
| **D8 持久数据模型·主键** | `sessions.json` 怎么存才能"一个 key 多会话"？ | 采用 `sessions{sessionId→SessionEntry}` 为一等键 + entry 加 `sessionKey` 字段；拒绝 `Map<key,entry>` 1:1 / `Map<key,Vec>` | 本仓 `core/session/store.rs`(现 `HashMap<key,SessionEntry>`)；codex `codex-rs/thread-store/src/local/mod.rs`+`state/src/runtime.rs`(threads 每行一会话)、cc-fork `src/utils/sessionStorage.ts`(每 cwd 多 jsonl) | 设计：sessionId 作主键、scope 降为 entry 上的分组字段；理由：直接满足"一个 sessionKey 多 sessionId / 一个 cwd 多 session"，列表=按 sessionKey 过滤 | `Map<key,entry>` 1:1(现状)：一个 key 只能一条、违背 G3；`Map<key,Vec<entry>>`：current 指针与并发更新更难维护 | 背景：现在一个 key 只挂一条会话，存不下多份对话。解决：改用 sessionId 当主键存档案、entry 自己记属于哪个作用域，一个作用域就能挂任意多份。 |
| **D9 运行时状态边界** | 哪些状态必须每会话一份、哪些可共享？ | 采用 per-session 状态收进 `SessionRuntime`、共享服务留共享层（再按 D16 细分 Global/Scope）；拒绝继续塞 `ChatContext` 单例 | 本仓 `api/chat/context.rs`(现 `cancel_token/plan_runtime/todos_runtime/context_state` 等单例)；codex `codex-rs/core/src/session/mod.rs`(每 thread 独立 `Session`)、hermes `gateway/session_context.py`(ContextVar per-task) | 设计：划清 per-session vs 共享边界；理由：这是并发正确性的根，混了必串台 | 继续塞 `ChatContext` 单例：并发时多会话共享同一 cancel/plan/context，必串台 | 背景：现在所有会话状态都在一个 `ChatContext` 单例里，没法并发。解决：把"每会话各一份"的状态搬进 `SessionRuntime`，"大家共用"的服务留共享层（共享层再按生命周期分 Global/Scope 两级，见 D16），边界划清才并发不打架。 |
| **D10 写并发协调（含 RMW 持锁修订）** | 多会话并发写 `sessions.json`/transcript 怎么不打架、不丢更新？ | 采用 `sessions.json` **整段 read-modify-write 持 `write_mutex`**（或 per-key 原子更新）+原子写、每条 transcript per-file 锁/串行队列；拒绝"锁只罩 save" / 全局大锁 / 无锁 | 本仓 `session_impl.rs`(`save_store` 已持 `write_mutex` 但 `load→改→save` 整段未锁 + `append_message` 单线程假设 TODO)；openclaw `src/config/sessions/transcript-append.ts`(per-path FIFO)、pi_rust `src/session.rs`(`{file}.jsonl.lock`) | 设计：不同 `.jsonl` 天然并行、同文件串行、`sessions.json` 把整段 RMW 圈进锁；理由：现有锁只保护 `save`，并发下两会话各读各写会丢更新——必须锁住"读+改+写"全程或退化为 per-key 原子改 | 锁只罩 save(现状)：RMW 间隙丢更新；全局大锁：并发退化为串行；无锁(pi-mono `session-manager.ts` 唯一文件名无锁)：TOCTOU、串文件 | 背景：现有 `write_mutex` 只罩住 `save_store`，但改 `sessions.json` 是"先读全量→改一处→写回"，两会话同时改会互相覆盖丢更新。解决：把整段读改写都圈进同一把锁（或只做单键原子更新）；每份对话文件再各自上锁互不挡，既并发又不损坏。 |
| **D11 同会话 turn 并发** | 同一会话能否同时跑两轮 turn？ | 采用同 sessionId 至多一个活跃 turn、新输入走 steer/排队；拒绝同会话并行多 turn | 本仓 `core/agent_loop/`(现单 turn 串行)；codex `codex-rs/core/src/session/mod.rs`(active_turn)、openclaw `src/auto-reply/reply/reply-run-registry.ts`(ReplyRunRegistry) | 设计：会话内串行、跨会话并行；理由：避免同会话双 loop 改同一 transcript 致历史交错不可复现 | 同会话并行多 turn：transcript 交错、结果不可复现 | 背景：我们要的并发是"多个会话同时"，不是"一个会话同时多轮"。解决：一个会话同一刻只跑一轮，新消息排队或打断当前轮，避免同一份历史被两个 loop 抢写。 |
| **D12 交付节奏** | 一次性大改还是分阶段？ | 采用分 P1/P2/P3 三阶段(见 §4.3)；拒绝一次性大爆改 | 本仓现状(单会话、改动横跨存储/CLI/运行时)；codex `codex-rs/app-server/src/in_process.rs`(embedded 渐进暴露，可分步参照) | 设计：数据模型先独立上线、并发后置；理由：P1 即可治串台、可回归、可回滚(G7) | 一次性大爆改：风险高、不可回滚、难定位回归 | 背景：改动横跨存储/CLI/运行时，一锅端风险高。解决：先上数据模型和命令(治串台)，再抽运行时，最后做真并发，每步能单独验收。 |
| **D13 持久数据模型·指针拆分** | `current` 指针与会话档案要不要拆成两表？ | 采用拆成 `current{key→id}` + `sessions{id→entry}` 两表；拒绝单表 / entry 内 `isCurrent` 布尔 | 本仓 `session_impl.rs::switch_current_to_session_id`(现 insert 覆盖即丢旧档)；codex `codex-rs/state/src/runtime.rs`(threads 档案表与 resume 指针分离)、hermes `gateway/session.py`(SessionStore 与 current 分离) | 设计：指针与档案解耦；理由：现状 1:1 把"哪个当前"和"档案"压在一条 entry，switch 覆盖即丢旧档、存不下多条；拆开后档案随 sessionId 永存、指针自由改 | 单表 `Map<key,entry>`：switch 必丢旧档、无法多会话；entry 内塞 `isCurrent` 布尔：多写点易不一致 | 背景：现在 switch 会覆盖那条 entry，旧会话档案就没了。解决：把"现在打开哪份"(便利贴)和"每份的档案"(档案柜)分两张表存，便利贴随便改、档案只增不被覆盖。 |
| **D14 运行时生命周期与驱逐** | 会话激活后何时留驻、何时移出 registry？ | 采用 `SessionRuntime` 常驻跨多轮、`AgentLoop` 按 turn spawn、turn 结束回 Idle 不移除；移出仅在 close 或超限/idle 的 LRU 驱逐 | 本仓 `core/agent_registry/mod.rs`(`MAX_CONCURRENT_AGENTS=16` 限流)、`api/chat/context.rs`(`register_root`/Drop 清理)；codex app-server `THREAD_UNLOADING_DELAY`(idle 30min unload) + `codex-rs/core/src/thread_manager.rs::remove_thread` | 设计：盒子常驻、loop 临时，超限按 LRU 驱逐 Idle；理由：保住已 hydrate 的上下文(热会话)，又给内存设上界 | 一轮结束即移出：每轮重建+重 hydrate、退化为单会话；永不驱逐：内存无界 | 背景：会话跑完一轮该不该清掉？清了下轮就得重读历史、白搭多 tab。解决：会话盒子打开期间一直留着，只在用户关掉、或太久没用且超额时按"最久没用"淘汰；淘汰只是省内存，数据还在。 |
| **D15 事件路由·sessionId demux** | 多会话并发时，事件/输出怎么路由到正确前端视图？ | 采用 EventBus / UI 事件统一携带 `sessionId`，前端按 `sessionId` demux；拒绝无标签广播或仅靠“当前前台”隐式猜测 | 本仓 `api/chat/context.rs`(`event_bus` 为共享服务)、`api/chat/run_loop/mod.rs`(现单前台渲染)；codex `codex-rs/app-server/src/thread_state.rs`(connection↔thread 订阅)、`codex-rs/tui/src/app.rs`(`thread_event_channels` / `active_thread_id`) | 设计：所有会话级事件显式带 `sessionId`，渲染层/协议连接再按 `sessionId` 过滤、缓冲、回放；理由：这是图 B“事件(带 sessionId)→前端按 sessionId demux”的协议化落点，不钉死就会在多会话时串输出 | 无标签广播：后台会话输出会污染前台；只靠“当前前台”隐式猜测：切 tab 时无法缓冲/回放后台事件，协议多连接也无法正确路由 | 背景：多个会话同时跑时，最容易串的不是存储，而是输出和通知。解决：从事件层就给每条消息贴上 `sessionId`，前端/协议按这个标签分发，谁的输出就回谁的 tab，后台的先缓冲，切回来再看。 |
| **D16 共享服务三层分层** | "共享层"该不该再细分？哪些是进程级、哪些其实带项目/会话态？ | 采用 **GlobalServices / ScopeServices / SessionRuntime** 三层：进程级共享放 Global（含 `gate` **经评审保留 Global**——用户授权代表全局意图）；带项目态的（`checkpoint`、project skill 层）按 work_tree/项目缓存放 Scope；带会话态的（含 `read_file_state`、per-session bash 任务）放 SessionRuntime；拒绝把全部塞进单层 `SharedServices` | 本仓 `api/chat/context.rs`(现 `checkpoint/gate/skill_set/bash_task_registry/read_file_state` 全混在 `ChatContext`)、`core/tools/pipeline/read_state.rs`("每会话独立"已写死)、`core/skill/discovery.rs`(`skill_roots` 依赖 `agent_workspace_dir`)；codex `codex-rs/core/src/session/mod.rs`(每 thread 独立 `Session`)、hermes `gateway/session_context.py`(per-task ContextVar) | 设计：按"无状态 / 项目态 / 会话态"三分；理由：`checkpoint`/project skill 绑 work_tree，放 Global 会串项目；`read_file_state` 放共享会让 A 的 read 替 B 满足"先读后写"门槛；`gate` 虽带 `SessionGrants`，但产品上认定授权=全局意图，保留 Global 简单且符合直觉 | 单层 `SharedServices`(上一版)：把项目态/会话态误当无状态共享→串项目、串"先读后写"门槛；把 `gate` 拆成 per-session：与"授权代表全局意图"的产品判断相悖、徒增复杂 | 背景：上一版只分"共享 vs 每会话"两层，但"共享"里混着其实绑项目（checkpoint）和绑会话（read 指纹）的东西，照搬会串台。解决：共享层再切两刀——进程级的归 Global、按项目共用的归 Scope、剩下带会话态的归 SessionRuntime；`gate` 按你的判断（一处授权=全局意图）留在 Global。 |
| **D17 LLM 共享实例 + 并发准入** | 多会话调 LLM：每会话 new 一个，还是共享一个 + 限流？ | 采用**单一共享 `Arc<dyn LlmProvider>` + `max_concurrent_requests` 信号量做全局准入**；拒绝 per-session new provider / 全局单队列串行 | 本仓 `core/llm/provider.rs`(`LlmProvider: Send+Sync`)、`core/llm/openai.rs`(`semaphore: Option<Semaphore>`、`chat_stream` 取回流后 `_permit` 即 drop)、`infra/config/types/llm.rs`(`max_concurrent_requests`,默认4)；codex `codex-rs/core/src/client.rs`(共享 client)、openclaw lane 限流 | 设计：共享 provider，准入控制集中在信号量；理由：上游配额/费用按 API key **全局**算，per-session 实例会让预算变 N×、连接池翻 N 份且仍管不住账号级限速；流式 permit 在拿到响应头后即释放→token 外流期不占额度→多会话**不会互相卡**，信号量只压"同时发起新请求"的瞬时并发；"串台"由 D15 事件带 sessionId 解决，与 LLM 实例无关 | per-session new provider：全局预算失效(N×)、连接池浪费、且不解决串台；全局单队列串行：B 必须等 A 整条流跑完→正是要避免的"排队卡死" | 背景：担心"A 会话流式输出时 B 卡住"。解决：共享一个 LLM 客户端，用一个全局信号量做准入；流式一拿到响应头就把名额还回去，所以同时在跑的流可以远超并发数，只有"同一瞬间发起新请求"超过 `max_concurrent_requests` 才短暂等待。把它设成 ≥ 期望并发会话数即可，它是全局成本闸门、不是 per-session 节流。 |
| **D18 checkpoint 作用域与 per-session 回退** | checkpoint 多会话怎么放、回退怎么不波及别的会话？ | 采用 **按 work_tree 缓存一个 store 实例、同项目所有会话共用**（保住总锁串行）；restore 用 `RestoreOptions.paths` **收窄为"本会话改过的文件"**；record 仍 `git add -A` 整树快照保一致性；拒绝 per-session new store | 本仓 `core/checkpoint/shadow_git.rs`(`git_dir=checkpoints_root/workdir_hash(work_tree)`、`lock: Mutex<()>` 串行 record/restore、`record_impl` 整树 `git add -A`、`restore` 走 `git checkout <commit> -- <pathspecs>`)、`core/checkpoint/types.rs`(`RestoreOptions.paths`)、`api/chat/commands/cmd_restore.rs`(已透传 paths)；codex 每 thread 维度回滚、cc-fork `src/utils/sessionStorage.ts`(按 cwd 分仓) | 设计：store 实例按项目缓存共用、回退按会话改动收窄；理由：`git_dir` 由 work_tree 推出——per-session new 会让同项目 N 个 store 指向**同一磁盘仓库却各持独立 `Mutex`**，跨会话串行失效→并发 `git commit/checkout` 撞 `index.lock`、可能损坏；共用一个实例则总锁天然串行（只排队不卡死）；restore 传 `paths=本会话改过的文件` 即只回退自己碰过的，不动别会话独占文件 | per-session new store：同项目多实例同 `git_dir`、各自独立锁→串行保证失效、git 损坏；整树无差别 restore：回退 A 时把 B 改过的文件一起退掉 | 背景：新架构下"每会话自带一个 checkpoint"反而危险，且整树回退会波及别的会话。解决：同一个项目共用一个 checkpoint 仓库实例（一把锁把并发 store 排好队，不会卡死），回滚时只挑"这个会话改过的文件"还原。**残留风险**：A、B 同时改了同一个文件时无法隔离——单工作树一个文件只有一份磁盘态，回退该文件必然丢另一会话的改动（需独立 worktree 才能根治，见 §12）。 |

### 4.2 实施点（按阶段闭环）

专业描述：`§4.1` 回答“为什么选这些设计”，`§4.2` 回答“这些设计怎么分批合进主线、每批交什么、改哪儿、拿什么验”。因此这里**不按文件流水账**写，而按**实施点 / 阶段边界**写：先给总表钉死交付边界、主要代码落点和验收锚点，再用 `§4.2.x` 小结把每一行展开成“技术要点 + ASCII”。

说人话：把这一节当成施工排期表来看，不是源码索引。上面的决策表是在回答“选哪条路”；这一节是在回答“先修哪堵墙、后铺哪根线、每次合入到底交什么”。表格是总包清单，后面的 `4.2.1 / 4.2.2 / 4.2.3` 是施工图。

> 与 `§4.1` 的映射：`4.2.1 ↔ D1/D2/D3/D4/D5/D6/D8/D13`，`4.2.2 ↔ D9/D10/D16/D17/D18`，`4.2.3 ↔ D7/D11/D14/D15`。阶段标签 `[P1]/[P2]/[P3·TUI]` 与 `§4.3` 对应。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **4.2.1 P1 作用域与会话索引重构** | 交付 `SessionMode` / `session_key_for` / `fnv1a_hex`；新 `sessions.json` 结构（`sessions{id→SessionEntry}` + `current{key→id}`）；`SessionManager::new_scoped`；`tomcat claw` / `tomcat code` / scope-aware `session list/new/switch`；`[session].default_mode` 与文档同步；开发阶段旧 `sessions.json` 遇旧结构或反序列化失败时直接重建 | `src/core/session/{scope.rs,store.rs,manager/session_impl.rs}`；`src/api/cli/{mod.rs,claw_cmd.rs,code_cmd.rs,session_cmd.rs}`；`src/infra/config/*`；`docs/{user-guide.md,architecture/session-storage.md,architecture/work-dir-and-data-layout.md}` | 见 `§11`：`session_key_for` 单元、旧 `sessions.json` 直接重建单元、A→B 跨项目隔离 / claw 跨目录连续 / 同 scope `switch` 集成 | 先把“文件夹名怎么算”“档案柜怎么存”“用户从哪个命令进门”三件事做对，本期就能先把串台问题治住。 |
| **4.2.2 P2 SessionRuntime 抽离与写安全** | 交付 GlobalServices/ScopeServices/SessionRuntime 三层边界（D16）；`ChatContext` 去单例化；每会话 `context_state/cancel_token/plan/todos/read_file_state` 下沉；`checkpoint` 改按 work_tree 缓存共用（D18）；`sessions.json` 整段 RMW 持锁（D10）+ transcript per-file 锁；LLM 维持共享实例+准入（D17）；保持 CLI 交互范式不变 | `src/api/chat/{context.rs,session_runtime.rs}`；`src/core/session/manager/session_impl.rs`；`src/core/checkpoint/*`（按 work_tree 缓存 store）；必要时补 `src/core/session/manager/*tests*` / `src/api/chat/*tests*` | 见 `§11`：并发(P2) 多线程 append 不丢/不串、`sessions.json` RMW 不丢更新、同项目并发 record 串行不卡、per-session paths restore 不波及他会话；现有 chat / plan / checkpoint suite 回归 | 先把每个会话的“独立工位”和写锁装好、把 checkpoint 按项目共用一份，外面看起来还像现在的单会话 CLI，但底下已经不会为将来并发打架。 |
| **4.2.3 P3·TUI 多会话调度与事件 demux**（本期不做，TODO-1） | 交付 `SessionRuntimeRegistry`；`spawn_turn/abort/close/evict`；EventBus 事件统一带 `sessionId`；前台/后台事件缓冲与回放；`session open/ps/attach`；必要时 TUI tab/pane 与可选 app-server | `src/api/chat/{session_registry.rs,run_loop/mod.rs,context.rs}`；`src/api/cli/{chat_cmd.rs,session_cmd.rs}`；后续 TUI 层（待立项） | 见 `§11`：并发(P3) 双会话并行、`abort` 单命中、前台切换后后台事件可回放；TUI 立项后补 UI 级验收 | 真正“像 Cursor 一样多 tab”是在这一步，不是本期。前两步先打地基，这一步才把多会话能力暴露给用户。 |

#### 4.2.1 P1：作用域与会话索引重构（D1/D2/D3/D4/D5/D6/D8/D13）

专业描述：本实施点的目标是**先解决串台，再把新结构和新入口钉死**。边界也很明确：它只改“会话按什么分组、`sessions.json` 怎么索引、CLI 怎么进门”，**不**引入并发 UI，也**不**改变现有 readline 的单前台交互范式。核心交付是把“全局 claw / 按项目 code”的作用域键钉死，把 `sessions.json` 升级为 `sessions{id→SessionEntry}+current{key→id}`，再把 `claw/code/session` 命令面连起来；开发阶段对旧 `sessions.json` 不做兼容迁移，统一直接重建成新结构。

说人话：这一阶段就是先把“文件夹名”和“档案柜”搭好。用户最痛的是“换项目还记着上一个项目”，所以先别急着做多 tab，先让 `code` 真按项目隔离、`claw` 真全局连续；至于开发期残留的旧 `sessions.json`，这轮不做迁移，统一按新结构重建，先把实现复杂度降下来。

技术要点：
- **`scope.rs`**：新增 `SessionMode { Code, Claw }`、`project_root(cwd)`、`fnv1a_hex()`、`session_key_for(mode,cwd)`；`Claw` 恒 `agent:main:main`，`Code` 取 `proj:<hash(git根|cwd)>`。
- **`store.rs`**：`SessionStore` 升级为：
  - `sessions: HashMap<sessionId, SessionEntry>`
  - `current: HashMap<sessionKey, sessionId>`
  同时给 `SessionEntry` 增 `session_key` 字段；开发阶段读到旧结构、空文件或反序列化失败时，直接重建为新结构，不做 v1→v2 迁移。
- **`session_impl.rs`**：新增 `session_key: String` 成员与 `new_scoped()`；`current_session_key()` 不再写死常量；`ensure_current_session(cwd)` 改为“查 `current[key]` 命中则复用，否则 create + 写指针”；`list/new/switch/delete/archive` 改为按 `session_key` 过滤或改指针，而不是覆盖整条记录。
- **CLI / config / docs**：
  - `api/cli/mod.rs` 增 `Claw { resume }` / `Code { resume }`
  - 隐藏兼容入口 `chat` 改接 `code`
  - `claw_cmd.rs` / `code_cmd.rs` 用 `session_key_for + new_scoped`
  - `session_cmd.rs` 支持 `--scope claw|code`
  - `infra/config` 增 `[session].default_mode` 与 `TOMCAT_SESSION_MODE`
  - 同步 `user-guide.md` / `session-storage.md` / `work-dir-and-data-layout.md`

```text
用户键入命令
   │
   ├─ tomcat claw ───────────────┐
   └─ tomcat code ───────────────┤
                                 ▼
                         scope::session_key_for(mode,cwd)
                                 │
                  claw → agent:main:main
                  code → agent:main:proj:<hash(git根|cwd)>
                                 │
                                 ▼
                    SessionManager::new_scoped(sessions_dir,key)
                                 │
                                 ▼
                        sessions.json (v2)
                  ┌────────────────────────────────┐
                  │ sessions: { id → SessionEntry }│
                  │ current:  { key → id }         │
                  └────────────────────────────────┘
                                 │
             ensure_current_session(key,cwd) → 命中 current? 复用 : 新建
                                 │
                                 ▼
                           <sessionId>.jsonl
```

#### 4.2.2 P2：SessionRuntime 抽离与写安全（D9/D10/D16/D17/D18）

专业描述：本实施点不向用户新增“多会话 UI”，它做的是**并发前的地基**：把当前 `ChatContext` 里混在一起的成员按生命周期拆成 **GlobalServices / ScopeServices / SessionRuntime** 三层（D16），把 transcript append 从“默认单线程”升级到“按 `<sessionId>.jsonl` 串行”，把 `sessions.json` 的写从“只罩 save”升级为“整段 RMW 持锁”（D10），并把 `checkpoint` 从“跟单 `ChatContext` 走”改为“按 work_tree 缓存、同项目共用一份”（D18）。LLM 维持单一共享实例 + 信号量准入不变（D17）。这样一来，就算 P2 结束时用户仍只看到单前台会话，底层也已经具备“多个会话各有自己的工位、写各自的文件不会打架、回退不波及他会话”的前提。

说人话：这一步是在后厨装隔板和上锁。客人暂时还只看到一个窗口，但每个会话的锅碗瓢盆已经分开摆了（自己的上下文、计划、取消信号、"先读后写"指纹都各放各的），共用的灶台（LLM、工具）和按桌共用的工具车（按项目共用的 checkpoint）也归好位，不会再把 A 会话的状态和 B 会话混在一锅里，也不会 A 一回退把 B 的菜也倒了。

技术要点：
- **`session_runtime.rs`**：新增 `SessionRuntime`，至少承载：
  - `session_id/session_key/cwd`
  - `context_state`
  - `cancel_token`
  - `plan_runtime` / `todos_runtime`
  - `follow_up_queue` / `completion_routes`
  - `openai_files_runtime`
  - `status` / `loop_handle`
- **`context.rs` 拆为三层（D16）**：现 `ChatContext` 不再是"单会话装配根"，而是按生命周期把成员归到三层：
  - **GlobalServices（进程级，1 份）**：`llm`（+并发准入 D17）/ `tool_registry` / `event_bus`（事件带 sessionId D15）/ `SessionManager`（RMW 持锁 D10）/ `gate`（经评审保留 Global——用户授权代表全局意图）/ `audit` / `web_*`。
  - **ScopeServices（按 work_tree/项目缓存，同项目多会话共用 1 份）**：`checkpoint_store`（每 work_tree 一个实例 D18）/ project skill 层。
  - **SessionRuntime（每会话 1 份）**：`context_state` / `cancel_token` / `plan_runtime` / `todos_runtime` / `follow_up_queue` / `completion_routes` / `openai_files_runtime` / `read_file_state`（"先读后写"指纹表，不可共享 D16）/ per-session bash 任务。
  - `agent_workspace_dir` 也不再取全局 cwd，而是从会话自己的 `cwd` 来。
- **`session_impl.rs` 写锁（D10）**：
  - `sessions.json`：把 `load → mutate → save` **整段 read-modify-write 都圈进 `write_mutex`**（或改 per-key 原子更新），不再只罩 `save_store`——否则两会话各读各写会丢更新
  - `append_message` 去掉“假设单线程串行”的前提
  - 针对 `<sessionId>.jsonl` 引入 per-file `Mutex`（或等价串行队列），必要时再叠 fs4 多进程兜底
- **`read_file_state` 归 per-session（D16）**：沿用 `read_state.rs` 既有"每会话独立"设计，随 `SessionRuntime` 走；**严禁**留在共享层（否则 A 会话的 read 会替 B 会话满足"先读后写"门槛，导致 B 没读就能改）。
- **CLI 维持现状**：这一步不碰 `chat_cmd.rs` 的 `block_on + rustyline` 模式，用户感知应尽量保持不变。

```text
            现状（单 ChatContext 单例）→ 拆为三层（D16）
┌──────────────────────────────────────────────────────────────┐
│ GlobalServices(进程级共享，1 份)                                │
│   llm(+并发准入 D17) / tool_registry / event_bus(带 sessionId) │
│   gate(授权=全局意图,保留 Global) / SessionManager(RMW持锁 D10) │
│   / audit / web_*                                              │
├──────────────────────────────────────────────────────────────┤
│ ScopeServices(按 work_tree/项目缓存，同项目多会话共用 1 份)      │
│   checkpoint_store(每项目一个实例,保串行 D18) / project skill   │
├──────────────────────────────────────────────────────────────┤
│ SessionRuntime(每会话 1 份)                                    │
│   session_id / session_key / context_state / cancel_token      │
│   plan / todos / follow_up / completion_routes                 │
│   read_file_state(先读后写指纹,D16) / bash tasks(per-session)  │
└───────────────────────────────┬──────────────────────────────┘
                                 ▼
              append_message(<sessionId>.jsonl)  per-file 锁
              sessions.json：整段 RMW 持 write_mutex（D10）
```

#### 4.2.3 P3·TUI：多会话调度与事件 demux（D7/D11/D14/D15，本期不做）

专业描述：这一实施点才是“用户真正看到多会话并发”的部分，因此它被明确放到 **P3·TUI**，而不是偷偷塞进 P1/P2。它引入 `SessionRuntimeRegistry`，把前台输入从“唯一 readline”改成“路由到目标 `sessionId` 的事件流”，并把 EventBus 事件统一贴上 `sessionId`，由前端做 demux/缓冲/回放。它同时承接 D11（同 session 单活跃 turn）、D14（runtime 常驻与驱逐）、D15（事件按 `sessionId` 路由）。

说人话：这一步才是多 tab 真正“长出来”的地方。前两步是在打地基、分工位、上锁；这一阶段才会出现“我能同时开 X-1 和 X-2 两个会话，切走前台后另一个还在后台跑，回来还能看回放”。因为这需要 TUI 交互层配合，所以本期明确不做，只留 TODO-1。

技术要点：
- **`session_registry.rs`**：新增 `SessionRuntimeRegistry { map: Mutex<HashMap<sessionId, Arc<SessionRuntime>>> }`
  - `open(session_id)`
  - `spawn_turn(session_id,input)`
  - `abort(session_id)`
  - `active_ids()`
  - `close(session_id)`
  - `evict_idle_lru()` / idle unload
- **`run_loop/mod.rs` / `chat_cmd.rs`**：
  - 不再把 readline 当作“唯一会话的主循环”
  - 输入层只做“把命令/提示词路由到哪个 `sessionId`”
  - `turn` 通过 `registry.spawn_turn()` 异步跑
- **事件路由**：
  - EventBus 统一携带 `sessionId`
  - 前端只渲染当前前台会话
  - 后台会话事件进入 buffer，attach 时回放
- **CLI / TUI 暴露**：
  - `session open / ps / attach`
  - 真正的多 tab/pane 需要 TUI 框架（TODO-1）
  - 可选 app-server 留作后续增强，不是本期阻塞项

```text
TUI / 多 tab（TODO-1）
   │
   ├─ 前台 tab X-1 输入 ──────────────┐
   ├─ 前台切到 X-2 attach ───────────┤
   └─ open / ps / abort 命令 ────────┤
                                      ▼
                         SessionRuntimeRegistry
                   { X-1 → Runtime , X-2 → Runtime , ... }
                           │            │
                    spawn_turn      spawn_turn
                           ▼            ▼
                     AgentLoop(X-1)  AgentLoop(X-2)
                           │            │
                           └──────┬─────┘
                                  ▼
                         EventBus(event.sessionId)
                                  │
                 ┌────────────────┴────────────────┐
                 ▼                                 ▼
           当前前台直接渲染                    后台会话先缓冲
                                                   │
                                                   ▼
                                             attach 时回放
```

### 4.3 分阶段交付（D12）

| 阶段 | 范围 | 上线后效果 | 风险 |
|------|------|-----------|------|
| **P1 数据模型 + 命令** | 实施点 1–3、9–13；运行时仍单前台会话 | 解决跨项目串台（G1/G2）+ 一个 scope 多会话可 `list/switch`（G3）+ 兼容旧数据（G6） | 低（纯存储/CLI 重构，可独立回归） |
| **P2 抽 SessionRuntime + transcript 锁** | 实施点 4、5、7；registry 已存在但仍单会话激活 | 并发地基就位、写安全（G5）；行为对用户不变 | 中（`ChatContext` 拆分面广，靠测试护栏） |
| **P3·TUI 并发调度 + 输入解耦**（本期不做，TODO-1） | 实施点 6、8；TUI 框架重写渲染/输入；可选 app-server | 单进程多会话真并发（G4）、像 Cursor 多 tab | 高（输入模型 + 输出 demux 改动大；**依赖 TUI 立项**，本期仅留地基不暴露） |

> **阶段与 CLI 的关系**：P1/P2 **不改 CLI 交互范式**（仍单前台会话、阻塞 readline），上线后用户感知仅"换项目不串台 + 能 `list/switch` 多会话"；P3·TUI 才把"同时看多个会话"暴露给用户，**本期不做**（§3.2 TODO-1）。

## 5. 协议与接口（Interfaces）

### 5.1 CLI 协议

```text
tomcat claw [--resume]        # 全局会话；sessionKey = agent:main:main
tomcat code [--resume]        # 项目会话；sessionKey = agent:main:proj:<hash(git根|cwd)>
tomcat                        # 走 [session].default_mode（默认 code）

tomcat session list   [--scope claw|code]   # 列当前/指定 scope 下的多个 sessionId
tomcat session new    [--scope claw|code] [--title T]
tomcat session switch <sessionId>           # 改 current[sessionKey] 指针
tomcat session delete <sessionId>
tomcat session archive <sessionId>
# [P3·TUI] 并发（本期不做，TODO-1：依赖 TUI 框架重写交互层）：
tomcat session open   <sessionId>           # 激活进 registry（不切前台）
tomcat session ps                            # 列出 registry 中活跃/后台会话及 turn 状态
tomcat session attach <sessionId>           # 把某后台会话切为前台
```

说人话：`claw`/`code` 决定"文件夹"，`session *`（list/new/switch/delete/archive）管"文件夹里的多份对话"——这些 **P1 就有**、走现状 CLI；`open/ps/attach` 是"同时跑哪些、看哪个"，**标 `[P3·TUI]`、本期不做**，等 TUI 立项再开放（见 §3.2 TODO-1）。本期 CLI 交互范式不变。

### 5.2 `sessions.json` 数据协议（新结构，D8 / D13）

`sessions.json` 不存对话历史（历史在各 `<sessionId>.jsonl`），它是**磁盘上的会话索引**，干两件事：(A) 记每个 scope"当前续哪个会话"的**指针**；(B) 记每个会话的**元数据档案**。现状因为是 `Map<sessionKey, entry>` 的 1:1，这两件事被压在同一条 entry 上——于是 `switch` 覆盖那条 entry 时旧档案就没了，一个 scope 也存不下第二条。新结构把两职拆成两张表（D13）：

```text
v1（现状，1:1，两职揉在一条 entry）            v2（本方案，两职拆开）
┌───────────────────────────────┐            ┌─────────────────────────────────────────────┐
│ sessions.json                  │            │ sessions.json                                │
│  "agent:main:main": {          │            │  sessions:  { <sessionId> → SessionEntry }   │ ← (B)档案，主键=sessionId，永存
│     sessionId, cwd,            │  ── 拆 ──▶ │     "…_bd20": {sessionId, sessionKey, cwd…}  │   一个 scope 可有多条
│     updatedAt, tokens…         │            │     "…_ef34": {sessionId, sessionKey, cwd…}  │
│  }   ↑ 既是"当前指针"           │            │  current:   { <sessionKey> → <sessionId> }   │ ← (A)指针，每 scope 一个，可改
│      又是"该会话的档案"          │            │     "agent:main:main"      → "…_bd20"         │   switch 只改这里，不动档案
└───────────────────────────────┘            └─────────────────────────────────────────────┘
   切会话=覆盖 entry → 旧档案丢失                 切会话=改 current 指针 → 旧档案仍在 sessions
```

说人话：把"现在打开哪份"（current 指针，便利贴）和"每份对话的卡片信息"（SessionEntry 档案，档案柜）分开存。便利贴随便改，档案柜里的卡片只增不被覆盖——这才装得下"一个文件夹多份对话"，也是 §1 里 `current[sessionKey]` 与 `SessionEntry` 被列成两个独立术语的原因。

```json
{
  "sessions": {
    "1733800000_ab12cd": { "sessionId": "1733800000_ab12cd", "sessionKey": "agent:main:proj:7f3a9c1b", "cwd": "/Users/me/projX", "updatedAt": 1733801234, "modelOverride": "gpt-5.4" },
    "1733801111_ef34gh": { "sessionId": "1733801111_ef34gh", "sessionKey": "agent:main:main",          "cwd": "/Users/me/notes", "updatedAt": 1733801299, "lastCheckpointId": "ck_1733801111" }
  },
  "current": {
    "agent:main:proj:7f3a9c1b": "1733800000_ab12cd",
    "agent:main:main":          "1733801111_ef34gh"
  }
}
```

- 列某 scope 的会话：`sessions.values().filter(|m| m.sessionKey == key)`。
- transcript 路径不变：每 `sessionId` 一份 `<sessionsDir>/<sessionId>.jsonl`。
- 开发阶段遇到旧结构/坏文件时直接重建新结构，不做自动迁移（§4.2.1，G6）。

### 5.3 sessionKey 语法（不变，D2/D3）

```text
agent:<agentId>:<channelKey>
  Claw → channelKey = "main"                    → agent:main:main
  Code → channelKey = "proj:<fnv1a_hex(根)>"     → agent:main:proj:7f3a9c1b
```

### 5.4 运行时接口（P2/P3）

```text
SessionRuntimeRegistry::open(session_id) -> Arc<SessionRuntime>
SessionRuntimeRegistry::spawn_turn(session_id, input)        // tokio::spawn AgentLoop（用本会话 cancel_token）
SessionRuntimeRegistry::abort(session_id)                    // 仅中断该会话
SessionRuntimeRegistry::active_ids() / close(session_id)
EventBus 事件统一携带 sessionId（D15）；前端按"当前前台 sessionId"demux

// 共享服务三层（D16）的获取边界：
GlobalServices  : 进程级 1 份（llm+准入 D17 / tool_registry / event_bus / gate / SessionManager / audit）
ScopeServices   : checkpoint_store_for(work_tree) → 同项目共用 1 个实例（D18，内部总锁串行）
SessionRuntime  : 每会话 1 份（含 read_file_state、per-session bash 任务）
restore(session_id, paths=本会话改过的文件)                  // D18：按会话收窄，不波及他会话独占文件
```

## 6. 文件职责总览（One-Glance Map）

专业描述：自顶向下覆盖本方案改到/新增的每个 `*.rs`；节点内列关键类型/函数/行为，箭头表"调用/装配/落盘"方向。阶段标 `[P1]/[P2]/[P3·TUI]`，性质标 `新/改/复用`（实施期若签名最终未改，在节点补 `【未改签名】` 标签，遵 `ARCHITECTURE_SPEC §6` 硬约束 6）。

```text
用户入口  api/cli/
┌────────────────────────────────────────────────────────────────────────┐
│ mod.rs [P1·改]   Commands::{Claw{resume}, Code{resume}}（隐藏兼容别名 Chat→Claw）│
│   ├─▶ claw_cmd.rs / code_cmd.rs [P1·新]（由 chat_cmd.rs 派生）            │
│   │       定 SessionMode → scope::session_key_for + SM::new_scoped        │
│   ├─▶ session_cmd.rs [P1·改]  list/new/switch/delete/archive 按 scope；    │
│   │       --scope claw|code 覆盖；list 展示同 scope 多 sessionId           │
│   └─▶ chat_cmd.rs [现状；P3·TUI 才改]  block_on(chat_loop)+readline 不动   │
└───────────────────────────────┬────────────────────────────────────────┘
                                 │ 装配 / 驱动
                                 ▼
运行时装配  api/chat/
┌────────────────────────────────────────────────────────────────────────┐
│ context.rs [P2·拆,D16]  ChatContext → 三层：                              │
│   ① GlobalServices(llm+准入D17/tool_registry/event_bus/gate(全局)/        │
│      SessionManager(RMW持锁D10)/audit/web_*)                              │
│   ② ScopeServices(按 work_tree 缓存:checkpoint_store D18 / project skill) │
│   ③ per-session 字段下沉 SessionRuntime；agent_workspace_dir 随会话 cwd   │
│        │ 持有                                 │ 创建/激活/驱逐            │
│        ▼                                       ▼                         │
│ session_runtime.rs [P2·新]            session_registry.rs [P3·新]         │
│   SessionRuntime{ cancel_token,         SessionRuntimeRegistry{          │
│     context_state, plan/todos_runtime,    map: Mutex<HashMap<id,Arc<..>>> │
│     follow_up_queue, completion_routes,   } open / spawn_turn / abort /   │
│     read_file_state(D16), bash tasks,     active_ids / close / evict(LRU,D14)│
│     loop_handle, status }  常驻跨多轮                                     │
└───────────────┬───────────────────────────────────┬─────────────────────┘
   spawn_turn    │ (每 turn 起一个)                   │ 读写
   (per-turn)    ▼                                   ▼
推理 core/agent_loop·agent_registry        存储 core/session/
┌──────────────────────────────┐  ┌──────────────────────────────────────┐
│ agent_loop/ [复用]           │  │ scope.rs [P1·新]  SessionMode /        │
│   AgentLoop（单 turn 三层循环）│  │   project_root(git rev-parse) /        │
│   用 SessionRuntime.cancel    │  │   session_key_for(mode,cwd) / fnv1a_hex│
│ agent_registry/mod.rs [复用]  │  ├──────────────────────────────────────┤
│   tokio::spawn 子 agent,      │  │ store.rs [P1·改]  SessionStore{        │
│   MAX_CONCURRENT_AGENTS=16,   │  │   sessions:{id→SessionEntry},          │
│   abort 级联（registry 借鉴）  │  │   current:{key→id} }; v1→v2 兼容反序列 │
└──────────────────────────────┘  │   SessionEntry +session_key 字段       │
                                   ├──────────────────────────────────────┤
                                   │ manager/session_impl.rs [P1·改+P2]     │
                                   │  +session_key 字段; new_scoped;        │
                                   │  current_session_key()->&str;          │
                                   │  ensure_current_session(cwd);          │
                                   │  list/new/switch/delete/archive 按 key;│
                                   │  [P2] append_message 去单线程假设       │
                                   │        +per-file 锁(D10)               │
                                   └──────────────────┬───────────────────┘
                                                      │ 原子写 / append
                                                      ▼
                                   sessions.json  +  <sessionId>.jsonl

配置  infra/config/ [P1·改]   [session].default_mode / [session].max_active_runtimes /
   idle_unload_secs；env TOMCAT_SESSION_MODE   ──▶ 兜底/覆盖 SessionMode 与 registry 上界

配套 tests/（独立目录，遵 UNIT_TEST_LAYOUT_SPEC）：
  · core/session/tests/scope_test.rs            session_key_for：claw 恒定 / code 同仓库根同 key
  · core/session/tests/store_test.rs            v1→v2 反序列化无损 + current 指针迁移
  · src/api/chat/tests/runtime_split_test.rs    [P2] checkpoint store 复用 + SessionRuntime/read_state 隔离
  · src/api/chat/commands/tests/cmd_restore_test.rs [P2] changedPaths 注记 + /restore 默认收窄
  · tests/session_tests.rs                      集成：A→B 跨项目隔离 / claw 跨目录连续
  · tests/session_concurrency_tests.rs          [P2] sessions.json RMW / transcript per-file 并发护栏
```

阅读顺序（说人话）：从上往下读一遍就是一条完整链路——**用户敲 `claw`/`code`（cli）→ 用 `scope.rs` 算出 sessionKey → `context.rs` 装配出共享服务和会话盒子 → 存储层 `store.rs/session_impl.rs` 落到 `sessions.json` 和各自 `.jsonl`**。P1 只动左上角命令和右侧存储两块（治串台、可多会话列表），中间的 `session_runtime/registry` 是 P2/P3 的并发地基，`chat_cmd.rs` 的交互范式本期不动（P3·TUI 才改）。看节点上的 `[P1/P2/P3·TUI]` 标签就知道哪块本期落、哪块留到 TUI。

### 6.1 一句话速记

- **两轴**：轴一=会话怎么分组（claw 全局 / code 按项目）；轴二=多会话怎么并发（单进程内存 registry + 多 loop）。
- **轴一**：`sessionKey` 从"1:1 指一个会话"升级为"1:N 的作用域键"；claw 永远 `agent:main:main`，code 每项目 `agent:main:proj:<hash>`；一个 scope 下多个 sessionId。
- **轴二**：对标 codex——`SessionRuntimeRegistry: HashMap<sessionId, Arc<SessionRuntime>>`，盒子常驻、`AgentLoop` 按 turn 起，**单进程不多进程**。
- **共享分三层（D16）**：`GlobalServices`（进程级：llm+准入 D17 / 工具 / event_bus / gate（全局）/ SessionManager）｜`ScopeServices`（按 work_tree 共用：checkpoint D18 / project skill）｜`SessionRuntime`（每会话：含 read_file_state、bash 任务）。LLM 共享一个实例 + 信号量准入，流式 permit 早释放→多会话不互卡。
- **数据**：`sessions.json` 改为 `sessions{sessionId→SessionEntry}` + `current{sessionKey→sessionId}`，写时整段 RMW 持锁（D10）；transcript 每会话一份 `.jsonl`、并发加 per-file 锁；checkpoint 按项目共用、restore 按本会话 paths 收窄（D18）。
- **落地**：P1 数据模型+命令（治串台、可多会话列表）→ P2 抽 SessionRuntime+写锁（地基）→ P3·TUI 并发调度+输入解耦（真多 tab，本期不做）。
- **兼容**：旧 `agent:main:main` 会话自动并入 claw，零脚本。

## 7. 关键流程时序（多会话）

### 7.1 启动续接（P1，单会话）

```text
tomcat code (in projX)
  └▶ session_key_for(Code, cwd) = agent:main:proj:7f3a9c1b
       └▶ SessionManager::new_scoped(dir, key)
            └▶ ensure_current_session(cwd):
                 current[key] 命中且未归档? ── 是 ─▶ 复用该 sessionId，hydrate 其 .jsonl
                                            └ 否 ─▶ create_session → 写 current[key] → 空历史
```

### 7.2 并发跑两个会话（P3，对标 codex）

```text
前台输入：":new" ─▶ registry.open(X-2) + attach 前台
用户在 X-2 发问 ─▶ registry.spawn_turn(X-2, q)  ─┐  tokio task #2: AgentLoop(X-2)
":attach X-1"  ─▶ 切前台到 X-1（X-2 task 仍在后台跑）│
X-1 发问       ─▶ registry.spawn_turn(X-1, q)  ─┤  tokio task #1: AgentLoop(X-1)
                                                 ▼
        两个 AgentLoop 各用自己的 cancel_token / context_state / plan/todos 并行
        事件带 sessionId → 前端只渲染当前前台(X-1)，X-2 事件缓冲；attach 时回放
        各自写各自的 <sessionId>.jsonl（per-file 锁）；sessions.json 写经 write_mutex
```

### 7.3 中断隔离（D11）

```text
Ctrl-C / :abort 当前前台 ─▶ registry.abort(前台 sessionId) ─▶ 只 cancel 该会话 turn
其他后台会话不受影响（各自独立 cancel_token）
```

## 8. 会话生命周期状态机（P3）

```text
              new/open                spawn_turn               turn 结束/abort
   (none) ───────────────▶ Idle ───────────────▶ Running ───────────────▶ Idle
                            │  ▲                    │                        ▲
                    attach  │  │ detach(切走前台)    │ 切走前台但 turn 未完     │
                            ▼  │                    ▼                        │
                         Foreground            Background(turn 继续) ────────┘
                            │ close/delete
                            ▼
                         Closed(出 registry；transcript 保留)
```

说人话：会话有"在不在 registry（激活）"和"是不是前台"两个维度。注意 `Running → Idle` 这条边——**一轮 turn 结束只是 AgentLoop task 终止、状态回 Idle，会话仍留在 registry**（盒子常驻、上下文还热），绝不是"跑完就出表"。真正"出表"（`Closed`）只有两条路：用户显式 `close/delete`，或空闲驱逐（§9：超 `max_active_runtimes` 时按 LRU 淘汰最久未用的 **Idle** 会话 / idle 超时，对标 codex 30 分钟 unload）。出表≠删数据：`.jsonl` 与 `sessions.json` 档案都在，下次 `open` 重新 hydrate 即可。后台会话的 turn 也不会因为你切走前台就停，跑完照样落盘、可经 follow-up 通知——这正是"多 tab"的关键。

## 9. 配置项

| 配置 | 默认 | 说明 |
|------|------|------|
| `[session].default_mode` | `code` | 裸 `tomcat` / 歧义入口时的作用域策略 |
| `TOMCAT_SESSION_MODE` (env) | — | 覆盖 default_mode（`code`/`claw`） |
| `[session].max_active_runtimes` (P3) | `8` | registry 同时**激活（驻留）**会话上限。超限时按 LRU 驱逐最久未用的 **Idle** 会话（Running/后台跑 turn 的不驱逐）；驱逐=出 registry 释放内存，`.jsonl`/档案保留，下次 `open` 重新 hydrate。对标 codex `THREAD_UNLOADING_DELAY`（idle 30min unload）/ openclaw lane 限流 |
| `[session].idle_unload_secs` (P3) | `1800` | 会话 Idle 超过该时长自动 unload 出 registry（0=禁用）。对标 codex 30 分钟空闲卸载 |

## 10. 错误与边界

| 场景 | 处理 |
|------|------|
| 非 git 目录跑 code | `project_root` 回退 cwd，key=`proj:<hash(cwd)>`（仍隔离，只是按目录而非仓库根） |
| git worktree / submodule | `--show-toplevel` 取当前 worktree 根（各 worktree 独立 scope，符合直觉） |
| `sessions.json` 损坏 | 读失败时备份并重建空 store（不阻断启动），日志 warn |
| `switch`/`attach` 到不存在 sessionId | 报错并 `list` 提示候选 |
| 同 sessionId 重复 `spawn_turn` | 拒绝/排队（D11），返回"该会话有进行中的 turn" |
| 旧 v1 `sessions.json` | 启动时透明迁入 v2（G6），写回前保留 `.bak` |

## 11. 测试策略

| 层级 | 用例 |
|------|------|
| 单元 | `session_key_for`：claw 任意 cwd 同 key；code 同仓库根/子目录同 key、不同仓库不同 key；FNV-1a 跨平台稳定 |
| 单元 | store v1→v2 迁移无损；按 sessionKey 过滤列表正确；`current` 指针更新 |
| 集成 | code 在 A 建会话→B 起 code 看不到 A（G1）；claw 跨目录续同一组（G2）；同 scope `new` 多会话并 `switch`（G3） |
| 并发(P2) | 多线程并发 `append_message` 不丢/不串（per-file 锁）；并发改 `sessions.json` 整段 RMW 不丢更新（D10：模拟两会话 `load→改→save` 交错，断言两处更新都在）；`read_file_state` 隔离（D16：A 会话 read 不让 B 会话通过"先读后写"门槛） |
| 并发·checkpoint(P2) | 同项目两会话并发 `record` 经共用 store 串行、不撞 `index.lock`、不死锁（D18）；A 会话以 `paths=自己改过的文件` restore 后，B 会话独占文件保持不变（D18 per-session 收窄）；同改一文件时记录残留风险用例（§12） |
| 并发(P3) | 两会话并发 `spawn_turn` 互不阻塞、各自落盘；`abort` 只命中目标会话；前台切换后后台 turn 跑完可回放；流式 permit 早释放下 ≥`max_concurrent_requests` 个流可并存不互卡（D17） |

## 12. 风险与回滚

| 风险 | 缓解 |
|------|------|
| `ChatContext` 拆分牵涉面广（P2） | 本轮已一次迁到 `GlobalServices / ScopeServices / SessionRuntime` 三层；用 runtime/chat/skill/CLI 回归盯住主要消费路径，后续新增字段按生命周期归位 |
| 输入模型改造大（P3） | P1/P2 可独立上线（G7）；P3 失败可只回滚调度层，保留数据模型 |
| 并发写竞态 | per-file 锁 + `sessions.json` 整段 RMW 持 `write_mutex`（D10）+ 原子写；并发用例做护栏（§11） |
| checkpoint per-session new 误用（D18） | 同项目多 store 实例会指向同一 `git_dir` 却各持独立锁→串行失效、git 损坏；架构上钉死"按 work_tree 缓存共用一个实例"，禁止 per-session new；并发 record 用例护栏（§11） |
| 跨会话回退波及（残留风险，D18） | restore 默认收窄到"本会话改过的 paths"，只回退自己碰过的文件；**A、B 同改一个文件无法隔离**（单工作树一份磁盘态）——检测到 paths 与他会话改动集重叠时告警/二次确认，根治需独立 worktree（未来项） |
| 开发阶段旧 `sessions.json` 被直接重建 | 文档提前声明“不保证旧索引兼容”；`init` 与直接使用路径都统一写回新结构，避免半迁移状态 |
| 用户对 claw/code 命名困惑 | `user-guide.md` 给"全局助手=claw / 项目编码=code"一句话心智 + `tomcat`(默认) 兜底 |

## 13. 历史与取舍记录

- 取代计划 `session-cwd-binding` 的"单一模式 + legacy 迁移脚本"：当前方向改为开发阶段**不**做旧 `sessions.json` 迁移，旧结构与坏文件统一直接重建成新结构。
- 早期方案命名为 `tomcat code`/`tomcat claw` 两模式且 `sessionKey` 1:1 映射单会话；本版按需求升级为 **`sessionKey` 1:N（作用域键）** 并新增 **轴二并发架构（SessionRuntimeRegistry）**；对外入口以 `claw/code` 为准，`tomcat chat` 仅保留隐藏兼容别名映射到 `code`。
- 并发模型在"单进程多任务（codex）"与"多进程（cc-fork/Qevos）"之间选择前者（D7），核心依据：tomcat 既有 tokio+AgentRegistry 底子、多进程跨进程 transcript 协调脆弱且重。

## 总结

本方案把"会话串台"和"无法多会话并发"两个问题，拆成两条正交轴一并解决：

- **轴一（组织）**：`tomcat claw`（全局，`sessionKey=agent:main:main`）与 `tomcat code`（按项目，`sessionKey=agent:main:proj:<hash>`）。`sessionKey` 升级为 1:N 的作用域键，`sessionId` 成为一等公民，`sessions.json` 改为 `sessions{id→SessionEntry}+current{key→id}`；开发阶段遇到旧 `sessions.json` 时直接重建，不做兼容迁移。
- **轴二（并发）**：对标 codex 的单进程 Tokio + `SessionRuntimeRegistry`，每个**已打开**会话一个常驻 `SessionRuntime`，提示词到来时按 turn `tokio::spawn(AgentLoop)`；各自独立 `cancel_token/context_state/plan/todos/read_file_state`。共享服务按生命周期分三层（D16）：`GlobalServices`（进程级，含 LLM 共享实例+并发准入 D17、`gate` 经评审保留 Global、`SessionManager` 整段 RMW 持锁 D10）、`ScopeServices`（按 work_tree 共用，checkpoint 每项目一个实例、restore 按本会话 paths 收窄 D18）、`SessionRuntime`（每会话）；事件统一带 `sessionId` 并按 `sessionId` demux；明确**不走多进程**。

落地分三阶段（P1 数据模型与命令 → P2 抽 SessionRuntime 与写锁 → P3·TUI 并发调度与输入解耦），P1 即可独立上线先治串台、再逐步逼近"像 Cursor 一样多会话并发"。



