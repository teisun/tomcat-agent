# Tomcat VSCode 扩展 · 02 落地选型与实施

> 总览见 [`../tomcat-vscode-extension.md`](../tomcat-vscode-extension.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§3 落地选型与实施（已定稿）**（§3.1 决策表 + §3.2 实施点）。
> 调研证据见 [`01-terminology-and-research.md`](01-terminology-and-research.md) §2；协议字段见 [`03-protocol-and-file-map.md`](03-protocol-and-file-map.md) §4。

---

## 3. 落地选型与实施（已定稿）

> 专业：§3.1 给出十条维度的取舍裁决（七列矩阵），§3.2 给出五列实施点表并按 Phase 1/2 拆节。证据"取自"列同时含本仓代码与外部 agent 仓代码。
> 说人话：先看每个分叉最后拍的板（§3.1），再看分几步交付、改哪些文件、怎么验收（§3.2）。

### 3.0 章节编排

文档内 `## 3` 对应 ARCHITECTURE_SPEC §3：§3.0 编排 → §3.1 七列决策表 → §3.2 五列实施点 + 逐点拆节。其前置的 §1/§2 在 [`01-terminology-and-research.md`](01-terminology-and-research.md)，其后续的 §4/§5 在 [`03-protocol-and-file-map.md`](03-protocol-and-file-map.md)，§6–§10 在 [`04-runtime-reference.md`](04-runtime-reference.md)。

### 3.1 落地选型决策表（七列）

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|---------------|--------|
| R1 集成形态 | 把 Tomcat 接进 VSCode 的根本姿势 | **采用 A=独立扩展 spawn `tomcat serve --stdio`；拒绝 B(降级为 LM 模型)、C(fork Copilot)** | 本仓 `tomcat/src/api/serve/mod.rs`(`run_serve`)；外部 `continue/binary/src/IpcMessenger.ts`、`vscode/extensions/copilot/`、`vscode/src/vs/workbench/.../extensionsProposedApi.ts` | 设计：扩展只做客户端+UI，Tomcat 保留 loop/工具/权限/多会话；理由：serve 已就绪、工程量最小、Continue 同构已验证、可上架 | B：把 Tomcat 降级成 `LanguageModelChat` 会绕过其 agent loop/工具/权限，自废武功且 `chatProvider` 为 proposed；C：fork Copilot 体量大、涉许可商标、招牌 UI 依赖 proposed 门禁，fork 后第三方仍无权限 | 让 Tomcat 当后端、扩展当前端，是最省力又能上架的路。 |
| R2 传输协议 | 扩展 ↔ serve 怎么通信 | **采用复用 Tomcat 既有 serve NDJSON wire（stdio），不另造协议** | 本仓 `tomcat/src/api/serve/types.rs`、`stdin.rs`、`writer.rs`；外部 `continue/binary/src/IpcMessenger.ts`（NDJSON 一行一帧） | 设计：stdin 发 `ServeCommand`、stdout 收 `OutFrame`，一行一 JSON；理由：协议已实现且自带握手/多会话/背压，零额外协议设计 | 自造 JSON-RPC / gRPC：重复造轮子且与 serve 现状漂移；`--ws`：Tomcat 侧明确 deferred to Phase 2（`mod.rs` 返回错误） | 直接用 Tomcat 现成的"一行一条 JSON"管道，别另起炉灶。 |
| R3 类型来源 | 扩展侧 wire 类型从哪来 | **采用构建期 `tomcat serve --print-schema` 生成 `serve.d.ts`，拷为扩展侧 `wire.d.ts`；禁止手写协议类型** | 本仓 `tomcat/src/api/serve/schema.rs`(`write_schema_bundle`→`serve.schema.json`+`serve.d.ts`，roots: ServeCommand/ControlFrame/ResponseFrame/WireEvent/OutFrame) | 设计：类型自动生成、随 Tomcat 演进；理由：协议漂移在 TS 编译期暴露，避免人肉对齐 | 手写 interface：易与 Rust 端 drift、无单一事实源 | 类型让 Tomcat 自己导出，扩展直接用，改协议立刻编译报错。 |
| R4 API 档位与 UI 路线 | 用稳定还是 proposed；participant 还是 webview | **采用稳定 API；UI 走 participant(Phase1)→webview(Phase2)，中夹 UI 无关桥接核心** | 本仓 `tomcat/src/api/serve/*`；外部 `vscode/src/vscode-dts/vscode.d.ts`(`createChatParticipant` L20121、`ChatResponseStream` L19907 稳定)、`cline/apps/vscode/package.json`、`continue/extensions/vscode/package.json`（皆无 `enabledApiProposals`） | 设计：硬要求可上架→只用稳定；桥接核心解耦 UI 故 Phase1/2 复用；理由：Cline/Continue 实证稳定 API 足以做完整聊天体验 | proposed(`chatParticipantAdditions`/`defaultChatParticipant`/`chatProvider`)：受身份门禁，第三方扩展会被置空，无法上架；其中 `defaultChatParticipant` 只影响 **VSCode 原生 Chat 公共默认槽位**，不影响我们在 Phase 2 自建 UI 中把 Tomcat 设为默认入口（`extensionsProposedApi.ts`） | 只用"人人能用"的 API：Phase 1 在 VSCode 原生聊天里不去抢公共默认位；Phase 2 若换自画 webview，默认当然就是 Tomcat。 |
| R5 编辑呈现 | 怎么显示/落地代码编辑 | **采用稳定 diff：虚拟只读文档(左)+真实文件(右)+`vscode.diff`+`WorkspaceEdit` 流式写入；内联行装饰列 Phase2 可选** | 外部 `cline/apps/vscode/src/integrations/editor/DiffViewProvider.ts`、`continue/extensions/vscode/.../diff/vertical/manager.ts`（进阶内联） | 设计：用打开 diff 标签页 + WorkspaceEdit 应用；理由：稳定、可上架、Cline 生产验证 | proposed `stream.textEdit`/`codeblockUri`（聊天内联流式 diff）：身份门禁不可上架 | 编辑用"开个对比标签页"展示，再一键应用，不靠聊天内联。 |
| R6 审批/提问呈现 | Agent 要你拍板时怎么交互 | **采用 `stream.button` 三态(同意/拒绝/带文本)或 QuickPick，对接 serve `control_request{ask_question}` 回环** | 本仓 `tomcat/src/api/serve/ask_question.rs`、`control.rs`(capabilities 含 `ask_question`)；外部 `cline/.../chat-view/shared/buttonConfig.ts` | 设计：把 `AskQuestionWireRequest.questions[].options` 渲染成按钮，答完回 `control_response`；理由：稳定 button 足够，回环已实现 | proposed `stream.confirmation` 卡片：门禁不可上架 | 审批用按钮组实现，点完把结果回传给 Agent。 |
| R7 多会话 → UI | 多个 chat 线程如何映射 | **采用 1 chat 线程 ↔ 1 `sessionId`；Phase1 用 participant `context` 关联、Phase2 用 webview tab** | 本仓 `tomcat/src/api/serve/registry.rs`(`ChatContextRegistry`, `max_sessions`)、`mod.rs`(`new_session`/`switch_session`) | 设计：扩展为每个聊天线程维护 sessionId 映射，命令均带 sessionId；理由：serve 原生多会话并发 | 单会话(像 Cline 单活跃 task)：放弃 Tomcat 的并发会话能力，体验降级 | 一个对话框对应一个会话 id，多开互不打架。 |
| R8 子进程生命周期 | serve 进程怎么起/复用/收 | **采用单常驻 serve 进程 + 多 sessionId 复用；崩溃→标记 failed + 提供重启入口** | 本仓 `tomcat/src/api/serve/mod.rs`(`run_stdio`,常驻)、`control.rs`(`handle_stdin_eof` 级联取消)；外部 `continue/extensions/vscode/.../IpcMessenger`(进程托管) | 设计：激活时 spawn 一次，所有会话复用；理由：避免每会话一进程的开销与 LLM 预热重复 | 每会话一进程：资源浪费、握手/预热重复；按需起停：首字延迟高 | 起一个后端进程，所有对话共用；挂了就提示重启。 |
| R9 背压消费 | UI 慢时怎么办 | **采用尊重 serve 既有背压：delta 可丢、生命周期/控制帧必达；UI 侧不得阻塞 stdout 读取** | 本仓 `tomcat/src/api/serve/writer.rs`(`max_buffered_frames`=64、`delta_coalesce_ms`=25、`llm_notice{backpressure}`) | 设计：扩展持续读 stdout 并快速入队渲染，遇 `llm_notice{backpressure}` 仅提示；理由：与 serve 写者策略一致，避免管道阻塞 | 扩展侧再加二级缓冲/重排：与 serve 单写者轮转语义冲突、易乱序 | 后端会自己丢中间字保流畅，扩展只管快读快画。 |
| R10 桥接消息层 | webview↔宿主、宿主↔serve 两段协议怎么设计 | **采用：宿主↔serve=NDJSON(学 Continue `IpcMessenger`)；webview↔宿主=typed postMessage 流式帧`{messageId,done,content}`(学 Continue `webviewProtocol`，或 Cline ProtoBus 双通道)。Phase1 无 webview，桥接核心直喂 `ChatResponseStream`** | 外部 `continue/binary/src/IpcMessenger.ts`、`continue/extensions/vscode/src/webviewProtocol.ts`、`cline/apps/vscode/src/core/controller/grpc-handler.ts` | 设计：两段协议都做成"请求 id ↔ 流式分块"；理由：与 serve 同构、可 AsyncGenerator 消费、Phase1/2 桥接核心复用 | 单段直连 webview↔serve：把子进程管理塞进 webview，违反 UI 无关原则、Phase1 无法复用 | 分两段管道，每段都"一问一流答"，中间桥接层两阶段都能用。 |

### 3.2 实施点（已闭环）

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| P1 桥接核心 MVP | `TomcatMessenger`（spawn/NDJSON 编解码/请求-应答/事件流/control 回环）+ 构建期生成 `wire.d.ts` + serve 路径解析/握手 | `serveClient/TomcatMessenger.ts`、`serveClient/wire.d.ts`(生成)、`serveClient/initialize.ts`、`scripts/gen-wire.ts` | 见 [`04-runtime-reference.md`](04-runtime-reference.md) §8：NDJSON 分帧/粘包、id↔Promise、control 回环；集成跑通 initialize+prompt | 先把"和子进程对话"的中间层写好测好。 |
| P2 Phase1 原生参与者 | `@tomcat` Chat Participant：prompt→stream 渲染、工具卡、审批按钮、稳定 diff、中断 | `package.json`(contributes.chatParticipants)、`extension.ts`、`ui/participant/handler.ts`、`ui/participant/render.ts`、`ide/VsCodeIde.ts` | §8 集成：一问一答端到端、ask_question 按钮回环、interrupt 生效、diff 打开+应用 | 用原生聊天框先把完整体验跑起来。 |
| P3 多会话与生命周期 | sessionId↔chat 线程映射、`new/switch/close/list_session`、崩溃重启入口、背压提示 | `serveClient/sessionRouter.ts`、`extension.ts`(进程守护)、`ui/participant/handler.ts` | §8 集成：双会话并发不串台、kill 子进程→failed+重启、背压 `llm_notice` 提示 | 支持多开对话、后端挂了能恢复。 |
| P4 Phase2 自建 Webview（可选增强） | React+Vite webview、typed `webviewProtocol`、partial/state 双通道、富工具卡/思考折叠、（可选）内联 diff 装饰 | `ui/webview/provider.ts`、`ui/webview/protocol.ts`、`gui/`(React)、复用 P1 桥接核心 | §8 集成：webview 流式渲染、与 participant 行为对齐；E2E 手测 | 想要更漂亮可控的 UI 时，自己画一套，桥接层不重写。 |

#### 3.2.1 P1 桥接核心（技术要点）

- **spawn**：`child_process.spawn(tomcatPath, ['serve','--stdio'], {cwd, env})`；`stdout` 行缓冲按 `\n` 切帧（serve 写者每帧后 `write_all(b"\n")+flush`，见 `writer.rs::write_frame`）。
- **解码分流**：每行 `JSON.parse` → 按结构判定 `OutFrame`：含 `type:"response"`→ResponseFrame（按 `id` resolve Promise）；`type∈{control_request/response/cancel}`→ControlFrame；其余→Event（按 `type` 分发，如 `message_update`/`tool_execution_*`/`agent_end`）。
- **请求-应答**：发送带 `id` 的 `prompt/get_state/...` 时登记 `Map<id, resolver>`；收到同 `id` 的 ResponseFrame 时 resolve。
- **control 回环**：收到 `control_request{subtype:"ask_question"}` → 调 UI 的 `askUser(AskQuestionWireRequest)` → 回 `control_response{requestId,payload:AskQuestionWireResponse}`；超时/取消发 `control_cancel`。
- **握手**：连接后先发 `control_request{subtype:"initialize"}`，等 `control_response` 拿 `protocolVersion/capabilities/sessionId`，校验能力含 `prompt`/`ask_question`。

```text
ASCII（P1 帧路由）：
 stdout line ─► JSON.parse ─► 判定 ─┬─ response{id}  ─► resolve(pendingReq[id])
                                    ├─ control_request{ask_question} ─► UI.askUser ─► send control_response
                                    ├─ control_response/cancel ─► (扩展极少收，主要扩展→serve)
                                    └─ event{type} ─► emit(type, payload) ─► UI 渲染
```

#### 3.2.2 P2 原生参与者（技术要点）

- **handler 签名（稳定）**：`(request, context, stream: ChatResponseStream, token) => Promise<ChatResult>`。
- **渲染映射**：`message_update`→`stream.markdown(增量)`；`tool_execution_start/end`→`stream.progress` + `stream.markdown`/`stream.filetree`/`stream.anchor`；`agent_end`→收尾。
- **审批**：`ask_question`→`stream.markdown(问题)` + 每个 option 一个 `stream.button({command:'tomcat.answer', arguments:[requestId,optionId]})`；或 `window.showQuickPick`。
- **编辑**：工具产出的文件改动→构造 `WorkspaceEdit`；展示用虚拟只读文档 + `vscode.diff`；用户确认后 `workspace.applyEdit`。
- **中断**：`token.onCancellationRequested`→`messenger.send(Interrupt{sessionId})`。
