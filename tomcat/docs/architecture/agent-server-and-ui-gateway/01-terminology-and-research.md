## 1. 术语统一

本节钉死全文反复出现且易混淆的命名，避免「gateway / server / CLI / IPC / SSE / NDJSON / JSON-RPC」各说各话。


| 术语                        | 语义                                                                                                                      | 数据载体 / 单一事实源                                                                                                                    | 行为约束                                                                                                        | 说人话                                                        |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Agent Server**          | 把宿主能力暴露给**进程外**调用方的最小单元：读命令 + 驱动 `AgentLoop` + 序列化事件下行                                                                  | 新增 `src/api/serve/`；入口 `Commands::Serve`                                                                                        | 只做翻译与编排，不重写核心；首发传输=stdio                                                                                    | 「让别的程序能远程喊 agent 干活」的那层壳。                                  |
| **Gateway（网关）**           | Agent Server 的**网络化泛化**：同一 dispatcher 多挂传输（WebSocket/HTTP），支持多客户端/远程/广播                                                 | Phase 2 `src/api/serve/gateway/`（axum）                                                                                          | 必须复用 Phase1 的 commands/event_pump/control，不另造协议                                                             | 把那层壳从「子进程管道」升级成「网络服务」。本文主张**不**先做它。                        |
| **Transport（传输）**         | wire 字节如何在两进程间流动                                                                                                        | `stdio`（首发）/ `ws`/`http`（Phase2）/ `acp`（可选）                                                                                     | 与协议解耦：换传输不改命令/事件语义                                                                                          | 同一份 JSON，可以走管道，也可以走 WebSocket。                             |
| **命令（Command，上行）**        | UI → agent 的请求帧                                                                                                         | `ServeCommand{ type, id?, ...params }`（stdin NDJSON）                                                                            | `type` 分派（**非** JSON-RPC2.0 信封）；`id` 与 JSON-RPC `id` 对齐（§3.2.5）；对标 pi-mono/pi_agent_rust                    | UI 让 agent 做事的指令，如 `prompt`/`new_session`。                 |
| **事件（Event，下行）**          | agent → UI 的可观测流帧                                                                                                       | **既有** `AgentEvent`（`src/infra/events/mod.rs`），`WireEnvelope` 加 `sessionId`                                                     | 不新增事件类型；serde `tag="type"` snake_case + payload camelCase                                                   | agent 干活时不断吐出的「进度直播」。                                      |
| **控制（Control，双向）**        | 必须等用户拍板的阻塞回环（审批 / 提问 / initialize / interrupt）                                                                          | `control_request` / `control_response` / `control_cancel`（与会话流**正交**）                                                           | `requestId` 配对（↔ JSON-RPC 双向请求 `id`，§3.2.5）；审批是**请求**不是通知（codex 教训）                                        | 「这命令危险，你点不点同意？」这种来回。                                       |
| **单写者 drain**             | 所有下行帧经一个 FIFO 队列、由唯一线程写 stdout                                                                                          | `src/api/serve/writer.rs`（`mpsc` + 独占 writer）                                                                                   | 任何模块**禁止**直接 `println!` 下行；防多线程交错                                                                           | 一个出口排队发，别几个线程抢着往屏幕上写。                                      |
| **NDJSON / 行分隔 JSON**     | **分帧层**：如何用换行把 JSON 切成可流式读写的帧；`\n` 为边界，每行须为完整可独立 `parse` 的一个 JSON 值（亦称 JSONL / JSON Lines）                              | `ndjson_safe_stringify`（`src/api/serve/ndjson.rs`，转义 U+2028/U+2029）；stdio 读写见 `src/api/serve/{stdin,writer}.rs`                                          | **不规定**字段名、请求/响应语义、版本协商——只解决「怎么切行、怎么流式吐」；解析器按 `\n` 切，禁止裸 `readline` 误切 Unicode 行分隔符                         | 一条消息一行，换行就是边界；子进程管道边读边写最省事。                                |
| **JSON-RPC 2.0**          | **应用层 RPC 协议**：规定帧内语义——请求含 `jsonrpc`/`method`/`params`/`id`，响应含 `result` 或 `error` 且 `id` 配对；无 `id` 的为 notification（推送） | Phase 2+ `src/api/serve/acp/*`（ACP 即 JSON-RPC2.0 over stdio）；对标 `pi_agent_rust/src/acp.rs`、`hermes-agent/tui_gateway/server.py` | Tomcat **首发不用** JSON-RPC 作主命令协议（见 R4）：自有 `{type}` 与 `AgentEvent` 已够用；JSON-RPC 作**可选兼容层**，映射到内部同一 dispatcher | 行业「普通话」：不光是一行 JSON，还规定调哪个方法、怎么对上回包；Zed/ACP 生态才需要。          |
| **NDJSON 与 JSON-RPC 的关系** | 正交两层，可组合：**NDJSON = 怎么装运；JSON-RPC = 装运物里的标准话术**                                                                         | Phase1 = NDJSON + 自有 `{type}`/`AgentEvent`（**不**套 JSON-RPC 信封，见 R4）；Phase2+ ACP = NDJSON（或 WS 一帧）+ JSON-RPC 2.0 帧内语义            | 换传输或换帧内协议只换 adapter/`acp/*`；自有 `id`/`requestId`/`type` 已对齐 JSON-RPC 字段，ACP 为机械翻译（§3.2.5）                   | 别混为一谈：「走 NDJSON」≠「走 JSON-RPC」；Tomcat Phase 1 是前者 + 结构对齐后者。 |
| **sessionId 信封**          | 每帧顶层携带的会话归属元数据                                                                                                          | `WireEnvelope.sessionId`（**已存在**，`ScopedEventEmitter` 自动写入）                                                                     | 多会话多路复用的路由键：命令按它选会话槽、事件按它 demux 回客户端                                                                        | 给每条消息贴上「属于哪个会话」的标签。                                        |
| `**ChatContextRegistry`** | serve 进程内「会话身份 → 会话壳」的注册表                                                                                               | 新增 `src/api/serve/registry.rs`，`DashMap<sessionId, SessionSlot>`（对齐 `multi-agent.md` 维度A/MA2）                                   | 进程级；与 `AgentRegistry`（控制面/限流）正交分工，靠 `root_session_id` 串联                                                    | 一张「哪个会话是哪个房间」的总台账。                                         |
| **会话槽 SessionSlot**       | 单个活跃会话的运行态聚合                                                                                                            | `{ ctx: Arc<ChatContext>, run_task, cancel_token, busy }`                                                                       | 每会话一份；`AgentLoop` 跑完即 drop，槽随会话存活                                                                           | 一个会话对应的「房间钥匙 + 正在干活的工人 + 急停按钮」。                            |
| **跨会话并发 / 同会话串行**         | 并发粒度约束                                                                                                                  | per-session `busy` 标志 + `cancel_token`；对齐 codex `SerializationScope::Thread`                                                    | 不同 `sessionId` 真并发流式；同一 `sessionId` 同时仅一个活跃 turn                                                            | 多个房间各干各的；同一个房间一次只办一件事。                                     |
| **embed（进程内嵌）**           | UI 直接链核心、不走进程边界                                                                                                         | 当前 CLI 即 embed（`run_cli` 直接 `AgentLoop.run`）                                                                                    | 不适用于 VSCode/桌面（语言/进程隔离）                                                                                     | TUI 那种「UI 和 agent 一个进程」的玩法。                                |
| **ACP**                   | Agent Client Protocol：在 JSON-RPC 2.0 之上约定的 IDE 方法集（`session/`*、permission 等），通常 **NDJSON over stdio** 承载                | Phase 2+ 兼容适配层 `src/api/serve/acp/`*                                                                                            | 与自有 wire 并存，按需开；帧内语义见上 **JSON-RPC 2.0** 条                                                                   | 行业 IDE 接入的「普通话」，可后补。                                       |


说人话：全文的「gateway」指的是**那层翻译 + 编排的 dispatcher**，不等于「网络服务器」。它的第一个化身是 **Agent Server（stdio 子进程）**；只有当你需要 Web/远程/多客户端时，才把它泛化成网络 **Gateway**。**NDJSON** 只管「一行一条 JSON 怎么流式传」；**JSON-RPC** 管「这一行里 method/id/result 怎么说」——Tomcat Phase 1 是 NDJSON + 自有 `{type}`（不套 JSON-RPC 外壳），但字段故意贴近 JSON-RPC 以便 Phase 2 ACP 机械翻译。

模糊时间词钉死：本文「阻塞工具继续」指**在 `tool_dispatcher` 把某个需要审批/提问的工具调用真正执行前**那一刻挂起，等 `control_response` 到达后才继续；不是指挂起整个 `AgentLoop` 事件循环。

---

## 2. 竞品 / 选型对比（调研）

> 本节是「读过 6 个 agent 源码后的调研材料」，为 §3 已定稿结论提供证据链；**不含** §3.1 七列决策矩阵与 §3.2 实施点表。

### 2.1 形态分类图（UI 怎么触达 agent 能力）

```text
                     「UI 如何调用 agent 能力」谱系（从轻到重）
  ─────────────────────────────────────────────────────────────────────────────►
  [A] embed 进程内            [B] stdio 子进程            [C] 一dispatcher+多传输      [D] 网络 daemon 网关
      UI 链核心                  UI spawn CLI                stdio + WebSocket            长生命周期 HTTP/WS 服务
      ───────────                ─────────────               ──────────────────          ────────────────────
   pi-mono TUI               pi_agent_rust `--mode rpc`   codex `app-server`          openclaw `gateway run`
   pi_agent_rust TUI         pi-mono `--mode rpc`          (stdio|UDS-WS|TCP-WS)        (WS frames, :18789)
   (AgentSession.subscribe)  cc-fork `stream-json`        hermes `tui_gateway`         openclaw web/远程
                             codex `app-server --stdio`    (stdio JSON-RPC + WS)        多客户端/广播/鉴权
                             pi_agent_rust `--acp`(Zed)
  ◄── 越轻：实现快、单机本地     中间：进程隔离、可换语言UI       既能嵌也能远程            越重：多端/远程/鉴权/背压 ──►
```

### 2.2 竞品横向对比


| 竞品 / 仓库                          | 主链路形态                              | 关键传输与文件                                                                                                                                                                                   | 事件/命令协议                                                                                                                                                  | SSE 用在哪                                                                                    | 我们借鉴的点                                                                                                                                                  | 说人话                                                            |
| -------------------------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| **codex**（Rust）                  | [C] 专用网关 crate `app-server`        | stdio JSON-RPC（默认）/ UDS-WS / TCP-WS：`codex-rs/app-server-transport/src/transport/{stdio,unix_socket,websocket}.rs`；in-process embed：`app-server/src/in_process.rs`                        | 内核 `Submission/Op`+`Event/EventMsg`（`codex-rs/protocol/src/protocol.rs`）→ 网关 JSON-RPC notification/request（`app-server-protocol/src/protocol/common.rs`） | **不用于 UI**：SSE 仅对上游 OpenAI Responses                                                       | 网关与核心分层；**in-process embed 复用同一 dispatcher**；审批=server→client **请求**；schema 导出为一等工件                                                                     | 教科书级「一份 dispatcher 多种传输」；VSCode 扩展 spawn `app-server` 走 stdio。 |
| **cc-fork-01**（TS，Claude Code 风） | [B] stdio `stream-json`            | `claude --print --input-format stream-json --output-format stream-json --verbose`；单写者 `src/cli/structuredIO.ts`（`outbound: Stream<StdoutMessage>`）；安全序列化 `src/cli/ndjsonSafeStringify.ts` | NDJSON 消息（`user/assistant/result/system/stream_event`）+ **正交控制** `control_request/response/cancel`（`src/entrypoints/sdk/controlSchemas.ts`）              | 仅云端读路径（`src/cli/transports/SSETransport.ts`），非本地 UI                                        | **单 FIFO drain + NDJSON 转义 U+2028/9**；控制通道与会话流正交；IDE 走 MCP 不走 stream-json                                                                               | 把「别让多线程抢 stdout」做成了纪律。                                         |
| **hermes-agent**（Python）         | [C] `tui_gateway` 一 dispatcher 多传输 | 同一 newline-JSON-RPC 既走 stdio 又走 WS：`tui_gateway/entry.py`、`tui_gateway/ws.py`、`tui_gateway/server.py`（`@method` 注册表 + `dispatch`）                                                         | JSON-RPC 请求/响应 + **push=通知 `method:"event"`**（`params.type`）；Ink 端 `ui-tui/src/gatewayTypes.ts`                                                          | 仅 OpenAI 兼容 API（`gateway/platforms/api_server.py`）                                         | **一个 dispatcher、多传输共享 handler 表**；即时 ack + 异步出 turn；stdout 重定向到 stderr 保护管道                                                                             | 证明「stdio 和 WS 可以是同一套方法表」。                                      |
| **openclaw**（TS）                 | [D] 网络网关（WS 为主）                    | Node http server :18789；WS 三帧信封 `packages/gateway-protocol/src/schema/frames.ts`（`req/res/event`，`PROTOCOL_VERSION=4`）；广播 `src/gateway/server-broadcast.ts`                               | WS：`chat`(state=delta/final/aborted/error) + `agent`/`session.tool`(phase=start/update/result)                                                           | **仅历史回放** `GET /sessions/{key}/history`（`src/gateway/sessions-history-http.ts`）+ OpenAI 兼容 | **背压 `dropIfSlow`**；delta 150ms 节流 + final 前 flush；**幂等 `idempotencyKey`**；审批 push→RPC resolve→push resolved；**双工具通道**（run-scoped + 晚加入 session-scoped） | 想做 Web/多端时的「最终形态」参考；但 SSE ≠ 主流。                                |
| **pi_agent_rust**（Rust）          | [A]+[B] embed + stdio RPC/ACP      | 单 crate 单 bin；`pi --mode rpc`（`src/rpc.rs`，JSONL `{type}`）/ `pi --acp`（`src/acp.rs`，JSON-RPC2.0 Zed）；SDK `SessionTransport::{InProcess,RpcSubprocess}`（`src/sdk.rs`）                      | 统一 `AgentEvent`（`#[serde(tag="type",snake_case)]`，`src/agent.rs`）；ACP 映射 `session/update`                                                                | **不用于 UI**（仅上游 LLM `src/sse.rs`）                                                           | **单一事件 enum + serde wire**；`SessionTransport` 统一进程内/子进程；transport 级 output-pressure 合并；**ACP 作 IDE 标准**                                                 | 和 Tomcat 同语言、同 `AgentEvent` 命名——最直接对标。                         |
| **pi-mono**（TS）                  | [A]+[B] embed + stdio RPC/JSON     | 无 daemon；`--mode json`（事件 JSONL）/ `--mode rpc`（`packages/coding-agent/src/modes/rpc/rpc-mode.ts`）；消费缝 `AgentSession.subscribe()`                                                          | 三层事件：`AssistantMessageEvent`→`AgentEvent`→`AgentSessionEvent`（`packages/agent/src/types.ts`）；命令 `RpcCommand{type}`；严格 JSONL（LF-only，`rpc/jsonl.ts`）      | **完全不用** SSE 作 UI                                                                          | **单一 in-process 事件 union + 薄 wire 适配**；两级流式（provider delta 嵌在 `message_update`）；审批走扩展 `beforeToolCall`+UI context                                       | Tomcat 的 `AgentEvent` 命名就是抄它——事件侧基本现成。                         |


### 2.3 维度词典（R1–R12）


| 维度            | 关切                             | 说人话                                                               |
| ------------- | ------------------------------ | ----------------------------------------------------------------- |
| R1 进程边界       | embed vs 子进程 vs 网络 daemon      | VSCode/桌面是别的进程/语言，embed 不可行。                                      |
| R2 首发传输       | stdio vs WS vs SSE vs gRPC     | 本地 IDE/桌面首选 stdio 子进程，最省事。                                        |
| R3 事件契约       | 复用 `AgentEvent` vs 另造          | 已和 pi-mono 对齐，别造第二套真相。                                            |
| R4 命令风格       | `{type}` 自有 vs JSON-RPC2.0/ACP | 自有 `{type}` 首发；**不**套 JSON-RPC 信封；帧结构**贴近** JSON-RPC 以利 ACP 机械翻译。 |
| R5 单写者        | 多处直写 vs 单 drain                | 不单写者，stdout 必交错乱序。                                                |
| R6 控制通道       | 复用会话流 vs 正交                    | 审批/中断必须正交，否则与 delta 抢序。                                           |
| R7 审批形态       | 通知 vs server→client 请求         | 审批要能阻塞等回包，必须是请求。                                                  |
| R8 会话多路       | 连接=会话 vs sessionId 多路          | 本期即做单进程多会话并发；信封字段已就位。                                             |
| R9 背压         | 无限缓冲 vs 合并/丢弃                  | delta 要合并，关键事件不能丢。                                                |
| R10 鉴权        | stdio 进程信任 vs 网络 token         | stdio 靠进程边界；联网才需 token+loopback。                                  |
| R11 schema 工件 | 手写 vs 生成 TS/JSON Schema        | 给 VSCode 扩展生成类型，省对接成本。                                            |
| R12 可见性下行     | 全量 fanout vs visible/hidden 降级 | 这套 visible/hidden 过滤先保留设计；本期先全量 fanout，Phase 2 再决定是否启用。                |


### 2.4 为什么选「先 stdio Agent Server、网关 Phase 2」而非「先做网络网关」

1. **被多数竞品验证**：6 个里 5 个的本地/IDE 主链路都是 **stdio 子进程行分隔 JSON**（cc-fork、pi_agent_rust、pi-mono、codex-stdio、hermes-stdio），openclaw 才是纯网络网关——而它也付出了 `embedded-backend.ts` ~1000 行重复会话逻辑的代价。
2. **Tomcat 事件侧几乎现成**：`AgentEvent` + `WireEnvelope.sessionId` 已与 pi-mono 对齐（见 `src/infra/events/mod.rs`），stdio 模式下「下行」基本是「订阅 EventBus → 序列化 → 写 stdout」，工作量集中在上行命令与控制回环。
3. **进程边界即安全边界**：stdio 由父进程（VSCode/Electron）独占管道，天然无需鉴权；先上网络服务则立刻要处理 token/loopback/CORS/origin（openclaw `src/gateway/auth.ts` 的复杂度），过早。
4. **同一 dispatcher 可平滑升级**：codex 与 hermes 都证明「命令分发 + 事件订阅 + 控制回环」可与传输解耦；Phase 2 只需把 `writer/stdin` 换成 axum WS 适配器即可，不返工。
5. **审批/提问已有 EventBus 回环**：`EventBusAskQuestionPanel`（`src/api/chat/panels/ask_question_wire.rs`）已是「host 监听 `plan.ask_question` + 向 `response_event` 回包」模式，serve 层直接把它桥到 stdio，几乎零新协议。

### 2.5 单进程多会话并发：6 个 agent 怎么做的（调研）

> 本节专门回答「**能不能像 Cursor / Copilot / Codex 一样，一个进程同时跑多个会话**」。结论：**单进程多会话并发是可行且被验证的主流形态**；6 个对象分成「单进程多路复用」「多进程隔离」「单活跃切换」三档，Tomcat 应走第一档（codex 模型）。落地见 §3.3。

#### 2.5.1 三档形态分类

```text
                「一个进程如何承载多个并发会话」三档
  ───────────────────────────────────────────────────────────────────────►
  [甲] 单进程多路复用（目标）       [乙] 多进程隔离              [丙] 单活跃切换（反例）
      一进程 N 会话并发              一会话一子进程              一进程仅 1 活跃会话
      registry<id, 运行态>           父进程管子进程池            switch 替换 / resume
      ──────────────────             ──────────────             ─────────────────
   codex app-server               cc-fork bridge             pi_agent_rust --mode rpc
   (ThreadManager)                (activeSessions Map         pi-mono --mode rpc
   hermes tui_gateway              + 每会话 spawn 子进程)      (AgentSessionRuntime
   (_sessions dict)                                            _session 单槽 + 切换)
   openclaw gateway               pi_agent_rust --acp 介于
   (runId/sessionKey 索引)         甲：HashMap<sid,Session>
  ◄── 共享重资源、隔离轻状态        进程级强隔离、内存翻倍       实现最省但开不了多 tab ──►
```

#### 2.5.2 多会话并发横向对比


| 竞品                            | 档位            | 会话注册表（文件+符号）                                                                                                                                                                   | 并发粒度                                                                                                                                                       | 事件按会话路由                                                                          | 中止/审批按会话路由                                                                                | 单连接 writer 公平性                                                             | Tomcat 借鉴                                                                                              |
| ----------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| **codex**（Rust，教科书）           | 甲             | `core/src/thread_manager.rs` `Arc<RwLock<HashMap<ThreadId, Arc<CodexThread>>>>`；订阅表 `app-server/src/thread_state.rs` `ThreadStateManager`（connId↔threadId 多对多）                 | **跨 thread 真并发 + 同 thread 串行**：`app-server-protocol/.../common.rs` `ClientRequestSerializationScope::Thread`，`request_serialization.rs` per-thread FIFO 队列 | `ThreadScopedOutgoingMessageSender`（`outgoing_message.rs`）按订阅连接发，通知带 `thread_id` | 审批 `PendingCallbackEntry.thread_id` + 闭包捕获 `Arc<CodexThread>`；`turn/interrupt` per-thread | **全局 outbound 单队列 + 每连接 FIFO writer**（`transport.rs`），慢连接满则断（无跨 thread 配额） | **三层 ID（conn+session+request）+ 每会话 listener actor + 共享重资源/隔离轻状态 + 声明式串行域**                             |
| **hermes**（Python）            | 甲             | `tui_gateway/server.py` `_sessions: dict[sid, {agent, history_lock, running, transport}]`（L119）                                                                                | 跨 session 真并发（每 turn 起 daemon 线程）+ 同 session `running` 守卫串行                                                                                                | `write_json` 按 `params.session_id` 查 `_sessions[sid].transport`（L366-384）        | `session.interrupt` → `_clear_pending(sid)` 只清本会话；审批按 `session_key` 队列                    | `_stdout_lock` 仅保证**行级原子**，多 session 事件按完成顺序行交错（无公平轮转）                     | **prompt fast-ack + 后台 turn；per-session `running` + `history_lock`；事件 session→transport 路由**           |
| **openclaw**（TS，网关）           | 甲             | run 态全局按 `runId` 索引：`chat-abort.ts` `ChatAbortControllerEntry`；执行态 `run-state.ts` `ACTIVE_EMBEDDED_RUNS`（每 `sessionId` 一条）                                                     | 跨 session 真并发 + **单 session 一条 active embedded run**（再发 replace/steer）                                                                                     | 双广播：可见时全局 fanout + 客户端按 `sessionKey` 过滤；隐藏时 `broadcastToConnIds` 定向              | `chat.abort` 校验 `sessionKey+runId` 精确中止；审批按发起 conn/device 绑定                              | **连接级背压 `dropIfSlow`（bufferedAmount），不分 session**（明确的公平缺口）                 | **幂等三层（防重复 run）；run-scoped + session-scoped 双工具通道**                                                    |
| **pi_agent_rust**（Rust，ACP 侧） | 甲（ACP）/丙（RPC） | ACP `src/acp.rs` `sessions: HashMap<sessionId, Arc<Mutex<AcpSessionState>>>` + `active_prompts: HashMap<sessionId, AbortHandle>`；RPC `src/rpc.rs` 单 `Arc<Mutex<AgentSession>>` | ACP：多 session 并存 + **每 session 单 prompt**（`PROMPT_IN_PROGRESS`）；RPC：单活跃                                                                                    | ACP `session/update{ sessionId, ... }` 带 sessionId；RPC 事件**无** sessionId         | ACP `session/cancel` → `active_prompts[sid].abort()`；RPC 全局单 abort 槽                      | 单 stdout，ACP 靠 sessionId demux；RPC 假设单会话                                   | `**HashMap<sid, state>` + `active_prompts<sid, AbortHandle>` + `session/update.sessionId` 路由（同语言可直抄）** |
| **cc-fork**（TS）               | 乙             | `bridge/bridgeMain.ts` `activeSessions: Map<sessionId, SessionHandle>`；每会话 `bridge/sessionRunner.ts` `spawn(claude --print --session-id …)`                                    | 多**进程**并发；单进程内 `getSessionId()` 单值 = 一进程一会话                                                                                                                | 每子进程独立 transport（sessionId 编进 `--sdk-url`），父只监控 stdout 不 fan-in                  | 每子进程独立 control 通道；中断走各自 transport                                                         | 不适用（无单连接多路复用）                                                              | **反例参考：进程隔离 vs 单进程多路复用的取舍；路由放 transport 而非合并 stdout**                                                  |
| **pi-mono**（TS）               | 丙             | `agent-session-runtime.ts` 单 `_session`；`newSession/switchSession/fork` = teardown→`dispose`→apply→`setRebindSession`                                                          | 单活跃（切换式替换，旧 session `dispose`）                                                                                                                             | RPC 事件**无** sessionId；`AgentSessionEvent` 无 sessionId 字段                         | `{type:"abort"}` 只停唯一活跃 session                                                           | 单 subscribe，无多路                                                            | **反例：switch 式替换会丢 in-flight turn，不能开多 tab；命令/事件必须带 sessionId**                                         |


#### 2.5.3 提炼：单进程多会话并发的 7 个必要件（对照 Tomcat 现状）


| #   | 必要件                             | 证据（谁做了）                                                                             | Tomcat 现状                                                                                                                                        |
| --- | ------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1   | **会话注册表** `Map<sessionId, 运行态>` | codex `ThreadManager`、hermes `_sessions`、pi-acp `sessions`                          | ⚠️ 缺：`AgentRegistry` 是控制面（handle+中止），**没有**会话壳注册表（`ChatContextRegistry` 维度A 规划未实现）                                                            |
| 2   | **每会话独立 run task + 中断/取消**    | codex per-thread session loop、hermes daemon 线程、pi-acp `active_prompts`              | ✅ 底座有：`AgentLoop` 可多实例、`AgentHandle.abort_signal` 独立、`cancel_token` per ChatContext                                                              |
| 3   | **事件带 sessionId 信封**            | codex `ThreadScopedSender`、hermes `write_json` 路由、pi-acp `session/update.sessionId` | ✅ **已具备**：`ScopedEventEmitter` → `WireEnvelope.sessionId`，命名已与 pi 对齐                                                                             |
| 4   | **命令按 sessionId 路由**            | codex `params.thread_id`、ACP `params.sessionId`                                     | ⚠️ 缺：serve 命令分发需新增 sessionId 选槽（命令帧字段已留好）                                                                                                        |
| 5   | **跨会话并发 + 同会话串行**               | codex `SerializationScope::Thread`、hermes `running` 守卫、pi-acp 单 prompt              | ⚠️ 缺：serve 层加 per-session `busy`（核心 `AgentLoop` 本身天然单 turn）                                                                                      |
| 6   | **单写者按会话 demux + 跨会话公平**        | codex 全局队列+每连接 FIFO（不公平）、hermes 行级原子（不公平）                                           | ⚠️ 缺：serve `writer` 需做 demux + **比竞品多做一步跨 session 公平轮转**（修正 openclaw/codex 的公平缺口）                                                                |
| 7   | **共享重资源、隔离轻状态**                 | codex `ThreadManager` 共享 auth/models/MCP；进程级 `Arc`                                  | ✅ **已具备**：`GlobalServices` 进程级 `Arc<dyn LlmProvider/PrimitiveExecutor/EventBus>`，会话级隔离 `ScopeServices/SessionRuntime`（与 `multi-agent.md` MA7 一致） |


**结论（说人话）**：7 件里 Tomcat **已具备 3 件半底座（2/3/7）**，要补的是 **1（会话壳注册表）、4（命令路由）、5（同会话串行守卫）、6（公平单写者）**——全部落在**新增的 serve 传输层**，`AgentLoop` 核心一行不改。这就是为什么「本期就能做到 codex/Cursor 级别的多会话并发」：核心早已为维度A 留好了缝，只差把 serve 这层壳按 codex `ThreadManager` 模型装上。

#### 2.5.4 为什么是 codex 模型而不是 cc-fork 多进程


| 取舍             | 单进程多路复用（codex / 本期选）                                | 多进程隔离（cc-fork）                                     |
| -------------- | --------------------------------------------------- | -------------------------------------------------- |
| 内存/启动          | 共享 LLM client / MCP / tokenizer，N 会话一份              | 每会话一个进程，重资源 ×N，冷启动慢                                |
| 与 Tomcat 现状契合度 | 高：`GlobalServices` 已进程级共享、`EventBus` 已 sessionId 信封 | 低：要造子进程管理 + stdout fan-in + token 注入               |
| 事件/中止路由        | 进程内 `Arc` + sessionId，零 IPC                         | 跨进程 IPC，sessionId 编进 transport URL                 |
| 隔离强度           | 逻辑隔离（独立 `ContextState`/`abort_signal`）              | 物理隔离（崩溃不互累）——但 Tomcat 已有 `catch_unwind` + panic 隔离 |
| 适用             | 本机多 tab、桌面 GUI 多会话（**本期**）                          | 海量并行 worker / 强崩溃隔离（非本期诉求）                         |


说人话：cc-fork 多进程是「为强隔离/水平扩展付内存税」，适合云端 worker 农场；Tomcat 要的是「VSCode 里同时开几个会话 tab」，codex 的单进程多路复用既省资源又贴合现有 `Arc` 共享 + `sessionId` 信封，是最短路径。

---

