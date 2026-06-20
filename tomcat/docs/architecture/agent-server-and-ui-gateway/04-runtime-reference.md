## 6. 配置与环境变量

总则：**env > config > 默认**。


| 变量                             | 取值                   | 含义                                                             | 优先级        | 说人话              |
| ------------------------------ | -------------------- | -------------------------------------------------------------- | ---------- | ---------------- |
| `TOMCAT_AGENT_ACTIVE`          | `1`                  | 标记当前已在 agent 会话内，serve 子进程继承后拒绝嵌套变更类命令（复用既有门禁）                 | env（最高）    | 防止 agent 自己又开自己。 |
| `serve.transport`              | `stdio`/`ws`(Phase2) | serve 默认传输                                                     | config     | 默认走管道。           |
| `serve.max_sessions`           | usize                | 单 serve 进程并发会话数上限（默认对齐 `MAX_CONCURRENT_AGENTS=16`，含子 Agent 占额） | config     | 一个进程最多开几个会话 tab。 |
| `serve.session_idle_unload_ms` | u32                  | **预留字段，本期未接线**：未来空闲会话（无活跃 run/订阅）多久后回收 `ChatContext`，`0`=不回收（对标 codex 30min）    | config     | 先留配置名，本期还不会真的自动清。    |
| `serve.delta_coalesce_ms`      | u32                  | delta 合并窗口（ms），`0`=不合并                                         | config     | 增量字攒多久发一批。       |
| `serve.max_buffered_frames`    | usize                | writer **每会话**队列上限，超限丢可丢帧（lossless 帧不丢）                        | config     | 单个会话队列多长算「慢客户端」。 |
| `serve.schema_out_dir`         | path                 | `--print-schema` 输出目录                                          | env/config | 类型文件吐到哪。         |
| `serve.gateway.bind`（Phase2）   | `127.0.0.1:PORT`     | 网关监听地址，默认 loopback                                             | config     | 联网时绑哪，默认只本机。     |
| `serve.gateway.token`（Phase2）  | string               | 网关 bearer token                                                | env（最高）    | 联网时的口令。          |


`serve.transport` 与 `--stdio/--ws` CLI 参数同义；CLI 参数 > env > config。

---

## 7. 错误模型 / 截断 / 警告

```text
命令 JSON 解析失败（坏行）
  -> 回 {type:"response", id?, success:false, error:"parse_error: ..."}（单行错），不退出循环

未知 type / 未知 control subtype
  -> 回 success:false + error:"unknown_command"，继续读下一行

命令携带未知 sessionId（registry 无此槽）
  -> 回 success:false + error:"unknown_session"，不影响其它会话

prompt 命中忙会话（该 sessionId 上一轮未结束）
  -> 回 success:false + error:"busy"（对齐 hermes 4009 语义），只针对该会话，不打断其它会话在跑的 run

会话数超过 serve.max_sessions
  -> new_session 回 success:false + error:"too_many_sessions"，已有会话不受影响

writer 背压超 max_buffered_frames
  -> content_delta/thinking_delta 合并或丢弃 + 一次性 warning 事件
  -> lifecycle/control 帧绝不丢（lossless）

control_response 携带未知 requestId
  -> debug 日志一行，丢弃该回包（防伪造/迟到），不影响主链路

initialize 之前收到 prompt
  -> 回 success:false + error:"not_initialized"（要求先握手）

stdin EOF / 管道关闭
  -> 触发 abort_signal，收口在跑的 run，干净退出
```


| 结局            | 抛错？ | 用户可见                      | 说人话                                        |
| ------------- | --- | ------------------------- | ------------------------------------------ |
| 命令行 JSON 坏    | 否   | error response 一行         | 单条坏命令别搞崩整条连接。                              |
| 未知 sessionId  | 否   | `unknown_session`         | 点错会话别影响别的会话。                               |
| 会话忙           | 否   | `busy`（仅该会话）              | 该会话上一轮没跑完，先排队，不碍别的会话。                      |
| 会话数超限         | 否   | `too_many_sessions`       | tab 开太多就先关几个。                              |
| 背压丢 delta     | 否   | warning（一次性，每会话独立）        | 慢就少发增量，但状态不丢。                              |
| 单会话 run panic | 否   | 该会话 `agent_end{error}`    | 一个会话炸了不连累其它会话（per-session `tokio::spawn`）。 |
| 管道断开          | 否   | 全部会话收口退出                  | UI 关了，所有会话干净停。                             |
| 核心 panic 隔离   | 否   | EventBus 已 `catch_unwind` | 单个监听器炸了不连累主流程。                             |


---

## 8. 测试矩阵（验收）

> 本方案为新增设计，实现前状态多为 `PENDING`；状态列仅允许 `✅ 日期`/`PENDING`/`阻塞于 X`。


| 维度   | 用例 / 编号                                                                     | 状态      | 说人话                              |
| ---- | --------------------------------------------------------------------------- | ------- | -------------------------------- |
| 单元   | `serve::writer::tests::serve_writer_single_drain_orders_frames`             | ✅ 2026-06-19 | 单写者保序锁死。                         |
| 单元   | `serve::ndjson::tests::ndjson_safe_stringify_escapes_line_separators`       | ✅ 2026-06-19 | U+2028/9 不破行。                    |
| 单元   | `serve::ndjson::tests::parse_command_line_rejects_unknown_command_type`     | ✅ 2026-06-19 | 未知命令类型不会伪装成 parse_error。 |
| 单元   | `serve::commands::tests::serve_prompt_drives_agent_run`                     | ✅ 2026-06-19 | 命令真能驱动 run。                      |
| 单元   | `serve::control::tests::serve_unknown_control_subtype_returns_unknown_command_error` | ✅ 2026-06-19 | 坏控制命令归一化错。                         |
| 单元   | `serve::event_pump::tests::serve_event_pump_streams_agent_events`           | ✅ 2026-06-19 | 事件确实被搬到下行。                       |
| 单元   | `serve::event_pump::tests::serve_lifecycle_events_not_dropped_for_other_sessions` | ✅ 2026-06-19 | 事件泵不漏生命周期事件（R3/P2）。              |
| 单元   | `serve::writer::tests::serve_writer_never_drops_lifecycle`                  | ✅ 2026-06-19 | 关键事件不丢（R9 锁死）。                   |
| 单元   | `serve::writer::tests::serve_writer_coalesces_deltas_under_pressure`        | ✅ 2026-06-19 | 背压下 delta 合并（R9）。                |
| 单元   | `serve::control::tests::serve_initialize_control_request_sets_ready_state`  | ✅ 2026-06-19 | 握手回能力位。                          |
| 单元   | `serve::control::tests::serve_interrupt_cancels_target_session`             | ✅ 2026-06-19 | 急停只停目标会话（R8/R6）。                 |
| 单元   | `serve::control::tests::serve_interrupt_unknown_session_returns_error_response` | ✅ 2026-06-19 | interrupt 命中未知会话时回标准错误。       |
| 单元   | `serve::registry::tests::new_session_registers_slot_in_registry`            | ✅ 2026-06-19 | new_session 入注册表（R8/P1.5）。       |
| 单元   | `serve::commands::tests::serve_command_routes_by_session_id`                | ✅ 2026-06-19 | 命令按 sessionId 选槽（R8）。            |
| 单元   | `serve::commands::tests::serve_same_session_second_prompt_is_busy`          | ✅ 2026-06-19 | 同会话串行 busy（R8）。                  |
| 单元   | `serve::commands::tests::serve_prompt_with_image_attachment_builds_multimodal_message` | ✅ 2026-06-19 | `prompt` 附件真正进多模态消息。 |
| 单元   | `serve::commands::tests::serve_follow_up_with_attachment_queues_multimodal_message_when_busy` | ✅ 2026-06-19 | 忙会话 `follow_up` 附件排队不丢。 |
| 单元   | `serve::commands::tests::serve_prompt_invalid_attachment_returns_error` | ✅ 2026-06-19 | 坏附件在入口直接被拒绝。 |
| 单元   | `serve::commands::tests::serve_prompt_without_attachments_falls_back_to_user_text` | ✅ 2026-06-19 | 没附件时仍走原来的纯文本路径。 |
| 单元   | `serve::commands::tests::serve_steer_ignores_attachments` | ✅ 2026-06-19 | `steer` 本期明确只吃 text，不偷偷消费附件。 |
| 单元   | `serve::commands::tests::serve_get_messages_uptoseq_is_null_placeholder` | ✅ 2026-06-19 | `upToSeq` 仍是 Phase 2 占位，不假装支持。 |
| 单元   | `serve::schema::tests::serve_emitted_event_validates_against_generated_schema` | ✅ 2026-06-19 | 真实事件样本能过生成 schema 校验。 |
| 集成   | `tests/serve_multi_session::serve_multi_session_concurrency_and_isolation`  | ✅ 2026-06-19 | 两会话真并发流式，且事件不串台（R8/维度A 锁死）。             |
| 集成   | `serve::writer::tests::serve_writer_round_robins_across_sessions`           | ✅ 2026-06-19 | 跨会话公平不饿死（R9/P5）。                 |
| 单元   | `serve::writer::tests::serve_hidden_session_drops_delta_keeps_lossless`     | PENDING | Phase 2：hidden 丢 delta、留 lossless（R12/P5.5）。 |
| 集成   | `tests/serve_visibility::serve_visibility_switch_resyncs_via_snapshot`      | PENDING | Phase 2：切前台用快照重建不缺帧/不重复（R12/P5.5）。       |
| 集成   | `tests/serve_visibility::serve_visibility_switch_midstream_message_not_truncated` | PENDING | Phase 2：流式中途切前台不只显示半截（飞行中消息，R12/P5.5）。  |
| 集成   | `tests/serve_visibility::serve_hidden_session_approval_still_delivered`     | PENDING | Phase 2：后台会话审批仍下发不卡死（R12/P5.5）。          |
| 集成   | `tests/serve_ask_question_tests::serve_ask_question_roundtrip_resumes_turn` | ✅ 2026-06-19 | 审批回环跑通（R7，复用 ask_question wire）。 |
| 集成   | `tests/serve_ask_question_tests::serve_ask_question_cancel_roundtrip_does_not_hang` | ✅ 2026-06-19 | 取消能传播，且不会挂死。                           |
| 集成   | `tests/serve_ask_question_tests::serve_ask_question_routes_by_session`      | ✅ 2026-06-19 | 两会话并发时审批只回正确 session。 |
| 集成   | `tests/serve_ask_question_tests::serve_interrupt_emits_agent_interrupted_and_tool_execution_end` | ✅ 2026-06-19 | 中断时 `agent_interrupted` 与在途工具收口一起锁住。 |
| 集成   | `tests/serve_robustness_tests::serve_unknown_command_returns_error_response` | ✅ 2026-06-19 | 未知命令回标准 `unknown_command`。 |
| 集成   | `tests/serve_robustness_tests::serve_parse_error_does_not_break_following_initialize` | ✅ 2026-06-19 | 坏 JSON 之后还能继续 initialize。 |
| 集成   | `tests/serve_robustness_tests::serve_eof_exits_cleanly` | ✅ 2026-06-19 | stdin EOF 能干净退出。 |
| 集成   | `tests/serve_stdio_e2e::serve_stdio_user_roundtrip_e2e`                     | ✅ 2026-06-19 | 端到端一次 turn 全链路。                  |
| 集成   | `tests/serve_stdio_e2e::serve_interrupt_emits_agent_interrupted_e2e`        | ✅ 2026-06-19 | stdio 端到端能看到 `agent_interrupted`。 |
| 集成   | `tests/serve_stdio_e2e::serve_stdout_only_emits_ndjson_frames`              | ✅ 2026-06-19 | stdout 只吐 NDJSON，不夹日志杂音。 |
| 集成   | `tests/serve_stdio_e2e::serve_prompt_with_attachment_roundtrip`             | ✅ 2026-06-19 | 附件 prompt 走完整子进程链路直到 `agent_end`。 |
| 单元   | `serve::writer::tests::serve_writer_backpressure_notice_emitted_once`       | ✅ 2026-06-19 | 慢客户端提醒只发一次，不刷屏。 |
| 单元   | `serve::control::tests::serve_not_initialized_returns_error_response`        | ✅ 2026-06-19 | 未 initialize 的命令会回标准错误。 |
| 单元   | `serve::commands::tests::serve_prompt_unknown_session_returns_error`         | ✅ 2026-06-19 | 命中未知 session 时回标准错误。 |
| 单元   | `serve::commands::tests::serve_prompt_panic_isolation_emits_agent_end_error` | ✅ 2026-06-19 | 单会话 panic 会被隔离并正常收口。 |
| 单元   | `serve::ask_question::tests::serve_ask_question_bridge_emits_control_request` | ✅ 2026-06-19 | ask_question bridge 能正确下发控制请求。 |
| 单元   | `serve::ask_question::tests::serve_ask_question_bridge_round_trips_control_response` | ✅ 2026-06-19 | ask_question bridge 能把 UI 回包送回原请求。 |
| 单元   | `serve::ask_question::tests::serve_ask_question_bridge_ignores_unknown_request_id` | ✅ 2026-06-19 | 乱入 requestId 不会污染正确会话。 |
| 快照   | `tests/serve_schema_fixture::serve_print_schema_matches_fixture`            | ✅ 2026-06-19 | 协议 schema / `.d.ts` 漂移防回归（R11/R13/R14）。            |
| 关键承诺 | §3.1 R5 单写者 / R7 审批请求 / R8 多会话并发 / R9 背压 / R13 schema / R14 附件 各有上面锁死测试；R12 visible/hidden 保留 Phase 2 设计测试锚点 | ✅ 2026-06-19 | 本期承诺与二期设计储备都留了测试钉。                       |
| 文档   | 本文与 `interaction-layer.md`、`multi-agent.md`、`README.md` 链接一致                | ✅ 2026-06-19 | 字跟得上代码。                          |


---

## 9. 风险与应对


| 风险                                | 影响          | 应对（具体动作）                                                                                                                                                                | 说人话                     |
| --------------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------- |
| stdout 被非 JSON 污染（log/print 误入下行） | 高           | 强制单写者 + serve 模式下把 tracing 输出钉到 stderr；新增 `tests/serve_stdio_e2e::serve_stdout_only_emits_ndjson_frames` 断言；参考 cc-fork `streamJsonStdoutGuard`                                                              | 别让日志混进数据管道把 UI 解析搞崩。    |
| EventBus 回调跨线程并发写导致帧交错            | 高           | 所有下行只准 `mpsc::send`，唯一 writer 任务 drain；`catch_unwind` 已隔离回调 panic（`event_bus/mod.rs`）                                                                                   | 一个出口排队，谁都不许插队直写。        |
| 审批回环卡死（UI 不回包）                    | 中           | `control_request` 带超时（对标 pi_agent_rust ACP 120s）+ `cancel_signal` 轮询（`ask_question_wire.rs` 已有 10ms 轮询）；超时按 cancelled 收口                                                | UI 不答就别永远等，超时按取消处理。     |
| 慢客户端撑爆内存                          | 中           | `max_buffered_frames` **每会话**上限 + delta 合并/丢（lossless 帧保留），对标 openclaw `dropIfSlow`                                                                                     | 客户端太慢就少发增量，别把自己撑死。      |
| 多会话单写者「快会话饿死慢会话」                  | 中           | writer 按 sessionId **round-robin 公平轮转**（修正 codex 全局队列 + openclaw 连接级 `bufferedAmount` 的公平缺口，见 §2.5.2）；每会话独立 pressure 计数                                                 | 别让一个刷屏会话把另一个会话的事件堵死。    |
| 多会话误用进程级可变态（cwd/secret 等）         | 中           | 进程级只共享**只读/线程安全** `Arc<dyn …>`（`GlobalServices`）；会话级隔离 `ScopeServices/SessionRuntime`（per `ChatContext`）；规避 hermes 全局 `secret callback`/`completion_queue` 串台教训（§2.5.2） | 水电共用、房间各锁；别把某会话的私货塞进全局。 |
| 协议演进两端漂移                          | 中           | `--print-schema` 生成 + fixture 快照测试锁定（对标 codex `schema_fixtures.rs`）                                                                                                     | 改协议时 CI 立刻报两边对不上。       |
| 嵌套调用污染会话/全局态                      | 中           | 复用 `guard_nested_invocation` + `TOMCAT_AGENT_ACTIVE=1`，serve 子进程继承后拒绝变更类命令                                                                                              | 防 agent 自己开自己改坏状态。      |
| Phase 2 联网鉴权缺失                    | 高（仅 Phase2） | 默认 loopback 绑定 + token + origin 校验（对标 openclaw `auth.ts`/`origin-check.ts`）；非 loopback 必须显式开                                                                            | 真要联网默认只本机 + 要口令。        |
| 中断时工具 Start/End 不配平               | 中           | interrupt 收口路径强制对已发 `tool_execution_start` 补 `tool_execution_end`（对齐现有 §状态机约束）                                                                                          | 急停也要把 UI 的「进行中」收干净。     |


---

## 10. 历史决策 / 跨文档修订

- ~~先实现网络 Gateway（HTTP/WebSocket daemon）再接 UI~~ → **否**：过早引入端口/鉴权/CORS/背压复杂度，且 openclaw `embedded-backend.ts` 证明纯网关会逼出 embed 重复实现。改为 **先 stdio Agent Server，网关 Phase 2 复用同一 dispatcher**。
- ~~为 serve 设计独立精简事件集~~ → **否**：codex 三套 schema（core/app-server/exec）并存导致 SDK 对接混乱。改为 **直接复用既有 `AgentEvent` wire**。
- ~~首发即采用 ACP 作唯一协议~~ → **否**：ACP 绑定 IDE 语义与 Tomcat 能力面不完全重合，会拖慢首发。改为 **自有 `{type}` NDJSON 首发，ACP 作 Phase 2+ 兼容层**。
- ~~Phase 1 采用 NDJSON + JSON-RPC 信封（即便 method 填自有名）~~ → **否**：Tomcat 上下行非对称——下行主战场是已与 CLI/审计共用的高频 `AgentEvent` 流（R3）；套 `jsonrpc`/`method` 只在 `content_delta` 等热路径增加固定样板，事件侧仍须按 `type` 分支，`method` 路由无收益；且会把命令/事件从同构 `{type}` 拆成两套心智。改为 **Phase 1 = NDJSON + 自有 `{type}`，不套 JSON-RPC 外壳**；**同时**自有帧结构上贴近 JSON-RPC（命令 `id`↔`id`、控制 `requestId`↔双向请求 `id`、`type`↔`method`，见 §3.2.5），使 Phase 2 `acp/`* 为机械翻译而非重写 dispatcher。拒绝理由：「现在上 JSON-RPC 信封以后接 ACP 更省」不成立——ACP 的适配工作量在方法语义映射，不在信封；Phase 1 上信封省不了 Phase 2 的 `session/*`、permission 等映射。
- ~~以 SSE 作 UI 主链路~~ → **否**：SSE 单向、需额外上行通道；竞品中 SSE 仅用于历史回放/兼容 API。改为 **stdio NDJSON 双工**。
- ~~MVP 先做「一连接一活跃会话」，多会话调度推迟~~ → **否**（本次修订）：核心已具备多会话并发底座（`AgentLoop` 多实例 + `WireEnvelope.sessionId` 信封 + `AgentRegistry` 登记/`cascade_abort`/上限**已实现**，见 §2.5.3），缺口仅在 serve 层。改为 **本期即做单进程多会话并发**：新增 `ChatContextRegistry`（落地 `multi-agent.md` 维度A/MA2）+ sessionId 命令路由 + 同会话串行 + 单写者公平 demux，对标 codex `ThreadManager`。拒绝 switch 式单活跃（pi-mono，会丢 in-flight turn）与多进程隔离（cc-fork，重资源 ×N）。
- ~~单写者只保序、背压只做合并/丢~~ → **补强**：多会话下单写者还须 **按 sessionId demux + 跨会话 round-robin 公平**，修正 codex（全局队列）/ openclaw（连接级 bufferedAmount）暴露的「快会话饿死慢会话」缺口。

### 跨文档修订意图

- `[interaction-layer.md](../interaction-layer.md)` §3 已补到本文的导航链接：上游继续声明统一交互层目标，本文负责给出 `tomcat serve` 的具体落地。
- `[multi-agent.md](../multi-agent.md)` **维度A（多会话并发）+ §14.3.2.1（`ChatContextRegistry` vs `AgentRegistry` 分工）** 是本文 §3.3 多会话落地的上位设计；链接已互相对齐，保持「维度A=设计、本文 §3.3=serve 落地」的一致引用。
- `[llm-stream-events-cli-pipeline.md](../llm-stream-events-cli-pipeline.md)` 的 `StreamEvent → AgentEvent → EventBus → CliTurnRenderer` 链路与本文复用同一 `AgentEvent`；本文不改其语义，仅新增 `EventBus → serve writer → stdout` 的并行订阅者（多会话下按 `sessionId` demux）。

---

