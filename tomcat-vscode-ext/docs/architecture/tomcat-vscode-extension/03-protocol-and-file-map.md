# Tomcat VSCode 扩展 · 03 协议与文件职责总览

> 总览见 [`../tomcat-vscode-extension.md`](../tomcat-vscode-extension.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§4 协议** 与 **§5 文件职责总览（One-Glance Map）**。
> 选型背景见 [`02-implementation-details.md`](02-implementation-details.md) §3；术语见 [`01-terminology-and-research.md`](01-terminology-and-research.md) §1。

---

## 4. 协议（复用 Tomcat serve wire）

> 专业：本扩展不定义新协议，完整复用 `tomcat serve` 的 stdio NDJSON wire。**单一事实源**：`ServeCommand`/`ResponseFrame`/`ControlFrame`/`OutFrame` 在 `tomcat/src/api/serve/types.rs`；事件 `AgentEvent`/`WireEvent` 在 `tomcat/src/infra/events/mod.rs`；可机读类型由 `tomcat serve --print-schema` 导出（`schema.rs`）。下表只描述「扩展侧关心的字段与消费方式」。
> 说人话：协议是 Tomcat 定的，扩展只是按它的格式收发。下面列出常用命令/事件的字段，外加几段真实 NDJSON 样例和扩展该怎么读。

### 4.1 命令（扩展 → serve）：`ServeCommand`

每行一个 JSON，`type` 字段判别（snake_case）。带 `id` 的命令会收到同 `id` 的 `ResponseFrame`。

| 字段 | JSON 类型 | 必填 | 默认 | 适用命令 | 说明 | 说人话 |
|------|-----------|------|------|----------|------|--------|
| `type` | string | 是 | — | 全部 | 14 种之一：`prompt`/`steer`/`follow_up`/`get_state`/`set_model`/`new_session`/`switch_session`/`get_messages`/`close_session`/`list_sessions`/`interrupt`/`control_request`/`control_response`/`control_cancel` | 这一行是什么命令。 |
| `id` | string | 否 | 缺省=不回执 | 业务命令 | 关联 `ResponseFrame.id`；建议总是带，用于一问一答 | 想要回执就带个流水号。 |
| `sessionId` | string | 否 | 缺省=活动会话 | 多数业务命令 | 路由到哪个会话；`switch_session` 必填 | 指明这条发给哪个对话。 |
| `text` | string | 是 | — | `prompt`/`steer`/`follow_up` | 用户输入正文 | 你说的话。 |
| `params.attachments[]` | ServeAttachment[] | 否 | `[]` | `prompt`/`steer`/`follow_up` | 附件：`{kind:"image"\|"file", mimeType?, dataBase64?, fileId?}` | 带的图/文件。 |
| `model` | string | 是 | — | `set_model` | 目标模型名 | 切模型。 |
| `params.cwd` | string | 否 | 进程 cwd | `new_session` | 新会话工作目录 | 新对话在哪个目录干活。 |
| `params.mode` | "code"\|"claw" | 否 | 配置默认 | `new_session` | 会话模式 | 新对话用哪种模式。 |
| `params.lastNTurns`/`limit` | number | 否 | — | `get_messages` | 拉取历史的范围 | 取最近几轮历史。 |
| `requestId` | string | 是 | — | `control_*` | 控制帧关联 id（与业务 `id` 不同字段） | 审批/握手的配对号。 |
| `subtype` | string | 是 | — | `control_request` | 如 `initialize`（其余 subtype 由 serve 决定是否支持） | 控制请求的子类型。 |
| `payload` | object | 否 | null | `control_*` | 控制帧负载（如 `AskQuestionWireResponse`） | 控制帧带的数据。 |

> 注：`control_response`/`control_cancel` 由扩展用于回答 serve 发来的 `ask_question`；`control_request{subtype:"initialize"}` 用于握手。serve 当前对 `initialize` 之外的客户端 `control_request` 子类型回 `unknown_command`（见 `control.rs`）。

### 4.2 输出帧（serve → 扩展）：`OutFrame`

`OutFrame` 为 untagged 联合，扩展按结构判别三类：

| 帧类 | 判别 | 关键字段 | 扩展消费 | 说人话 |
|------|------|----------|----------|--------|
| `ResponseFrame` | `type=="response"` | `id, success, sessionId?, error?, payload?` | 按 `id` resolve 对应请求；`success=false` 读 `error` 码 | 命令的回执。 |
| `ControlFrame` | `type∈{control_request,control_response,control_cancel}` | `requestId, subtype?, sessionId?, payload` | `control_request{ask_question}`→弹 UI；答完回 `control_response` | 审批/提问的带外通道。 |
| `Event`(WireEvent) | 其它 `type`（snake_case 事件名） | `sessionId?` + 事件专属字段 | 按 `type` 渲染（流式/工具/生命周期） | 干活过程中的实时事件。 |

#### 4.2.1 事件（`WireEvent` = `{sessionId?, ...AgentEvent}`）

`event_pump.rs` 把以下 `AgentEvent` 转发为 `OutFrame::Event`（按 `sessionId` 过滤）。扩展侧重点消费：

| 事件 `type` | 关键字段 | 扩展渲染 | 说人话 |
|-------------|----------|----------|--------|
| `agent_start` | — | 标记 turn 开始 | 一轮开跑。 |
| `message_update` | `assistantMessageEvent{kind:"content_delta"\|"thinking_delta", delta, source?, signature?}` | `kind=content_delta`→正文增量 `stream.markdown`；`thinking_delta`→折叠思考块 | 正文/思考一点点蹦出来。 |
| `message_start`/`message_end` | `message` | 消息边界 | 一条消息的头尾。 |
| `tool_execution_start` | `toolCallId, toolName, args` | 起一张工具卡（`stream.progress`） | Agent 开始用某工具。 |
| `tool_execution_end` | `toolCallId, toolName, result, display?{kind:file\|plan\|text}, isError` | 收尾工具卡；`display.file`→`stream.anchor`/diff | 工具用完了，给结果。 |
| `tool_call_streaming`/`tool_execution_update` | `toolCallId, toolName, args(Preview)/partialResult` | 工具卡进度更新 | 工具参数/中间结果在路上。 |
| `agent_end` | `messages[], error?` | 结束本 turn；`error` 非空→错误收尾 | 一轮结束（成/败）。 |
| `agent_interrupted` | `partialTextLen, toolResultsCount` | 标记被中断收尾 | 用户中断的收尾统计。 |
| `llm_error` | `reason, errorCode?, errorMessage` | 错误提示 | 模型终局报错。 |
| `llm_notice` | `finishReason, message` | `finishReason:"backpressure"`→仅提示；其它如截断提示 | 非错误的轻提示（含背压告警）。 |
| `context_metrics_update` | `inputTokensUsed, contextUtilizationRatio, ...` | 用量/上下文占用徽标 | token 用了多少、上下文多满。 |
| `auto_compaction_*`/`context_overflow_trim_*`/`boundary_switched`/`layer0_context_release` | 压缩/截断记账 | 可选状态提示 | 上下文太长时的自动整理。 |
| `sub_agent_start`/`sub_agent_end` | `parentSessionId, childSessionId, subagentType, ...` | 子代理关联展示 | 派生了子 Agent。 |

> 完整事件枚举与字段以 `tomcat/src/infra/events/mod.rs::AgentEvent` 为准；`event_pump.rs::EVENT_NAMES` 是转发白名单。

### 4.3 调用样例（stdio NDJSON）

```jsonc
// ── 握手（扩展 → serve）──
{"type":"control_request","requestId":"init-1","subtype":"initialize"}
// ── 握手应答（serve → 扩展）──
{"type":"control_response","requestId":"init-1","sessionId":"s-abc",
 "payload":{"protocolVersion":1,
            "capabilities":["prompt","steer","follow_up","get_state","set_model",
                            "new_session","switch_session","get_messages",
                            "close_session","list_sessions","interrupt","ask_question"],
            "sessionId":"s-abc"}}

// ── 发起一轮（扩展 → serve）──
{"type":"prompt","id":"p-1","sessionId":"s-abc","text":"重构这个函数并加测试"}

// ── 流式正文（serve → 扩展，可能很多行；writer 可合并相邻 delta）──
{"type":"agent_start","sessionId":"s-abc"}
{"type":"message_update","sessionId":"s-abc",
 "assistantMessageEvent":{"kind":"content_delta","delta":"好的，"}}
{"type":"tool_execution_start","sessionId":"s-abc",
 "toolCallId":"call_1","toolName":"edit","args":{"path":"src/foo.rs"}}
{"type":"tool_execution_end","sessionId":"s-abc","toolCallId":"call_1",
 "toolName":"edit","result":{"ok":true},"display":{"kind":"file","file":"src/foo.rs"},"isError":false}

// ── 审批/提问回环 ──
// serve → 扩展：
{"type":"control_request","requestId":"askq-s-abc-1","subtype":"ask_question","sessionId":"s-abc",
 "payload":{"requestId":"askq-s-abc-1","responseEvent":"plan.ask_question.response.askq-s-abc-1",
            "questions":[{"id":"apply","prompt":"应用这些编辑？",
                          "options":[{"id":"yes","label":"应用","recommended":true},
                                     {"id":"no","label":"放弃","recommended":false}]}]}}
// 扩展 → serve（用户点了"应用"）：
{"type":"control_response","requestId":"askq-s-abc-1","sessionId":"s-abc",
 "payload":{"requestId":"askq-s-abc-1",
            "result":{"answers":[{"questionId":"apply","optionIds":["yes"],
                                  "customText":null,"skipped":false,"pickedRecommended":true}],
                      "cancelled":false}}}

// ── 一轮回执 + 结束 ──
{"type":"agent_end","sessionId":"s-abc","messages":[/* ... */],"error":null}
{"type":"response","id":"p-1","success":true,"sessionId":"s-abc"}

// ── 中断（扩展 → serve）──
{"type":"interrupt","id":"int-1","sessionId":"s-abc"}
{"type":"response","id":"int-1","success":true,"sessionId":"s-abc","payload":{"interrupted":true}}
```

### 4.4 扩展侧消费契约（要点）

1. **必须先握手**：连接后立即发 `initialize`，否则业务命令回 `ResponseFrame{success:false, error:"not_initialized"}`。
2. **持续读 stdout 不阻塞**：渲染要异步入队（见 R9 背压）；读到 `llm_notice{finishReason:"backpressure"}` 仅提示，不要回压管道。
3. **按 `id` 配对请求**：业务命令用 `id`，控制帧用 `requestId`，两者字段不同、不可混用。
4. **未知会话**：对已关闭/不存在 `sessionId` 操作回 `error:"unknown_session"`，扩展应清理本地映射。
5. **类型来源**：`import` 构建期生成的 `wire.d.ts`（来自 `serve.d.ts`），不手写。

---

## 5. 文件职责总览（One-Glance Map）

> 专业：下图覆盖扩展侧将新增/改动的每个文件（`tomcat-vscode-ext/`），自顶向下读一遍即复现"激活 → 桥接 → 渲染 → 编辑/审批"链路；右侧标注其对接的 Tomcat serve 单一事实源文件（不在本仓改）。
> 说人话：从上往下扫一遍，就知道每个文件干嘛、跟谁说话。带 `[tests]` 的是配套测试落点。

```text
tomcat-vscode-ext/
├── package.json
│     - contributes.chatParticipants（@tomcat，Phase1）
│     - contributes.viewsContainers + views（webview，Phase2）
│     - contributes.configuration（tomcat.path / serve 参数，见 §6）
│     - engines.vscode（稳定基线，无 enabledApiProposals）
│            │ activate()
│            ▼
├── src/extension.ts
│     - activate/deactivate；解析配置→tomcat 可执行路径
│     - new TomcatMessenger() + initialize 握手
│     - 注册 participant（P2）/ webview provider（P4）
│     - 进程守护：exit→failed 状态 + 「重启 serve」命令
│            │
│            ▼
├── src/serveClient/                         ◀── 对接 Tomcat 单一事实源 ──┐
│   ├── TomcatMessenger.ts                                                │
│   │     - spawn('tomcat serve --stdio')                                 │  tomcat/src/api/serve/
│   │     - sendLine(ServeCommand) / 行缓冲 split('\n') 解析 OutFrame      │   ├── stdin.rs (run_stdio_loop)
│   │     - pendingReq: Map<id, resolver>（请求-应答）                     │   ├── commands.rs (handle_command)
│   │     - emit(eventType, payload)（事件流）                            │   ├── writer.rs (单写者/轮转/背压)
│   │     - control 回环：ask_question→askUser→control_response           │   ├── event_pump.rs (事件白名单)
│   │     - [tests] serveClient/tests/messenger_test.ts                   │   ├── control.rs (initialize/interrupt)
│   ├── initialize.ts  - 握手+capabilities 校验                           │   ├── ask_question.rs (审批桥)
│   ├── sessionRouter.ts - sessionId↔chat 线程映射（P3）                  │   ├── registry.rs (多会话)
│   │     - new/switch/close/list_session 封装                            │   ├── types.rs ★命令/帧定义
│   ├── wire.d.ts  【生成物，勿手改】← scripts/gen-wire.ts                 │   └── schema.rs (--print-schema)
│   │     - 由 `tomcat serve --print-schema` 的 serve.d.ts 拷贝            │  tomcat/src/infra/events/
│   └── [tests] serveClient/tests/ndjson_framing_test.ts                  │   └── mod.rs ★AgentEvent/WireEvent
│            │ 事件流 / askUser 回调                                       ┘
│            ▼
├── src/ide/VsCodeIde.ts
│     - 稳定 vscode API 封装：openTextDocument / applyEdit(WorkspaceEdit)
│     - 虚拟只读文档 provider + vscode.diff 打开对比（R5）
│     - 文件树/anchor 数据构造
│            │
│            ├───────────────► （Phase 1）
├── src/ui/participant/
│   │   handler.ts  - createChatParticipant handler（request/stream/token）
│   │       - token.onCancellationRequested → Interrupt
│   │   render.ts   - AgentEvent → ChatResponseStream（markdown/progress/
│   │                 button/filetree/anchor）；ask_question→button 三态
│   │   commands.ts - 'tomcat.answer'(回 control_response) / 'tomcat.applyEdit'
│   │   [tests] ui/participant/tests/render_test.ts
│            │
│            └───────────────► （Phase 2，可选增强）
└── src/ui/webview/
    │   provider.ts  - WebviewView 注入 React 资源 + CSP（学 cline WebviewProvider）
    │   protocol.ts  - typed postMessage 流式帧 {messageId,done,content}
    │                  （学 continue webviewProtocol；或 cline ProtoBus 双通道）
    └── gui/         - React+Vite 前端（复用 P1 桥接核心，不重写协议）
        [tests] ui/webview/tests/protocol_test.ts
```

阅读顺序（说人话）：从 `package.json` 的贡献点进入，`extension.ts` 激活时拉起 `TomcatMessenger`（桥接核心）；桥接核心右侧严格对接 Tomcat `serve/*` 那一摞文件（事实源，★ 为协议/事件定义，不改）。往下，`VsCodeIde` 用稳定 API 干"开文件/改文件/开 diff"的活；最后 Phase1 走 `ui/participant/*` 把事件画进原生聊天框，Phase2 才加 `ui/webview/*` 自画 React UI——两条 UI 路线都吃同一个桥接核心。
