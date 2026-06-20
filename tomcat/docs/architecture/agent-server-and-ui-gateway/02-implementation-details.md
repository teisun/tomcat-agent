### 3.2 实施点（已闭环定义）

> 与 §3.1 行可一对多映射：P0–P5 = Phase 1（stdio Agent Server，闭环 MVP，**含本期多会话并发**）；P5.5 = 可见性感知下行（**Phase 2 设计储备，当前不做**）；P6 = schema 工件；P7–P8 = Phase 2（网关 / ACP，**PENDING**，本方案先钉边界不强制本期落地）。多会话并发的整体落地设计见 **§3.3**；验收锚点见 **§8 测试矩阵**。


| 实施点                              | 交付范围（含交付物）                                                                                                                                                                             | 主要代码落点（含落地点）                                                                                                                                                 | 验收锚点（示例）                                                                                                                      | 说人话                                   |
| -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| **P0** 入口 + 传输骨架                 | `Commands::Serve{transport}` 子命令 + stdio 读写循环 + 单写者 drain + `ndjson_safe_stringify`（转义 U+2028/9）；交付物：`tomcat serve --stdio` 可启动并 echo `initialize_result`                              | `src/api/cli/mod.rs`（新增枚举与路由）、`src/api/serve/mod.rs`（`run_serve`）、`src/api/serve/{stdin.rs,writer.rs,ndjson.rs}`、`src/api/mod.rs`（导出）                                  | `serve_writer_single_drain_orders_frames`、`ndjson_safe_stringify_escapes_line_separators`                                     | 先把「子进程能开、能一行行收发、stdout 不乱」搭起来。        |
| **P1** 命令分发（按 sessionId 路由）      | `ServeCommand` 解析 + 按 `sessionId` 选会话槽 + 映射到该会话 `AgentLoop`（`prompt/steer/follow_up/get_state/set_model/new_session/switch_session/get_messages/close_session/list_sessions`）；缺省 sessionId 落活跃会话             | `src/api/serve/commands.rs`、复用 `src/api/chat/run_loop/`*、调 `registry.rs` 选槽                                                                                  | `serve_prompt_drives_agent_run`、`serve_command_routes_by_session_id`、`serve_prompt_unknown_session_returns_error`           | UI 发的指令先认领「哪个会话」，再翻译成「让那个 agent 跑一轮」。 |
| **P1.5** 会话注册表 + 多会话并发（**本期目标**） | `ChatContextRegistry`（`DashMap<sessionId, SessionSlot>`）+ per-session `run_task`/`cancel_token`/`busy` + 进程级 `Arc` 服务共享；交付物：两个 `sessionId` 可同时 `prompt` 并各自流式（对齐 `multi-agent.md` 维度A） | **新增** `src/api/serve/registry.rs`；复用 `src/core/agent_registry/`*（已实现登记/`cascade_abort`/上限）、`GlobalServices` 进程级 `Arc`、`ChatContext::from_config`            | `new_session_registers_slot_in_registry`、`serve_multi_session_concurrency_and_isolation`、`serve_same_session_second_prompt_is_busy` | 一个进程里同时开几个会话 tab，各跑各的，互不串台。           |
| **P2** 事件下行泵（带 sessionId）        | 每会话 `EventBus.on(WIRE_*)` 订阅全部 `AgentEvent` → 带 `sessionId` enqueue 到 writer；交付物：一次 prompt 能在 stdout 看到 `agent_start…message_update…tool_execution_*…agent_end`                        | `src/api/serve/event_pump.rs`、复用 `src/infra/event_bus/mod.rs`、`src/infra/events/mod.rs`                                                                      | `serve_event_pump_streams_agent_events`、`serve_lifecycle_events_not_dropped_for_other_sessions`                                                  | 把 EventBus 上的事件原样搬到管道里，标签照旧带着。        |
| **P3** 控制通道 + 中断（按 sessionId）    | `control_request/response/cancel` 帧 + `initialize` 握手（连接级）+ `interrupt{sessionId}`（只置该会话中断信号、收口已发 Start 的工具）                                                                      | `src/api/serve/control.rs`、复用 per-session `cancel_token`/`AgentRegistry::cascade_abort`、`src/infra/events`（`agent_interrupted`）                              | `serve_initialize_control_request_sets_ready_state`、`serve_interrupt_cancels_target_session`、`serve_interrupt_emits_agent_interrupted_e2e`                                         | 「握手、急停」单独走控制线；急停只停被点名的那个会话。           |
| **P4** 审批 / 提问回环                 | `ServeAskQuestionPanel`：把 `plan.ask_question` 事件转 `control_request`（带 `sessionId`），stdin 回包按 `requestId+sessionId` emit 回 `responseEvent`；替换 `IdeAskQuestionPanel` 占位                | `src/api/serve/ask_question.rs`、复用 `src/api/chat/panels/ask_question_wire.rs`（`EventBusAskQuestionPanel`）、替换 `src/api/chat/panels/ide_ask_question_panel.rs` | `serve_ask_question_roundtrip_resumes_turn`、`serve_ask_question_cancel_roundtrip_does_not_hang`     | 「要不要同意/请你选一项」走控制线，等 UI 回，且回到正确的会话。    |
| **P5** 背压 / 合并 + 跨会话公平           | delta 合并 + 慢客户端丢弃；关键事件（message_end/turn_end/agent_end/control）必达；**跨 session 公平轮转**（修正 codex/openclaw 的「快会话饿死慢会话」缺口）；交付物：背压门限可配                                                        | `src/api/serve/writer.rs`（per-session pressure + round-robin）、`src/infra/config`（`serve.`* 键）                                                                | `serve_writer_coalesces_deltas_under_pressure`、`serve_writer_never_drops_lifecycle`、`serve_writer_round_robins_across_sessions`       | 增量能合着发、扛不住能丢，结束/审批必送到，且别让一个会话刷屏饿死另一个。 |
| **P5.5** 可见性感知下行（visible/hidden，**Phase 2 设计储备**） | 控制命令 `set_session_visibility{sessionId, visible}`（per-session、可多 `visible`）；writer 按可见性过滤：`visible` 全量、`hidden` 只放行 lossless 集（生命周期/`error`/`control_*`）并丢 `content_delta`/`thinking_delta`/工具流式增量；每帧带 per-session 单调 `seq`（hidden 丢弃帧也消耗 `seq`）；`get_messages{sessionId,lastNTurns}` 返回**快照（已落盘 transcript ＋ 飞行中消息内存累加文本）＋ 截止游标 `upToSeq`** 供 `hidden→visible` 重建，UI 严格只接 `seq>upToSeq`；**不变量**：丢弃只在 wire 层（被丢 delta 仍累积进消息状态）、`hidden` 审批绝不降级；交付物：切前台不缺帧/不重复、**飞行中消息不只显示半截**、后台审批仍可弹。**范围约束**：本节保留设计与验收锚点，**当前版本不纳入实现范围**。 | `src/api/serve/writer.rs`（per-session visibility 位 + `seq`）、`src/api/serve/control.rs`（`set_session_visibility`）、复用 `commands.rs` 的 `get_messages`（快照需含飞行中消息内存状态 + `upToSeq`） | `serve_hidden_session_drops_delta_keeps_lossless`、`serve_visibility_switch_resyncs_via_snapshot`、`serve_visibility_switch_midstream_message_not_truncated`、`serve_hidden_session_approval_still_delivered` | 先把设计钉住，等 Phase 2 真要压 UI/带宽成本时再落地；本期先全量 fanout。 |
| **P6** schema 工件                 | `tomcat serve --print-schema`：导出命令/事件 JSON Schema + TS `.d.ts`；fixture 漂移测试                                                                                                            | `src/api/serve/schema.rs`（`schemars` 派生）+ `src/infra/events/mod.rs`（`AgentEvent` 加 `JsonSchema` 派生 + `WireEvent` 信封，见 §3.2.7）、`tests/serve_schema_fixture`                                                                                        | `serve_print_schema_matches_fixture`                                                                                      | 吐一份类型给 VSCode 扩展直接用。                  |
| **P7** 网关传输（Phase 2，PENDING）     | 复用同一 dispatcher，新增 axum WebSocket 传输 + loopback 绑定 + token 鉴权 + origin 校验                                                                                                              | `src/api/serve/gateway/{server.rs,ws.rs,auth.rs}`（新）、复用 `registry.rs/commands.rs/event_pump.rs/control.rs`                                                   | `gateway_ws_reuses_dispatcher`（PENDING）                                                                                       | 以后要 Web/远程，就给同一套逻辑多挂个 WebSocket。      |
| **P8** ACP 兼容层（Phase 2+，PENDING） | `tomcat serve --acp`：JSON-RPC2.0 `session/`* ↔ 内部自有 wire **机械翻译**（`type`↔`method`、`id`↔`id`、`requestId`↔`id`；事件 `AgentEvent` 内侧不变，外侧包 `session/update`）；供 Zed 等 IDE                   | `src/api/serve/acp/`*（新）；映射表见 **§3.2.5**                                                                                                                     | `acp_session_prompt_maps_to_run`、`acp_control_request_id_roundtrip`（PENDING）                                                  | IDE 普通话只做薄适配，内核 dispatcher 不动。        |


实施小节索引：传输骨架与单写者算法见 **§3.2.1**；命令/事件/控制三流的端到端串联见 **§3.2.2**；审批/提问复用既有 wire 的接线见 **§3.2.3**；Phase 2 网关如何复用同一 dispatcher 见 **§3.2.4**；**自有 wire 形状约束与 ACP 机械翻译见 §3.2.5**；**可见性感知下行（visible/hidden）的落地见 §3.2.6**；**单进程多会话并发的完整落地（P1.5/P3/P5）见 §3.3**。

#### 3.2.1 传输骨架与单写者 drain（P0 / P5）

**映射**：实施点 **P0/P5**；决策行 **R2/R5/R9**。

**技术要点**：

- 一个进程内共享的 `WriterHandle` 作为唯一下行入口；命令响应、`event_pump`、`control` 全部只能 `send(frame)` 到它。
- `WriterHandle::send` 不是把帧先塞进无界 channel，而是**直接进入受 `serve.max_buffered_frames` 约束的 per-session 缓冲**，再 `notify` 唤醒独占 writer 任务。
- 独占 writer 任务从共享缓冲按「全局帧优先 + 会话 round-robin」取下一帧，`ndjson_safe_stringify(frame)` → `stdout.write_all + flush`。
- 背压：发送侧就执行 `coalesce`/丢弃判定——连续 `content_delta`/`thinking_delta` 在会话缓冲堆积时合并；超限时只丢可丢 delta，并注入一次 `llm_notice`；`message_end`/`turn_end`/`agent_end`/`control_request` 等 lossless 帧绝不丢。

```text
event_pump ─┐
command  ───┼─► WriterHandle::send(frame)
control  ───┘         │
                      ▼
              per-session bounded buffer
         （发送侧直接 coalesce / drop delta / 保留 lossless）
                      │ notify
                      ▼
         ┌────────────── 唯一 writer 任务 ──────────────┐
         │ round-robin dequeue → ndjson_safe_stringify │
         │ → stdout.write_all + flush                  │
         └─────────────────────────────────────────────┘
```

说人话：把「往 stdout 写」收敛成一条流水线，谁都得排队进队列，出口只有一个线程。增量字在队列里挤住了就合并，省带宽；但「结束了 / 要审批」这种帧贴上「不许合并/丢」的标签，保证 UI 状态不丢。

#### 3.2.2 命令 / 事件 / 控制三流端到端（P1 / P2 / P3）

**映射**：实施点 **P1/P2/P3**；决策行 **R3/R4/R6**。

**技术要点**：

- 上行：`stdin.rs` 按 `\n` 切行 → `serde_json::from_str::<ServeCommand>` → `commands.rs` 先按 `sessionId` 从 `ChatContextRegistry` 选会话槽（缺省落活跃会话），再按 `type` 分派；`prompt` 在该会话槽内走 `api/chat/run_loop` 驱动 `AgentLoop.run()`（详见 §3.3）。
- 下行：`event_pump.rs` 为每个会话在 run 开始前 `EventBus.on(WIRE_AGENT_START..)` 注册一组监听，回调里把 `EventContext.payload`（已是 `WireEnvelope` 形态，含 `sessionId`）原样 `send` 进 writer；writer 用 `sessionId` 把多会话事件 demux 回客户端。
- 控制：`control.rs` 处理 `initialize`（连接级回能力位）与 `interrupt{sessionId}`（只置目标会话槽的中断信号，如 `cancel_token` / `abort_signal`）。

```text
UI ──stdin {type:prompt,id}──► stdin.rs ─► commands.rs ─► run_loop ─► AgentLoop.run()
                                                                          │ emit AgentEvent
UI ◄─stdout {type:agent_start/message_update/...} ◄─ writer ◄─ event_pump ◄┘ (EventBus.on)
UI ──stdin {type:control_request,...}──► control.rs ─► interrupt path / initialize_result ─► writer ─► UI
```

说人话：三条线各走各的——命令进来驱动 run，事件订阅 EventBus 后原样吐出去，控制帧单独处理握手与急停。注意事件**不需要新造**：`event_pump` 拿到的 payload 已经是带 `sessionId` 的 wire JSON，直接转发即可。

#### 3.2.3 审批 / 提问回环：复用既有 EventBus wire（P4）

**映射**：实施点 **P4**；决策行 **R6/R7**。

**技术要点**：现有 `EventBusAskQuestionPanel`（`ask_question_wire.rs`）已经把「提问」做成 EventBus 上的 request/response：发 `plan.ask_question`（含 `requestId` + `responseEvent`），监听 `responseEvent` 收回包。serve 只需在两端各搭一座桥：

- 订阅 `WIRE_PLAN_ASK_QUESTION` → 把 `AskQuestionWireRequest` 包成 `control_request{subtype:"ask_question", requestId, payload}` 发 UI。
- stdin 收到 `control_response{requestId, payload}` → `emit_payload(responseEvent, AskQuestionWireResponse)` 回 EventBus，原 panel 的 `oneshot` 收到回包解阻塞。

```text
AgentLoop(工具需提问) ─► EventBusAskQuestionPanel.ask()
        │ emit plan.ask_question{requestId, responseEvent, questions}
        ▼
   serve event 桥 ─► control_request{subtype:ask_question,...} ──stdout──► UI 弹窗
                                                                              │ 用户选择
   serve stdin 桥 ◄── control_response{requestId, answers} ◄──stdin────────┘
        │ emit_payload(responseEvent, AskQuestionWireResponse)
        ▼
   panel.oneshot 收到回包 ─► AskQuestionResult ─► 工具继续/取消
```

说人话：审批/提问这条最容易踩坑的回环，Tomcat 其实已经写好了一半（EventBus 上的一问一答）。serve 做的只是把这一问一答「转译」到 stdio 的控制帧上，用户在 VSCode 里点完「同意」，回包顺着原路 emit 回去，工具就接着跑。`IdeAskQuestionPanel` 那个 `cancelled` 占位就是留给这里实现的。

#### 3.2.4 Phase 2 网关复用同一 dispatcher（P7 / P8，PENDING）

**映射**：实施点 **P7/P8**；决策行 **R1/R10**。

**技术要点**：Phase 2 不重写业务，只把「传输适配器」从 stdio 换/加成 axum WebSocket：`commands.rs`（命令分发）、`event_pump.rs`（事件订阅）、`control.rs`（控制回环）保持不变，新增 `gateway/ws.rs` 把 WS text frame ↔ `OutFrame/ServeCommand` 对接，`gateway/auth.rs` 加 loopback 绑定 + token + origin 校验（对标 openclaw `auth.ts`）。ACP 路径（P8）同理：在 `acp/`* 做 JSON-RPC 信封 + `method` rename 的**机械翻译**（§3.2.5），内侧 dispatcher 不动。多客户端时按 `sessionId` 信封路由 + 广播。

说人话：这一步只是把「管道」换成「网线」，agent 那套命令/事件/控制逻辑一行不动。等真要做 Web 控制台或远程多端再上，本期只钉死「能平滑升级」这件事，不强制实现。

#### 3.2.5 自有 wire 形状约束与 ACP 机械翻译（P1 / P3 / P8）

**映射**：实施点 **P1/P3**（Phase 1 自有帧形状）、**P8**（ACP 适配）；决策行 **R4/R3**。

**Phase 1 实施约束：不套 JSON-RPC 信封（即便 method 可填自有名）**


| 关切               | 实施结论                                                                                                                         |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 上下行非对称           | 上行命令/控制低频、请求-响应式；下行 `AgentEvent` 高频流式（`content_delta` 每 token 一帧），且**已与 CLI/审计共用** `tag="type"`（R3）                          |
| JSON-RPC 在事件侧的收益 | 若包成 `{"jsonrpc":"2.0","method":"event","params":{...AgentEvent...}}`，每条热路径多背固定样板，**仍须**按 `params.type` 分支——`method` 路由在事件侧无效 |
| 与 ACP 的关系        | ACP ≠ 仅 JSON-RPC 信封，而是一组固定 `session/`*、permission 等方法语义；Phase 1 上信封**省不了** Phase 2 语义映射，却会在热路径持续交税                           |
| 同构性              | 命令、事件、控制均用 `{type}` 分派，UI 单一心智；套 JSON-RPC 会把上行劈成 `method/params/id`、下行仍 `type`-tagged                                        |


**自有帧 ↔ JSON-RPC 结构对齐（P8 `acp/`* 机械翻译契约）**


| 自有 wire（Phase 1，P1/P3）  | JSON-RPC 2.0 / ACP（Phase 2 外侧，P8）                         | 翻译方向                                                    | 实施约束                                                                 |
| ----------------------- | --------------------------------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------- |
| 命令 `type`               | `method`                                                  | 双向：`prompt` ↔ `session/prompt`（表驱动，`acp/method_map.rs`） | **判别字段**；`commands.rs` 仍只认 `type`，ACP 适配层负责 rename                   |
| 命令 `id`                 | 请求 `id`                                                   | 双向：同值透传                                                 | 仅用于命令 ack / `{type:"response",id}` 配对；**不**用于流式事件配对                  |
| 控制 `requestId`          | server→client 请求的 `id`；client 回包同 `id`                    | 双向透传                                                    | 审批/提问等双向阻塞回环（P3/P4）；与命令 `id` 命名空间正交                                  |
| 事件 `type`（`AgentEvent`） | ACP `session/update` 的 `params.update`（或 notification 载荷） | 外侧包装；**内侧不改** `AgentEvent` 形状                           | 事件**不**强行改成 `method:"message_update"`，避免与 `AgentEvent.type` 重复打架（R3） |
| `sessionId`             | ACP `params.sessionId` / 方法上下文                            | 双向透传                                                    | 多会话路由键；JSON-RPC 本身无 session 概念                                       |


说人话：Phase 1 不用 JSON-RPC 外壳，不是不认 JSON-RPC——而是**下行事件流套它只交税不办事**。但 P1/P3 故意把 `id`、`requestId`、`type` 摆成和 JSON-RPC 同构，这样 P8 做 ACP 时，`acp/`* 主要是**改字段名和包一层信封**，不用重写 `commands`/`event_pump`/`control` 内核。

#### 3.2.6 可见性感知下行 visible/hidden（P5.5，Phase 2 设计储备）

**映射**：实施点 **P5.5**；决策行 **R12**（背压复用 R9、控制命令复用 R6、快照复用 P1 `get_messages`）。**范围说明**：本节保留完整设计，供 Phase 2 评估/落地；**当前版本不实现 visible/hidden 过滤**，运行策略仍为全量 fanout + 公平轮转。

**技术要点**：可见性是 writer 的一个 **per-session 状态位**，不是新传输、不碰核心。

- **声明**：UI 发控制命令 `set_session_visibility{sessionId, visible:bool}`；per-session 独立、可多 `visible` 并存（split view）。默认值：会话新建时按 UI 是否立即聚焦决定，缺省 `hidden`（避免后台预建会话抢带宽）。
- **过滤**：writer 在 per-session 背压前置一个可见性闸门——
  - `visible`：全量 fanout（沿用 R9 合并/节流）。
  - `hidden`：只放行 **lossless 集**（`agent_start/agent_end`、`turn_end`、`error`、`control_request/response/cancel`、状态徽标），**丢弃** `content_delta`/`thinking_delta`/工具流式增量。
- **不变量**：① 丢弃只发生在 **wire 层**；服务端事件订阅照常把 delta 累积进**会话消息状态（内存累加器）**，turn 结束再落 transcript（故快照内容不缺，见下「飞行中消息」）。② `hidden` 的审批/提问 `control_request` **绝不降级**，否则后台工具静默卡死。

**`seq` 与快照的精确语义（本设计核心，务必照此实现）**

- **`seq`**：writer 给**每条下行 wire 帧**打的 **per-session 单调递增整数**（每个 `AgentEvent`、`control_*` 各一个；hidden 丢弃的帧也消耗 `seq`，保证编号连续无歧义）。
- **快照来源 = 已落盘 transcript ＋ 当前飞行中消息的内存累加文本**，**不是只读 transcript**。这是正确性的关键：见「飞行中消息」。
- **截止游标 `upToSeq`**：服务端构造快照时返回的「**本快照内容已涵盖到的最高 `seq`**」。语义是「`seq ≤ upToSeq` 的所有帧效果都已包含在快照里」。
- **续接规则**：UI 渲染快照后，对 live 帧**严格只取 `seq > upToSeq`**（`seq ≤ upToSeq` 丢弃）。边界左闭右开 → **不缺帧**（快照覆盖 `≤upToSeq`、live 覆盖 `>upToSeq`，无空洞）、**不重复**（边界互斥，无交叠）。

**飞行中消息（in-flight message）——必须正确处理的杀手场景**

- 场景：`hidden` 期间某条消息正在流式输出（delta 1..50 已发但被丢弃），此刻 `message_end` 未到、**transcript 里尚无该消息**。用户切 `visible`。
- 若快照**只读 transcript**：这条消息前半段（1..50）wire 丢了、transcript 没有，live 只会接到后半段（51..），UI **只显示半截消息** → 错误。
- 正解：**因不变量①，被丢弃的 delta 仍在服务端累积成「会话消息状态」**，故快照把**当前飞行中消息的累加文本**一并带出，`upToSeq` 设为此刻已分配的最高 `seq`。于是快照已含 1..50，live 从 51.. 续接 → 无缝。
- 推论：**单读 transcript 或单看 stdout 都不够**；必须「快照（transcript ＋ 内存消息状态）＋ `seq` 续接」配合。

- **未决审批**：`hidden` 期间已下发的 `control_request` 由 UI 按 `sessionId` 暂存，转 `visible` 时随快照一并重渲染（保证审批框不丢、不重复弹）。

```text
event_pump ─► [visibility gate]                         ┌──────── 唯一 writer ────────┐
              ├ visible → 全量（含 delta）──► seq 标记 ──►│ R9 coalesce → stdout         │
              └ hidden  → 仅 lossless（丢 delta，仍累积进消息状态）─► seq ─►│ lossless 旁路直发 │
                                                          └──────────────────────────────┘
hidden→visible：
  UI ──get_messages{lastNTurns}──► 快照 = 已落盘 transcript ＋ 飞行中消息累加文本，带 upToSeq
     ──► 重渲染 ──► 续接 live：只取 seq > upToSeq（seq ≤ upToSeq 丢弃）
```

说人话：`seq` 是给 stdout 每行贴的会话内单号（像快递单号）。切回前台时，先找服务端要一张「截图」——这截图不光是硬盘上的聊天记录，还包含**这会儿正打到一半那句话的当前内容**（因为后台丢掉的字其实一直在服务端攒着）。截图会告诉你「我截到第几号为止」，之后你只接收号码更大的直播帧，号码更小的丢掉。这样既不会漏掉后台那段没推给你的字，也不会把截图里已有的内容再画一遍。最怕的坑是「一句话正打到一半时切过去」——只要截图带上这半句的当前文本，就不会只显示后半截。后台期间弹的审批一直在 UI 手里存着，切回来照样显示。

**与 Phase 2 一致性**：快照走 wire（`get_messages`）而非 UI 读磁盘 transcript——所以 WebSocket/远程（extension host 与 agent 异机、webview 无本地 FS）下行为完全一致。transcript 路径仅作 `initialize` 的可选 metadata，供超长历史懒加载，不作切换重建主路径。

#### 3.2.7 事件 schema/TS 工件 与 多模态附件（P6 事件侧 / 附件，本期落地）

**映射**：决策行 **R13**（事件 schema/TS）、**R14**（多模态附件）；协议形状见 §4.1.1 / §4.2.1。

##### A. 事件 schema / TS（R13）

技术要点（单一事实源，不另造第二套）：

- 在 `src/infra/events/mod.rs` 给 `AgentEvent` 加 `#[derive(JsonSchema)]`；新增 `WireEvent { sessionId, #[flatten] event: AgentEvent }`（与既有 `WireEnvelope` 同形）作为事件帧的 schema 入口。
- **压低 blast radius（关键事实）**：`Message`/`ToolOutput`/`AssistantMessageEvent` 在 `events/mod.rs` 里本就是 `pub struct X(pub serde_json::Value)`，派生 `JsonSchema` 后**天然是 open object**（无需 `#[schemars(with=...)]` 额外注解）；`ToolDisplay` 是具名枚举、各事件标量字段（`type`/`sessionId`/`toolCallId`/`toolName`/`isError`/`turnIndex`/`finishReason`/...）天然出精确类型。**精度清单见 §4.2.1。**
- **`AssistantMessageEvent` 维持 open object（已定）**：它当前是 `Value` 包装，本期**不**提升为具名结构；`Message`/`ToolOutput`/`args` 同样维持 open object。其形状在源码注释里已钉死（`kind`/`delta`/`source?`/`signature?`），UI 按该约定自行解析。
- `src/api/serve/schema.rs`：把 `WireEvent` 加进 `ServeSchemaBundle`；`serve_dts()` 从 JSON Schema 经 in-crate 轻量 emitter 生成具名 TS，替换 `unknown` 空壳。
- 测试：`serve_schema_fixture` 覆盖事件；新增「真实 `emit` 的事件能过生成 schema 校验」的 round-trip，防 open-object 注解漂移。

```text
AgentEvent(#[derive(JsonSchema)]) ─┐
WireEvent{sessionId, #flatten}  ───┼─► ServeSchemaBundle ─► serve.schema.json
ServeCommand/ControlFrame/...   ───┘                      └► serve.d.ts（in-crate emitter，具名类型）
```

风险提示：`AgentEvent` 被 CLI/审计等共用，加派生需保证不动既有 `Serialize` 行为；以回归测试（CLI 渲染、审计落盘）兜底。

##### B. 多模态附件（R14）

技术要点（底层已支持，serve 只接线）：

- 上行：`prompt`/`follow_up` 的 `params.attachments`（结构见 §4.1.1）。`steer` 本期忽略 `attachments`。
- `src/api/serve/commands.rs`：解析 `attachments` → `Vec<ChatMessageContentPart>`（复用 `image_b64`/`image_file_id`/file 构造器与 MIME 白名单）；与 `text` 的 `input_text` part 组装成 `ChatMessage::user_with_parts`；空附件退回 `ChatMessage::user(text)`。
- `src/api/serve/mod.rs`：把 `run_slot_turn` 与 `src/api/chat/run_loop` 的 `run_chat_turn` 入口从「只收 `&str`」扩成「可收预构造的多模态 user 消息」（新增一个接受 `ChatMessage`/`TurnInput` 的入口，纯文本路径保持不变）。
- 非法附件 → 该 `prompt` 回 `error:"invalid_attachment: ..."`，不进 turn、不影响其它会话。大小上限/MIME 校验**复用底层** `image_b64`(`IMAGE_MAX_BYTES` 4.5MB)/`file_b64`(`FILE_MAX_BYTES` 25MB) 构造器，serve 不另写。
- 测试：`serve_prompt_with_image_attachment_builds_multimodal_message`、`serve_prompt_invalid_attachment_returns_error`。

```text
prompt{params.attachments} ─► commands.rs 解析 ─► [text part + image/file parts]
                                                  └► ChatMessage::user_with_parts ─► run_slot_turn(多模态入口) ─► AgentLoop.run()
```

### 3.3 单进程多会话并发（本期目标·落地）

**映射**：实施点 **P1/P1.5/P3/P5**；决策行 **R8**；对齐 `[multi-agent.md](../multi-agent.md)` **维度A（多会话并发）+ MA1/MA2/MA7**。对标 codex `app-server` 的 `ThreadManager` 模型。

#### 3.3.1 为什么本期能做：核心已留好缝

`multi-agent.md` §14.3.1 已把多会话并发的设计原则钉死，且 Tomcat 现状代码已落地其中关键三件（见 §2.5.3 的 2/3/7）：


| 已具备（不改）                                 | 现状证据                                                                                                        | 对应竞品件                                                        |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `AgentLoop` 无全局单例，可按 `session_id` 多实例并发 | `multi-agent.md` §14.3.1；`chat_loop` 每输入 `AgentLoop::new().run()`                                           | codex 每 thread session loop                                  |
| 事件已按会话打信封                               | `ScopedEventEmitter::emit → WireEnvelope.sessionId`（`src/infra/event_bus/mod.rs` / `events/mod.rs`）         | codex `ThreadScopedSender`、pi-acp `session/update.sessionId` |
| 进程级注册 + 级联中止 + 并发上限**已实现**              | `src/core/agent_registry/mod.rs`：`AgentRegistry`、`register_root`、`cascade_abort`、`MAX_CONCURRENT_AGENTS=16` | codex `ThreadManager` + 子 agent 上限                           |
| 重资源进程级共享、会话级隔离                          | `GlobalServices`（`Arc<dyn LlmProvider/PrimitiveExecutor/EventBus>`）共享；`ScopeServices/SessionRuntime` per 会话 | codex 共享 auth/models/MCP（MA7）                                |


**还缺的（全在 serve 层，本期补）**：会话壳注册表（`ChatContextRegistry`）、命令按 sessionId 选槽、同会话串行守卫、单写者跨会话公平 demux。

#### 3.3.2 `ChatContextRegistry` 与现有 `AgentRegistry` 分工（对齐 MA1/MA2）

`multi-agent.md` §14.3.2.1 已规定两张进程级表正交分工，本文 serve 层据此落地：


| 维度              | `ChatContextRegistry`（serve **新增**，MA2）                                                    | `AgentRegistry`（**已实现**，MA1）                           |
| --------------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------------ |
| Key             | `sessionId`（serve 连接上的会话身份）                                                                | `session_id`（运行时 Agent 实例 id，含 `:sub:` / `-child-` 前缀） |
| Value           | `SessionSlot{ ctx: Arc<ChatContext>, run_task, cancel_token, busy }`                       | `Arc<AgentHandle>`（控制面元数据 + `abort_signal`）            |
| 关心              | 会话壳生命周期、命令路由、事件 demux、`busy` 串行                                                            | 并发上限、`spawn_depth`、`cascade_abort`、`SubAgentStart/End` |
| 持有 `AgentLoop`？ | 否（run 在 `run_task` 里，跑完即 drop）                                                             | 否（只持 Handle）                                           |
| 串联点             | `SessionSlot.ctx.agent_registry` 即同一进程级 `AgentRegistry`；`ChatContext` 装配时已 `register_root` | 同左                                                     |


> 注：现状一个 `ChatContext` 装配时绑定单个 `session_key`（`from_config_with_mode_and_overrides` 内 `SessionManager::new_scoped(session_key)` + 单 `_root_agent_guard`）。serve 多会话即「按需构造多个 `ChatContext` 并装进 registry」，每个 ctx 仍各自 `register_root` 到共享 `AgentRegistry`。**这正是 §14.3.2.1「一个 `session_key` 对应唯一 `ChatContext`」不变量的多实例化。**

#### 3.3.3 运行时结构与并发粒度

```text
              tomcat serve（单进程）
  ┌──────────────────────────────────────────────────────────────────────┐
  │ 进程级共享（Arc）：LlmProvider / PrimitiveExecutor / EventBus / AgentRegistry │
  └───────────────────────────────┬──────────────────────────────────────┘
                                   │ 注入
        ┌──────────────────────────┼──────────────────────────┐
        ▼                          ▼                          ▼
  SessionSlot s1            SessionSlot s2            SessionSlot s3
  ┌──────────────────┐      ┌──────────────────┐      ┌──────────────────┐
  │ ctx: ChatContext │      │ ctx: ChatContext │      │ ctx: ChatContext │
  │ run_task(s1)     │      │ run_task(s2)     │      │ (idle, 无活跃 run)│
  │ cancel_token_1   │      │ cancel_token_2   │      │ cancel_token_3   │
  │ busy=true        │      │ busy=true        │      │ busy=false       │
  └────────┬─────────┘      └────────┬─────────┘      └──────────────────┘
           │ emit(sessionId=s1)       │ emit(sessionId=s2)
           └──────────────┬───────────┘
                          ▼
            writer：按 sessionId demux + 跨会话 round-robin（公平）
                          ▼  NDJSON（每行带 sessionId）
                    UI（多 tab 各认领自己的 sessionId）
```

**并发粒度（对齐 codex `SerializationScope::Thread`）**：

- **跨会话真并发**：s1、s2 各自一个 `tokio::spawn` 的 `run_task`，同时流式，互不阻塞（s1 在 `awaiting_user` 等审批时 s2 照跑）。
- **同会话串行**：`SessionSlot.busy` 守卫；该会话 `busy=true` 时再来 `prompt` → 走 `steer/follow_up` 入队（复用现有 `follow_up_queue`）或返回 `error:"busy"`（对齐 hermes `4009`）。`AgentLoop` 本身天然单 turn，这一层只是把「忙」语义暴露给 UI。

#### 3.3.4 命令 / 事件 / 中止的会话路由


| 路径            | 机制                                                                                                                     | 复用 / 新增                       |
| ------------- | ---------------------------------------------------------------------------------------------------------------------- | ----------------------------- |
| 命令路由          | `commands.rs` 用 `cmd.sessionId` 从 registry 取 slot；缺省→活跃会话；未知→`error:"unknown_session"`                                 | 新增（registry 查表）               |
| `new_session` | 构造新 `ChatContext` 装进 registry，分配 `sessionId`，回 `initialize`-like ack                                                   | 复用 `ChatContext::from_config` |
| 事件 demux      | `event_pump` 每会话订阅；`WireEnvelope.sessionId` 已带 → writer 直接路由                                                           | **零新增**（信封已具备）                |
| 中止            | `interrupt{sessionId}` → 取 slot 的 `cancel_token.cancel()` + `AgentRegistry::cascade_abort(root_session_id)`（连带子 Agent） | 复用 `cascade_abort`（已实现）       |
| 审批回环          | `control_request` 带 `sessionId`；回包按 `requestId`（全局唯一）配对，`sessionId` 仅用于 UI 归属                                         | 复用 `ask_question_wire.rs`     |


#### 3.3.5 资源治理（对齐 multi-agent.md §14.9）

- **会话数上限**：serve 进程 `serve.max_sessions`（默认与 `MAX_CONCURRENT_AGENTS=16` 对齐；注意该上限是「会话 + 所有子 Agent」总和，子 Agent 派发会占额）。
- **空闲回收**：对标 codex 30min idle unload——空闲会话（无活跃 run 且无订阅）可配置 `serve.session_idle_unload_ms` 后 drop `ChatContext`（释放 scope 缓存/插件 VM）。MVP 可先不做自动回收，靠 `new_session`/显式 `close_session` 管理。
- **panic 隔离**：每会话 run 已由 `AgentRegistry::spawn_subagent_internal` 的 `tokio::spawn + JoinHandle` 模式覆盖子 Agent；serve 的会话 `run_task` 同样 `tokio::spawn` + `catch_unwind` 收口（对齐 R8），单会话 panic 仅转该会话 `error`（即 `agent_end{error}`，见 §7），不波及 writer/其它会话与主循环。

**说人话（§3.3 总览）**：本期落地就是「把 `multi-agent.md` 维度A 那张规划表（`ChatContextRegistry`）真正建出来，并接到 serve 的命令/事件/中止三条线上」。核心一行不改：`AgentLoop` 多实例、`EventBus` 的 `sessionId` 信封、`AgentRegistry` 的登记与级联中止全是现成的。serve 只补「按 sessionId 选槽 + 同会话串行 + 单写者公平 demux」这层壳——这恰好是 codex `ThreadManager` 在做的事，只是换成 Tomcat 的对象名。

---

