# Tomcat VSCode 扩展 · 01 术语与竞品调研

> 总览见 [`../tomcat-vscode-extension.md`](../tomcat-vscode-extension.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§1 术语统一** 与 **§2 竞品 / 选型对比（调研）**。
> 单一事实源：协议与类型以 `tomcat/src/api/serve/types.rs` + `tomcat/src/infra/events/mod.rs` 为准。
> 外部参考仓库（与本仓同级，位于 `/Users/yankeben/workspace/`，仅作证据引用）：`vscode/`、`cline/`、`continue/`。

---

## 1. 术语统一

> 专业：本节钉死扩展侧与 Tomcat serve 协议交界处所有易混淆命名；后文出现一律以此为准。
> 说人话：先把"参与者、桥接核心、会话、控制帧、提问、转向、中断"这几个词说清楚，免得越读越糊。

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| Chat Participant（参与者） | VSCode 原生聊天里的一个 `@tomcat` 代理 | `vscode.chat.createChatParticipant(id, handler)`（稳定 API） | 由 `package.json` `contributes.chatParticipants` 声明；handler 拿到 `request/context/stream/token` | 就是聊天框里 @ 得到的那个机器人。 |
| 桥接核心 / Bridge | UI 无关的「Tomcat serve 客户端」 | 扩展侧 `serveClient/TomcatMessenger.ts` 等 | 不依赖任何 vscode UI 类型；输入 `ServeCommand`，输出事件流/回调 | 把命令行后端翻译成 UI 能用的东西的中间层。 |
| `tomcat serve` | Tomcat 暴露 agent 能力的子命令 | `tomcat/src/api/serve/mod.rs` `run_serve` | 本方案只用 `--stdio`；`--ws` 在 Tomcat 侧标注 deferred to Phase 2 | Tomcat 专门给外部 UI 用的"服务端模式"。 |
| wire / OutFrame | serve → 扩展的每行输出帧 | `tomcat/src/api/serve/types.rs::OutFrame`（untagged：Response\|Control\|Event） | 一行一个 JSON；`sessionId` 在顶层 envelope | 子进程吐出来的每一行 JSON。 |
| ServeCommand | 扩展 → serve 的每行命令帧 | `types.rs::ServeCommand`（`#[serde(tag="type")]`） | `type` 字段区分 14 种命令；多数带可选 `id`/`sessionId` | 扩展发给子进程的每一行 JSON 命令。 |
| sessionId | 一个独立会话（≈ 一个 chat 线程） | wire 顶层 `sessionId` 字段 | 由 `new_session` 创建；`registry.rs` 多会话并发；上限 `cfg.serve.max_sessions` | 一个对话线程的身份证，多开互不串台。 |
| ResponseFrame | 对某条带 `id` 命令的应答 | `types.rs::ResponseFrame{type:"response",id,success,error,payload}` | 与 `ServeCommand.id` 一一对应；`success=false` 带 `error` 码 | 命令的"收到/成功/失败"回执。 |
| control frame（控制帧） | 双向带外请求/应答/取消 | `types.rs::ControlFrame`（control_request/response/cancel） | `requestId` 关联一来一回；不是 turn 数据流 | 审批、提问这类"需要你拍板"的带外通道。 |
| ask_question | Agent 主动向用户提结构化选择题 | `control_request{subtype:"ask_question"}` + `AskQuestionWireRequest` | serve `ask_question.rs` 桥接；UI 答完回 `control_response` | Agent 弹给你的选择题/确认框。 |
| initialize | 建链握手 | `control_request{subtype:"initialize"}` → `control_response{protocolVersion,capabilities,sessionId}` | 未握手时一切业务命令回 `not_initialized` | 先打个招呼确认协议版本和能力，再干活。 |
| steer / 转向 | 当前轮进行中追加引导 | `ServeCommand::Steer` | 与 `prompt` 同形（text+params），但语义是"轮内插话" | 趁 Agent 还在干，往里塞一句新指示。 |
| follow_up | 轮结束后的下一句 | `ServeCommand::FollowUp` | 同 `prompt` 形态 | 上一轮收尾后接着说下一句。 |
| interrupt / 中断 | 软取消当前轮 | `ServeCommand::Interrupt{sessionId}` | serve 触发 `cancel_token` + `cascade_abort`；回 `agent_interrupted`/`agent_end` | 点"停止"，但不杀进程、不丢会话。 |
| message_update / delta | 流式正文/思考增量 | `AgentEvent::MessageUpdate.assistantMessageEvent{kind,delta}` | writer 可按 25ms 窗口合并相邻 delta | 一个字一个字往外蹦的增量。 |
| 背压降级 | 慢消费者时丢弃 delta | `writer.rs` `max_buffered_frames`(默认64) | 仅丢 `message_update`；发一次 `llm_notice{finishReason:"backpressure"}`；生命周期/控制帧绝不丢 | UI 太慢就丢中间字、但绝不丢"结束/审批"。 |

---

## 2. 竞品 / 选型对比（调研）

> 专业：本节是 [`02-implementation-details.md`](02-implementation-details.md) §3 已定稿结论的证据链。先回答"参考哪份代码"，再横向对比四类 VSCode AI 扩展形态，最后给出 proposed API 必要性清单与稳定替代，证明"独立扩展 + 稳定 API"既可行又可上架。
> 说人话：先确认该看谁的源码，再看别人都怎么做、各自踩了什么坑，最后逐条列出"那些花哨 UI 到底要不要 proposed API、不用能不能凑出来"。

### 2.1 参考哪份仓库（结论先行）

| 关切 | 结论 | 取自 | 说人话 |
|------|------|------|--------|
| Copilot 源码看哪份 | 看 **`vscode/extensions/copilot/`**（已合入 VSCode 本体并在此维护）；老的 `vscode-copilot-chat` 仅作历史对照 | `vscode/extensions/copilot/`、`vscode-copilot-chat/README.md`（标注已迁入主仓） | Copilot 现在住在 VSCode 主仓里，看那份才是权威。 |
| Copilot 的成熟 UI 能否照搬上架 | **不能**。其 agent 模式 / 编辑 diff / thinking / 审批卡由 VSCode **core** 渲染，靠 **proposed API** 驱动，且 proposed 受**扩展身份门禁**（内置/可信名单才放行） | `vscode/src/vs/workbench/services/extensions/common/extensionsProposedApi.ts`（`isBuiltin` 或 `product.json` 名单才授予）、`vscode/src/vscode-dts/vscode.proposed.chatParticipantAdditions.d.ts` | 那些花活是 VSCode 自带的、只对"自己人"开放，照抄代码也拿不到权限。 |
| 第三方扩展可行参照 | **Cline / Continue**：均为纯 webview、零 proposed、已上架 | `cline/apps/vscode/package.json`、`continue/extensions/vscode/package.json`（均无 `enabledApiProposals`） | 真正能上架的第三方，都没碰 proposed，照他们学。 |

> 说人话：这三行把"能不能 copy Copilot 上架"一锤定音——**UI 那层 copy 不来**（权限在 VSCode 手里），但**后端桥接和稳定 UI 画法可以学 Cline/Continue**。

### 2.2 四类形态横向对比

| 竞品 | 形态 | 关键设计 | 我们借鉴的点 | 说人话 |
|------|------|----------|---------------|--------|
| GitHub Copilot Chat | 原生 chat + 内置 UI + proposed API | `createChatParticipant` + `chatParticipantAdditions`（textEdit/thinkingProgress/confirmation/beginToolInvocation）；`defaultChatParticipant` 占据默认代理 | 仅借鉴**交互范式**（流式、工具卡、编辑预览的信息架构），不借代码/不依赖 proposed | 体验天花板，但靠的是"自己人特权"，我们只学它长什么样。 |
| Cline | 纯 Webview（侧栏）+ ProtoBus | `WebviewProvider` 注入 React + CSP；`grpc-handler.ts` unary/streaming 路由；`ui.proto` 定义 `ClineMessage(ask/say)`；`DiffViewProvider` 用 `vscode.diff`+`WorkspaceEdit` 做编辑 | webview 加载与 CSP、**双通道流式（全量 state + 增量 partial）**、稳定 diff 写法、审批按钮三态 | 自己画 UI 的范本：连"怎么显示 diff、怎么弹审批"都用稳定 API 做出来了。 |
| Continue | core/gui/ide 三层分离 + `binary/` NDJSON 子进程 | `IMessenger`/`IpcMessenger.ts`（NDJSON 一行一帧、`\r\n` 分隔、AsyncGenerator 分块）；`IDE` 抽象 + `VsCodeIde` 实现；`webviewProtocol.ts` 流式帧 | **桥接核心范式（与 tomcat serve 几乎同构）**、IDE 能力抽象、流式协议 `{done,content}` | 跟我们最像：它的"二进制子进程 + NDJSON 客户端"几乎就是 TomcatMessenger 的蓝本。 |
| Claude/Copilot CLI session 类 | 外部 CLI 进程 + 轻 UI 桥接 | spawn CLI、解析其 stream 协议、贴进面板 | 进程托管/重启/协议解码的工程化思路 | 跟我们形态同类：把一个命令行 agent 套个壳。 |

选型理由（为什么选 A=独立扩展桥接 serve，不选 B/C）：

1. **Tomcat 的价值在它自己的 loop/工具/权限/多会话**——B 把 Tomcat 降级成"一个 LM 模型"会绕过这些，等于自废武功；
2. **C（fork Copilot）重且不可上架**：体量大、涉许可/商标，且其招牌 UI 依赖 proposed，fork 出来的第三方扩展仍拿不到权限；
3. **serve 已就绪**：`tomcat serve --stdio` 的 NDJSON 协议、多会话、审批桥、schema 导出都已实现（见 [`03-protocol-and-file-map.md`](03-protocol-and-file-map.md)），A 只需写"客户端 + UI 映射"，工程量最小、风险最低；
4. **Continue 实证**：同构的"子进程 + NDJSON + 三层分离"已被 Continue 在生产验证且可上架。

### 2.3 proposed API 必要性清单（逐条给稳定替代）

> 专业：枚举 Copilot 招牌体验依赖的 proposed 能力，标注对 Tomcat 的必要性与稳定替代。来源：`vscode/src/vscode-dts/vscode.proposed.chatParticipantAdditions.d.ts`（其 `ChatResponseStream` 增量方法行：`thinkingProgress` L581、`textEdit` L583/585、`notebookEdit` L587/589、`workspaceEdit` L595、`markdownWithVulnerabilities` L605、`codeblockUri` L606、`confirmation` L619、`beginToolInvocation/updateToolInvocation` L661/668、`usage` L679）。
> 说人话：把"想要的花活"一条条列出来，标清楚"是不是非要不可""不用 proposed 怎么凑"。

| Proposed 能力 | 作用 | 对 Tomcat 必要性 | 稳定替代（可上架） | 说人话 |
|---------------|------|------------------|--------------------|--------|
| `stream.textEdit(uri, edits)` / `codeblockUri` | 聊天内联流式编辑 + diff 归因 | 想要但非必需 | `vscode.workspace.applyEdit(WorkspaceEdit)` + 虚拟只读文档 + `vscode.diff` 命令打开对比（取自 `cline/apps/vscode/src/integrations/editor/DiffViewProvider.ts`） | 内联流式 diff 拿不到，就用"打开一个 diff 标签页"代替。 |
| `stream.confirmation(title,msg,data,buttons)` | 聊天内审批卡 | 想要但非必需 | `stream.button(Command)`（稳定）渲染"同意/拒绝/带文本回复"按钮；或 `window.showQuickPick`（取自 `cline/.../chat-view/shared/buttonConfig.ts` 三态） | 审批卡画不了，就用按钮组/弹层代替。 |
| `stream.thinkingProgress(delta)` | 思考过程折叠块 | 可选 | 把 `MessageUpdate{kind:"thinking_delta"}` 渲染成普通 markdown 折叠引用块（`<details>` 或前缀样式） | 思考块用普通可折叠文本顶上。 |
| `stream.beginToolInvocation/updateToolInvocation` | 富工具调用卡（状态/计时/子代理） | 可选 | 用 `stream.progress(text)` + `stream.markdown` + `stream.filetree`/`anchor` 拼工具卡 | 工具卡用进度条+markdown+文件树拼出来。 |
| `stream.usage(ChatResultUsage)` | token 用量徽标 | 可选 | 用 `stream.markdown` 在末尾打一行用量统计（数据来自 `context_metrics_update` 事件） | 用量直接写一行文字。 |
| `defaultChatParticipant` | 占据"默认无 @ 即用"代理 | **不采纳** | 用户显式 `@tomcat`；不抢默认槽（默认槽属内置 Copilot，且受门禁） | 不抢默认位，老老实实做 @tomcat。 |
| `lm.registerChatModelProvider`（`chatProvider`） | 把 Tomcat 注册成可选模型（=形态 B） | **不采纳**（见 §3.1 R1） | N/A（形态 B 本身被否决） | 这条等于走 B 路线，已否。 |

> 说人话：清单结论——**没有一条 proposed 是 Tomcat 必需的**。招牌体验里"编辑 diff、审批、思考块、工具卡、用量"全部有稳定替代（Cline 已证明），代价只是"聊天内联"变成"打开 diff 标签 / 按钮 / 折叠文本"。因此锁死稳定档位，换取可上架可安装。
