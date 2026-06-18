## 4. 协议（入参 / 出参 / Schema）

> 单一事实源：**命令/控制** = `src/api/serve/types.rs`（新）；**事件** = 既有 `src/infra/events/mod.rs`（`AgentEvent` + `WireEnvelope`，**不在本文重定义**）。所有帧均为 stdio 上的一行 NDJSON。

### 4.1 上行命令帧 `ServeCommand`（UI → agent）


| 字段          | JSON 类型 | 必填  | 默认值    | 适用场景                         | 说明                                                                                                                                                                     | 说人话                     |
| ----------- | ------- | --- | ------ | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------- |
| `type`      | string  | 是   | —      | 全部                           | 命令分派键：`prompt`/`steer`/`follow_up`/`get_state`/`set_model`/`new_session`/`switch_session`/`get_messages`/`close_session`/`list_sessions`/`control_request`/`control_response`/`control_cancel` | 这条命令要干嘛。                |
| `id`        | string  | 否   | 自动生成   | 需要响应配对的命令                    | 命令关联 ID；`response`/`control_response` 用它回指                                                                                                                             | 给命令编号，方便对上回包。           |
| `sessionId` | string  | 否   | 当前活跃会话 | **多会话路由键**                   | 目标会话；从 `ChatContextRegistry` 选会话槽；缺省落连接活跃会话；未知值→`error:"unknown_session"`                                                                                              | 这条命令属于哪个会话（多 tab 靠它分流）。 |
| `text`      | string  | 条件  | —      | `prompt`/`steer`/`follow_up` | 用户输入正文                                                                                                                                                                 | 用户说的话。                  |
| `model`     | string  | 条件  | —      | `set_model`                  | 目标模型 id                                                                                                                                                                | 切模型用。                   |
| `params`    | object  | 否   | `{}`   | 各命令扩展位                       | 命令专属附加参数（如 `prompt` 的附件、`new_session` 的 cwd/mode）                                                                                                                      | 其它零碎参数。                 |


`sessionId` 三态语义：缺省=活跃会话；显式值=指定会话（命中 registry）；显式 `null`=拒绝（返回 error）。

多会话命令补充（对应 §3.3 `ChatContextRegistry`）：


| `type`                       | sessionId 语义 | 行为                                            | 说人话         |
| ---------------------------- | ------------ | --------------------------------------------- | ----------- |
| `new_session`                | 不带（服务端分配并回传） | 构造新 `ChatContext` 装进 registry，回 `{sessionId}` | 开一个新会话 tab。 |
| `prompt`/`steer`/`follow_up` | 必带（缺省=活跃）    | 该会话 `busy` 则入队或回 `error:"busy"`               | 在指定会话里说话。   |
| `interrupt`                  | 必带           | 只停目标会话的 run（`cancel_token` + `cascade_abort`） | 只急停被点名的会话。  |
| `close_session`              | 必带           | 收口该会话 run 后从 registry drop `ChatContext`      | 关掉某个会话 tab。 |
| `list_sessions`              | 不带           | 回当前 registry 内所有 `{sessionId, busy}`          | 看现在开着哪些会话。  |

`interrupt` 只表示**取消该会话当前活跃 turn**（约等于 Ctrl+C）；关闭会话用 `close_session`，不是 `interrupt`。

### 4.2 下行事件帧（agent → UI）——复用既有 `AgentEvent`

事件帧**不新增**，直接是 `WireEnvelope` 序列化结果（见 `src/infra/events/mod.rs`）。关键既有变体（节选）：


| `type`（wire）                      | 关键 payload 字段                                          | 必填  | 说明                                                             | 说人话         |
| --------------------------------- | ------------------------------------------------------ | --- | -------------------------------------------------------------- | ----------- |
| `agent_start`                     | `sessionId`                                            | —   | 一次 run 开始                                                      | 开干了。        |
| `message_update`                  | `assistantMessageEvent{kind,delta,source?,signature?}` | 是   | `kind=content_delta`(正文) / `thinking_delta`(思考，`source=summary | raw`)       |
| `tool_execution_start`            | `toolCallId`,`toolName`,`args`                         | 是   | 工具开始                                                           | 开始跑工具。      |
| `tool_execution_end`              | `toolCallId`,`toolName`,`result`,`isError`,`display?`  | 是   | 工具结束（含状态）                                                      | 工具跑完了。      |
| `usage`（`ContextMetricsUpdate` 等） | token/比例字段                                             | —   | 上下文/计费记账                                                       | 用了多少 token。 |
| `agent_interrupted`               | `partialTextLen`,`toolResultsCount`                    | —   | 软中断                                                            | 被急停了。       |
| `agent_end`                       | `messages`,`error?`                                    | 是   | 一次 run 收口                                                      | 这回合结束。      |


> 完整变体与字段命名以 `src/infra/events/mod.rs` 的 `AgentEvent` 为准；serde 合约：顶层 `tag="type"` snake_case，payload 字段 camelCase，顶层附加 `sessionId`。

### 4.3 控制帧 `control_request / control_response / control_cancel`（双向）


| 字段           | JSON 类型 | 必填  | 默认值 | 适用场景                                                                | 说明                                                                              | 说人话         |
| ------------ | ------- | --- | --- | ------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ----------- |
| `type`       | string  | 是   | —   | 全部                                                                  | `control_request`(server→UI) / `control_response`(UI→server) / `control_cancel` | 控制帧三态。      |
| `request_id` | string  | 是   | —   | 全部                                                                  | 请求/响应**全局唯一**配对键（回包只认它，不靠 sessionId 路由）                                         | 哪个请求的答复。    |
| `sessionId`  | string  | 条件  | —   | `ask_question`/`permission`/`interrupt` 等会话级控制；`initialize` 不带（连接级） | 控制请求归属的会话，供 UI 展示分流；回包可省（用 `request_id` 即可）                                     | 这条控制属于哪个会话。 |
| `subtype`    | string  | 是   | —   | `control_request`                                                   | `initialize` / `ask_question` / `permission` / `interrupt`                      | 这条控制要干嘛。    |
| `payload`    | object  | 条件  | —   | 按 subtype                                                           | `ask_question` 即 `AskQuestionWireRequest`；响应即 `AskQuestionWireResponse`         | 控制的具体内容/回包。 |


### 4.4 调用样例（stdio NDJSON）

```jsonc
// ── UI → agent（stdin）──
{"type":"control_request","subtype":"initialize","request_id":"init-0","payload":{"clientInfo":{"name":"tomcat-vscode","version":"0.1.0"}}}
{"type":"new_session","id":"n1"}                         // 开会话 s1
{"type":"prompt","id":"c1","sessionId":"s1","text":"重构 src/main.rs 并跑测试"}
{"type":"new_session","id":"n2"}                         // 再开一个 tab：s2
{"type":"prompt","id":"c2","sessionId":"s2","text":"给 src/lib.rs 写文档注释"}

// ── agent → UI（stdout，节选，每行一帧；s1/s2 事件交错，靠 sessionId demux）──
{"type":"control_response","request_id":"init-0","payload":{"protocolVersion":1,"capabilities":["prompt","interrupt","ask_question","new_session"]}}
{"type":"response","id":"n1","success":true,"sessionId":"s1"}
{"type":"agent_start","sessionId":"s1"}
{"type":"response","id":"n2","success":true,"sessionId":"s2"}
{"type":"agent_start","sessionId":"s2"}                                  // s1、s2 同时在跑
{"type":"message_update","sessionId":"s1","assistantMessageEvent":{"kind":"thinking_delta","delta":"先看 main.rs","source":"summary"}}
{"type":"message_update","sessionId":"s2","assistantMessageEvent":{"kind":"content_delta","delta":"/// lib.rs 提供…"}}
{"type":"tool_execution_start","sessionId":"s1","toolCallId":"call_1","toolName":"read","args":{"path":"src/main.rs"}}
{"type":"tool_execution_end","sessionId":"s1","toolCallId":"call_1","toolName":"read","isError":false,"result":{"lines":238}}

// ── s1 的危险工具触发审批（control_request 带 sessionId）；此时 s2 不受影响仍可流式 ──
{"type":"control_request","subtype":"ask_question","request_id":"askq-0","sessionId":"s1","payload":{"requestId":"askq-0","responseEvent":"plan.ask_question.response.askq-0","questions":[{"prompt":"允许执行 cargo build 吗？","options":[{"id":"yes","label":"同意"},{"id":"no","label":"拒绝"}]}]}}
{"type":"message_update","sessionId":"s2","assistantMessageEvent":{"kind":"content_delta","delta":"…的核心入口。"}}   // s1 等审批时 s2 照跑
// ── UI → agent control_response（request_id 全局唯一，sessionId 仅用于 UI 归属）──
{"type":"control_response","request_id":"askq-0","payload":{"requestId":"askq-0","result":{"answers":[{"questionId":"0","optionId":"yes"}],"cancelled":false}}}

{"type":"agent_end","sessionId":"s2","messages":[]}
{"type":"agent_end","sessionId":"s1","messages":[]}
```

说人话：一次完整交互就是「UI 先握手 → `new_session` 开 tab → 发 prompt → agent 流式吐 thinking/正文/工具事件 → 碰到危险工具弹一个控制请求等用户点同意 → 收口」。多会话时 s1、s2 的事件在同一条 stdout 上**交错**，但每行都带 `sessionId`，UI 按它分到对应 tab；s1 卡在审批不会冻住 s2。事件部分的 JSON 形状和现在 CLI/审计看到的完全一样，UI 不用学新事件。

---

## 5. 文件职责总览（One-Glance Map）

```text
┌──────────────────────────────────────────────────────────────────────┐
│ src/api/cli/mod.rs                                                    │
│  enum Commands { ... , Serve { transport } }  ← 新增，与 Claw/Code 平级│
│  run_cli(): 路由 Serve → run_serve(); 复用 guard_nested_invocation     │
└───────────────────────────────┬────────────────────────────────────────┘
                                 │ 调
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│ src/api/serve/mod.rs           run_serve(transport)                    │
│   - 建进程级共享服务（Arc<LlmProvider/PrimitiveExecutor/EventBus>）     │
│   - 装配 registry / writer / stdin / event_pump / control / ask_question│
└───┬──────────────┬──────────────┬──────────────┬──────────────┬────────┘
    ▼              ▼              ▼              ▼              ▼
┌─────────┐ ┌─────────────┐ ┌──────────────┐ ┌─────────────┐ ┌──────────────┐
│stdin.rs │ │registry.rs  │ │commands.rs   │ │event_pump.rs│ │control.rs    │
│按\n切行 │ │ChatContext  │ │按 sessionId  │ │每会话        │ │initialize    │
│→ServeCmd│ │Registry:    │ │选槽→驱动该会 │ │EventBus.on  │ │interrupt(sid)│
│         │ │DashMap<sid, │ │话 AgentLoop  │ │(WIRE_*)→     │ │control_req/  │
│         │ │SessionSlot> │ │.run          │ │writer(+sid) │ │resp          │
└────┬────┘ └──────┬──────┘ └──────┬───────┘ └──────┬──────┘ └──────┬───────┘
     │             │ 复用          │ 复用           │ 复用          │ 复用
     │             ▼               ▼                ▼               ▼
     │   src/core/agent_registry/* src/api/chat/   src/infra/      src/api/serve/
     │   (AgentRegistry 已实现:     run_loop/*       event_bus       ask_question.rs
     │    register_root/            src/core/        /mod.rs         ServeAskQuestionPanel
     │    cascade_abort/上限)       agent_loop/*    (on/emit_sync)   └─复用 ask_question_wire.rs
     │   ChatContext::from_config                   src/infra/       （替换 ide_ask_question_panel.rs）
     ▼   (每会话一壳，共享 Arc 服务)                events/mod.rs
┌──────────────────────────────────────────────────────────────────────┐
│ src/api/serve/writer.rs   单写者 FIFO(mpsc)+coalesce+lossless          │
│   + 按 sessionId demux + 跨会话 round-robin 公平                        │
│   → src/api/serve/ndjson.rs  ndjson_safe_stringify(转义 U+2028/2029)   │
│   → stdout（唯一 writer 任务独占）                                      │
└───────────────────────────────┬────────────────────────────────────────┘
                                 ▼  [src/api/serve/tests/]
                            UI 进程（VSCode 扩展 / 桌面 GUI，多 tab 按 sessionId 分流）

  src/api/serve/types.rs    ServeCommand / ControlFrame 单一事实源（+ schemars 派生）
  src/api/serve/schema.rs   --print-schema 导出 JSON Schema + TS d.ts  [tests/serve_schema_fixture]
  src/api/serve/gateway/*   Phase 2（PENDING）：axum WS + auth，复用 registry + dispatcher
```

阅读顺序（说人话）：从 `cli/mod.rs` 新增的 `Serve` 入口进，`serve/mod.rs` 把六个零件装起来；`registry` 持「sessionId → 会话槽」总台账（**本期多会话并发的核心新增**），`stdin/commands` 负责「按 sessionId 选槽、收命令、驱动 run」，`event_pump` 负责「每会话订阅 EventBus、带 sessionId 把事件搬出去」，`control/ask_question` 负责「审批/握手/按会话急停」，所有下行最后都汇到 `writer` 一个出口并按 sessionId demux。注意带「复用」箭头的都是**现有模块**——尤其 `agent_registry` 的登记/级联中止已实现；serve 是薄薄一层编排，不重写 agent。

---

